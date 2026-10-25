//! Explicit pre-uninstall cleanup. Plans are private, short-lived Rust objects;
//! callers cannot submit paths or reconstruct deletion authority from JSON.
//! The desktop additionally proves strict business-window and Node shutdown.
use crate::{file_identity::PersistedIdentity, ApiError, RuntimePaths, RuntimeService};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs::{self, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::OpenOptionsExt, fs::PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const CONTROL: &str = ".local-cleanup";
const TTL: Duration = Duration::from_secs(10 * 60);
const CONFIRMATION: &str = "删除全部本机数据";
const RETAINED: [&str; 3] = [".environment.json", "instance.lock", CONTROL];

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
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
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
    control: Option<BoundPath>,
    targets: Vec<BoundPath>,
    workspace_roots: Vec<PathBuf>,
    metadata: BTreeMap<PathBuf, String>,
    preserved: Vec<PathBuf>,
}

impl LocalCleanupPlan {
    /// Read-only. It does not create cleanup records, stop services or hash any
    /// project database, attachment or code file.
    pub fn prepare(paths: &RuntimePaths, runtime: &RuntimeService) -> Result<Self, ApiError> {
        crate::platform_support::require_runtime()?;
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
        let control_path = paths.data_root().join(CONTROL);
        let control = if control_path.try_exists()? {
            let bound = BoundPath::capture(control_path.clone())?;
            if !bound.directory {
                return Err(fail("清理安全记录目录类型异常。"));
            }
            let previous: Value =
                serde_json::from_slice(&read_small(&control_path.join("receipt.json"))?)?;
            if previous["schemaVersion"] != 1 || previous["phase"] != "completed" {
                return Err(fail("上次清理尚未完成，请保留清理回执并核对剩余目录。"));
            }
            verify_control_entries(&control_path)?;
            Some(bound)
        } else {
            None
        };
        let (metadata, preserved) = scope_metadata(paths, &workspace_roots)?;
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
            control,
            targets,
            workspace_roots,
            metadata,
            preserved,
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
                "外部原始源码目录、删除范围之外的独立导出文件和程序安装包保留；清理完成后仅保留不含项目内容的最小身份、锁和清理回执。",
                "系统生成的 WebKit 网页存储（可能含会话和本地存储）、软件缓存和系统偏好不在本入口范围；卸载程序后这些内容也可能保留。",
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
            .chain(self.control.as_ref().map(|control| control.path.clone()))
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
                    source.starts_with(&target.path) || target.path.starts_with(source)
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
        let lock = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.lock.path)?;
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
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(control) = &self.control {
            control.verify()?;
        }
        if let Some(control) = &self.control {
            verify_control_entries(&control.path)?;
        }
        Ok(())
    }

    /// The caller must additionally have joined strict native windows, Node
    /// processes/proxies and native sessions. Core checks its actual owned
    /// shutdown state, never a caller-supplied JSON report or boolean.
    pub fn execute_after_shutdown(mut self, runtime: &RuntimeService) -> Result<Value, ApiError> {
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
            .map_err(|error| error.outcome("not_started"))?;
        let control = self.paths.data_root().join(CONTROL);
        if self.control.is_none() {
            fs::create_dir(&control)
                .map_err(|error| ApiError::from(error).outcome("not_started"))?;
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700))
                .map_err(|error| ApiError::from(error).outcome("not_started"))?;
            self.control = Some(
                BoundPath::capture(control.clone())
                    .map_err(|error| error.outcome("not_started"))?,
            );
        }
        let receipt_path = control.join("receipt.json");
        let receipt = |phase: &str, remaining: &[PathBuf]| {
            json!({"schemaVersion":1,"planId":self.id,
            "instanceId":self.instance_id,"phase":phase,"updatedAt":now(),"remainingPaths":remaining,
            "retainedControlPaths":RETAINED.iter().map(|name|self.paths.data_root().join(name)).collect::<Vec<_>>()})
        };
        let all_paths = self
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        crate::builds::atomic_json(&receipt_path, &receipt("deleting", &all_paths))
            .map_err(|error| error.outcome("not_started"))?;
        let result = (|| -> Result<(), ApiError> {
            for (index, target) in self.targets.iter().enumerate() {
                self.verify_controls()?;
                target.verify()?;
                if target.directory {
                    let progress = control.join(format!("{}-{index}.ndjson", self.id));
                    crate::history_cleanup::remove_owned_directory(
                        &target.path,
                        &target.identity,
                        &progress,
                    )?;
                } else {
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
            // Successful cleanup retains no per-file history or project names,
            // including any old progress of an already completed cleanup.
            for entry in fs::read_dir(&control)? {
                let path = entry?.path();
                if path.file_name().is_some_and(|name| name != "receipt.json") {
                    if !path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(valid_progress_name)
                    {
                        return Err(fail("清理安全目录新增了未知记录，请保留现场。"));
                    }
                    remove_bound_file(&BoundPath::capture(path)?)?;
                }
            }
            self.verify_controls()?;
            crate::builds::atomic_json(&receipt_path, &receipt("completed", &[]))?;
            Ok(())
        })();
        let mut remaining = self
            .targets
            .iter()
            .filter(|target| fs::symlink_metadata(&target.path).is_ok())
            .map(|target| target.path.clone())
            .collect::<BTreeSet<_>>();
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
            let _ = crate::builds::atomic_json(&receipt_path, &receipt("incomplete", &remaining));
            return Err(ApiError::new(
                "LOCAL_CLEANUP_INCOMPLETE",
                "全部本机数据清理未完成，请保留清理回执并核对剩余目录；不要自动重试。",
            )
            .outcome("delete_incomplete")
            .detail("remainingPaths", json!(remaining))
            .detail("receiptPath", json!(receipt_path))
            .detail("cause", json!(error)));
        }
        Ok(
            json!({"operationOutcome":"deleted","restoreAvailable":false,"remainingPaths":[],
            "receiptPath":receipt_path,"retainedControlPaths":RETAINED.iter().map(|name|self.paths.data_root().join(name)).collect::<Vec<_>>(),
            "preservedPaths":self.preserved,"exitRequired":true}),
        )
    }
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
    let node_root = paths.data_root().join("node-apps");
    crate::projects::check_no_symlinks(&node_root, true)?;
    if node_root.try_exists()? {
        for entry in fs::read_dir(&node_root)? {
            let entry = entry?;
            for name in ["registration.json", "development-source.json"] {
                let registration = entry.path().join(name);
                crate::projects::check_no_symlinks(&registration, true)?;
                if !registration.try_exists()? {
                    continue;
                }
                let value: Value = serde_json::from_slice(&read_small(&registration)?)?;
                for external in node_references(&node_root, &entry.path(), name, value)? {
                    add_preserved(&mut preserved, &external, paths)?;
                }
                files.push(registration);
            }
        }
        let installations = node_root.join(".installations");
        crate::projects::check_no_symlinks(&installations, true)?;
        if installations.try_exists()? {
            for entry in fs::read_dir(&installations)? {
                let path = entry?.path().join("install.json");
                let value: Value = serde_json::from_slice(&read_small(&path)?)?;
                for external in installation_references(&node_root, &path, &value)? {
                    add_preserved(&mut preserved, &external, paths)?;
                }
                files.push(path);
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
            // A build candidate points to an owned build output, not the
            // original author tree; that payload is part of this cleanup.
            if value["capturePolicy"] != "node-development-build-v1" {
                if let Some(external) = value["localSourcePath"].as_str() {
                    add_preserved(&mut preserved, external, paths)?;
                }
            }
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExternalRegistration {
    version: u32,
    id: String,
    source_path: String,
    manifest_digest: String,
}

fn node_references(
    node_root: &Path,
    slot: &Path,
    name: &str,
    value: Value,
) -> Result<Vec<String>, ApiError> {
    let id = slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(changed)?;
    if Uuid::parse_str(id).is_err() {
        return Err(fail("Node 应用登记所在目录身份无效，未开始清理。"));
    }
    let binding_path = |value| -> Result<String, ApiError> {
        let binding: crate::node_apps::development::NodeDevelopmentBinding =
            serde_json::from_value(value)
                .map_err(|_| fail("源码绑定记录无效，请先核对外部原始源码的位置。"))?;
        binding.validate_record()?;
        binding
            .source_root
            .into_os_string()
            .into_string()
            .map_err(|_| fail("源码绑定目录不是有效文本路径。"))
    };
    if name == "development-source.json" {
        return Ok(vec![binding_path(value)?]);
    }
    if value["version"] == 1 {
        let record: ExternalRegistration = serde_json::from_value(value)
            .map_err(|_| fail("外部应用登记无效，无法核对原始源码目录。"))?;
        if record.version != 1
            || record.id != id
            || record.manifest_digest.len() != 64
            || !record
                .manifest_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(fail("外部应用登记身份无效，未开始清理。"));
        }
        Ok(vec![record.source_path])
    } else if value["version"] == 2 {
        let record: crate::node_app_install::ManagedNodeRegistration =
            serde_json::from_value(value)
                .map_err(|_| fail("受管应用登记无效，无法核对原始源码绑定。"))?;
        if record.id != id {
            return Err(fail("受管应用登记身份不符，未开始清理。"));
        }
        crate::node_app_install::managed_program_path(node_root, &record)?;
        record
            .development_source
            .map(|binding| {
                binding.validate_record()?;
                binding
                    .source_root
                    .into_os_string()
                    .into_string()
                    .map_err(|_| fail("源码绑定目录不是有效文本路径。"))
            })
            .transpose()
            .map(|path| path.into_iter().collect())
    } else {
        Err(fail("未知应用登记格式，无法核对外部原始源码；未开始清理。"))
    }
}

fn installation_references(
    node_root: &Path,
    path: &Path,
    value: &Value,
) -> Result<Vec<String>, ApiError> {
    let bad = || fail("历史安装记录无效，无法核对原始源码目录；未开始清理。");
    let id = path
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .ok_or_else(bad)?;
    let fields = [
        "version",
        "candidateId",
        "prepareTaskId",
        "applyTaskId",
        "state",
        "target",
        "prepared",
        "registration",
        "error",
    ];
    if value["version"] != 1
        || value["candidateId"] != id
        || !id.strip_prefix("install_").is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || !value
            .as_object()
            .is_some_and(|object| object.keys().all(|key| fields.contains(&key.as_str())))
        || !value["prepareTaskId"]
            .as_str()
            .is_some_and(crate::projects::valid_id)
        || !value["state"].as_str().is_some_and(|state| {
            matches!(
                state,
                "preparing"
                    | "prepared"
                    | "committing"
                    | "committed"
                    | "failed"
                    | "interrupted"
                    | "needs-attention"
            )
        })
    {
        return Err(bad());
    }
    let target: crate::node_app_install::InstallTargetSnapshot =
        serde_json::from_value(value["target"].clone()).map_err(|_| bad())?;
    target.validate()?;
    let mut paths = Vec::new();
    if let Some(previous) = target.external_registration {
        paths.extend(node_references(
            node_root,
            &node_root.join(&target.app_id),
            "registration.json",
            previous,
        )?);
    }
    if let Some(binding) = target.development_source {
        paths.push(
            binding
                .source_root
                .into_os_string()
                .into_string()
                .map_err(|_| bad())?,
        );
    }
    if !value["registration"].is_null() {
        paths.extend(node_references(
            node_root,
            &node_root.join(&target.app_id),
            "registration.json",
            value["registration"].clone(),
        )?);
    }
    Ok(paths)
}

fn validate_source_record(path: &Path, value: &Value) -> Result<(), ApiError> {
    let bad = || fail("源码来源记录无效，无法核对原始源码目录；未开始清理。");
    let fields = [
        "schemaVersion",
        "sourceId",
        "taskId",
        "name",
        "status",
        "phase",
        "url",
        "branch",
        "provider",
        "localSourcePath",
        "capturePolicy",
        "buildSourceDigest",
        "excludedPaths",
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
            .is_some_and(|provider| matches!(provider, "local" | "github" | "gitee"))
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
        || (!value["localSourcePath"].is_null() && value["localSourcePath"].as_str().is_none())
        || (!value["capturePolicy"].is_null() && value["capturePolicy"].as_str().is_none())
    {
        return Err(bad());
    }
    if value["provider"] == "local" {
        if value["localSourcePath"].as_str().is_none()
            || !value["branch"].is_null()
            || !matches!(
                value["capturePolicy"].as_str(),
                Some("local-source-v1" | "node-development-build-v1")
            )
        {
            return Err(bad());
        }
    } else if !value["localSourcePath"].is_null() || !value["capturePolicy"].is_null() {
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

fn valid_progress_name(name: &str) -> bool {
    name.strip_prefix("local_cleanup_")
        .and_then(|name| name.strip_suffix(".ndjson"))
        .and_then(|name| name.rsplit_once('-'))
        .is_some_and(|(id, index)| Uuid::parse_str(id).is_ok() && index.parse::<usize>().is_ok())
}

fn verify_control_entries(control: &Path) -> Result<(), ApiError> {
    for entry in fs::read_dir(control)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(changed)?;
        if (name != "receipt.json" && !valid_progress_name(name)) || !ordinary(&path)?.is_file() {
            return Err(fail("清理安全目录存在未知内容，请保留现场并核对。"));
        }
    }
    Ok(())
}

fn remove_bound_file(target: &BoundPath) -> Result<(), ApiError> {
    target.verify()?;
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(target.path.parent().ok_or_else(changed)?)?;
    let parent_identity = PersistedIdentity::from_file(&parent).map_err(|_| changed())?;
    if !target
        .parents
        .first()
        .is_some_and(|(_, expected)| expected.same_runtime_object(&parent_identity))
    {
        return Err(changed());
    }
    let name = CString::new(target.path.file_name().ok_or_else(changed)?.as_bytes())
        .map_err(|_| changed())?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_dev as u64 != target.identity.device
        || stat.st_ino != target.identity.inode
    {
        return Err(changed());
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    parent.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "local_cleanup_tests.rs"]
mod tests;
