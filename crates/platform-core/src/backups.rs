//! Portable complete-project archives used by explicit export, import and copy.
//! The retired backup history (listing, retention and ZIP restore) is gone; these
//! archive primitives remain for the ZIP files a user explicitly exports.
//! The caller owns the project operation gate and
//! freezes business operations only while `prepare_snapshot` reads the live project.
//! Compression, verification and publication exclusively read the prepared snapshot.
pub use crate::archive_permissions::{ArchivePermissions, FileEntry};
use crate::{
    builds::{self, BuildProvenance, StarterProvenance},
    projects::{check_no_symlinks, checked_relative, validate_manifest},
    ApiError, ProjectHandle, ProjectManifest,
};
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const FORMAT_VERSION: u32 = 1;
// Parser resource guards, not public product quota promises. Tests can use smaller guards.
const MAX_ENTRIES: usize = 1_000_000;
const MAX_EXPANDED_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotState {
    Ready,
    Maintenance,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingBuild {
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Deliberately excludes local executable paths, build/task IDs and credentials.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableBuildProvenance {
    pub origin: String,
    pub completed_at: u64,
    pub digest_rule_version: u32,
    pub input_digest: String,
    pub artifact_digest: String,
    pub node_version: String,
    pub npm_version: String,
    pub sdk_version: Option<String>,
    pub entry_relative: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoredBuildRecord {
    pub schema_version: u32,
    pub build_id: String,
    pub provenance: PortableBuildProvenance,
}

#[derive(Clone, Debug)]
pub struct SnapshotOptions {
    pub backup_id: String,
    pub kind: String,
    pub note: Option<String>,
    pub snapshot_state: SnapshotState,
    pub pending_build: PendingBuild,
    pub build_provenance: Option<PortableBuildProvenance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format_version: u32,
    pub backup_id: String,
    pub project_id: String,
    pub created_at: String,
    pub kind: String,
    pub note: Option<String>,
    pub platform_version: String,
    pub sqlite_version: String,
    pub sdk_api_version: u32,
    pub has_artifact: bool,
    #[serde(default)]
    pub snapshot_state: SnapshotState,
    #[serde(default)]
    pub pending_build: PendingBuild,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_provenance: Option<PortableBuildProvenance>,
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub excluded_paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PreparedSnapshot {
    pub root: PathBuf,
    pub manifest: BackupManifest,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub manifest: BackupManifest,
    pub path: PathBuf,
    pub bytes: u64,
    pub database: DatabaseDiagnostics,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyViolation {
    pub table: String,
    pub row_id: Option<i64>,
    pub parent: String,
    pub constraint: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnostics {
    pub integrity_ok: bool,
    pub foreign_key_violations: Vec<ForeignKeyViolation>,
    pub foreign_key_violations_truncated: bool,
}
#[derive(Clone, Debug)]
pub struct ValidatedArchive {
    pub manifest: BackupManifest,
    pub project_manifest: ProjectManifest,
    pub database: DatabaseDiagnostics,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("BACKUP_INVALID", message)
}
fn zip_error(error: zip::result::ZipError) -> ApiError {
    invalid(error.to_string())
}
fn db_error(error: rusqlite::Error) -> ApiError {
    ApiError::new("DATABASE_INVALID", error.to_string())
}
fn clean_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.contains(['\\', '\0', ':'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(format!("Unsafe archive path: {path}")));
    }
    Ok(())
}
pub fn valid_backup_id(id: &str) -> bool {
    id.starts_with("backup_")
        && id.len() == 39
        && id[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn valid_project_id(id: &str) -> bool {
    crate::projects::valid_id(id)
}
fn safe_file(path: &Path) -> Result<File, ApiError> {
    check_no_symlinks(path, false)?;
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinks,
    )?;
    if !file.metadata()?.is_file() {
        return Err(invalid("Expected an ordinary file"));
    }
    Ok(file)
}
fn new_file(path: &Path) -> Result<File, ApiError> {
    check_no_symlinks(path, true)?;
    Ok(platform_native::filesystem::create_new_private_file(
        path, false,
    )?)
}
fn mkdir(path: &Path) -> Result<(), ApiError> {
    crate::project_file_io::ensure_directory(path)
}
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ApiError> {
    let mut output = new_file(path)?;
    output.write_all(&serde_json::to_vec_pretty(value)?)?;
    output.sync_all()?;
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    let file = safe_file(path)?;
    if file.metadata()?.len() > MAX_METADATA_BYTES {
        return Err(invalid("Metadata file is too large"));
    }
    Ok(serde_json::from_reader(file)?)
}
fn sha256_file(path: &Path) -> Result<(u64, String), ApiError> {
    let mut file = safe_file(path)?;
    let before = platform_native::tool_files::Facts::from_file(&file)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        digest.update(&buffer[..n]);
    }
    let after = platform_native::tool_files::Facts::capture(path)?;
    if before.len() != total || !before.same_modified_file(&after) || after.is_reparse() {
        return Err(ApiError::new(
            "BACKUP_SOURCE_CHANGED",
            "A project file changed during snapshot preparation",
        ));
    }
    Ok((total, format!("{:x}", digest.finalize())))
}

/// Exact owned roots only. In particular attachment folders called `.git`,
/// `node_modules`, `.cache` or `.platform` remain ordinary persistent content.
fn exclusions(manifest: &ProjectManifest) -> Vec<String> {
    let mut paths = vec![
        ".platform".into(),
        ".git".into(),
        ".hg".into(),
        ".svn".into(),
    ];
    for name in ["node_modules", ".git", ".hg", ".svn", ".vite", ".cache"] {
        paths.push(format!("{}/{name}", manifest.source_dir));
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        paths.push(format!("{}{suffix}", manifest.database));
    }
    paths.sort();
    paths
}
fn excluded(relative: &str, paths: &[String]) -> bool {
    paths.iter().any(|prefix| {
        relative == prefix
            || relative
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
#[derive(Debug, PartialEq, Eq)]
struct Inventory {
    files: Vec<FileEntry>,
    directories: Vec<String>,
}
fn inventory(root: &Path, excluded_paths: &[String]) -> Result<Inventory, ApiError> {
    fn visit(
        root: &Path,
        relative: &str,
        excluded_paths: &[String],
        result: &mut Inventory,
    ) -> Result<(), ApiError> {
        let current = root.join(relative);
        check_no_symlinks(&current, false)?;
        let mut entries = fs::read_dir(&current)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("Non UTF-8 filenames are unsupported"))?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            clean_path(&path)?;
            if excluded(&path, excluded_paths) {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() {
                result.directories.push(path.clone());
                visit(root, &path, excluded_paths, result)?;
            } else if ty.is_file() {
                let (bytes, sha256) = sha256_file(&root.join(&path))?;
                let mode = ArchivePermissions::capture(&fs::metadata(root.join(&path))?);
                result.files.push(FileEntry {
                    path,
                    bytes,
                    sha256,
                    mode,
                });
            } else {
                return Err(ApiError::new(
                    "BACKUP_UNSAFE_FILE",
                    format!("Only ordinary files and directories may be archived: {path}"),
                ));
            }
            if result.files.len() + result.directories.len() > MAX_ENTRIES {
                return Err(invalid("Too many project files"));
            }
        }
        Ok(())
    }
    let mut result = Inventory {
        files: vec![],
        directories: vec![],
    };
    visit(root, "", excluded_paths, &mut result)?;
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    result.directories.sort();
    Ok(result)
}
fn copy_file_verified(source: &Path, target: &Path, expected: &FileEntry) -> Result<(), ApiError> {
    mkdir(target.parent().unwrap())?;
    let mut input = safe_file(source)?;
    let mut output = new_file(target)?;
    let bytes = std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    expected.mode.apply(target)?;
    if bytes != expected.bytes || sha256_file(target)?.1 != expected.sha256 {
        return Err(ApiError::new(
            "BACKUP_SOURCE_CHANGED",
            "File changed while the immutable snapshot was copied",
        ));
    }
    Ok(())
}
fn ensure_space(parent: &Path, bytes: u64) -> Result<(), ApiError> {
    let free = fs2::available_space(parent)?;
    if free < bytes {
        return Err(ApiError::new(
            "SPACE_INSUFFICIENT",
            "Insufficient space for the complete temporary project copy",
        )
        .detail("requiredBytes", bytes)
        .detail("availableBytes", free));
    }
    Ok(())
}

/// Creates a closed, standalone SQLite file. No live WAL/SHM bytes are copied.
pub fn sqlite_snapshot(source: &Path, destination: &Path) -> Result<(), ApiError> {
    sqlite_snapshot_cancellable(source, destination, || Ok(()))
}

/// The same consistent SQLite copy with bounded-step cancellation for explicit
/// Git data capture. Existing archive callers retain their original behavior.
pub(crate) fn sqlite_snapshot_cancellable(
    source: &Path,
    destination: &Path,
    mut checkpoint: impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    checkpoint()?;
    for suffix in ["-wal", "-shm", "-journal"] {
        check_no_symlinks(
            &PathBuf::from(format!("{}{suffix}", source.display())),
            true,
        )?;
    }
    let source_file = safe_file(source)?;
    if platform_native::tool_files::Facts::from_file(&source_file)?.links() != 1 {
        return Err(invalid("Database hard links are unsupported"));
    }
    drop(source_file);
    let source_connection = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    source_connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(db_error)?;
    let created = new_file(destination)?;
    drop(created);
    let mut destination_connection = Connection::open_with_flags(
        destination,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    {
        let backup =
            Backup::new(&source_connection, &mut destination_connection).map_err(db_error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            checkpoint()?;
            match backup.step(256).map_err(db_error)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => (),
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => (),
            }
            if Instant::now() >= deadline {
                return Err(ApiError::new(
                    "BACKUP_DATABASE_BUSY",
                    "SQLite snapshot did not finish within the internal operation budget",
                ));
            }
        }
    }
    // A backup of a WAL database can retain the WAL journal-mode flag. Explicitly
    // close into DELETE mode so this copy never depends on companion files.
    destination_connection
        .pragma_update(None, "journal_mode", "DELETE")
        .map_err(db_error)?;
    destination_connection
        .close()
        .map_err(|(_, error)| db_error(error))?;
    source_connection
        .close()
        .map_err(|(_, error)| db_error(error))?;
    File::open(destination)?.sync_all()?;
    Ok(())
}

/// Mechanical integrity and relational diagnostics deliberately remain separate.
pub fn check_database(path: &Path) -> Result<DatabaseDiagnostics, ApiError> {
    drop(safe_file(path)?);
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    let integrity = connection
        .prepare("PRAGMA integrity_check")
        .map_err(db_error)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    if integrity != ["ok"] {
        return Err(
            ApiError::new("DATABASE_INTEGRITY_FAILED", "SQLite integrity_check failed")
                .detail("integrity", serde_json::json!(integrity)),
        );
    }
    let mut violations = vec![];
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    let mut truncated = false;
    while let Some(row) = rows.next().map_err(db_error)? {
        if violations.len() == 1000 {
            truncated = true;
            break;
        }
        violations.push(ForeignKeyViolation {
            table: row.get(0).map_err(db_error)?,
            row_id: row.get(1).map_err(db_error)?,
            parent: row.get(2).map_err(db_error)?,
            constraint: row.get(3).map_err(db_error)?,
        });
    }
    Ok(DatabaseDiagnostics {
        integrity_ok: true,
        foreign_key_violations: violations,
        foreign_key_violations_truncated: truncated,
    })
}

pub fn prepare_snapshot(
    project_root: &Path,
    project_manifest: &ProjectManifest,
    snapshot_dir: &Path,
    options: SnapshotOptions,
) -> Result<PreparedSnapshot, ApiError> {
    validate_manifest(project_manifest, &project_manifest.id, project_root)?;
    if !valid_backup_id(&options.backup_id) {
        return Err(invalid("Invalid backup ID"));
    }
    if snapshot_dir.exists() {
        return Err(invalid("Snapshot directory already exists"));
    }
    check_no_symlinks(snapshot_dir, true)?;
    let excluded_paths = exclusions(project_manifest);
    let mut live_exclusions = excluded_paths.clone();
    live_exclusions.push(project_manifest.database.clone());
    let before = inventory(project_root, &live_exclusions)?;
    let required = before.files.iter().try_fold(
        fs::metadata(project_root.join(&project_manifest.database))?.len(),
        |total, file| {
            total
                .checked_add(file.bytes)
                .ok_or_else(|| invalid("Project size overflow"))
        },
    )?;
    let parent = snapshot_dir
        .parent()
        .ok_or_else(|| invalid("Snapshot requires a parent directory"))?;
    mkdir(parent)?;
    // Snapshot + worst-case ZIP + the independent verification candidate may
    // coexist. Include WAL bytes in the estimate although they are never copied.
    let wal = project_root.join(format!("{}-wal", project_manifest.database));
    check_no_symlinks(&wal, true)?;
    let estimated = required.saturating_add(fs::metadata(wal).map(|m| m.len()).unwrap_or(0));
    ensure_space(
        parent,
        estimated.saturating_mul(3).saturating_add(16 * 1024 * 1024),
    )?;
    mkdir(snapshot_dir)?;
    let result = (|| {
        let destination = snapshot_dir.join("project");
        mkdir(&destination)?;
        for directory in &before.directories {
            mkdir(&destination.join(directory))?;
        }
        for entry in &before.files {
            copy_file_verified(
                &project_root.join(&entry.path),
                &destination.join(&entry.path),
                entry,
            )?;
        }
        let db = destination.join(&project_manifest.database);
        mkdir(db.parent().unwrap())?;
        sqlite_snapshot(&project_root.join(&project_manifest.database), &db)?;
        if inventory(project_root, &live_exclusions)? != before {
            return Err(ApiError::new(
                "BACKUP_SOURCE_CHANGED",
                "Project files changed during snapshot preparation; pause external edits and retry",
            ));
        }
        let copied: ProjectManifest = read_json(&destination.join("project.json"))?;
        if serde_json::to_value(&copied)? != serde_json::to_value(project_manifest)? {
            return Err(ApiError::new(
                "BACKUP_SOURCE_CHANGED",
                "Project manifest changed before the snapshot",
            ));
        }
        let files = inventory(&destination, &[])?;
        let has_artifact = destination.join(&project_manifest.entry).is_file();
        if options.build_provenance.is_some() && !has_artifact {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_MISSING",
                "A registered webpage is missing",
            ));
        }
        let manifest = BackupManifest {
            format_version: FORMAT_VERSION,
            backup_id: options.backup_id,
            project_id: project_manifest.id.clone(),
            created_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|e| invalid(e.to_string()))?,
            kind: options.kind,
            note: options.note,
            platform_version: crate::APP_VERSION.into(),
            sqlite_version: rusqlite::version().into(),
            sdk_api_version: project_manifest.sdk_api_version,
            has_artifact,
            snapshot_state: options.snapshot_state,
            pending_build: options.pending_build,
            build_provenance: options.build_provenance,
            files: files.files,
            directories: files.directories,
            excluded_paths,
        };
        write_json(&snapshot_dir.join("backup.json"), &manifest)?;
        sync_directories(&destination, &manifest.directories)?;
        platform_native::filesystem::sync_directory(snapshot_dir)?;
        Ok(PreparedSnapshot {
            root: snapshot_dir.to_owned(),
            manifest,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(snapshot_dir);
    }
    result
}

fn sync_directories(root: &Path, directories: &[String]) -> Result<(), ApiError> {
    for path in directories.iter().rev() {
        platform_native::filesystem::sync_directory(&root.join(path))?;
    }
    platform_native::filesystem::sync_directory(root)?;
    Ok(())
}

pub fn publish_snapshot(
    snapshot: &PreparedSnapshot,
    archive_path: &Path,
) -> Result<BackupInfo, ApiError> {
    let parent = archive_path
        .parent()
        .ok_or_else(|| invalid("Archive requires a parent directory"))?;
    mkdir(parent)?;
    if archive_path.exists() {
        return Err(ApiError::new(
            "BACKUP_CONFLICT",
            "An archive already exists at this destination",
        ));
    }
    let total = snapshot.manifest.files.iter().try_fold(0u64, |n, f| {
        n.checked_add(f.bytes)
            .ok_or_else(|| invalid("Archive size overflow"))
    })?;
    ensure_space(
        parent,
        total.saturating_mul(2).saturating_add(16 * 1024 * 1024),
    )?;
    let temporary = parent.join(format!(
        ".{}.{}.partial",
        snapshot.manifest.backup_id,
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let project = snapshot.root.join("project");
        let actual = inventory(&project, &[])?;
        if actual.files != snapshot.manifest.files
            || actual.directories != snapshot.manifest.directories
        {
            return Err(invalid("Prepared snapshot changed before compression"));
        }
        validate_project(&project, &snapshot.manifest)?;
        let mut zip = ZipWriter::new(new_file(&temporary)?);
        let compressed_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        let file_options = compressed_options.unix_permissions(0o600);
        zip.start_file("backup.json", file_options)
            .map_err(zip_error)?;
        zip.write_all(&serde_json::to_vec_pretty(&snapshot.manifest)?)?;
        zip.add_directory(
            "project/",
            SimpleFileOptions::default().unix_permissions(0o700),
        )
        .map_err(zip_error)?;
        for directory in &snapshot.manifest.directories {
            zip.add_directory(
                format!("project/{directory}/"),
                SimpleFileOptions::default().unix_permissions(0o700),
            )
            .map_err(zip_error)?;
        }
        for entry in &snapshot.manifest.files {
            zip.start_file(
                format!("project/{}", entry.path),
                entry
                    .mode
                    .unix_mode()
                    .map(|mode| file_options.unix_permissions(mode))
                    .unwrap_or(compressed_options),
            )
            .map_err(zip_error)?;
            let mut input = safe_file(&project.join(&entry.path))?;
            std::io::copy(&mut input, &mut zip)?;
        }
        zip.finish().map_err(zip_error)?.sync_all()?;
        let mut verified = read_info(&temporary)?;
        // Atomic create-if-absent publication. A same-ID history file is never overwritten.
        fs::hard_link(&temporary, archive_path)?;
        fs::remove_file(&temporary)?;
        platform_native::filesystem::sync_directory(parent)?;
        verified.path = archive_path.to_owned();
        Ok(verified)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Check all ZIP paths before any extraction. Canonicalized path components catch
/// case/Unicode collisions, including implicit parent directories on macOS.
fn inspect_zip(zip: &mut ZipArchive<File>) -> Result<BackupManifest, ApiError> {
    if zip.len() > MAX_ENTRIES {
        return Err(invalid("Archive contains too many entries"));
    }
    let mut exact = HashSet::new();
    let mut identities: HashMap<String, (String, bool)> = HashMap::new();
    let mut total = 0u64;
    let mut manifest_index = None;
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(zip_error)?;
        let raw =
            std::str::from_utf8(file.name_raw()).map_err(|_| invalid("ZIP names must be UTF-8"))?;
        let path = raw.strip_suffix('/').unwrap_or(raw);
        clean_path(path)?;
        if !exact.insert(path.to_owned()) {
            return Err(invalid(format!("Duplicate archive entry: {path}")));
        }
        let directory = file.is_dir();
        if let Some(mode) = file.unix_mode() {
            let ty = mode & 0o170000;
            if ![0, 0o100000, 0o040000].contains(&ty)
                || (ty == 0o040000 && !directory)
                || (ty == 0o100000 && directory)
            {
                return Err(invalid(format!(
                    "Only ordinary files and directories are permitted: {path}"
                )));
            }
        }
        let pieces: Vec<_> = path.split('/').collect();
        for count in 1..=pieces.len() {
            let prefix = pieces[..count].join("/");
            let key = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            let is_dir = count < pieces.len() || directory;
            if let Some((existing, existing_dir)) = identities.get(&key) {
                if existing != &prefix || existing_dir != &is_dir {
                    return Err(invalid(format!("Conflicting archive path: {path}")));
                }
            } else {
                identities.insert(key, (prefix, is_dir));
            }
        }
        if path == "backup.json" && !directory {
            manifest_index = Some(index);
        } else if path != "project" && !path.starts_with("project/") {
            return Err(invalid("Unexpected archive root entry"));
        }
        if directory && file.size() != 0 {
            return Err(invalid("Directory entries cannot contain bytes"));
        }
        total = total
            .checked_add(file.size())
            .ok_or_else(|| invalid("Archive size overflow"))?;
        if total > MAX_EXPANDED_BYTES {
            return Err(invalid(
                "Expanded archive exceeds the internal resource guard",
            ));
        }
    }
    let mut metadata = zip
        .by_index(manifest_index.ok_or_else(|| invalid("Archive is missing backup.json"))?)
        .map_err(zip_error)?;
    if metadata.size() > MAX_METADATA_BYTES {
        return Err(invalid(
            "Archive metadata exceeds the internal resource guard",
        ));
    }
    let mut data = vec![];
    metadata
        .by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > MAX_METADATA_BYTES {
        return Err(invalid("Archive metadata is too large"));
    }
    let manifest: BackupManifest =
        serde_json::from_slice(&data).map_err(|e| invalid(e.to_string()))?;
    validate_backup_manifest(&manifest)?;
    Ok(manifest)
}
fn validate_backup_manifest(manifest: &BackupManifest) -> Result<(), ApiError> {
    if manifest.format_version != FORMAT_VERSION || manifest.sdk_api_version != crate::API_VERSION {
        return Err(ApiError::new(
            "BACKUP_VERSION_UNSUPPORTED",
            "Unsupported archive format or SDK API major version",
        ));
    }
    if !valid_backup_id(&manifest.backup_id) || !valid_project_id(&manifest.project_id) {
        return Err(invalid("Invalid archive identity"));
    }
    if manifest.files.len() + manifest.directories.len() > MAX_ENTRIES {
        return Err(invalid("Too many manifest entries"));
    }
    let mut paths = HashSet::new();
    let mut total = 0u64;
    for entry in &manifest.files {
        clean_path(&entry.path)?;
        if !paths.insert(entry.path.clone()) || !valid_digest(&entry.sha256) || !entry.mode.valid()
        {
            return Err(invalid("Invalid or duplicate file manifest entry"));
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or_else(|| invalid("Manifest size overflow"))?;
    }
    if total > MAX_EXPANDED_BYTES {
        return Err(invalid("Manifest exceeds the expanded size guard"));
    }
    for directory in &manifest.directories {
        clean_path(directory)?;
        if !paths.insert(directory.clone()) {
            return Err(invalid("Duplicate manifest path"));
        }
    }
    if let Some(provenance) = &manifest.build_provenance {
        validate_portable(provenance)?;
        if !manifest.has_artifact {
            return Err(invalid(
                "Unbuilt archive cannot declare published provenance",
            ));
        }
    }
    Ok(())
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// zip 2.x indexes central records by name, so duplicate names would otherwise be
/// silently collapsed before `by_index` sees them. Inspect the raw directory first,
/// also bounding the advertised count before the ZIP library allocates its index.
fn open_archive(path: &Path) -> Result<ZipArchive<File>, ApiError> {
    let mut file = safe_file(path)?;
    let length = file.metadata()?.len();
    let tail_length = length.min(65535 + 22) as usize;
    file.seek(SeekFrom::End(-(tail_length as i64)))?;
    let mut tail = vec![0; tail_length];
    file.read_exact(&mut tail)?;
    let u16_at = |bytes: &[u8], offset: usize| {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    };
    let u32_at = |bytes: &[u8], offset: usize| {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    };
    let u64_at = |bytes: &[u8], offset: usize| {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    };
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&index| {
            tail[index..].starts_with(b"PK\x05\x06")
                && index + 22 + u16_at(&tail, index + 20) as usize == tail.len()
        })
        .ok_or_else(|| invalid("ZIP end-of-directory record is missing"))?;
    let eocd = &tail[end..];
    if u16_at(eocd, 4) != 0 || u16_at(eocd, 6) != 0 || u16_at(eocd, 8) != u16_at(eocd, 10) {
        return Err(invalid("Multi-disk ZIP archives are unsupported"));
    }
    let end_position = length - tail_length as u64 + end as u64;
    let mut count = u16_at(eocd, 10) as u64;
    let mut size = u32_at(eocd, 12) as u64;
    let mut offset = u32_at(eocd, 16) as u64;
    if count == u16::MAX as u64 || size == u32::MAX as u64 || offset == u32::MAX as u64 {
        if end_position < 20 {
            return Err(invalid("ZIP64 locator is missing"));
        }
        file.seek(SeekFrom::Start(end_position - 20))?;
        let mut locator = [0; 20];
        file.read_exact(&mut locator)?;
        if &locator[..4] != b"PK\x06\x07" || u32_at(&locator, 4) != 0 || u32_at(&locator, 16) != 1 {
            return Err(invalid("Invalid ZIP64 locator"));
        }
        let zip64_position = u64_at(&locator, 8);
        if zip64_position
            .checked_add(56)
            .is_none_or(|end| end > end_position - 20)
        {
            return Err(invalid("Invalid ZIP64 directory position"));
        }
        file.seek(SeekFrom::Start(zip64_position))?;
        let mut zip64 = [0; 56];
        file.read_exact(&mut zip64)?;
        if &zip64[..4] != b"PK\x06\x06"
            || u64_at(&zip64, 4) < 44
            || u32_at(&zip64, 16) != 0
            || u32_at(&zip64, 20) != 0
            || u64_at(&zip64, 24) != u64_at(&zip64, 32)
        {
            return Err(invalid("Invalid ZIP64 directory"));
        }
        count = u64_at(&zip64, 32);
        size = u64_at(&zip64, 40);
        offset = u64_at(&zip64, 48);
    }
    if count > MAX_ENTRIES as u64
        || offset
            .checked_add(size)
            .is_none_or(|end| end > end_position)
    {
        return Err(invalid(
            "Archive directory exceeds its internal size/count guard",
        ));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut names = HashSet::new();
    for _ in 0..count {
        let mut header = [0; 46];
        file.read_exact(&mut header)
            .map_err(|_| invalid("Truncated ZIP directory"))?;
        if &header[..4] != b"PK\x01\x02" {
            return Err(invalid("Invalid ZIP central directory record"));
        }
        let name_len = u16_at(&header, 28) as usize;
        let skip = u16_at(&header, 30) as u64 + u16_at(&header, 32) as u64;
        let mut raw_name = vec![0; name_len];
        file.read_exact(&mut raw_name)?;
        let name =
            std::str::from_utf8(&raw_name).map_err(|_| invalid("ZIP names must be UTF-8"))?;
        let path = name.strip_suffix('/').unwrap_or(name);
        clean_path(path)?;
        if !names.insert(path.to_owned()) {
            return Err(invalid(format!(
                "Duplicate ZIP central-directory path: {path}"
            )));
        }
        let position = file
            .stream_position()?
            .checked_add(skip)
            .ok_or_else(|| invalid("ZIP directory offset overflow"))?;
        if position > offset + size {
            return Err(invalid("ZIP directory record exceeds its declared size"));
        }
        file.seek(SeekFrom::Start(position))?;
    }
    if file.stream_position()? != offset + size {
        return Err(invalid("ZIP directory count and size disagree"));
    }
    let zip = ZipArchive::new(file).map_err(zip_error)?;
    if zip.len() as u64 != count || zip.offset() != 0 {
        return Err(invalid("Ambiguous ZIP file index or prepended data"));
    }
    Ok(zip)
}

/// Hash all file contents, including uncompressed sizes, and optionally write a
/// new candidate. No successful extraction can contain an unlisted file.
fn verify_files(
    zip: &mut ZipArchive<File>,
    manifest: &BackupManifest,
    destination: Option<&Path>,
) -> Result<(), ApiError> {
    let mut expected: BTreeMap<&str, &FileEntry> = manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let directories: HashSet<&str> = manifest.directories.iter().map(String::as_str).collect();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(zip_error)?;
        let name = file.name().to_owned();
        if name == "backup.json" || name == "project/" {
            continue;
        }
        let relative = name
            .strip_prefix("project/")
            .ok_or_else(|| invalid("Unexpected archive entry"))?;
        if file.is_dir() {
            let directory = relative.strip_suffix('/').unwrap();
            if !directories.contains(directory) && !manifest.directories.is_empty() {
                return Err(invalid("Unlisted archive directory"));
            }
            if let Some(target) = destination {
                mkdir(&target.join(directory))?;
            }
            continue;
        }
        let entry = expected
            .remove(relative)
            .ok_or_else(|| invalid(format!("Unlisted archive file: {relative}")))?;
        if file.size() != entry.bytes {
            return Err(invalid(format!("Archive size mismatch: {relative}")));
        }
        let mut output = if let Some(root) = destination {
            let target = root.join(relative);
            mkdir(target.parent().unwrap())?;
            Some(new_file(&target)?)
        } else {
            None
        };
        let mut digest = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = file
                .read(&mut buffer)
                .map_err(|e| invalid(format!("Corrupt ZIP payload: {e}")))?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or_else(|| invalid("Entry size overflow"))?;
            if total > entry.bytes {
                return Err(invalid("Expanded entry exceeds its declared size"));
            }
            digest.update(&buffer[..n]);
            if let Some(output) = &mut output {
                output.write_all(&buffer[..n])?;
            }
        }
        if total != entry.bytes
            || format!("{:x}", digest.finalize()) != entry.sha256.to_ascii_lowercase()
        {
            return Err(invalid(format!("File checksum mismatch: {relative}")));
        }
        if let Some(output) = output {
            output.sync_all()?;
        }
        if let Some(root) = destination {
            entry.mode.apply(&root.join(relative))?;
        }
    }
    if !expected.is_empty() {
        return Err(invalid("Archive omits files promised by its manifest"));
    }
    if let Some(root) = destination {
        for directory in &manifest.directories {
            mkdir(&root.join(directory))?;
        }
    }
    Ok(())
}

/// Read validated archive metadata without unpacking or writing beside a
/// potentially read-only external ZIP. Import must still extract_validated
/// before registering any project; this only selects and reserves its identity.
pub(crate) fn inspect_manifest(archive: &Path) -> Result<BackupManifest, ApiError> {
    inspect_zip(&mut open_archive(archive)?)
}

pub fn read_info(archive: &Path) -> Result<BackupInfo, ApiError> {
    let bytes = fs::metadata(archive)?.len();
    let parent = archive
        .parent()
        .ok_or_else(|| invalid("Archive has no parent directory"))?;
    let candidate = parent.join(format!(".verification-{}", Uuid::new_v4().simple()));
    // A successful list/detail verification means the archived project is usable
    // as a restore candidate, not merely that somebody recomputed matching hashes.
    let result = extract_validated(archive, &candidate, None);
    let cleanup = if candidate.exists() {
        fs::remove_dir_all(&candidate)
    } else {
        Ok(())
    };
    let validated = result?;
    cleanup?;
    let manifest = validated.manifest;
    Ok(BackupInfo {
        manifest,
        path: archive.to_owned(),
        bytes,
        database: validated.database,
    })
}

pub fn extract_validated(
    archive: &Path,
    candidate: &Path,
    expected_project_id: Option<&str>,
) -> Result<ValidatedArchive, ApiError> {
    if candidate.exists() {
        return Err(invalid("Candidate directory must not already exist"));
    }
    check_no_symlinks(candidate, true)?;
    let mut zip = open_archive(archive)?;
    let manifest = inspect_zip(&mut zip)?;
    if expected_project_id.is_some_and(|id| id != manifest.project_id) {
        return Err(ApiError::new(
            "BACKUP_PROJECT_MISMATCH",
            "Selected backup belongs to a different project",
        ));
    }
    let bytes = manifest.files.iter().map(|f| f.bytes).sum::<u64>();
    let parent = candidate
        .parent()
        .ok_or_else(|| invalid("Candidate needs a parent directory"))?;
    mkdir(parent)?;
    ensure_space(parent, bytes.saturating_add(16 * 1024 * 1024))?;
    mkdir(candidate)?;
    let result = (|| {
        verify_files(&mut zip, &manifest, Some(candidate))?;
        let (project_manifest, database) = validate_project(candidate, &manifest)?;
        sync_directories(candidate, &manifest.directories)?;
        Ok(ValidatedArchive {
            manifest,
            project_manifest,
            database,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(candidate);
    }
    result
}

fn validate_project(
    root: &Path,
    archive: &BackupManifest,
) -> Result<(ProjectManifest, DatabaseDiagnostics), ApiError> {
    let manifest: ProjectManifest = read_json(&root.join("project.json"))?;
    validate_manifest(&manifest, &archive.project_id, root)?;
    if manifest.sdk_api_version != archive.sdk_api_version {
        return Err(invalid("Archive and project API versions differ"));
    }
    let excluded_paths = exclusions(&manifest);
    if archive
        .files
        .iter()
        .any(|file| excluded(&file.path, &excluded_paths))
        || archive
            .directories
            .iter()
            .any(|path| excluded(path, &excluded_paths))
    {
        return Err(invalid(
            "Archive includes excluded local control, dependency or database companion files",
        ));
    }
    if !checked_relative(root, &manifest.source_dir, false)?.is_dir()
        || !checked_relative(root, &manifest.attachments_dir, false)?.is_dir()
    {
        return Err(invalid(
            "Project source or attachments directory is missing",
        ));
    }
    for key in ["readme", "database", "platform"] {
        let path = manifest.docs[key]
            .as_str()
            .ok_or_else(|| invalid("Invalid project documentation path"))?;
        if !checked_relative(root, path, false)?.is_file() {
            return Err(invalid(format!(
                "Declared project documentation is missing: {path}"
            )));
        }
    }
    // M1 projects legitimately have no package yet. For a package that does exist,
    // check its lock and every declared vendored SDK/dependency without invoking npm.
    let source = root.join(&manifest.source_dir);
    if source.join("package.json").is_file() {
        let package: serde_json::Value = read_json(&source.join("package.json"))?;
        if !source.join("package-lock.json").is_file() {
            return Err(invalid("Project dependency lockfile is missing"));
        }
        let _: serde_json::Value = read_json(&source.join("package-lock.json"))?;
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            if let Some(dependencies) = package[section].as_object() {
                for value in dependencies.values().filter_map(serde_json::Value::as_str) {
                    if let Some(relative) = value.strip_prefix("file:") {
                        let clean = relative.strip_prefix("./").unwrap_or(relative);
                        if !checked_relative(&source, clean, false)?.exists() {
                            return Err(invalid("A fixed local package is missing"));
                        }
                    }
                }
            }
        }
    }
    let entry = root.join(&manifest.entry);
    if archive.has_artifact {
        if !entry.is_file() {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_MISSING",
                "Archive declares a webpage, but its entry is missing",
            ));
        }
        let entry_relative = Path::new(&manifest.entry)
            .strip_prefix(&manifest.output_dir)
            .map_err(|_| invalid("Invalid webpage entry"))?
            .to_str()
            .ok_or_else(|| invalid("Invalid entry encoding"))?;
        builds::validate_artifact(&root.join(&manifest.output_dir), entry_relative)?;
        if let Some(provenance) = &archive.build_provenance {
            if provenance.entry_relative != entry_relative
                || builds::tree_digest(&root.join(&manifest.output_dir), false)?
                    != provenance.artifact_digest
            {
                return Err(invalid(
                    "Portable build provenance does not match archived webpage",
                ));
            }
        }
    } else if entry.exists() || archive.build_provenance.is_some() {
        return Err(invalid("Unbuilt archive contradicts its contents"));
    }
    let diagnostics = check_database(&root.join(&manifest.database))?;
    Ok((manifest, diagnostics))
}

fn validate_portable(provenance: &PortableBuildProvenance) -> Result<(), ApiError> {
    if !matches!(
        provenance.origin.as_str(),
        "bundledTemplate" | "projectBuild"
    ) || provenance.digest_rule_version != 1
        || !valid_digest(&provenance.input_digest)
        || !valid_digest(&provenance.artifact_digest)
    {
        return Err(invalid("Invalid portable build provenance"));
    }
    clean_path(&provenance.entry_relative)?;
    Ok(())
}

pub fn capture_provenance(
    project: &ProjectHandle,
) -> Result<Option<PortableBuildProvenance>, ApiError> {
    let workspace = project
        .root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("Project workspace is missing"))?;
    let build_path = workspace
        .join(".platform/project-state/builds")
        .join(format!("{}.json", project.manifest.id));
    let restored_path = project.root.join(".platform/restored-build.json");
    let starter_path = project.root.join(".platform/starter.json");
    let (portable, build_id) = if build_path.exists() {
        let build: BuildProvenance = read_json(&build_path)?;
        (
            PortableBuildProvenance {
                origin: "projectBuild".into(),
                completed_at: build.completed_at,
                digest_rule_version: build.digest_rule_version,
                input_digest: build.input_digest,
                artifact_digest: build.artifact_digest,
                node_version: build.node.version,
                npm_version: build.npm.version,
                sdk_version: build.sdk_version,
                entry_relative: build.entry_relative,
                template_version: None,
            },
            build.build_id,
        )
    } else if restored_path.exists() {
        let restored: RestoredBuildRecord = read_json(&restored_path)?;
        if restored.schema_version != 1 {
            return Err(invalid("Invalid restored build provenance version"));
        }
        (restored.provenance, restored.build_id)
    } else if starter_path.exists() {
        let starter: StarterProvenance = read_json(&starter_path)?;
        if starter.schema_version != 1 || starter.origin != "bundledTemplate" {
            return Err(invalid("Invalid bundled starter provenance"));
        }
        (
            PortableBuildProvenance {
                origin: "bundledTemplate".into(),
                completed_at: starter.distribution.built_at,
                digest_rule_version: starter.digest_rule_version,
                input_digest: starter.input_digest,
                artifact_digest: starter.distribution.artifact_digest,
                node_version: starter.distribution.node_version,
                npm_version: starter.distribution.npm_version,
                sdk_version: Some(starter.distribution.sdk_version),
                entry_relative: starter.distribution.entry_relative,
                template_version: Some(starter.distribution.template_version),
            },
            starter.build_id,
        )
    } else {
        return Ok(None);
    };
    validate_portable(&portable)?;
    clean_path(&build_id)?;
    if !["task_", "starter_", "restored_"].iter().any(|prefix| {
        build_id
            .strip_prefix(prefix)
            .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
    }) {
        return Err(invalid("Invalid published generation identity"));
    }
    for root in [
        project.root.join(&project.manifest.output_dir),
        project
            .root
            .join(format!(".platform/artifacts/{build_id}/dist")),
    ] {
        if !root.is_dir() {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_MISSING",
                "Registered published artifact is missing",
            ));
        }
        if builds::tree_digest(&root, false)? != portable.artifact_digest {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_CHANGED",
                "Published artifact differs from registered provenance",
            ));
        }
    }
    Ok(Some(portable))
}

/// Rebuild only local generation state from validated portable provenance. This
/// creates no synthetic task record and never requires Node or runs project code.
pub fn restore_provenance(
    root: &Path,
    _workspace: &Path,
    manifest: &ProjectManifest,
    provenance: Option<&PortableBuildProvenance>,
) -> Result<Option<RestoredBuildRecord>, ApiError> {
    let Some(provenance) = provenance else {
        return Ok(None);
    };
    validate_portable(provenance)?;
    let output = root.join(&manifest.output_dir);
    if builds::tree_digest(&output, false)? != provenance.artifact_digest {
        return Err(invalid("Cannot restore provenance for a different webpage"));
    }
    let record = RestoredBuildRecord {
        schema_version: 1,
        build_id: format!("restored_{}", Uuid::new_v4().simple()),
        provenance: provenance.clone(),
    };
    let generation = root.join(format!(".platform/artifacts/{}/dist", record.build_id));
    mkdir(&generation)?;
    let tree = inventory(&output, &[])?;
    for directory in &tree.directories {
        mkdir(&generation.join(directory))?;
    }
    for entry in &tree.files {
        copy_file_verified(
            &output.join(&entry.path),
            &generation.join(&entry.path),
            entry,
        )?;
    }
    sync_directories(&generation, &tree.directories)?;
    write_json(&root.join(".platform/restored-build.json"), &record)?;
    platform_native::filesystem::sync_directory(&root.join(".platform"))?;
    Ok(Some(record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateProjectInput, ProjectStore};
    use serde_json::json;

    async fn fixture() -> (tempfile::TempDir, ProjectStore, ProjectHandle) {
        let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = ProjectStore::new(temporary.path().canonicalize().unwrap()).unwrap();
        let info = store
            .create(CreateProjectInput {
                name: "Disposable M3 archive test".into(),
                description: Some("Temporary automated test only".into()),
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        (temporary, store, project)
    }
    fn put(root: &Path, relative: &str, content: &[u8]) {
        fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
        fs::write(root.join(relative), content).unwrap();
    }
    fn options(project: &ProjectHandle) -> SnapshotOptions {
        SnapshotOptions {
            backup_id: format!("backup_{}", Uuid::new_v4().simple()),
            kind: "manual".into(),
            note: Some("test".into()),
            snapshot_state: SnapshotState::Ready,
            pending_build: PendingBuild::default(),
            build_provenance: capture_provenance(project).unwrap(),
        }
    }
    fn backup(temporary: &Path, project: &ProjectHandle) -> BackupInfo {
        let prepared = prepare_snapshot(
            &project.root,
            &project.manifest,
            &temporary.join(format!("snapshot-{}", Uuid::new_v4().simple())),
            options(project),
        )
        .unwrap();
        publish_snapshot(
            &prepared,
            &temporary
                .join("backups")
                .join(format!("{}.zip", prepared.manifest.backup_id)),
        )
        .unwrap()
    }
    fn add_starter(project: &ProjectHandle) {
        put(&project.root, "dist/index.html", b"<!doctype html><html><head><script src='./assets/app.js'></script></head><body>Welcome</body></html>");
        put(
            &project.root,
            "dist/assets/app.js",
            b"window.testStarter = true",
        );
        let build_id = format!("starter_{}", Uuid::new_v4().simple());
        let tree = inventory(&project.root.join("dist"), &[]).unwrap();
        let generation = project
            .root
            .join(format!(".platform/artifacts/{build_id}/dist"));
        mkdir(&generation).unwrap();
        for entry in &tree.files {
            copy_file_verified(
                &project.root.join("dist").join(&entry.path),
                &generation.join(&entry.path),
                entry,
            )
            .unwrap();
        }
        let record = StarterProvenance {
            schema_version: 1,
            origin: "bundledTemplate".into(),
            build_id,
            installed_at: 1234,
            digest_rule_version: 1,
            input_digest: builds::digest_project_inputs(&project.root, &project.manifest).unwrap(),
            distribution: builds::StarterDistribution {
                schema_version: 1,
                origin: "bundledTemplate".into(),
                template_version: "react-ts-vite-0.1.1".into(),
                sdk_version: "0.1.0".into(),
                source_digest: builds::tree_digest(&project.root.join("app"), true).unwrap(),
                artifact_digest: builds::tree_digest(&project.root.join("dist"), false).unwrap(),
                entry_relative: "index.html".into(),
                built_at: 123,
                node_version: "24.14.1".into(),
                npm_version: "11.11.0".into(),
            },
        };
        write_json(&project.root.join(".platform/starter.json"), &record).unwrap();
    }
    fn rewrite_zip(
        source: &Path,
        destination: &Path,
        extra: Option<(&str, &[u8])>,
        corrupt: Option<&str>,
    ) {
        let mut input = ZipArchive::new(File::open(source).unwrap()).unwrap();
        let mut writer = ZipWriter::new(File::create(destination).unwrap());
        for index in 0..input.len() {
            let mut entry = input.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .large_file(true);
            if entry.is_dir() {
                writer.add_directory(name, options).unwrap();
            } else {
                writer.start_file(&name, options).unwrap();
                if corrupt == Some(name.as_str()) {
                    writer.write_all(b"changed").unwrap();
                } else {
                    std::io::copy(&mut entry, &mut writer).unwrap();
                }
            }
        }
        if let Some((name, bytes)) = extra {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[tokio::test]
    async fn complete_roundtrip_preserves_hidden_material_and_path_owned_attachments() {
        let (temporary, _store, project) = fixture().await;
        for (path, bytes) in [
            (
                ".agents/skills/local-projects-app/SKILL.md",
                "persistent skill",
            ),
            (".custom-hidden", "persistent hidden material"),
            ("app/vendor/fixed-sdk.tgz", "fixed SDK bytes"),
            ("meta/icon.png", "invalid optional icon still preserved"),
            (
                "data/attachments/node_modules/file",
                "attachment dependency-named directory",
            ),
            (
                "data/attachments/.git/file",
                "attachment git-named directory",
            ),
            (
                "data/attachments/.platform/file",
                "attachment platform-named directory",
            ),
            (
                "data/attachments/.cache/file",
                "attachment cache-named directory",
            ),
            ("docs/node_modules/readme", "custom docs directory"),
            (
                "app/nested/.cache/source",
                "unrecognized nested source directory",
            ),
            ("app/node_modules/excluded", "dependency"),
            ("app/.vite/excluded", "cache"),
            (".git/excluded", "vcs"),
            (".platform/credentials", "local only"),
        ] {
            put(&project.root, path, bytes.as_bytes());
        }
        let backup = backup(temporary.path(), &project);
        let candidate = temporary.path().join("candidate");
        let validated =
            extract_validated(&backup.path, &candidate, Some(&project.manifest.id)).unwrap();
        assert!(!validated.manifest.has_artifact);
        assert_eq!(validated.manifest.snapshot_state, SnapshotState::Ready);
        for path in [
            ".agents/skills/local-projects-app/SKILL.md",
            ".custom-hidden",
            "app/vendor/fixed-sdk.tgz",
            "meta/icon.png",
            "data/attachments/node_modules/file",
            "data/attachments/.git/file",
            "data/attachments/.platform/file",
            "data/attachments/.cache/file",
            "docs/node_modules/readme",
            "app/nested/.cache/source",
        ] {
            assert_eq!(
                fs::read(candidate.join(path)).unwrap(),
                fs::read(project.root.join(path)).unwrap(),
                "{path}"
            );
        }
        for path in ["app/node_modules", "app/.vite", ".git", ".platform"] {
            assert!(!candidate.join(path).exists(), "{path}");
        }
        assert!(candidate.join("data/attachments").is_dir());
        assert!(validated.database.integrity_ok);
        assert!(validated.database.foreign_key_violations.is_empty());
    }

    #[tokio::test]
    async fn sqlite_backup_includes_uncheckpointed_wal_and_is_standalone() {
        let (temporary, _store, project) = fixture().await;
        let connection = Connection::open(project.root.join(&project.manifest.database)).unwrap();
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE notes(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO notes VALUES(1,'committed WAL');").unwrap();
        assert!(project.root.join("data/main.sqlite-wal").is_file());
        let archive = backup(temporary.path(), &project);
        let candidate = temporary.path().join("wal-candidate");
        extract_validated(&archive.path, &candidate, Some(&project.manifest.id)).unwrap();
        assert!(!candidate.join("data/main.sqlite-wal").exists());
        assert!(!candidate.join("data/main.sqlite-shm").exists());
        let restored = Connection::open_with_flags(
            candidate.join("data/main.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            restored
                .query_row("SELECT value FROM notes", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "committed WAL"
        );
        assert_eq!(
            restored
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        assert!(project.root.join("data/main.sqlite-wal").is_file());
    }

    #[tokio::test]
    async fn starter_provenance_survives_without_excluded_platform_state_or_node() {
        let (temporary, _store, project) = fixture().await;
        add_starter(&project);
        let archive = backup(temporary.path(), &project);
        let portable = archive.manifest.build_provenance.as_ref().unwrap();
        assert_eq!(portable.origin, "bundledTemplate");
        let portable_json = serde_json::to_string(portable).unwrap();
        assert!(
            !portable_json.contains("buildId")
                && !portable_json.contains("taskId")
                && !portable_json.contains("/Users/")
        );
        let candidate = temporary.path().join("starter-candidate");
        let valid =
            extract_validated(&archive.path, &candidate, Some(&project.manifest.id)).unwrap();
        assert!(!candidate.join(".platform").exists());
        let local = restore_provenance(
            &candidate,
            temporary.path(),
            &valid.project_manifest,
            valid.manifest.build_provenance.as_ref(),
        )
        .unwrap()
        .unwrap();
        assert!(local.build_id.starts_with("restored_"));
        assert!(candidate
            .join(format!(
                ".platform/artifacts/{}/dist/index.html",
                local.build_id
            ))
            .is_file());
        assert!(!candidate.join(".platform/starter.json").exists());
        assert_eq!(
            builds::tree_digest(&candidate.join("dist"), false).unwrap(),
            portable.artifact_digest
        );
    }

    #[tokio::test]
    async fn user_build_provenance_strips_absolute_paths_and_original_task_dependency() {
        let (temporary, _store, project) = fixture().await;
        add_starter(&project);
        let starter: StarterProvenance =
            read_json(&project.root.join(".platform/starter.json")).unwrap();
        let build_id = format!("task_{}", Uuid::new_v4().simple());
        fs::rename(
            project
                .root
                .join(format!(".platform/artifacts/{}", starter.build_id)),
            project.root.join(format!(".platform/artifacts/{build_id}")),
        )
        .unwrap();
        let build = BuildProvenance {
            schema_version: 1,
            build_id: build_id.clone(),
            task_id: build_id,
            completed_at: 77,
            digest_rule_version: 1,
            input_digest: starter.input_digest,
            artifact_digest: starter.distribution.artifact_digest,
            node: crate::environment::ExecutableInfo {
                path: PathBuf::from("/private/machine/node"),
                version: "24.14.1".into(),
            },
            npm: crate::environment::ExecutableInfo {
                path: PathBuf::from("/private/machine/npm"),
                version: "11.11.0".into(),
            },
            sdk_version: Some("0.1.0".into()),
            entry_relative: "index.html".into(),
        };
        let path = temporary
            .path()
            .join(".platform/project-state/builds")
            .join(format!("{}.json", project.manifest.id));
        mkdir(path.parent().unwrap()).unwrap();
        write_json(&path, &build).unwrap();
        let archive = backup(temporary.path(), &project);
        let portable = archive.manifest.build_provenance.unwrap();
        assert_eq!(portable.origin, "projectBuild");
        let data = serde_json::to_string(&portable).unwrap();
        assert!(!data.contains("/private/machine") && !data.contains("task_"));
        assert_eq!(portable.completed_at, 77);
    }

    #[tokio::test]
    async fn registered_artifact_missing_is_not_silently_treated_as_unbuilt() {
        let (_temporary, _store, project) = fixture().await;
        add_starter(&project);
        fs::remove_dir_all(project.root.join("dist")).unwrap();
        assert_eq!(
            capture_provenance(&project).unwrap_err().code,
            "BUILD_ARTIFACT_MISSING"
        );
    }

    #[tokio::test]
    async fn registered_artifact_digest_mismatch_is_rejected_before_snapshot() {
        let (_temporary, _store, project) = fixture().await;
        add_starter(&project);
        fs::write(
            project.root.join("dist/assets/app.js"),
            b"changed published bytes",
        )
        .unwrap();
        assert_eq!(
            capture_provenance(&project).unwrap_err().code,
            "BUILD_ARTIFACT_CHANGED"
        );
    }

    #[tokio::test]
    async fn detail_verification_rejects_bad_database_even_with_recomputed_checksums() {
        let (temporary, _store, project) = fixture().await;
        let archive = backup(temporary.path(), &project);
        let bad = temporary.path().join("bad-database.zip");
        let replacement = b"not a SQLite database";
        let mut metadata = archive.manifest.clone();
        let entry = metadata
            .files
            .iter_mut()
            .find(|entry| entry.path == "data/main.sqlite")
            .unwrap();
        entry.bytes = replacement.len() as u64;
        entry.sha256 = format!("{:x}", Sha256::digest(replacement));
        let mut input = open_archive(&archive.path).unwrap();
        let mut output = ZipWriter::new(File::create(&bad).unwrap());
        for index in 0..input.len() {
            let mut file = input.by_index(index).unwrap();
            let name = file.name().to_owned();
            if file.is_dir() {
                output
                    .add_directory(name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                output
                    .start_file(&name, SimpleFileOptions::default())
                    .unwrap();
                if name == "backup.json" {
                    output
                        .write_all(&serde_json::to_vec(&metadata).unwrap())
                        .unwrap();
                } else if name == "project/data/main.sqlite" {
                    output.write_all(replacement).unwrap();
                } else {
                    std::io::copy(&mut file, &mut output).unwrap();
                }
            }
        }
        output.finish().unwrap();
        assert_eq!(read_info(&bad).unwrap_err().code, "DATABASE_INVALID");
        assert!(fs::read_dir(temporary.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".verification-")));
    }

    #[tokio::test]
    async fn source_change_and_staging_write_failure_leave_original_intact() {
        let (temporary, _store, project) = fixture().await;
        let source = project.root.join("README.md");
        let (bytes, sha256) = sha256_file(&source).unwrap();
        let expected = FileEntry {
            path: "README.md".into(),
            bytes,
            sha256,
            mode: ArchivePermissions::Unix(0o600),
        };
        fs::write(&source, b"external edit").unwrap();
        assert_eq!(
            copy_file_verified(&source, &temporary.path().join("changed-copy"), &expected)
                .unwrap_err()
                .code,
            "BACKUP_SOURCE_CHANGED"
        );
        let destination = temporary.path().join("cannot-create/file/snapshot");
        fs::write(temporary.path().join("cannot-create"), b"existing file").unwrap();
        assert!(prepare_snapshot(
            &project.root,
            &project.manifest,
            &destination,
            options(&project)
        )
        .is_err());
        assert_eq!(fs::read(source).unwrap(), b"external edit");
        assert_eq!(
            fs::read(temporary.path().join("cannot-create")).unwrap(),
            b"existing file"
        );
        assert!(
            check_database(&project.root.join("data/main.sqlite"))
                .unwrap()
                .integrity_ok
        );
    }

    #[tokio::test]
    async fn corrupted_payload_traversal_aliases_and_unlisted_files_are_rejected() {
        let (temporary, _store, project) = fixture().await;
        let archive = backup(temporary.path(), &project);
        for (name, extra, corrupt) in [
            ("hash", None, Some("project/README.md")),
            (
                "traversal",
                Some(("project/../escape", b"bad".as_slice())),
                None,
            ),
            ("absolute", Some(("/escape", b"bad".as_slice())), None),
            (
                "backslash",
                Some(("project\\escape", b"bad".as_slice())),
                None,
            ),
            ("case", Some(("project/readme.md", b"bad".as_slice())), None),
            (
                "unlisted",
                Some(("project/unlisted", b"bad".as_slice())),
                None,
            ),
        ] {
            let bad = temporary.path().join(format!("{name}.zip"));
            rewrite_zip(&archive.path, &bad, extra, corrupt);
            let candidate = temporary.path().join(format!("{name}-candidate"));
            assert!(extract_validated(&bad, &candidate, None).is_err(), "{name}");
            assert!(!candidate.exists(), "{name}");
        }
        assert!(!temporary.path().join("escape").exists());
        assert!(project.root.join("README.md").is_file());
    }

    #[tokio::test]
    async fn symlink_source_is_rejected_but_excluded_dependency_symlinks_are_ignored() {
        let (temporary, _store, project) = fixture().await;
        mkdir(&project.root.join("app/node_modules")).unwrap();
        std::os::unix::fs::symlink(
            temporary.path(),
            project.root.join("app/node_modules/outside"),
        )
        .unwrap();
        backup(temporary.path(), &project);
        std::os::unix::fs::symlink(
            temporary.path(),
            project.root.join("data/attachments/outside"),
        )
        .unwrap();
        let destination = temporary.path().join("unsafe-snapshot");
        assert_eq!(
            prepare_snapshot(
                &project.root,
                &project.manifest,
                &destination,
                options(&project)
            )
            .unwrap_err()
            .code,
            "BACKUP_UNSAFE_FILE"
        );
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn zip_symlink_and_duplicate_and_unicode_paths_are_rejected() {
        let (temporary, _store, project) = fixture().await;
        let archive = backup(temporary.path(), &project);
        let symlink = temporary.path().join("symlink.zip");
        let mut writer = ZipWriter::new(File::create(&symlink).unwrap());
        writer
            .start_file("backup.json", SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&serde_json::to_vec(&archive.manifest).unwrap())
            .unwrap();
        writer
            .add_symlink("project/link", "/tmp", SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        assert!(read_info(&symlink).is_err());
        let duplicate = temporary.path().join("duplicate.zip");
        let readme = fs::read(project.root.join("README.md")).unwrap();
        rewrite_zip(
            &archive.path,
            &duplicate,
            Some(("project/READMX.md", &readme)),
            None,
        );
        let bytes = fs::read(&duplicate).unwrap();
        let mut rewritten = bytes.clone();
        for index in 0..bytes.len().saturating_sub(6) {
            if &bytes[index..index + 6] == b"READMX" {
                rewritten[index..index + 6].copy_from_slice(b"README");
            }
        }
        fs::write(&duplicate, rewritten).unwrap();
        assert!(read_info(&duplicate)
            .unwrap_err()
            .message
            .contains("Duplicate ZIP"));
        let unicode = temporary.path().join("unicode.zip");
        let mut writer = ZipWriter::new(File::create(&unicode).unwrap());
        for name in ["backup.json", "project/é/file", "project/e\u{301}/other"] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"{}").unwrap();
        }
        writer.finish().unwrap();
        assert!(read_info(&unicode).is_err());
    }

    #[tokio::test]
    async fn foreign_key_diagnostics_are_not_confused_with_database_integrity() {
        let (temporary, _store, project) = fixture().await;
        let connection = Connection::open(project.root.join("data/main.sqlite")).unwrap();
        connection.execute_batch("PRAGMA foreign_keys=OFF; CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent_id REFERENCES parent(id)); INSERT INTO child VALUES(7);").unwrap();
        drop(connection);
        let archive = backup(temporary.path(), &project);
        let valid =
            extract_validated(&archive.path, &temporary.path().join("fk-candidate"), None).unwrap();
        assert!(valid.database.integrity_ok);
        assert_eq!(valid.database.foreign_key_violations.len(), 1);
        assert_eq!(valid.database.foreign_key_violations[0].table, "child");
    }

    #[tokio::test]
    async fn corrupt_database_and_project_mismatch_leave_live_project_unchanged() {
        let (temporary, _store, project) = fixture().await;
        let archive = backup(temporary.path(), &project);
        let candidate = temporary.path().join("wrong-project");
        assert_eq!(
            extract_validated(&archive.path, &candidate, Some("other_project"))
                .unwrap_err()
                .code,
            "BACKUP_PROJECT_MISMATCH"
        );
        assert!(!candidate.exists());
        fs::write(project.root.join("data/main.sqlite"), b"not sqlite").unwrap();
        let root = temporary.path().join("broken-snapshot");
        assert!(
            prepare_snapshot(&project.root, &project.manifest, &root, options(&project)).is_err()
        );
        assert!(!root.exists());
        assert_eq!(
            fs::read(project.root.join("data/main.sqlite")).unwrap(),
            b"not sqlite"
        );
    }

    #[tokio::test]
    async fn snapshot_change_and_publication_conflict_never_replace_history() {
        let (temporary, _store, project) = fixture().await;
        let prepared = prepare_snapshot(
            &project.root,
            &project.manifest,
            &temporary.path().join("immutable"),
            options(&project),
        )
        .unwrap();
        let path = temporary.path().join("history.zip");
        let original = publish_snapshot(&prepared, &path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            publish_snapshot(&prepared, &path).unwrap_err().code,
            "BACKUP_CONFLICT"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(prepared.root.join("project/README.md"), b"mutation").unwrap();
        let other = temporary.path().join("unpublished.zip");
        assert!(publish_snapshot(&prepared, &other).is_err());
        assert!(!other.exists());
        assert_eq!(
            read_info(&path).unwrap().manifest.backup_id,
            original.manifest.backup_id
        );
    }

    #[tokio::test]
    async fn zip64_directory_is_supported_and_oversized_declared_count_is_rejected() {
        let (temporary, _store, project) = fixture().await;
        let archive = backup(temporary.path(), &project);
        let bytes = fs::read(&archive.path).unwrap();
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
        let count = u16::from_le_bytes(bytes[end + 10..end + 12].try_into().unwrap()) as u64;
        let size = u32::from_le_bytes(bytes[end + 12..end + 16].try_into().unwrap()) as u64;
        let offset = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as u64;
        let mut zip64 = bytes[..end].to_vec();
        zip64.extend_from_slice(b"PK\x06\x06");
        zip64.extend_from_slice(&44u64.to_le_bytes());
        zip64.extend_from_slice(&45u16.to_le_bytes());
        zip64.extend_from_slice(&45u16.to_le_bytes());
        zip64.extend_from_slice(&0u32.to_le_bytes());
        zip64.extend_from_slice(&0u32.to_le_bytes());
        zip64.extend_from_slice(&count.to_le_bytes());
        zip64.extend_from_slice(&count.to_le_bytes());
        zip64.extend_from_slice(&size.to_le_bytes());
        zip64.extend_from_slice(&offset.to_le_bytes());
        zip64.extend_from_slice(b"PK\x06\x07");
        zip64.extend_from_slice(&0u32.to_le_bytes());
        zip64.extend_from_slice(&(end as u64).to_le_bytes());
        zip64.extend_from_slice(&1u32.to_le_bytes());
        let mut footer = bytes[end..].to_vec();
        footer[8..12].fill(0xff);
        footer[12..20].fill(0xff);
        zip64.extend_from_slice(&footer);
        let path = temporary.path().join("zip64.zip");
        fs::write(&path, &zip64).unwrap();
        assert_eq!(
            read_info(&path).unwrap().manifest.backup_id,
            archive.manifest.backup_id
        );
        for range in [end + 24..end + 32, end + 32..end + 40] {
            zip64[range].copy_from_slice(&(MAX_ENTRIES as u64 + 1).to_le_bytes());
        }
        fs::write(&path, zip64).unwrap();
        assert!(read_info(&path)
            .unwrap_err()
            .message
            .contains("size/count guard"));
    }

    #[test]
    fn old_metadata_preserves_unknown_state_and_pending_build_independently() {
        let mut manifest = json!({"formatVersion":1,"backupId":format!("backup_{:032x}",1),"projectId":"old_project","createdAt":"2026-10-15T00:00:00Z","kind":"manual","note":null,"platformVersion":"0.0.0","sqliteVersion":"3","sdkApiVersion":1,"hasArtifact":false,"files":[],"pendingBuild":{"required":true,"reason":"interrupted edit"}});
        let parsed: BackupManifest = serde_json::from_value(manifest.clone()).unwrap();
        assert_eq!(parsed.snapshot_state, SnapshotState::Unknown);
        assert!(parsed.pending_build.required);
        manifest.as_object_mut().unwrap().remove("pendingBuild");
        let parsed: BackupManifest = serde_json::from_value(manifest).unwrap();
        assert_eq!(parsed.snapshot_state, SnapshotState::Unknown);
        assert!(!parsed.pending_build.required);
    }
}
