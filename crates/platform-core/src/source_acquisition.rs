//! Internal, non-executing Git acquisition for the owned direct-run import task.
//! The enclosing import controls publication, registration and cleanup; there
//! is no independently retained source library or source task API.
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::{GitEnvironmentManager, GitToolchain},
    node_app_manifest::{TrialManifest, TrialRuntime},
    projects::{check_no_symlinks, checked_relative, valid_id, validate_manifest},
    ApiError, ProjectManifest, ProjectStore, RuntimePaths,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
    time::Instant,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

#[path = "source_acquisition_process.rs"]
mod process;
#[path = "source_publication.rs"]
mod publication;
#[cfg(test)]
use process::SourceHooks;
use process::{execute, materialize, resolve_source_git, SourceScope};

const TIMEOUT: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 10_000;
const MAX_BLOB: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 256 * 1024 * 1024;
const MAX_DISK: u64 = 512 * 1024 * 1024;
const MIN_FREE: u64 = 64 * 1024 * 1024;
const MAX_METADATA: u64 = 256 * 1024;
const MAX_TREE_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireSourceInput {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceInspection {
    pub kind: String,
    pub name: Option<String>,
    pub next_step: String,
    pub execution_state: String,
    #[serde(default)]
    pub requires_build: bool,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceRecord {
    schema_version: u32,
    source_id: String,
    owner_app_id: String,
    task_id: String,
    name: String,
    status: String,
    phase: String,
    url: String,
    #[serde(default)]
    branch: Option<String>,
    provider: String,
    created_at: u64,
    completed_at: Option<u64>,
    repository: String,
    shallow: bool,
    git: Option<ExecutableInfo>,
    commit: Option<String>,
    file_count: Option<usize>,
    total_bytes: Option<u64>,
    content_digest: Option<String>,
    inspection: Option<SourceInspection>,
    error: Option<ApiError>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCandidate {
    pub source_id: String,
    pub task_id: String,
    pub name: String,
    pub status: String,
    pub phase: String,
    pub url: String,
    pub branch: Option<String>,
    pub provider: String,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub source_path: Option<PathBuf>,
    pub evidence_path: PathBuf,
    pub shallow: bool,
    pub git: Option<ExecutableInfo>,
    pub commit: Option<String>,
    pub file_count: Option<usize>,
    pub total_bytes: Option<u64>,
    pub content_digest: Option<String>,
    pub inspection: Option<SourceInspection>,
    pub error: Option<ApiError>,
}

pub struct SourceAcquisitionManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    git: GitEnvironmentManager,
    #[cfg(test)]
    hooks: Arc<SourceHooks>,
}
impl SourceAcquisitionManager {
    #[cfg(test)]
    pub(crate) fn pause_import_writer(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.hooks.writer.lock().unwrap() = Some((entered, release));
    }
    pub fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        git: GitEnvironmentManager,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            paths,
            store,
            tasks,
            git,
            #[cfg(test)]
            hooks: Arc::new(SourceHooks::default()),
        }))
    }
    fn root(&self) -> PathBuf {
        self.paths.workspace_root().join(".platform/sources")
    }
    fn slot(&self, id: &str) -> Result<PathBuf, ApiError> {
        if !source_id(id) {
            return Err(error("SOURCE_NOT_FOUND", "未找到该源码候选。"));
        }
        let path = self.root().join(id);
        check_no_symlinks(&path, false)?;
        if !fs::metadata(&path)?.is_dir() {
            return Err(invalid("源码候选目录无效。"));
        }
        Ok(path)
    }
    fn read_record(&self, id: &str) -> Result<SourceRecord, ApiError> {
        let value: serde_json::Value =
            serde_json::from_slice(&read_small(&self.slot(id)?.join("source.json"))?)
                .map_err(|_| invalid("源码候选记录无效，请保留现场。"))?;
        let record: SourceRecord =
            serde_json::from_value(value).map_err(|_| invalid("源码候选记录无效，请保留现场。"))?;
        if record.schema_version != 1
            || record.source_id != id
            || !valid_id(&record.task_id)
            || record.repository != "repository"
            || record
                .branch
                .as_deref()
                .is_some_and(|branch| validate_branch(branch).is_err())
            || !matches!(
                record.status.as_str(),
                "acquiring" | "ready" | "failed" | "cancelled" | "interrupted"
            )
        {
            return Err(invalid("源码候选记录身份不符，请保留现场。"));
        }
        Ok(record)
    }
    fn save_record(&self, record: &SourceRecord) -> Result<(), ApiError> {
        let path = self.slot(&record.source_id)?.join("source.json");
        if path.try_exists()? {
            ordinary_file(&path)?;
        }
        atomic_json(&path, record)
    }
    fn view(&self, record: SourceRecord) -> Result<SourceCandidate, ApiError> {
        let slot = self.slot(&record.source_id)?;
        let source_path = if record.status == "ready" {
            let payload = slot.join("repository");
            check_no_symlinks(&payload, false)?;
            if !fs::metadata(&payload)?.is_dir() {
                return Err(invalid("已获取的源码目录不可用，请保留候选记录。"));
            }
            Some(payload)
        } else {
            None
        };
        Ok(SourceCandidate {
            source_id: record.source_id,
            task_id: record.task_id,
            name: record.name,
            status: record.status,
            phase: record.phase,
            url: record.url,
            provider: record.provider,
            branch: record.branch,
            created_at: record.created_at,
            completed_at: record.completed_at,
            source_path,
            evidence_path: slot,
            shallow: record.shallow,
            git: record.git,
            commit: record.commit,
            file_count: record.file_count,
            total_bytes: record.total_bytes,
            content_digest: record.content_digest,
            inspection: record.inspection,
            error: record.error,
        })
    }
    /// The caller owns the exact source task lease for the entire use/copy of
    /// this verified candidate. Do not attempt a second transient reservation.
    pub(crate) fn verify_ready_leased(
        &self,
        id: &str,
        expected_digest: &str,
        task_id: &str,
    ) -> Result<SourceCandidate, ApiError> {
        if self.store.active_task(&format!("source:{id}")).as_deref() != Some(task_id) {
            return Err(error(
                "SOURCE_LEASE_REQUIRED",
                "操作尚未持有此源码候选的专属使用权限。",
            ));
        }
        self.verify_ready_contents(id, expected_digest)
    }
    fn verify_ready_contents(
        &self,
        id: &str,
        expected_digest: &str,
    ) -> Result<SourceCandidate, ApiError> {
        let record = self.read_record(id)?;
        if record.status != "ready" || record.content_digest.as_deref() != Some(expected_digest) {
            return Err(error(
                "SOURCE_CHANGED",
                "源码状态或摘要已变化，请重新获取并核对。",
            ));
        }
        let candidate = self.view(record)?;
        let path = candidate
            .source_path
            .as_ref()
            .ok_or_else(|| invalid("源码目录不可用。"))?;
        if directory_digest(path)? != expected_digest {
            return Err(error(
                "SOURCE_CHANGED",
                "候选源码已经改变，未执行接入。请重新获取或明确处理修改。",
            ));
        }
        Ok(candidate)
    }
    pub(crate) fn owner_app_id(&self, id: &str) -> Result<String, ApiError> {
        let record = self.read_record(id)?;
        if Uuid::parse_str(&record.owner_app_id).is_err() {
            return Err(invalid("源码项目归属无效。"));
        }
        Ok(record.owner_app_id)
    }
    /// The direct-run importer owns the task and instance before downloading.
    /// This entry never creates another task, registration, or instance ID.
    pub(crate) async fn acquire_for_import(
        &self,
        input: AcquireSourceInput,
        owner: &str,
        task_id: &str,
        source_id: &str,
        stop: watch::Receiver<bool>,
        cancel: watch::Receiver<bool>,
    ) -> Result<SourceCandidate, ApiError> {
        let (url, provider, repository_name) = validate_url(&input.url)?;
        if let Some(branch) = &input.branch {
            validate_branch(branch)?;
        }
        if !self::source_id(source_id) || Uuid::parse_str(owner).is_err() {
            return Err(invalid("导入来源身份无效。"));
        }
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        ownership.record_source(owner, source_id)?;
        self.tasks.record_node_source(task_id, owner, source_id)?;
        let lease = self
            .store
            .reserve_task(&format!("source:{source_id}"), task_id)?;
        private_dir(&self.root())?;
        new_dir(&self.root().join(source_id))?;
        ownership.record_source(owner, source_id)?;
        let anchor = fs::File::open(self.root().join(source_id))?;
        let scope = Arc::new(SourceScope::new(
            lease,
            self.tasks.clone(),
            task_id.into(),
            self.root().join(source_id),
            anchor,
        ));
        let mut record = SourceRecord {
            schema_version: 1,
            source_id: source_id.into(),
            owner_app_id: owner.into(),
            task_id: task_id.into(),
            name: input.name.unwrap_or(repository_name),
            status: "acquiring".into(),
            phase: "downloading".into(),
            url,
            branch: input.branch,
            provider,
            created_at: now(),
            completed_at: None,
            repository: "repository".into(),
            shallow: true,
            git: None,
            commit: None,
            file_count: None,
            total_bytes: None,
            content_digest: None,
            inspection: None,
            error: None,
        };
        self.save_record(&record)?;
        let mut control = RunControl {
            stop,
            cancel,
            deadline: Instant::now() + TIMEOUT,
            monitor: self.root().join(source_id),
            processes: self.store.processes().with_writer_owner(
                crate::durable_writer::WriterOwner::node(self.paths.data_root(), owner)?,
            ),
            scope: Some(scope.clone()),
            #[cfg(test)]
            hooks: self.hooks.clone(),
        };
        let acquired = self.acquire(&mut record, &mut control).await;
        scope.check_identity()?;
        record.completed_at = Some(now());
        match acquired {
            Ok(()) => {
                record.status = "ready".into();
                record.phase = "downloaded".into();
                self.save_record(&record)?;
                self.view(record)
            }
            Err(issue) => {
                record.status = "failed".into();
                record.error = Some(issue.clone());
                let _ = self.save_record(&record);
                Err(issue)
            }
        }
    }
    fn phase(&self, record: &mut SourceRecord, phase: &str) -> Result<(), ApiError> {
        record.phase = phase.into();
        self.save_record(record)?;
        self.tasks.phase(&record.task_id, phase)
    }
    async fn acquire(
        &self,
        record: &mut SourceRecord,
        control: &mut RunControl,
    ) -> Result<(), ApiError> {
        control.check()?;
        self.phase(record, "environment")?;
        let toolchain = resolve_source_git(&self.git, control).await?;
        if !supported_version(&toolchain.git.version) {
            return Err(error(
                "SOURCE_GIT_VERSION_UNSUPPORTED",
                "源码获取目前仅支持已核验的 Git 2.54.0 或 2.56.0。",
            ));
        }
        record.git = Some(toolchain.git.clone());
        self.save_record(record)?;
        let slot = self.slot(&record.source_id)?;
        let stage = slot.join(".acquiring");
        new_dir(&stage)?;
        let home = stage.join("home");
        new_dir(&home)?;
        let template = stage.join("empty-template");
        new_dir(&template)?;
        let repository = stage.join("repository");
        control.check()?;
        self.phase(record, "downloading")?;
        let mut clone = isolated_command(&toolchain, &home, &home, &template);
        clone
            .args([
                "clone",
                "--no-checkout",
                "--depth=1",
                "--single-branch",
                "--no-tags",
                "--no-local",
                "--no-recurse-submodules",
            ])
            .arg(format!("--template={}", template.display()));
        if let Some(branch) = &record.branch {
            clone.arg("--branch").arg(branch);
        }
        clone.arg("--").arg(&record.url).arg(&repository);
        execute(clone, control, MAX_OUTPUT).await?;
        if let Some(branch) = &record.branch {
            // --branch can also name a tag. Accept only an actual local branch
            // created from the explicitly requested remote branch, never a
            // detached tag/commit or a fallback to the remote default branch.
            let mut symbolic = isolated_command(&toolchain, &repository, &home, &template);
            symbolic.args(["symbolic-ref", "--quiet", "HEAD"]);
            let actual = execute(symbolic, control, MAX_OUTPUT)
                .await
                .map_err(|issue| {
                    if issue.code == "SOURCE_GIT_FAILED" {
                        error(
                            "SOURCE_BRANCH_UNAVAILABLE",
                            "未获取到指定分支，请核对公开仓库中的分支名称。",
                        )
                    } else {
                        issue
                    }
                })?;
            if actual != format!("refs/heads/{branch}\n").as_bytes() {
                return Err(error(
                    "SOURCE_BRANCH_UNAVAILABLE",
                    "获取结果与指定分支不一致，未发布源码候选。",
                ));
            }
        }
        control.check()?;
        self.phase(record, "validating-objects")?;
        let mut head = isolated_command(&toolchain, &repository, &home, &template);
        head.args(["rev-parse", "--verify", "HEAD^{commit}"]);
        let commit = parse_commit(&execute(head, control, MAX_OUTPUT).await?)?;
        let mut fsck = isolated_command(&toolchain, &repository, &home, &template);
        fsck.args(["fsck", "--full", "--strict", "--no-reflogs"]);
        execute(fsck, control, MAX_OUTPUT).await?;
        let mut tree = isolated_command(&toolchain, &repository, &home, &template);
        tree.args(["ls-tree", "-r", "-z", "--long", &commit]);
        let entries = parse_tree(&execute(tree, control, MAX_TREE_OUTPUT).await?)?;
        record.commit = Some(commit.clone());
        record.file_count = Some(entries.len());
        record.total_bytes = Some(entries.iter().map(|entry| entry.size).sum());
        control.check()?;
        self.phase(record, "materializing")?;
        record.content_digest =
            Some(materialize(&toolchain, &repository, &home, &template, &entries, control).await?);
        // Populate only Git's index. No checkout, attributes filters or project
        // scripts run, and later status sees the materialized tree accurately.
        let mut index = isolated_command(&toolchain, &repository, &home, &template);
        index.args(["read-tree", &commit]);
        execute(index, control, MAX_OUTPUT).await?;
        control.check()?;
        self.phase(record, "inspecting")?;
        control.check()?;
        record.inspection = Some(inspect_directory(&repository)?);
        check_disk(&slot)?;
        control.check()?;
        let published = slot.join("repository");
        if published.try_exists()? {
            return Err(invalid("候选目录已经存在，未覆盖。"));
        }
        fs::rename(&repository, &published)?;
        fs::File::open(&slot)?.sync_all()?;
        Ok(())
    }
}

