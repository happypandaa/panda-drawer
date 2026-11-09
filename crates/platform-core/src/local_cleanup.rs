//! Explicit pre-uninstall cleanup. Plans are private, short-lived Rust objects;
//! callers cannot submit paths or reconstruct deletion authority from JSON.
//! The desktop additionally proves strict application-window and backend shutdown.
use crate::{file_identity::PersistedIdentity, ApiError, RuntimePaths, RuntimeService};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[path = "local_cleanup_io.rs"]
mod io;

const TTL: Duration = Duration::from_secs(10 * 60);
const CONFIRMATION: &str = "删除全部本机数据";
const RETAINED: [&str; 2] = [".environment.json", "instance.lock"];

fn fail(message: impl Into<String>) -> ApiError {
    ApiError::new("LOCAL_CLEANUP_UNAVAILABLE", message).outcome("not_started")
}
fn changed() -> ApiError {
    ApiError::new(
        "LOCAL_CLEANUP_CHANGED",
        "清理范围或目录身份已变化，请重新核对两次确认；未继续删除。",
    )
    .outcome("not_started")
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn ordinary(path: &Path) -> Result<fs::Metadata, ApiError> {
    crate::projects::check_no_symlinks(path, false)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(fail("清理范围含链接或特殊文件，请先核对，未开始删除。"));
    }
    Ok(metadata)
}
fn read_small(path: &Path) -> Result<Vec<u8>, ApiError> {
    if !ordinary(path)?.is_file() {
        return Err(fail("清理控制记录不是普通文件。"));
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinks,
    )?;
    let mut bytes = Vec::new();
    file.take(32_000_001).read_to_end(&mut bytes)?;
    if bytes.len() > 32_000_000 {
        return Err(fail("清理控制记录过大，无法核对。"));
    }
    Ok(bytes)
}

struct BoundPath {
    path: PathBuf,
    identity: PersistedIdentity,
    directory: bool,
    parents: Vec<(PathBuf, PersistedIdentity)>,
}
struct WebOwnedFile {
    project_id: String,
    path: PathBuf,
    identity: PersistedIdentity,
    staging_path: Option<PathBuf>,
}
impl BoundPath {
    fn capture(path: PathBuf) -> Result<Self, ApiError> {
        let directory = ordinary(&path)?.is_dir();
        let identity = PersistedIdentity::capture(&path).map_err(|e| fail(e.to_string()))?;
        let parents = path
            .ancestors()
            .skip(1)
            .map(|parent| {
                Ok((
                    parent.to_path_buf(),
                    PersistedIdentity::capture(parent).map_err(|e| fail(e.to_string()))?,
                ))
            })
            .collect::<Result<_, ApiError>>()?;
        Ok(Self {
            path,
            identity,
            directory,
            parents,
        })
    }
    fn verify(&self) -> Result<(), ApiError> {
        if ordinary(&self.path)?.is_dir() != self.directory
            || !self.identity.matches_path(&self.path).unwrap_or(false)
            || self
                .parents
                .iter()
                .any(|(path, id)| !id.matches_path(path).unwrap_or(false))
        {
            return Err(changed());
        }
        Ok(())
    }
}

/// Single-use authority retained by the trusted desktop. No Deserialize, Clone,
/// public fields or arbitrary-root execution API is provided.
pub struct LocalCleanupPlan {
    id: String,
    instance_id: String,
    paths: RuntimePaths,
    created: Instant,
    expires_at: u64,
    root: BoundPath,
    lock: BoundPath,
    marker: BoundPath,
    targets: Vec<BoundPath>,
    workspace_roots: Vec<PathBuf>,
    metadata: BTreeMap<PathBuf, String>,
    preserved: Vec<PathBuf>,
    project_owned_paths: BTreeSet<PathBuf>,
    web_owned_files: Vec<WebOwnedFile>,
}

