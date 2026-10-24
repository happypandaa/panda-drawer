//! Explicit project-v1 data snapshots for the project's ordinary Git repository.
//! This module prepares and verifies one immutable candidate. It never updates
//! Git, publishes version-data, replaces live data, or creates a ZIP/history.
use crate::{
    backups, control::Freeze, files, projects::check_no_symlinks, ApiError, ProjectHandle,
    ProjectManifest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{watch, OwnedRwLockWriteGuard};
use unicode_normalization::UnicodeNormalization;

pub(crate) const DIRECTORY: &str = "version-data";
pub(crate) const MANIFEST: &str = "snapshot.json";
const MAX_MANIFEST: u64 = 8 * 1024 * 1024;

/// Separate from source-file budgets: an SDK attachment can be 256,000,000 bytes.
#[derive(Clone, Copy)]
pub(crate) struct SnapshotLimits {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_entries: usize,
}
impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 256_000_000,
            max_total_bytes: 1024 * 1024 * 1024,
            max_entries: 10_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DataFile {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DataSnapshotManifest {
    pub format_version: u32,
    pub project_id: String,
    pub sdk_api_version: u32,
    pub database_path: String,
    pub attachments_dir: String,
    pub database: DataFile,
    pub attachments: BTreeMap<String, DataFile>,
    pub directories: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedDataSnapshot {
    pub root: PathBuf,
    pub manifest: DataSnapshotManifest,
    /// Hash of the exact tracked manifest bytes; no timestamps cause empty commits.
    pub manifest_digest: String,
    pub manifest_bytes: Vec<u8>,
}

/// The guard and freeze move into the blocking task with the snapshot operation.
/// Dropping an async caller must never release business access while its detached
/// blocking copy is still running. The caller already owns a project task lease;
/// do not acquire this while holding that project's read gate.
pub(crate) struct SnapshotLease {
    project: ProjectHandle,
    _gate: OwnedRwLockWriteGuard<()>,
    _freeze: Freeze,
}
impl SnapshotLease {
    pub async fn acquire(
        project: ProjectHandle,
        mut stop: watch::Receiver<bool>,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<Self, ApiError> {
        project.check()?;
        project.control.check(false, None, false)?;
        check_cancel(&stop, &cancel)?;
        let freeze = Freeze::new(project.control.clone());
        let gate = tokio::select! {
            biased;
            _ = stop.changed() => return Err(cancelled()),
            _ = cancel.changed() => return Err(cancelled()),
            _ = tokio::time::sleep(Duration::from_secs(5)) => return Err(error("PROJECT_DATA_BUSY", "业务操作尚未结束；本次没有生成数据快照，请等待后重试。")),
            guard = project.operation_gate.clone().write_owned() => guard,
        };
        check_cancel(&stop, &cancel)?;
        project.check()?;
        Ok(Self {
            project,
            _gate: gate,
            _freeze: freeze,
        })
    }
}

/// Consume the lease in the blocking worker. Business requests are excluded only
/// while reading SQLite and copying attachments; Git object writes happen later.
pub(crate) fn prepare_snapshot(
    lease: SnapshotLease,
    destination: &Path,
    limits: SnapshotLimits,
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<PreparedDataSnapshot, ApiError> {
    let project = &lease.project;
    check_cancel(stop, cancel)?;
    validate_layout(&project.manifest)?;
    project.check()?;
    check_no_symlinks(destination, true)?;
    if destination.try_exists()? {
        return Err(error(
            "PROJECT_DATA_CANDIDATE_EXISTS",
            "数据候选目录已存在；未覆盖或采用已有内容。",
        ));
    }
    // Git's snapshot directory cannot be adopted as development/runtime material.
    if destination.starts_with(&project.root) {
        return Err(invalid("数据快照必须先在独立操作目录准备。"));
    }
    fs::create_dir(destination)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    let result = (|| {
        let live_attachments = project.root.join(&project.manifest.attachments_dir);
        let before =
            attachment_inventory(&live_attachments, &project.manifest, limits, stop, cancel)?;
        let required = before
            .bytes
            .saturating_add(fs::metadata(project.root.join(&project.manifest.database))?.len())
            .saturating_add(64 * 1024 * 1024);
        if fs2::available_space(destination)? < required {
            return Err(error(
                "PROJECT_DATA_SNAPSHOT_SPACE",
                "当前可用空间不足以生成数据库和附件快照；未截断或替换活库。",
            ));
        }
        let source_root = files::root(&live_attachments)?;
        let attachment_destination = destination.join("attachments");
        fs::create_dir(&attachment_destination)?;
        fs::set_permissions(&attachment_destination, fs::Permissions::from_mode(0o700))?;
        let destination_root = files::root(&attachment_destination)?;
        for directory in &before.directories {
            check_cancel(stop, cancel)?;
            let sentinel = format!("{directory}/.directory-placeholder");
            let _ = files::parent(&destination_root, &sentinel, true)?;
        }
        for (path, entry) in &before.files {
            check_cancel(stop, cancel)?;
            let source = files::read(&source_root, path)?;
            let mut output = files::create(&destination_root, path)?;
            let copied = copy_hashed(source, &mut output, limits.max_file_bytes, stop, cancel)?;
            if copied != entry.file {
                return Err(changed());
            }
            output.sync_all()?;
        }
        check_cancel(stop, cancel)?;
        backups::sqlite_snapshot_cancellable(
            &project.root.join(&project.manifest.database),
            &destination.join("database.sqlite"),
            || check_cancel(stop, cancel),
        )?;
        check_cancel(stop, cancel)?;
        let database = hash_file(
            files::read(&files::root(destination)?, "database.sqlite")?,
            limits.max_file_bytes,
            stop,
            cancel,
        )?
        .file;
        if database
            .bytes
            .checked_add(before.bytes)
            .is_none_or(|bytes| bytes > limits.max_total_bytes)
        {
            return Err(limit());
        }
        backups::check_database(&destination.join("database.sqlite"))?;
        if attachment_inventory(&live_attachments, &project.manifest, limits, stop, cancel)?
            != before
        {
            return Err(changed());
        }
        project.check()?;
        let manifest = DataSnapshotManifest {
            format_version: 1,
            project_id: project.manifest.id.clone(),
            sdk_api_version: project.manifest.sdk_api_version,
            database_path: project.manifest.database.clone(),
            attachments_dir: project.manifest.attachments_dir.clone(),
            database,
            attachments: before
                .files
                .into_iter()
                .map(|(path, file)| (path, file.file))
                .collect(),
            directories: before.directories,
        };
        let bytes = serde_json::to_vec_pretty(&manifest)?;
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err(limit());
        }
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(destination.join(MANIFEST))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        sync_directories(destination, &manifest.directories)?;
        validate_snapshot(destination, &project.manifest, limits, stop, cancel)
    })();
    result.map_err(|error: ApiError| {
        error
            .detail("partialFilesRetained", true)
            .detail("candidatePath", destination.display().to_string())
    })
}

/// Validate the entire exact payload before Git inclusion or future data restore.
/// Configuration, source files, dependencies, WAL/SHM and unrelated data paths are
/// never discovered or copied. Business attachment bytes remain private content.
pub(crate) fn validate_snapshot(
    root: &Path,
    expected: &ProjectManifest,
    limits: SnapshotLimits,
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<PreparedDataSnapshot, ApiError> {
    validate_layout(expected)?;
    check_cancel(stop, cancel)?;
    let root_fd = files::root(root)?;
    let manifest_file = files::read(&root_fd, MANIFEST)?;
    if manifest_file.metadata()?.len() > MAX_MANIFEST {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    manifest_file
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(limit());
    }
    let manifest: DataSnapshotManifest = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("数据快照清单无效；未采用已有目录。"))?;
    if manifest.format_version != 1
        || manifest.project_id != expected.id
        || manifest.sdk_api_version != expected.sdk_api_version
        || manifest.database_path != expected.database
        || manifest.attachments_dir != expected.attachments_dir
    {
        return Err(invalid("数据快照不属于当前项目或数据路径已变化。"));
    }
    let mut names = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected_names = ["attachments", "database.sqlite", MANIFEST]
        .map(std::ffi::OsString::from)
        .to_vec();
    expected_names.sort();
    if names != expected_names {
        return Err(invalid("数据快照含缺失或额外的文件。"));
    }
    if hash_file(
        files::read(&root_fd, "database.sqlite")?,
        limits.max_file_bytes,
        stop,
        cancel,
    )?
    .file
        != manifest.database
    {
        return Err(changed());
    }
    let attachments =
        attachment_inventory(&root.join("attachments"), expected, limits, stop, cancel)?;
    if attachments.directories != manifest.directories
        || attachments
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.file.clone()))
            .collect::<BTreeMap<_, _>>()
            != manifest.attachments
    {
        return Err(changed());
    }
    if attachments
        .bytes
        .checked_add(manifest.database.bytes)
        .is_none_or(|bytes| bytes > limits.max_total_bytes)
    {
        return Err(limit());
    }
    backups::check_database(&root.join("database.sqlite"))?;
    Ok(PreparedDataSnapshot {
        root: root.to_path_buf(),
        manifest,
        manifest_digest: format!("{:x}", Sha256::digest(&bytes)),
        manifest_bytes: bytes,
    })
}

pub(crate) fn validate_layout(manifest: &ProjectManifest) -> Result<(), ApiError> {
    for path in [
        &manifest.source_dir,
        &manifest.output_dir,
        &manifest.database,
        &manifest.attachments_dir,
    ] {
        let folded = path.to_ascii_lowercase();
        if folded == DIRECTORY
            || folded.starts_with(&format!("{DIRECTORY}/"))
            || DIRECTORY.starts_with(&format!("{folded}/"))
        {
            return Err(invalid(
                "项目已将 version-data 用作其他目录；未覆盖或移动已有内容。",
            ));
        }
    }
    Ok(())
}

/// Exact payload paths for raw Git object insertion. Never call `git add -A`.
pub(crate) fn tracked_files(snapshot: &PreparedDataSnapshot) -> BTreeMap<String, DataFile> {
    let mut result = snapshot
        .manifest
        .attachments
        .iter()
        .map(|(path, file)| (format!("{DIRECTORY}/attachments/{path}"), file.clone()))
        .collect::<BTreeMap<_, _>>();
    result.insert(
        format!("{DIRECTORY}/database.sqlite"),
        snapshot.manifest.database.clone(),
    );
    result.insert(
        format!("{DIRECTORY}/{MANIFEST}"),
        DataFile {
            bytes: snapshot.manifest_bytes.len() as u64,
            sha256: snapshot.manifest_digest.clone(),
        },
    );
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    file: DataFile,
    identity: (u64, u64, u32, u64, i64, i64, i64, i64),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inventory {
    files: BTreeMap<String, FileStamp>,
    directories: Vec<String>,
    bytes: u64,
}

fn stamp(metadata: &fs::Metadata) -> (u64, u64, u32, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}
fn hash_file(
    mut input: File,
    max_bytes: u64,
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<FileStamp, ApiError> {
    let before = input.metadata()?;
    if !before.is_file() || before.nlink() != 1 {
        return Err(invalid("数据快照仅接受无硬链接的普通文件。"));
    }
    if before.len() > max_bytes {
        return Err(limit());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        check_cancel(stop, cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(limit)?;
        if total > max_bytes {
            return Err(limit());
        }
        digest.update(&buffer[..count]);
    }
    if stamp(&input.metadata()?) != stamp(&before) || total != before.len() {
        return Err(changed());
    }
    Ok(FileStamp {
        file: DataFile {
            bytes: total,
            sha256: format!("{:x}", digest.finalize()),
        },
        identity: stamp(&before),
    })
}
fn copy_hashed(
    mut input: File,
    output: &mut File,
    max_bytes: u64,
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<DataFile, ApiError> {
    let before = input.metadata()?;
    if before.len() > max_bytes {
        return Err(limit());
    }
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancel(stop, cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(limit)?;
        if total > max_bytes {
            return Err(limit());
        }
        output.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
    }
    if stamp(&input.metadata()?) != stamp(&before) || total != before.len() {
        return Err(changed());
    }
    Ok(DataFile {
        bytes: total,
        sha256: format!("{:x}", digest.finalize()),
    })
}
fn attachment_inventory(
    root: &Path,
    manifest: &ProjectManifest,
    limits: SnapshotLimits,
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<Inventory, ApiError> {
    let root_fd = files::root(root)?;
    let mut result = Inventory {
        files: BTreeMap::new(),
        directories: vec![],
        bytes: 0,
    };
    let mut pending = vec![String::new()];
    let mut paths = BTreeSet::new();
    let mut count = 0usize;
    while let Some(relative) = pending.pop() {
        check_cancel(stop, cancel)?;
        let directory = root.join(&relative);
        check_no_symlinks(&directory, false)?;
        let before = fs::symlink_metadata(&directory)?;
        if !before.is_dir() {
            return Err(changed());
        }
        for entry in fs::read_dir(&directory)? {
            check_cancel(stop, cancel)?;
            count += 1;
            if count > limits.max_entries {
                return Err(limit());
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("附件路径须是有效 UTF-8。"))?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            files::parts(&path)?;
            if super::protected(&format!("version-data/attachments/{path}"), manifest).is_some() {
                return Err(invalid("附件路径使用受保护的凭据、依赖或控制名称；本次拒绝保存，不会跳过附件生成残缺版本。"));
            }
            if path.split('/').any(|part| {
                [".git", ".gitattributes", ".gitignore", ".gitmodules"]
                    .iter()
                    .any(|reserved| part.eq_ignore_ascii_case(reserved))
            }) {
                return Err(invalid("附件中含 Git 控制文件名；未生成不完整的数据版本。"));
            }
            let folded = path.nfc().flat_map(char::to_lowercase).collect::<String>();
            if !paths.insert(folded) {
                return Err(invalid("附件路径在当前卷上可能发生大小写或 Unicode 碰撞。"));
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                // Open the same path relative to the original descriptor before
                // treating its path-based enumeration as an ordinary directory.
                let _ = files::parent(&root_fd, &format!("{path}/.snapshot-probe"), false)?;
                result.directories.push(path.clone());
                pending.push(path);
            } else if metadata.is_file() && metadata.nlink() == 1 {
                let file = hash_file(
                    files::read(&root_fd, &path)?,
                    limits.max_file_bytes,
                    stop,
                    cancel,
                )?;
                if file.identity != stamp(&metadata) {
                    return Err(changed());
                }
                result.bytes = result
                    .bytes
                    .checked_add(file.file.bytes)
                    .ok_or_else(limit)?;
                if result.bytes > limits.max_total_bytes {
                    return Err(limit());
                }
                result.files.insert(path, file);
            } else {
                return Err(invalid("附件含链接或特殊文件；未跳过文件后保存残缺版本。"));
            }
        }
        if stamp(&fs::symlink_metadata(directory)?) != stamp(&before) {
            return Err(changed());
        }
    }
    result.directories.sort();
    Ok(result)
}
fn sync_directories(root: &Path, directories: &[String]) -> Result<(), ApiError> {
    for directory in directories.iter().rev() {
        File::open(root.join("attachments").join(directory))?.sync_all()?;
    }
    File::open(root.join("attachments"))?.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(())
}
fn check_cancel(
    stop: &watch::Receiver<bool>,
    cancel: &watch::Receiver<bool>,
) -> Result<(), ApiError> {
    if *stop.borrow()
        || *cancel.borrow()
        || stop.has_changed().is_err()
        || cancel.has_changed().is_err()
    {
        Err(cancelled())
    } else {
        Ok(())
    }
}
fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("not_committed")
}
fn invalid(message: &str) -> ApiError {
    error("PROJECT_DATA_SNAPSHOT_INVALID", message)
}
fn changed() -> ApiError {
    error(
        "PROJECT_DATA_SNAPSHOT_CHANGED",
        "数据、附件或快照在准备期间变化；候选已保留，未替换当前数据或提交版本。",
    )
}
fn limit() -> ApiError {
    error(
        "PROJECT_DATA_SNAPSHOT_LIMIT",
        "数据库或附件超过本次内部快照预算；没有截断数据后继续。",
    )
}
fn cancelled() -> ApiError {
    error(
        "PROJECT_DATA_SNAPSHOT_CANCELLED",
        "数据快照已停止；未替换当前数据或提交版本。",
    )
}

#[cfg(test)]
#[path = "project_version_data_tests.rs"]
mod tests;