fn error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn invalid(message: &str) -> ApiError {
    error("SOURCE_INVALID", message)
}
fn interrupted() -> ApiError {
    error(
        "SOURCE_ACQUISITION_INTERRUPTED",
        "服务已停止，源码获取中断；已保留本次现场，不会自动重试。",
    )
}
fn cancelled() -> ApiError {
    error(
        "SOURCE_ACQUISITION_CANCELLED",
        "源码获取已取消，已保留本次现场。",
    )
}
fn limit() -> ApiError {
    error(
        "SOURCE_LIMIT_EXCEEDED",
        "仓库超过本次源码获取的文件、大小、路径或磁盘预算。",
    )
}
fn process_failed() -> ApiError {
    error(
        "SOURCE_GIT_FAILED",
        "源码获取未成功；请检查公开仓库地址和网络，或重新发起获取。",
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn source_id(id: &str) -> bool {
    id.len() == 36 && id.starts_with("src_") && id[4..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn new_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    if !path.try_exists()? {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("预期为普通目录。"));
    }
    Ok(())
}
fn ordinary_file(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("仅支持普通且无链接的文件。"));
    }
    Ok(())
}
fn read_small(path: &Path) -> Result<Vec<u8>, ApiError> {
    ordinary_file(path)?;
    let before = fs::symlink_metadata(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.nlink() != 1
        || opened.dev() != before.dev()
        || opened.ino() != before.ino()
    {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    if opened.len() > MAX_METADATA {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    let after = fs::symlink_metadata(path)?;
    if after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != bytes.len() as u64
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    Ok(bytes)
}

fn directory_digest(root: &Path) -> Result<String, ApiError> {
    fn collect(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<String, PathBuf>,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 32 {
            return Err(limit());
        }
        check_no_symlinks(path, false)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if path == root && entry.file_name() == ".git" {
                continue;
            }
            let full = entry.path();
            let metadata = fs::symlink_metadata(&full)?;
            if metadata.is_dir() {
                collect(root, &full, files, depth + 1)?;
            } else {
                ordinary_file(&full)?;
                let relative = full
                    .strip_prefix(root)
                    .map_err(|_| invalid("源码路径越界。"))?
                    .to_str()
                    .ok_or_else(|| invalid("源码路径不是 UTF-8。"))?
                    .to_owned();
                validate_tree_path(&relative)?;
                if metadata.len() > MAX_BLOB || files.len() >= MAX_FILES {
                    return Err(limit());
                }
                files.insert(relative, full);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(root, root, &mut files, 0)?;
    let mut tree = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in files {
        let before = fs::symlink_metadata(&path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let opened = file.metadata()?;
        if !opened.is_file()
            || opened.nlink() != 1
            || opened.dev() != before.dev()
            || opened.ino() != before.ino()
        {
            return Err(invalid("源码在校验时改变。"));
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 65536];
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            total += n as u64;
            if size > MAX_BLOB || total > MAX_TREE {
                return Err(limit());
            }
            digest.update(&buffer[..n]);
        }
        let after = fs::symlink_metadata(&path)?;
        if after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.len() != size
            || after.mode() != before.mode()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
        {
            return Err(invalid("源码在校验时改变。"));
        }
        tree.update(relative.as_bytes());
        tree.update([0]);
        tree.update(size.to_le_bytes());
        tree.update([u8::from(before.mode() & 0o111 != 0)]);
        tree.update(digest.finalize());
    }
    Ok(format!("{:x}", tree.finalize()))
}
pub(crate) fn validate_url(value: &str) -> Result<(String, String, String), ApiError> {
    let bad = || {
        error(
            "SOURCE_URL_INVALID",
            "请输入标准 GitHub 或 Gitee 的公开 HTTPS 仓库地址，不包含凭据、端口、查询或额外路径。",
        )
    };
    let (host, provider, path) = if let Some(path) = value.strip_prefix("https://github.com/") {
        ("github.com", "github", path)
    } else if let Some(path) = value.strip_prefix("https://gitee.com/") {
        ("gitee.com", "gitee", path)
    } else {
        return Err(bad());
    };
    if value.len() > 1024 {
        return Err(bad());
    }
    let mut parts = path.strip_suffix('/').unwrap_or(path).split('/');
    let owner = parts.next().ok_or_else(bad)?;
    let repo = parts.next().ok_or_else(bad)?;
    let safe = |s: &str| {
        (1..=255).contains(&s.len())
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if parts.next().is_some() || !safe(owner) || !safe(repo) {
        return Err(bad());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe(repo) {
        return Err(bad());
    }
    Ok((
        format!("https://{host}/{owner}/{repo}.git"),
        provider.into(),
        repo.into(),
    ))
}
pub(crate) fn validate_branch(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || value == "HEAD"
        || value.starts_with("refs/")
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with('.')
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
        || value
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        || ([40, 64].contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(error(
            "SOURCE_BRANCH_INVALID",
            "请输入普通分支名称，例如 main 或 codex/tts-providers。",
        ));
    }
    Ok(())
}
fn supported_version(version: &str) -> bool {
    ["2.54.0", "2.56.0"].iter().any(|base| {
        version == *base
            || version
                .strip_prefix(base)
                .and_then(|s| s.strip_prefix(" (Apple Git-")?.strip_suffix(')'))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
    })
}
fn inspect_directory(path: &Path) -> Result<SourceInspection, ApiError> {
    if !path.is_absolute() {
        return Err(invalid("请选择绝对路径的源码目录。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("请选择普通源码目录。"));
    }
    let mut result = SourceInspection {
        kind: "needs-adaptation".into(),
        name: None,
        next_step: "review-and-adapt-source".into(),
        execution_state: "not-executed".into(),
        requires_build: false,
        issues: Vec::new(),
    };
    let project = path.join("project.json");
    let app = path.join("local-app.json");
    if project.try_exists()? && app.try_exists()? {
        result
            .issues
            .push(invalid("目录同时包含两种项目声明，需明确接入类型。"));
        return Ok(result);
    }
    if project.try_exists()? {
        let parsed = (|| {
            let manifest: ProjectManifest = serde_json::from_slice(&read_small(&project)?)
                .map_err(|_| invalid("project.json 不是有效项目声明。"))?;
            if !valid_id(&manifest.id) {
                return Err(invalid("项目声明 ID 无效。"));
            }
            validate_manifest(&manifest, &manifest.id, path)?;
            let source = checked_relative(path, &manifest.source_dir, false)?;
            for name in ["package.json", "package-lock.json"] {
                let _: serde_json::Value = serde_json::from_slice(&read_small(&source.join(name))?)
                    .map_err(|_| invalid("项目缺少有效的 npm 构建材料。"))?;
            }
            Ok::<_, ApiError>(manifest)
        })();
        match parsed {
            Ok(manifest) => {
                result.kind = "project-v1-source".into();
                result.name = Some(manifest.name);
                result.next_step = "review-project-source-before-integration".into();
            }
            Err(problem) => result
                .issues
                .push(error("SOURCE_DECLARATION_INVALID", &problem.message)),
        }
    } else if app.try_exists()? {
        let parsed = read_small(&app).and_then(|bytes| {
            // Retired data-contract/build-output declarations never gate the
            // one-directory runtime. Only the import's build kind is read.
            let declaration: serde_json::Value = serde_json::from_slice(&bytes)?;
            let requires_build = match declaration["development"]["build"]["kind"].as_str() {
                None | Some("none") => false,
                Some("npm") => true,
                Some(_) => return Err(invalid("构建方式无法识别，请让 AI 修改后重新添加。")),
            };
            let manifest = TrialManifest::parse_runtime(&bytes)
                .map_err(|_| invalid("应用声明不是有效的受支持格式。"))?;
            manifest
                .inspect(path)
                .map_err(|_| invalid("应用声明所需文件不完整。"))?;
            Ok((manifest, requires_build))
        });
        match parsed {
            Ok((manifest, requires_build)) if manifest.runtime == TrialRuntime::NodeService => {
                result.requires_build = requires_build;
                result.kind = "node-service-source".into();
                result.name = Some(manifest.app.name);
                result.next_step = "prepare-node-runtime-before-registration".into();
            }
            Ok(_) => result
                .issues
                .push(invalid("检测到应用包声明；本步不提供成品安装。")),
            Err(_) => result
                .issues
                .push(invalid("应用声明或所声明的文件不完整，需要适配后再接入。")),
        }
    } else {
        // Never inspect .env, configuration/credentials, database contents or an
        // arbitrary recursive local tree. These two public declarations suffice.
        for name in ["package.json", "package-lock.json"] {
            let file = path.join(name);
            if file.try_exists()?
                && read_small(&file)
                    .and_then(|bytes| {
                        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(ApiError::from)
                    })
                    .is_err()
            {
                result.issues.push(invalid("npm 声明文件无效或超限。"));
            }
        }
        result.issues.push(invalid(
            "尚无受支持的项目声明；源码获取不自动推断安装或启动方式。",
        ));
    }
    Ok(result)
}

#[derive(Clone)]
struct RunControl {
    stop: watch::Receiver<bool>,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
    monitor: PathBuf,
    processes: crate::owned_process::OwnedProcessRegistry,
    scope: Option<Arc<SourceScope>>,
    #[cfg(test)]
    hooks: Arc<SourceHooks>,
}
impl RunControl {
    fn check(&self) -> Result<(), ApiError> {
        if let Some(scope) = &self.scope {
            scope.check_identity()?;
        }
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            return Err(interrupted());
        }
        if *self.cancel.borrow() {
            return Err(cancelled());
        }
        if Instant::now() >= self.deadline {
            return Err(error(
                "SOURCE_ACQUISITION_TIMEOUT",
                "源码获取超过 10 分钟预算；已保留现场，可稍后重试。",
            ));
        }
        Ok(())
    }
}
async fn signal(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn supervise<T, F: std::future::Future<Output = Result<T, ApiError>>>(
    control: &mut RunControl,
    future: F,
) -> Result<T, ApiError> {
    control.check()?;
    tokio::pin!(future);
    let mut stop = control.stop.clone();
    let mut cancel = control.cancel.clone();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            biased;
            _=signal(&mut stop)=>return Err(interrupted()),
            _=signal(&mut cancel)=>return Err(cancelled()),
            _=tokio::time::sleep_until(control.deadline)=>return Err(error("SOURCE_ACQUISITION_TIMEOUT","源码获取超过 10 分钟预算；已保留现场，可稍后重试。")),
            result=&mut future=>return result,
            _=interval.tick()=>{control.check()?;check_disk(&control.monitor)?;}
        }
    }
}
fn check_disk(root: &Path) -> Result<(), ApiError> {
    fn scan(path: &Path, bytes: &mut u64, count: &mut usize) -> Result<(), ApiError> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => continue,
                Err(problem) => return Err(problem.into()),
            };
            *count += 1;
            if *count > 60_000 {
                return Err(limit());
            }
            if metadata.is_dir() {
                scan(&entry.path(), bytes, count)?;
            } else if metadata.is_file() && metadata.nlink() == 1 {
                *bytes = bytes.checked_add(metadata.len()).ok_or_else(limit)?;
                if *bytes > MAX_DISK {
                    return Err(limit());
                }
            } else {
                return Err(invalid("获取目录包含不支持的链接或特殊文件。"));
            }
        }
        Ok(())
    }
    check_no_symlinks(root, false)?;
    scan(root, &mut 0, &mut 0)?;
    if fs2::available_space(root)? < MIN_FREE {
        return Err(error(
            "SOURCE_SPACE_INSUFFICIENT",
            "磁盘可用空间不足，源码获取已停止。",
        ));
    }
    Ok(())
}
fn isolated_command(tools: &GitToolchain, cwd: &Path, home: &Path, template: &Path) -> Command {
    let mut command = Command::new(&tools.git.path);
    command
        .env_clear()
        .current_dir(cwd)
        .env("PATH", &tools.path)
        .env("LC_ALL", "C")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap_or(cwd))
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "https")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_SSH", "/usr/bin/false")
        .env("GIT_SSH_COMMAND", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_TEMPLATE_DIR", template)
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    if let Some(path) = &tools.exec_path {
        command.env("GIT_EXEC_PATH", path);
    }
    for setting in [
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=/usr/bin/false",
        "protocol.allow=never",
        "protocol.https.allow=always",
        "http.followRedirects=false",
        "http.emptyAuth=false",
        "http.proactiveAuth=none",
        "http.delegation=none",
        "http.extraHeader=",
        "http.proxy=",
        "http.sslVerify=true",
        "http.saveCookies=false",
        "core.fsmonitor=false",
        "core.attributesFile=/dev/null",
        "gc.auto=0",
        "maintenance.auto=false",
        "fetch.fsckObjects=true",
        "transfer.fsckObjects=true",
        "fetch.unpackLimit=1",
        "core.protectHFS=true",
        "core.protectNTFS=true",
        "submodule.recurse=false",
    ] {
        command.arg("-c").arg(setting);
    }
    command
        .arg("-c")
        .arg(format!("core.hooksPath={}", template.display()));
    command
}
async fn bounded(
    mut reader: impl AsyncRead + Unpin,
    maximum: usize,
    retain: bool,
) -> Result<Vec<u8>, ApiError> {
    let mut result = Vec::new();
    let mut count = 0usize;
    // Keep I/O buffers out of nested async state machines on worker stacks.
    let mut buffer = vec![0u8; 8192];
    loop {
        let n = reader
            .read(&mut buffer)
            .await
            .map_err(|_| process_failed())?;
        if n == 0 {
            break;
        }
        count = count.checked_add(n).ok_or_else(limit)?;
        if count > maximum {
            return Err(limit());
        }
        if retain {
            result.extend_from_slice(&buffer[..n]);
        }
    }
    Ok(result)
}
fn parse_commit(bytes: &[u8]) -> Result<String, ApiError> {
    let s = std::str::from_utf8(bytes).map_err(|_| invalid("Git 未返回有效提交。"))?;
    let oid = s
        .strip_suffix('\n')
        .ok_or_else(|| invalid("Git 未返回有效提交。"))?;
    if !valid_oid(oid) {
        return Err(invalid("Git 未返回有效提交。"));
    }
    Ok(oid.into())
}
fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && !oid.bytes().all(|b| b == b'0')
}
#[derive(Clone, Debug)]
struct TreeEntry {
    path: String,
    oid: String,
    size: u64,
    executable: bool,
}
fn parse_tree(bytes: &[u8]) -> Result<Vec<TreeEntry>, ApiError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(invalid("Git 文件树不完整。"));
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut ancestors = BTreeMap::<String, String>::new();
    let mut total = 0u64;
    for record in bytes[..bytes.len() - 1].split(|b| *b == 0) {
        let record = std::str::from_utf8(record).map_err(|_| invalid("仅支持 UTF-8 文件路径。"))?;
        let (header, path) = record
            .split_once('\t')
            .ok_or_else(|| invalid("Git 文件树无效。"))?;
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4
            || !matches!(fields[0], "100644" | "100755")
            || fields[1] != "blob"
            || !valid_oid(fields[2])
        {
            return Err(invalid("本步不获取符号链接、子模块或非普通文件。"));
        }
        let size = fields[3]
            .parse::<u64>()
            .map_err(|_| invalid("Git 文件大小无效。"))?;
        if size > MAX_BLOB {
            return Err(limit());
        }
        total = total.checked_add(size).ok_or_else(limit)?;
        if total > MAX_TREE || result.len() >= MAX_FILES {
            return Err(limit());
        }
        validate_tree_path(path)?;
        let mut prefix = String::new();
        for component in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let normalized = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if let Some(previous) = ancestors.insert(normalized.clone(), prefix.clone()) {
                if previous != prefix {
                    return Err(invalid("仓库路径在本机存在大小写或 Unicode 冲突。"));
                }
            }
            if prefix != path && seen.contains(&normalized) {
                return Err(invalid("仓库文件与目录路径冲突。"));
            }
        }
        let normalized = path.nfd().flat_map(char::to_lowercase).collect::<String>();
        if !seen.insert(normalized.clone())
            || ancestors
                .range(format!("{normalized}/")..)
                .next()
                .is_some_and(|(p, _)| p.starts_with(&format!("{normalized}/")))
        {
            return Err(invalid("仓库文件路径重复或冲突。"));
        }
        result.push(TreeEntry {
            path: path.into(),
            oid: fields[2].into(),
            size,
            executable: fields[0] == "100755",
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}
fn validate_tree_path(path: &str) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > 1024
        || path.split('/').count() > 32
        || path.chars().any(|c| c.is_control() || c == '\\')
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(invalid("仓库包含不安全或超限路径。"));
    }
    for part in path.split('/') {
        let folded = part
            .nfd()
            .filter(|c| !matches!(c, '\u{200c}' | '\u{200d}' | '\u{feff}'))
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || folded == ".git"
            || folded.starts_with("git~")
            || folded.contains(':')
        {
            return Err(invalid("仓库包含保留或不安全路径。"));
        }
    }
    Ok(())
}
#[cfg(test)]
#[path = "source_acquisition_tests.rs"]
mod tests;