impl LocalCleanupPlan {
    /// Read-only. It does not create cleanup records, stop services or hash any
    /// project database, attachment or code file.
    pub fn prepare(paths: &RuntimePaths, runtime: &RuntimeService) -> Result<Self, ApiError> {
        crate::platform_support::require_runtime()?;
        crate::durable_writer::require_all_clear(paths.data_root())?;
        verify_backend_preview(paths)?;
        let info = runtime.info();
        if info.data_root != paths.data_root()
            || info.workspace_root != paths.workspace_root()
            || runtime.shutdown_status().attempt != 0
        {
            return Err(fail("当前服务与清理工作区不一致，或已进入关闭流程。"));
        }
        let workspace_roots = crate::workspace::local_cleanup_roots(paths)?;
        let root = BoundPath::capture(paths.data_root().into())?;
        let lock = BoundPath::capture(paths.data_root().join("instance.lock"))?;
        let marker = BoundPath::capture(paths.data_root().join(".environment.json"))?;
        if lock.directory || marker.directory {
            return Err(fail("固定应用身份或实例锁不是普通文件。"));
        }
        let (metadata, preserved) = scope_metadata(paths, &workspace_roots)?;
        let project_owned_paths = project_owned_paths(paths)?;
        let web_owned_files = web_owned_files(paths)?;
        let mut target_paths = BTreeSet::new();
        for entry in fs::read_dir(paths.data_root())? {
            let entry = entry?;
            if !RETAINED.iter().any(|name| entry.file_name() == *name) {
                target_paths.insert(entry.path());
            }
        }
        for path in &workspace_roots {
            safe_workspace_root(path, paths.data_root())?;
            if !path.starts_with(paths.data_root()) {
                target_paths.insert(path.clone());
            }
        }
        for path in &project_owned_paths {
            if path.exists() {
                target_paths.insert(path.clone());
            }
        }
        // A root already covered by another reviewed root needs no separate
        // traversal; every recorded workspace identity was validated above.
        let paths_all = target_paths.clone();
        target_paths.retain(|path| {
            !paths_all
                .iter()
                .any(|parent| parent != path && path.starts_with(parent))
        });
        let mut targets = target_paths
            .into_iter()
            .map(BoundPath::capture)
            .collect::<Result<Vec<_>, _>>()?;
        // Keep selection metadata until all business directories are removed.
        targets.sort_by_key(|target| {
            (
                target.path == paths.data_root().join("settings.json"),
                target.path.starts_with(paths.data_root()),
                ["app-ownership", "source-import-ownership"]
                    .iter()
                    .any(|name| target.path == paths.data_root().join(name)),
                target.path.clone(),
            )
        });
        for target in &targets {
            if target.path.starts_with(paths.data_root())
                && target.identity.device != root.identity.device
            {
                return Err(fail("固定应用数据目录含额外挂载点，未开始清理。"));
            }
        }
        let plan = Self {
            id: format!("local_cleanup_{}", Uuid::new_v4().simple()),
            instance_id: info.instance_id,
            paths: paths.clone(),
            created: Instant::now(),
            expires_at: now().saturating_add(TTL.as_millis() as u64),
            root,
            lock,
            marker,
            targets,
            workspace_roots,
            metadata,
            preserved,
            project_owned_paths,
            web_owned_files,
        };
        plan.verify_scope()?;
        Ok(plan)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn preview(&self) -> Value {
        json!({"planId":self.id,"instanceId":self.instance_id,"expiresAt":self.expires_at,
            "confirmationText":CONFIRMATION,"dataRoot":self.paths.data_root(),
            "workspaceRoot":self.paths.workspace_root(),"expiresInSeconds":TTL.as_secs(),
            "deletePaths":self.targets.iter().map(|target|json!({"path":target.path,"label":if target.path.starts_with(self.paths.data_root()) {"本机应用数据与项目资料"}else{"已登记的迁移工作区"}})).collect::<Vec<_>>(),
            "preservedPaths":self.preserved,"retainedControlPaths":RETAINED.iter().map(|name|self.paths.data_root().join(name)).collect::<Vec<_>>(),
            "warnings":["通常仅在卸载软件前使用；此操作会永久删除全部平台管理的本机项目数据和代码，不进入废纸篓，无法撤销。",
                "包括项目源码、Git 版本、数据库、附件、已安装应用、配置与密钥，以及备份、旧回收站和恢复资料。",
                "列出的受管目录会整体清空，其中自行放入或已取消登记的文件也会删除；需要保留的文件请先移出这些目录。",
                "外部原始源码目录、删除范围之外的独立导出文件和程序安装包保留；清理完成后仅保留不含项目内容的环境标记和原实例锁。",
                "项目 WebKit 网页存储会等待系统确认删除，管理器网页偏好也会清空；其他系统级应用缓存及偏好文件仍可能保留。",
                "请先保存并关闭外部编辑器、AI 和数据库工具；平台无法阻止其他程序继续写入这些目录。"]})
    }

    fn verify_scope(&self) -> Result<(), ApiError> {
        if self.created.elapsed() >= TTL {
            return Err(ApiError::new(
                "LOCAL_CLEANUP_EXPIRED",
                "清理预览已过期，请重新核对两次确认。",
            )
            .outcome("not_started"));
        }
        self.verify_controls()?;
        if crate::workspace::local_cleanup_roots(&self.paths)? != self.workspace_roots
            || scope_metadata(&self.paths, &self.workspace_roots)?.0 != self.metadata
        {
            return Err(changed());
        }
        let current = fs::read_dir(self.paths.data_root())?
            .map(|entry| Ok(entry?.path()))
            .collect::<Result<BTreeSet<_>, std::io::Error>>()?;
        let expected = self
            .targets
            .iter()
            .filter(|target| target.path.parent() == Some(self.paths.data_root()))
            .map(|target| target.path.clone())
            .chain([self.lock.path.clone(), self.marker.path.clone()])
            .collect::<BTreeSet<_>>();
        if current != expected {
            return Err(changed());
        }
        for target in &self.targets {
            target.verify()?;
        }
        for source in &self.preserved {
            let resolved = resolve_external(source)?;
            if self.targets.iter().any(|target| {
                [source, &resolved].iter().any(|source| {
                    source.starts_with(&target.path)
                        || (target.path.starts_with(source)
                            && !self.project_owned_paths.contains(&target.path))
                })
            }) {
                return Err(fail(
                    "外部原始源码与已登记工作区重叠，请先核对目录归属，未开始清理。",
                ));
            }
        }
        Ok(())
    }

    fn verify_controls(&self) -> Result<(), ApiError> {
        self.root.verify()?;
        self.lock.verify()?;
        self.marker.verify()?;
        // Deleting/replacing the original lock would let a second process
        // initialize another instance while this one is still removing data.
        let lock = platform_native::filesystem::open_read(
            &self.lock.path,
            platform_native::filesystem::ReadOptions::NoLinks,
        )?;
        if !self
            .lock
            .identity
            .same_runtime_object(&PersistedIdentity::from_file(&lock).map_err(|_| changed())?)
        {
            return Err(changed());
        }
        match fs2::FileExt::try_lock_exclusive(&lock) {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&lock);
                return Err(fail("平台实例锁不再由运行实例持有，未开始清理。"));
            }
            Err(error) if io::lock_is_contended(&error) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    /// The caller must additionally have joined strict native windows, backend
    /// processes/proxies and native sessions. Core checks its actual owned
    /// shutdown state, never a caller-supplied JSON report or boolean.
    pub fn execute_after_shutdown(self, runtime: &RuntimeService) -> Result<Value, ApiError> {
        self.execute_with(runtime, crate::web_file_deletion::remove_owned_file)
    }

    /// Check the same frozen disk authority before the desktop erases any
    /// native browser data. Native cleanup cannot bypass orphan writers or a
    /// changed user-reviewed scope simply because it lives outside this root.
    pub fn verify_before_native_cleanup(&self, runtime: &RuntimeService) -> Result<(), ApiError> {
        crate::durable_writer::require_all_clear(self.paths.data_root())?;
        crate::backend_run_marker::require_data_drained(self.paths.data_root())?;
        let info = runtime.info();
        if info.instance_id != self.instance_id
            || info.data_root != self.paths.data_root()
            || info.workspace_root != self.paths.workspace_root()
            || !runtime.shutdown_status().is_complete()
        {
            return Err(fail(
                "后台任务、数据库或文件工作尚未完成安全关闭，未开始删除。",
            ));
        }
        self.verify_scope()
            .map_err(|error| error.outcome("not_started"))
    }

    fn execute_with(
        self,
        runtime: &RuntimeService,
        mut remove_web_file: impl FnMut(
            &crate::web_project_ownership::WebProjectOwnership,
            &str,
            &Path,
            &PersistedIdentity,
        ) -> Result<(), ApiError>,
    ) -> Result<Value, ApiError> {
        crate::webview_storage::StoreLedger::new(self.paths.data_root().into())
            .require_all_clear()?;
        self.verify_before_native_cleanup(runtime)?;
        let result = (|| -> Result<(), ApiError> {
            let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            let mut removed_file_paths = BTreeSet::new();
            // Keep every owner index until all externally placed material has
            // finished. A failed capture must remain recoverable by a fresh
            // explicit preview, even after this runtime has shut down.
            for file in &self.web_owned_files {
                self.verify_controls()?;
                remove_web_file(&ownership, &file.project_id, &file.path, &file.identity)?;
                ownership.mark_material_removed(&file.project_id, &file.path, &file.identity)?;
                removed_file_paths.insert(file.path.clone());
                if let Some(stage) = &file.staging_path {
                    removed_file_paths.insert(stage.clone());
                }
            }
            for target in &self.targets {
                self.verify_controls()?;
                if removed_file_paths.contains(&target.path)
                    && target
                        .identity
                        .absent_on_expected_volume(&target.path)
                        .map_err(|error| fail(error.to_string()))?
                {
                    continue;
                }
                target.verify()?;
                if target.directory {
                    crate::history_cleanup::remove_owned_directory(&target.path, &target.identity)?;
                } else {
                    if !target.path.starts_with(self.paths.data_root()) {
                        return Err(fail("外部普通文件缺少已受理的项目文件删除归属，未删除。"));
                    }
                    remove_bound_file(target)?;
                }
            }
            self.verify_controls()?;
            let remaining = fs::read_dir(self.paths.data_root())?
                .map(|entry| Ok(entry?.file_name()))
                .collect::<Result<Vec<_>, std::io::Error>>()?;
            if remaining
                .iter()
                .any(|name| !RETAINED.iter().any(|retained| name == retained))
            {
                return Err(fail("清理期间新增了未核对内容，已停止并保留剩余文件。"));
            }
            Ok(())
        })();
        let mut remaining = self
            .targets
            .iter()
            .filter(|target| fs::symlink_metadata(&target.path).is_ok())
            .map(|target| target.path.clone())
            .collect::<BTreeSet<_>>();
        if result.is_err() {
            let ownership = crate::web_project_ownership::WebProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            for project_id in self
                .web_owned_files
                .iter()
                .map(|file| &file.project_id)
                .collect::<BTreeSet<_>>()
            {
                if let Ok(plans) = ownership.file_deletion_plans(project_id) {
                    for plan in plans {
                        for path in [plan.original, plan.staging_path] {
                            if fs::symlink_metadata(&path).is_ok() {
                                remaining.insert(path);
                            }
                        }
                    }
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.paths.data_root()) {
            for entry in entries.flatten() {
                if !RETAINED.iter().any(|name| entry.file_name() == *name) {
                    remaining.insert(entry.path());
                }
            }
        }
        let remaining = remaining.into_iter().collect::<Vec<_>>();
        if let Err(error) = result {
            // Do not automatically retry an unknown or partially deleted scope.
            return Err(ApiError::new(
                "LOCAL_CLEANUP_INCOMPLETE",
                "全部本机数据清理未完成，请核对剩余目录后重新扫描；不要自动重试。",
            )
            .outcome("delete_incomplete")
            .detail("remainingPaths", json!(remaining))
            .detail("cause", json!(error)));
        }
        Ok(
            json!({"operationOutcome":"deleted","restoreAvailable":false,"remainingPaths":[],
            "retainedControlPaths":RETAINED.iter().map(|name|self.paths.data_root().join(name)).collect::<Vec<_>>(),
            "preservedPaths":self.preserved,"exitRequired":true}),
        )
    }
}

fn verify_backend_preview(paths: &RuntimePaths) -> Result<(), ApiError> {
    let apps = paths.data_root().join("apps");
    crate::projects::check_no_symlinks(&apps, true)?;
    if !apps.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(apps)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() {
            continue;
        }
        if let Some(status) = crate::backend_run_marker::status(&entry.path(), &id)? {
            if status["state"] == "unconfirmed" {
                return Err(ApiError::new(
                    "APP_RUN_RECOVERY_REQUIRED",
                    "无法确认上次后台运行是否已停止，请先完成应用停止或恢复，再清理本机数据。",
                ));
            }
        }
    }
    Ok(())
}

fn safe_workspace_root(path: &Path, data: &Path) -> Result<(), ApiError> {
    if !path.is_absolute()
        || path == data
        || data.starts_with(path)
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path.components().count() < 3
        || dirs::home_dir().is_some_and(|home| path == home)
        || path.components().any(|part| {
            part.as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".app")
        })
    {
        return Err(fail("工作区清理范围与受保护目录重叠，未开始删除。"));
    }
    Ok(())
}

fn scope_metadata(
    paths: &RuntimePaths,
    workspace_roots: &[PathBuf],
) -> Result<(BTreeMap<PathBuf, String>, Vec<PathBuf>), ApiError> {
    let mut files = vec![
        paths.data_root().join("settings.json"),
        paths.data_root().join(".environment.json"),
    ];
    for entry in fs::read_dir(paths.migrations_dir())? {
        files.push(entry?.path().join("journal.json"));
    }
    let mut preserved = BTreeSet::new();
    for source in crate::web_project_ownership::WebProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    )
    .user_source_roots()?
    {
        let text = source
            .to_str()
            .ok_or_else(|| fail("用户应用路径不是有效文本。"))?;
        add_preserved(&mut preserved, text, paths)?;
    }
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    let owned_paths = project_owned_paths(paths)?;
    // Download transaction journals remain separate from accepted app identity.
    // Their bound scopes cover only source-import-owned temporary material.
    for (ledger, _) in ownership.all_owned_scopes()? {
        let id = ledger
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| fail("项目源码归属标识无效，未开始清理。"))?;
        for source in ownership.source_roots(id)? {
            let source = source
                .to_str()
                .ok_or_else(|| fail("项目源码目录不是有效文本路径。"))?;
            add_preserved_unless_owned(&mut preserved, source, paths, &owned_paths)?;
        }
        files.push(ledger);
    }
    files.extend(
        crate::web_project_ownership::WebProjectOwnership::new(
            paths.data_root().into(),
            paths.workspace_root().into(),
        )
        .all_owned_scopes()?
        .into_iter()
        .map(|(path, _)| path),
    );
    // Use the one strict registration decoder. Preview never opens a business
    // database and a malformed record cannot hide a user-owned source directory.
    for record in
        crate::projects::registered_project_sources(paths.data_root(), paths.workspace_root())?
    {
        files.push(crate::projects::registration_path(
            paths.data_root(),
            &record.id,
        ));
        if record.ownership == crate::projects::ProjectOwnership::User {
            crate::projects::check_no_symlinks(&record.root, true)?;
            match fs::symlink_metadata(&record.root) {
                Ok(_) if record.identity.matches_path(&record.root).unwrap_or(false) => {}
                Ok(_) => return Err(changed()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !record
                        .identity
                        .absent_on_expected_volume(&record.root)
                        .map_err(|_| changed())?
                    {
                        return Err(changed());
                    }
                }
                Err(error) => return Err(error.into()),
            }
            let source = record
                .root
                .to_str()
                .ok_or_else(|| fail("用户应用目录不是有效文本路径。"))?;
            add_preserved(&mut preserved, source, paths)?;
        }
    }
    let apps = paths.data_root().join("apps");
    crate::projects::check_no_symlinks(&apps, true)?;
    if apps.try_exists()? {
        for entry in fs::read_dir(apps)? {
            let reservation = entry?.path().join("reservation.json");
            crate::projects::check_no_symlinks(&reservation, true)?;
            if reservation.try_exists()? {
                files.push(reservation);
            }
        }
    }
    for workspace in workspace_roots {
        let sources = workspace.join(".platform/sources");
        crate::projects::check_no_symlinks(&sources, true)?;
        if !sources.try_exists()? {
            continue;
        }
        for entry in fs::read_dir(sources)? {
            let source = entry?.path().join("source.json");
            crate::projects::check_no_symlinks(&source, true)?;
            if !source.try_exists()? {
                continue;
            }
            let value: Value = serde_json::from_slice(&read_small(&source)?)?;
            validate_source_record(&source, &value)?;
            files.push(source);
        }
    }
    let mut metadata = BTreeMap::new();
    for path in files {
        metadata.insert(
            path.clone(),
            format!("{:x}", Sha256::digest(read_small(&path)?)),
        );
    }
    Ok((metadata, preserved.into_iter().collect()))
}

fn validate_source_record(path: &Path, value: &Value) -> Result<(), ApiError> {
    let bad = || fail("源码来源记录无效，无法核对原始源码目录；未开始清理。");
    let fields = [
        "schemaVersion",
        "sourceId",
        "ownerAppId",
        "taskId",
        "name",
        "status",
        "phase",
        "url",
        "branch",
        "provider",
        "createdAt",
        "completedAt",
        "repository",
        "shallow",
        "git",
        "commit",
        "fileCount",
        "totalBytes",
        "contentDigest",
        "inspection",
        "error",
    ];
    let id = path
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .ok_or_else(bad)?;
    if !value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| fields.contains(&key.as_str())))
        || value["schemaVersion"] != 1
        || value["sourceId"] != id
        || !id.strip_prefix("src_").is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || !value["taskId"]
            .as_str()
            .is_some_and(crate::projects::valid_id)
        || value["repository"] != "repository"
        || !value["provider"]
            .as_str()
            .is_some_and(|provider| matches!(provider, "github" | "gitee"))
        || !value["status"].as_str().is_some_and(|status| {
            matches!(
                status,
                "acquiring" | "ready" | "failed" | "cancelled" | "interrupted"
            )
        })
        || ["name", "phase", "url"]
            .iter()
            .any(|field| value[field].as_str().is_none())
        || value["createdAt"].as_u64().is_none()
        || value["shallow"].as_bool().is_none()
        || !value["ownerAppId"]
            .as_str()
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
    {
        return Err(bad());
    }
    Ok(())
}

fn add_preserved(
    preserved: &mut BTreeSet<PathBuf>,
    external: &str,
    paths: &RuntimePaths,
) -> Result<(), ApiError> {
    let path = PathBuf::from(external);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path.starts_with(paths.data_root())
        || paths.data_root().starts_with(&path)
    {
        return Err(fail("外部原始源码与平台数据根重叠，未开始清理。"));
    }
    preserved.insert(path);
    Ok(())
}
fn project_owned_paths(paths: &RuntimePaths) -> Result<BTreeSet<PathBuf>, ApiError> {
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    let mut owned: BTreeSet<_> = ownership
        .all_owned_scopes()?
        .into_iter()
        .flat_map(|(_, paths)| paths)
        .collect();
    owned.extend(
        crate::web_project_ownership::WebProjectOwnership::new(
            paths.data_root().into(),
            paths.workspace_root().into(),
        )
        .all_owned_scopes()?
        .into_iter()
        .flat_map(|(_, paths)| paths),
    );
    Ok(owned)
}

fn web_owned_files(paths: &RuntimePaths) -> Result<Vec<WebOwnedFile>, ApiError> {
    let ownership = crate::web_project_ownership::WebProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    let mut files: BTreeMap<PathBuf, WebOwnedFile> = BTreeMap::new();
    for (record, _) in ownership.all_owned_scopes()? {
        let project_id = record
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(changed)?
            .to_string();
        for plan in ownership.file_deletion_plans(&project_id)? {
            plan.validate()?;
            if plan.original.starts_with(paths.data_root()) {
                continue;
            }
            if files.contains_key(&plan.original) {
                return Err(changed());
            }
            files.insert(
                plan.original.clone(),
                WebOwnedFile {
                    project_id: project_id.clone(),
                    path: plan.original,
                    identity: plan.expected,
                    staging_path: Some(plan.staging_path),
                },
            );
        }
        for path in ownership.material_paths(&project_id)? {
            if path.starts_with(paths.data_root()) {
                continue;
            }
            if let Some(previous) = files.get(&path) {
                if previous.project_id != project_id {
                    return Err(changed());
                }
                continue;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !metadata.is_file() {
                continue;
            }
            let identity = ownership
                .material_identity(&project_id, &path)?
                .ok_or_else(changed)?;
            if !identity.matches_path(&path).unwrap_or(false) {
                return Err(changed());
            }
            files.insert(
                path.clone(),
                WebOwnedFile {
                    project_id: project_id.clone(),
                    path,
                    identity,
                    staging_path: None,
                },
            );
        }
    }
    Ok(files.into_values().collect())
}
fn add_preserved_unless_owned(
    preserved: &mut BTreeSet<PathBuf>,
    external: &str,
    paths: &RuntimePaths,
    owned: &BTreeSet<PathBuf>,
) -> Result<(), ApiError> {
    let source = PathBuf::from(external);
    if owned.iter().any(|root| source.starts_with(root)) {
        return Ok(());
    }
    add_preserved(preserved, external, paths)
}

fn resolve_external(path: &Path) -> Result<PathBuf, ApiError> {
    let mut ancestor = path;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let resolved = ancestor.canonicalize()?;
                return Ok(resolved.join(path.strip_prefix(ancestor).map_err(|_| changed())?));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or_else(changed)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn remove_bound_file(target: &BoundPath) -> Result<(), ApiError> {
    target.verify()?;
    crate::cleanup_file_io::remove_file(&target.path, &target.identity, &target.parents)
}

#[cfg(all(test, unix))]
#[path = "local_cleanup_tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "local_cleanup_windows_tests.rs"]
mod windows_tests;
