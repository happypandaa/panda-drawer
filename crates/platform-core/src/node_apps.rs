//! Native-manager-only source applications, separate from project-v1 storage.
//! Registration reads declarations; only an explicit start executes trusted code.

use crate::{
    node_app_configuration::{LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest},
    node_app_proxy::NodeAppProxy,
    node_registration::{
        self, DirectNodeProvenance as DirectSourceProvenance,
        DirectNodeRegistration as ExternalRegistration, StoredNodeRegistration,
        DIRECT_NODE_REGISTRATION_VERSION,
    },
    node_runtime_trial::{NodeRuntimeTrial, RuntimeLaunchConfig, RuntimeStopReason, TrialStatus},
    panda_manifest::{PandaManifest, PandaRuntime},
    ApiError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[cfg(unix)]
#[path = "node_apps/unix_files.rs"]
mod file_io;
#[cfg(windows)]
#[path = "node_apps/windows_files.rs"]
mod file_io;
#[cfg(test)]
use file_io::IntentCleanupPoint;
use file_io::{
    move_node_record_exclusive, ordinary_path, private_dir, read_small, write_new_json,
    StartIntentOwnership,
};

#[path = "node_app_delete.rs"]
mod deletion;
pub use deletion::DeleteNodeAppInput;
#[path = "node_app_operations.rs"]
mod operations;
pub use operations::{NodeDeletionPermit, NodeOperationPermit};
#[path = "node_deletion_tasks.rs"]
mod delete_tasks;
pub use delete_tasks::NodeDeletionBeforeStop;
#[path = "node_app_directory.rs"]
mod directory;
#[path = "node_app_migration.rs"]
mod migration;
#[path = "node_run_marker.rs"]
mod run_marker;
pub use directory::DirectoryInitializeInput;
pub use migration::NodeWorkspacePause;
#[path = "node_source_ownership.rs"]
pub(crate) mod source_ownership;
const START_INTENT_CLEANUP_PREFIX: &str = ".start-intent-cleanup-";

type Result<T> = std::result::Result<T, ApiError>;
fn fail(message: impl Into<String>) -> ApiError {
    ApiError::new("NODE_APP_ERROR", message)
}

fn clean_shutdown(status: &TrialStatus) -> bool {
    status.process_group_exited && status.state == "stopped"
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportSlotOwnership {
    app_id: String,
    source_root: PathBuf,
    source_identity: crate::file_identity::PersistedIdentity,
    slot_identity: crate::file_identity::PersistedIdentity,
}

struct Registration {
    id: String,
    source_path: PathBuf,
    manifest_digest: String,
    provenance: Option<DirectSourceProvenance>,
    pub address: crate::FixedAppAddress,
}
impl From<ExternalRegistration> for Registration {
    fn from(record: ExternalRegistration) -> Self {
        Self {
            id: record.id,
            source_path: record.source_path,
            manifest_digest: record.manifest_digest,
            provenance: record.provenance,
            address: record.address,
        }
    }
}

struct Running {
    runtime: NodeRuntimeTrial,
    proxy: Option<NodeAppProxy>,
}

const RUNTIME_STARTING: u8 = 0;
const RUNTIME_RUNNING: u8 = 1;
const RUNTIME_STOPPING: u8 = 2;
const RUNTIME_FAILED: u8 = 3;

/// The stable entry stays in the table throughout launch and cleanup. Its
/// resource lock is per instance; status reads use only the small phase snapshot.
struct RuntimeEntry {
    resources: Arc<Mutex<Option<Running>>>,
    revision: u64,
    registration_digest: String,
    manifest: Arc<PandaManifest>,
    workspace_root: Option<PathBuf>,
    source_root: Option<PathBuf>,
    phase: AtomicU8,
}
impl RuntimeEntry {
    fn new(revision: u64, registration_digest: String, manifest: Arc<PandaManifest>) -> Self {
        Self {
            resources: Arc::new(Mutex::new(None)),
            revision,
            registration_digest,
            manifest,
            workspace_root: None,
            source_root: None,
            phase: AtomicU8::new(RUNTIME_STARTING),
        }
    }
    fn phase(&self) -> u8 {
        self.phase.load(Ordering::Acquire)
    }
    fn set_phase(&self, phase: u8) {
        self.phase.store(phase, Ordering::Release);
    }
    fn cleanup_pending(&self) -> bool {
        self.phase() >= RUNTIME_STOPPING
    }
}
/// Unexpected cancellation/panic leaves a visible retryable cleanup state;
/// the map, not this guard, continues to own every actual resource.
struct RuntimePhaseGuard(Arc<RuntimeEntry>);
impl Drop for RuntimePhaseGuard {
    fn drop(&mut self) {
        if matches!(self.0.phase(), RUNTIME_STARTING | RUNTIME_STOPPING) {
            self.0.set_phase(RUNTIME_FAILED);
        }
    }
}

struct PreparedRuntimeStart {
    record: Registration,
    manifest: Arc<PandaManifest>,
    slot: PathBuf,
    config: RuntimeLaunchConfig,
    address: crate::FixedAppAddress,
}

#[derive(Default)]
struct Inner {
    running: BTreeMap<String, Arc<RuntimeEntry>>,
    markers: BTreeMap<String, StartIntentOwnership>,
    errors: BTreeMap<String, String>,
    shutting_down: bool,
}

#[cfg(test)]
type StartIntentPause = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[derive(Clone)]
pub struct NodeApps {
    root: PathBuf,
    data_root: PathBuf,
    node_path: PathBuf,
    digest: String,
    processes: crate::owned_process::OwnedProcessRegistry,
    inner: Arc<Mutex<Inner>>,
    operations: Arc<operations::NodeOperationCoordinator>,
    deletion_tasks: Arc<delete_tasks::NodeDeletionTasks>,
    runtime_operations: Arc<std::sync::Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    directory_changes: Arc<std::sync::Mutex<()>>,
    directory_stopping: Arc<std::sync::atomic::AtomicBool>,
    source_workspace: Arc<std::sync::RwLock<Option<PathBuf>>>,
    task_manager: Arc<std::sync::Mutex<Option<Arc<crate::builds::BuildManager>>>>,
    runtime: Option<crate::RuntimeHandle>,
    #[cfg(test)]
    start_intent_pause: Arc<std::sync::Mutex<Option<StartIntentPause>>>,
}

impl NodeApps {
    pub fn new(data_root: PathBuf, node_path: PathBuf) -> Result<Self> {
        Self::with_processes(
            data_root,
            node_path,
            crate::owned_process::OwnedProcessRegistry::default(),
        )
    }
    pub fn with_runtime(
        data_root: PathBuf,
        node_path: PathBuf,
        runtime: &crate::RuntimeHandle,
    ) -> Result<Self> {
        let mut apps = Self::with_processes(data_root, node_path, runtime.store().processes())?;
        apps.set_source_workspace(runtime.info().workspace_root)?;
        apps.set_deletion_store(runtime.store())?;
        apps.runtime = Some(runtime.clone());
        Ok(apps)
    }
    fn with_processes(
        data_root: PathBuf,
        node_path: PathBuf,
        processes: crate::owned_process::OwnedProcessRegistry,
    ) -> Result<Self> {
        if crate::build_profile::PRODUCTION_PROFILE {
            crate::platform_support::require_runtime()?;
            let bin = node_path
                .parent()
                .filter(|path| path.file_name().is_some_and(|name| name == "bin"))
                .ok_or_else(|| fail("正式 Node 路径与固定资源位置不一致。"))?;
            if node_path.file_name().is_none_or(|name| name != "node") {
                return Err(fail("正式 Node 文件名与固定资源位置不一致。"));
            }
            let resource_root = bin.parent().ok_or_else(|| fail("正式 Node 资源根缺失。"))?;
            crate::production_resources::verify_tool_payload(
                resource_root,
                crate::production_resources::PayloadKind::NodeRuntime,
            )?;
        }
        ordinary_path(&data_root, true)?;
        let root = data_root.join("node-apps");
        let digest = if crate::build_profile::PRODUCTION_PROFILE {
            crate::production_resources::runtime_node_digest()?
        } else {
            let release: Value = serde_json::from_str(crate::node_tool_layout::RUNTIME_RELEASE)?;
            release["binary"]["sha256"]
                .as_str()
                .ok_or_else(|| fail("内置 Node 校验信息无效。"))?
                .to_owned()
        };
        let apps = Self {
            root,
            data_root,
            node_path,
            digest,
            processes,
            inner: Arc::new(Mutex::new(Inner::default())),
            operations: Arc::new(operations::NodeOperationCoordinator::default()),
            deletion_tasks: Arc::new(delete_tasks::NodeDeletionTasks::default()),
            runtime_operations: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            directory_changes: Arc::new(std::sync::Mutex::new(())),
            directory_stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            source_workspace: Arc::new(std::sync::RwLock::new(None)),
            task_manager: Arc::new(std::sync::Mutex::new(None)),
            runtime: None,
            #[cfg(test)]
            start_intent_pause: Arc::new(std::sync::Mutex::new(None)),
        };
        apps.load_delete_tasks()?;
        Ok(apps)
    }

    pub fn inspect(&self, source: &Path) -> Result<Value> {
        let manifest = self.load_source(source)?;
        Ok(serde_json::to_value(
            manifest.inspect(source).map_err(|e| fail(e.to_string()))?,
        )?)
    }

    fn load_source(&self, source: &Path) -> Result<PandaManifest> {
        ordinary_path(source, true)?;
        if source.starts_with(&self.data_root) && !self.is_published_source(source)
            || self.data_root.starts_with(source)
            || source
                .components()
                .any(|part| part.as_os_str() == ".platform")
        {
            return Err(fail(
                "请选择独立的应用源码目录，不能使用平台的数据目录或其父目录。",
            ));
        }
        let bytes = PandaManifest::read_bytes(source).map_err(|e| fail(e.to_string()))?;
        let manifest = PandaManifest::parse(&bytes).map_err(|e| fail(e.to_string()))?;
        if manifest.runtime != PandaRuntime::Node {
            return Err(ApiError::new(
                "NODE_RUNTIME_NOT_SUPPORTED",
                "此入口当前只登记 Node 应用。",
            ));
        }
        manifest.inspect(source).map_err(|e| fail(e.to_string()))?;
        Ok(manifest)
    }

    pub(crate) fn set_source_workspace(&self, workspace: PathBuf) -> Result<()> {
        ordinary_path(&workspace, true)?;
        *self
            .source_workspace
            .write()
            .map_err(|_| fail("无法核对项目目录。"))? = Some(workspace);
        Ok(())
    }
    pub(crate) fn is_published_source(&self, source: &Path) -> bool {
        source
            .strip_prefix(self.deletion_workspace().join("project-sources"))
            .is_ok_and(|relative| {
                relative.components().count() == 1
                    && matches!(relative.components().next(), Some(Component::Normal(_)))
            })
    }

    pub(super) fn current_node_task_manager(
        &self,
    ) -> Result<Option<Arc<crate::builds::BuildManager>>> {
        if let Some(runtime) = &self.runtime {
            return Ok(Some(runtime.node_task_manager()));
        }
        Ok(self
            .task_manager
            .lock()
            .map_err(|_| fail("任务管理器不可用。"))?
            .clone())
    }

    pub(crate) fn set_import_task_manager(
        &self,
        tasks: Arc<crate::builds::BuildManager>,
    ) -> Result<()> {
        // A live RuntimeHandle always resolves the current workspace manager.
        if self.runtime.is_some() {
            return Ok(());
        }
        let mut current = self
            .task_manager
            .lock()
            .map_err(|_| fail("任务管理器不可用。"))?;
        if current
            .as_ref()
            .is_some_and(|previous| !Arc::ptr_eq(previous, &tasks))
        {
            return Err(fail("项目任务管理器已变化，请重新核对工作区。"));
        }
        *current = Some(tasks);
        Ok(())
    }

    fn registrations(&self) -> Result<Vec<Registration>> {
        if !self.root.try_exists()? {
            return Ok(Vec::new());
        }
        ordinary_path(&self.root, true)?;
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if Uuid::parse_str(&name).is_err() {
                continue;
            }
            ordinary_path(&entry.path(), true)?;
            let path = entry.path().join("registration.json");
            // An interrupted registration remains preserved, never executed.
            if !path.try_exists()? {
                continue;
            }
            let bytes = read_small(&path)?;
            let value: Value = serde_json::from_slice(&bytes)?;
            let record = match node_registration::decode_registration(value, &name, &self.root)? {
                StoredNodeRegistration::Direct(record) => Registration::from(record),
            };
            crate::app_address::require_app_address(
                &self.data_root,
                crate::app_address::AppAddressKind::Node,
                &name,
                record.address,
            )?;
            if record.id != name {
                return Err(fail("Node 应用登记记录无效，请保留现场。"));
            }
            result.push(record);
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }

    fn registration(&self, id: &str) -> Result<Registration> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("Node 应用标识无效。"));
        }
        self.registrations()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| fail("找不到 Node 应用登记。"))
    }

    fn require_not_removing(&self, id: &str) -> Result<()> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用标识无效。"));
        }
        let path = self.root.join(id).join("registration.json");
        crate::projects::check_no_symlinks(&path, true)?;
        // Fresh installation checks the empty slot before publishing its first
        // registration. Callers that require an existing app read it separately.
        if !path.try_exists()? {
            return Ok(());
        }
        Ok(())
    }

    fn manifest(&self, record: &Registration) -> Result<PandaManifest> {
        let manifest = self.load_source(&record.source_path).map_err(|error| {
            if error.code == "NODE_RUNTIME_NOT_SUPPORTED" {
                ApiError::new("APP_RUNTIME_CHANGED", "应用类型变了，请删除后重新添加。")
            } else {
                error
            }
        })?;
        let binding = self
            .read_source_binding(&record.id)?
            .ok_or_else(|| fail("项目文件夹归属记录缺失，请重新检查。"))?;
        if binding.source_root != record.source_path {
            return Err(fail("项目文件夹与当前登记不符，请重新检查。"));
        }
        binding.validate_directory()?;
        Ok(manifest)
    }

    fn store(&self, id: &str) -> Result<NodeConfigurationStore> {
        self.require_configuration_access(id)?;
        self.require_not_removing(id)?;
        let directory = self.root.join(id).join("configuration");
        let instance = Uuid::parse_str(id).map_err(|_| fail("应用标识无效。"))?;
        let vault = LocalFileVault::new(directory.join("credentials"), instance)
            .map_err(|e| fail(e.to_string()))?;
        NodeConfigurationStore::new(directory, instance, Arc::new(vault))
            .map_err(|e| fail(e.to_string()))
    }

    fn detail(
        &self,
        record: &Registration,
        manifest: &PandaManifest,
        inner: &Inner,
    ) -> Result<Value> {
        let view = self
            .store(&record.id)?
            .view(manifest)
            .map_err(|e| fail(e.to_string()))?;
        let inspected = serde_json::to_value(
            manifest
                .inspect(&record.source_path)
                .map_err(|e| fail(e.to_string()))?,
        )?;
        let running = inner
            .running
            .get(&record.id)
            .filter(|run| !run.cleanup_pending());
        let marker = self
            .root
            .join(&record.id)
            .join("active.json")
            .try_exists()?;
        let runtime_phase = inner.running.get(&record.id).map(|run| run.phase());
        let retained_cleanup = inner
            .running
            .get(&record.id)
            .is_some_and(|run| run.cleanup_pending());
        let cleanup_pending =
            retained_cleanup || has_start_intent_cleanup(&self.root.join(&record.id))?;
        let error = if cleanup_pending {
            inner.errors.get(&record.id).cloned().or_else(|| {
                Some(if runtime_phase == Some(RUNTIME_STOPPING) {
                    "正在停止应用，等待进程、网页连接和文件工作完成收尾。".into()
                } else if retained_cleanup {
                    "应用进程或运行标记尚未确认完整收尾，请重试停止；当前不能再次启动。".into()
                } else if running.is_some() {
                    "应用仍在运行，启动收尾记录需要核对；已阻止再次启动，请保留运行记录。".into()
                } else {
                    run_marker::unknown().message
                })
            })
        } else {
            inner.errors.get(&record.id).cloned().or_else(|| {
                (marker && running.is_none())
                    .then(|| "上次运行尚未核对完成，已阻止再次启动。请查看运行状态。".into())
            })
        };
        let state = if runtime_phase == Some(RUNTIME_STOPPING) {
            "stopping"
        } else if running.is_some_and(|r| r.phase() == RUNTIME_STARTING) {
            "starting"
        } else if running.is_some() {
            "running"
        } else if error.is_some() {
            "failed"
        } else {
            "stopped"
        };
        let mut object_layout = crate::object_layout::node_layout(
            &record.source_path,
            &self.root.join(&record.id),
            Some(manifest),
        );
        let platform_owned = self
            .project_ownership()
            .owns_source_directory(&record.id, &record.source_path)?;
        object_layout["runtime"]["external"] = json!(!platform_owned);
        object_layout["source"] = json!({"state":"available","path":record.source_path,
            "ownership":if platform_owned {"platform"} else {"external"},
            "preservedOnDelete":!platform_owned});
        object_layout["build"] = json!({"state":"unavailable","reason":"直接运行项目文件夹；修改和构建由外部 AI 完成，关闭后重新进入生效。"});
        Ok(json!({
            "id":record.id,"name":manifest.app.name,"version":manifest.app.version,"description":manifest.app.description,"iconDataUrl":manifest.icon_data_url(&record.source_path),"sourcePath":record.source_path,"runtime":"node","window":manifest.window,
            "manifestDigest":manifest.source_digest,"gitSource":record.provenance.as_ref().map(|source|json!({"url":source.source_url,"branch":source.source_branch})),
            "ownership":if platform_owned {"managed"} else {"external"},
            "objectLayout":object_layout,
            "state":state,"restartRequired":running.is_some_and(|r|r.revision != view.revision || r.manifest.source_digest != manifest.source_digest),
            "runningRevision":running.map(|r|r.revision),"lastError":error,
            "configuration":{"revision":view.revision,"values":view.saved_values,"secretStatus":view.secrets,
                "missingRequired":view.missing_required,"invalidFields":view.invalid_fields,"canStart":view.can_start && runtime_phase != Some(RUNTIME_STARTING) && !cleanup_pending && (!marker || running.is_some()) && !inner.shutting_down},
            "fields":inspected["configuration"]["fields"],
            "dependencyWarnings":inspected["dependencyWarnings"]
        }))
    }

    pub async fn register(&self, source: PathBuf) -> Result<Value> {
        let _source_scope = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.source_registration_scope())
            .transpose()?;
        let _migration = self.operations.admit_global("登记本地项目")?;
        let inner = self.inner.lock().await;
        self.register_locked(source, &inner)
    }

    /// Only the Git importer can supply this capability. The instance and
    /// directory were claimed before download; this is never an external
    /// registration shortcut or a second allocation of the instance UUID.
    pub(crate) async fn register_published_source(
        &self,
        id: &str,
        source: crate::node_git_import::VerifiedPublishedSource,
    ) -> Result<Value> {
        let _permit = self.admit_operation(id, "登记已发布的项目")?;
        let _operation = self.runtime_operation(id)?.lock_owned().await;
        let inner = self.inner.lock().await;
        if let Some(detail) = self.reconcile_published_locked(id, &source, &inner)? {
            return Ok(detail);
        }
        if !self.is_published_source(&source.source_root)
            || !source
                .source_identity
                .matches_path(&source.source_root)
                .unwrap_or(false)
        {
            return Err(fail("已发布的项目文件夹身份不符，未登记。"));
        }
        let manifest = self.load_source(&source.source_root)?;
        if manifest.source_digest != source.manifest_digest {
            return Err(fail("项目说明在添加期间已变化，请重新添加。"));
        }
        let ownership = self.project_ownership();
        ownership.record_generated_source(id, &source.source_root)?;
        private_dir(&self.root)?;
        let slot = self.root.join(id);
        let marker = ownership.directory(id)?.join("import-slot.json");
        let mut slot_identity = None;
        let prepared = (|| -> Result<ExternalRegistration> {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&slot)?;
            let identity = crate::file_identity::PersistedIdentity::capture(&slot)
                .map_err(|error| fail(error.to_string()))?;
            slot_identity = Some(identity.clone());
            crate::builds::atomic_json(
                &marker,
                &ImportSlotOwnership {
                    app_id: id.into(),
                    source_root: source.source_root.clone(),
                    source_identity: source.source_identity.clone(),
                    slot_identity: identity,
                },
            )?;
            for name in ["configuration", "cache", "runs"] {
                private_dir(&slot.join(name))?;
            }
            let record = ExternalRegistration {
                version: DIRECT_NODE_REGISTRATION_VERSION,
                id: id.into(),
                address: crate::app_address::reserve_app_address(
                    &self.data_root,
                    crate::app_address::AppAddressKind::Node,
                    id,
                )?,
                source_path: source.source_root.clone(),
                manifest_digest: manifest.source_digest.clone(),
                provenance: Some(DirectSourceProvenance {
                    source_url: source.source_url.clone(),
                    source_branch: source.source_branch.clone(),
                }),
            };
            self.initialize_source_binding(&record)?;
            if !source
                .source_identity
                .matches_path(&source.source_root)
                .unwrap_or(false)
                || digest_manifest(&source.source_root)? != source.manifest_digest
            {
                return Err(fail("项目文件夹在登记前已变化，未提交登记。"));
            }
            Ok(record)
        })();
        let record = match prepared {
            Ok(record) => record,
            Err(error) => {
                let cleanup =
                    self.discard_unpublished_registration(id, slot_identity.as_ref(), true);
                if cleanup.is_ok() && marker.try_exists()? {
                    fs::remove_file(&marker)?;
                }
                return Err(error
                    .outcome("not_started")
                    .detail("appId", id)
                    .detail("cleanupError", cleanup.err().map(|error| json!(error))));
            }
        };
        // A partial write is never published as a registration. Once rename
        // succeeds, a lost reply or parent-sync error is a committed/unknown
        // outcome that the importer must reconcile before deciding rollback.
        let temporary = slot.join(".registration-import.json");
        let publication = (|| -> Result<()> {
            write_new_json(&temporary, &record)?;
            move_node_record_exclusive(&temporary, &slot.join("registration.json"))?;
            self.sync_registration_parents(id)
        })();
        publication.map_err(|error| {
            error
                .outcome("unknown")
                .detail("registrationAttempted", true)
                .detail("requiresRecheck", true)
                .detail("appId", id)
        })?;
        self.reconcile_published_locked(id, &source, &inner)?
            .ok_or_else(|| fail("登记提交结果需要重新核对。"))
    }

    pub(crate) async fn reconcile_published_registration(
        &self,
        id: &str,
        source: &crate::node_git_import::VerifiedPublishedSource,
    ) -> Result<Option<Value>> {
        let _permit = self.admit_operation(id, "核对项目登记")?;
        let _operation = self.runtime_operation(id)?.lock_owned().await;
        let inner = self.inner.lock().await;
        self.reconcile_published_locked(id, source, &inner)
    }

    fn reconcile_published_locked(
        &self,
        id: &str,
        source: &crate::node_git_import::VerifiedPublishedSource,
        inner: &Inner,
    ) -> Result<Option<Value>> {
        if Uuid::parse_str(id).is_err() || !self.is_published_source(&source.source_root) {
            return Err(fail("项目登记归属无效，未清理。"));
        }
        let slot = self.root.join(id);
        crate::projects::check_no_symlinks(&slot, true)?;
        let registration = slot.join("registration.json");
        crate::projects::check_no_symlinks(&registration, true)?;
        let marker = self
            .project_ownership()
            .directory(id)?
            .join("import-slot.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if registration.try_exists()? {
            let record = self.registration(id)?;
            let binding = self
                .read_source_binding(id)?
                .ok_or_else(|| fail("已提交登记缺少项目文件夹归属。"))?;
            let matches = record.provenance.as_ref().is_some_and(|provenance| {
                provenance.source_url == source.source_url
                    && provenance.source_branch == source.source_branch
            });
            if record.source_path != source.source_root
                || !matches
                || !binding.matches_identity(&source.source_root, &source.source_identity)
            {
                return Err(fail("当前登记与本次添加不符，已保留项目供核对。"));
            }
            self.sync_registration_parents(id)?;
            if marker.try_exists()? {
                fs::remove_file(&marker)?;
            }
            return Ok(Some(self.available_detail(&record, inner)));
        }
        if slot.try_exists()? {
            let owned: ImportSlotOwnership = serde_json::from_slice(&read_small(&marker)?)?;
            if owned.app_id != id
                || owned.source_root != source.source_root
                || !owned
                    .source_identity
                    .matches_captured(&source.source_identity)
                || !owned.slot_identity.matches_path(&slot).unwrap_or(false)
            {
                return Err(fail("未提交实例目录的身份无法确认，未删除。"));
            }
            self.discard_unpublished_registration(id, Some(&owned.slot_identity), true)?;
        }
        if marker.try_exists()? {
            fs::remove_file(marker)?;
        }
        Ok(None)
    }

    fn register_locked(&self, source: PathBuf, inner: &Inner) -> Result<Value> {
        self.register_with_check(source, inner, None, || Ok(()))
    }

    fn register_with_check(
        &self,
        source: PathBuf,
        inner: &Inner,
        expected_manifest: Option<&str>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Value> {
        check()?;
        self.require_external_source_boundary(&source)?;
        let manifest = if let Some(expected) = expected_manifest {
            let bytes = read_small(&source.join("panda.json"))?;
            if format!("{:x}", Sha256::digest(&bytes)) != expected {
                return Err(directory::stale());
            }
            // Parse the declaration whose bytes the user actually previewed.
            // A reload followed by a later matching digest could observe an ABA
            // swap and attach an unpreviewed declaration to the registration.
            let manifest = PandaManifest::parse(&bytes).map_err(|e| fail(e.to_string()))?;
            if manifest.runtime != PandaRuntime::Node {
                return Err(directory::stale());
            }
            manifest.inspect(&source).map_err(|e| fail(e.to_string()))?;
            manifest
        } else {
            self.load_source(&source)?
        };
        check()?;
        private_dir(&self.root)?;
        for record in self.registrations()? {
            if record.source_path == source {
                let _operation = self.admit_operation(&record.id, "重新登记项目")?;
                check()?;
                // A previous publication may have reached disk but failed its
                // parent sync. Explicit reuse must finish that durability step.
                self.sync_registration_parents(&record.id)?;
                self.project_ownership()
                    .finish_initialization_source(&record.id, &record.source_path)
                    .map_err(|error| {
                        error
                            .outcome("registered_needs_attention")
                            .detail("registrationCommitted", true)
                            .detail("requiresRecheck", true)
                            .detail("appId", record.id.clone())
                    })?;
                return self
                    .manifest(&record)
                    .and_then(|manifest| self.detail(&record, &manifest, inner))
                    .map_err(|error| {
                        error
                            .outcome("registered_needs_attention")
                            .detail("registrationCommitted", true)
                            .detail("appId", record.id.clone())
                    });
            }
        }
        let pending = self.pending_generated_source_id(&source)?;
        if pending.is_none() {
            self.require_unclaimed_source(&source)?;
        }
        let id = pending
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let slot = self.root.join(&id);
        let mut slot_identity = None;
        let preparation = (|| -> Result<ExternalRegistration> {
            if pending.is_none() {
                self.project_ownership().initialize(&id)?;
            }
            // An instance directory is created once, never adopted by retrying
            // another registration or by a coincidentally existing UUID path.
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&slot)?;
            slot_identity = Some(
                crate::file_identity::PersistedIdentity::capture(&slot)
                    .map_err(|error| fail(error.to_string()))?,
            );
            for name in ["configuration", "cache", "runs"] {
                private_dir(&slot.join(name))?;
            }
            let manifest_digest = manifest.source_digest.clone();
            if expected_manifest.is_some_and(|expected| expected != manifest_digest) {
                return Err(ApiError::new(
                    "NODE_DIRECTORY_CHANGED",
                    "应用声明已变化，请重新检查后登记。",
                )
                .outcome("not_started"));
            }
            let record = ExternalRegistration {
                version: DIRECT_NODE_REGISTRATION_VERSION,
                id: id.clone(),
                address: crate::app_address::reserve_app_address(
                    &self.data_root,
                    crate::app_address::AppAddressKind::Node,
                    &id,
                )?,
                source_path: source.clone(),
                manifest_digest,
                provenance: None,
            };
            // Establish exclusive source ownership with the initial registration.
            // Detail reads no longer upgrade missing legacy bindings.
            self.initialize_source_binding(&record)?;
            check()?;
            Ok(record)
        })();
        let record = match preparation {
            Ok(record) => record,
            Err(error) => {
                let cleanup = self.discard_unpublished_registration(
                    &id,
                    slot_identity.as_ref(),
                    pending.is_some(),
                );
                return Err(error
                    .outcome("not_started")
                    .detail("appId", id.as_str())
                    .detail("cleanupError", cleanup.err().map(|error| json!(error))));
            }
        };
        let publication = (|| {
            write_new_json(&slot.join("registration.json"), &record)?;
            self.sync_registration_parents(&record.id)
        })();
        publication.map_err(|error| {
            error
                .outcome("unknown")
                .detail("registrationAttempted", true)
                .detail("requiresRecheck", true)
                .detail("appId", record.id.clone())
        })?;
        self.project_ownership()
            .finish_initialization_source(&record.id, &record.source_path)
            .map_err(|error| {
                error
                    .outcome("registered_needs_attention")
                    .detail("registrationCommitted", true)
                    .detail("requiresRecheck", true)
                    .detail("appId", record.id.clone())
            })?;
        let id = record.id.clone();
        self.detail(&record.into(), &manifest, inner)
            .map_err(|error| {
                error
                    .outcome("registered_needs_attention")
                    .detail("registrationCommitted", true)
                    .detail("appId", id)
            })
    }

    fn sync_registration_parents(&self, id: &str) -> Result<()> {
        let synced = (|| {
            // Sync the UUID slot plus both ancestors for new publication and
            // explicit recovery of an existing registration alike.
            platform_native::filesystem::sync_directory(&self.root.join(id))?;
            platform_native::filesystem::sync_directory(&self.root)?;
            platform_native::filesystem::sync_directory(&self.data_root)?;
            Ok::<(), ApiError>(())
        })();
        synced.map_err(|error| {
            error
                .outcome("unknown")
                .detail("registrationAttempted", true)
                .detail("requiresRecheck", true)
                .detail("appId", id)
        })
    }

    pub async fn list(&self) -> Result<Vec<Value>> {
        let inner = self.inner.lock().await;
        Ok(self
            .registrations()?
            .iter()
            .filter(|record| !self.pending_delete_for(&record.id))
            .map(|record| self.available_detail(record, &inner))
            .collect())
    }

    fn available_detail(&self, record: &Registration, inner: &Inner) -> Value {
        let result = self
            .manifest(record)
            .and_then(|manifest| self.detail(record, &manifest, inner));
        let mut value = match result {
            Ok(detail) => detail,
            Err(error) => {
                // Existing resources keep the declaration captured at launch;
                // an invalid edit must not rewrite their runtime configuration.
                let previous = (|| -> Result<Value> {
                    self.require_not_removing(&record.id)?;
                    let running = inner.running.get(&record.id).ok_or_else(|| error.clone())?;
                    let mut detail = self.detail(record, &running.manifest, inner)?;
                    detail["manifestError"] = json!(error.message);
                    detail["lastError"] = json!(error.message);
                    detail["restartRequired"] = json!(true);
                    detail["configuration"]["canStart"] = json!(false);
                    Ok(detail)
                })();
                previous.unwrap_or_else(|_| json!({
                    "id":record.id,"name":record.source_path.file_name().and_then(|n|n.to_str()).unwrap_or("不可用的 Node 应用"),
                    "sourcePath":record.source_path,"runtime":"node","state":"failed",
                    "ownership":if self.project_ownership().owns_source_directory(&record.id,&record.source_path).unwrap_or(false) {"managed"} else {"external"},
                    "gitSource":record.provenance.as_ref().map(|source|json!({"url":source.source_url,"branch":source.source_branch})),
                    "objectLayout":null,
                    "restartRequired":false,"runningRevision":null,"lastError":error.message,
                    "configuration":{"unavailable":true,"revision":0,"values":{},"secretStatus":{},"missingRequired":[],"invalidFields":[],"canStart":false},"fields":[]
                }))
            }
        };
        if !inner.running.contains_key(&record.id) {
            let recovery = run_marker::inspect(
                &self.root.join(&record.id),
                &record.id,
                run_marker::boot_time_or_unknown(),
            );
            let view = match recovery {
                Ok(value) => value.view(),
                Err(_) => Some(
                    json!({"state":"unconfirmed","canStop":false,"message":run_marker::unknown().message}),
                ),
            };
            if let Some(view) = view {
                value["state"] = json!("failed");
                value["lastError"] = view["message"].clone();
                value["configuration"]["canStart"] = json!(false);
                value["runRecovery"] = view;
            }
        }
        if let Ok(deletion) = self.deletion_operation_status(&record.id) {
            if deletion["active"] == true {
                let waiting = deletion["waitingFor"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("、")
                    })
                    .unwrap_or_default();
                let message = if waiting.is_empty() {
                    match deletion["phase"].as_str() {
                        Some("stopping") => "正在关闭窗口并停止应用",
                        Some("deleting") => "正在删除",
                        _ => "正在核对操作归属",
                    }
                    .to_owned()
                } else {
                    format!("等待{waiting}完成")
                };
                value["state"] = json!("stopping");
                value["lastError"] = json!(message);
                value["configuration"]["canStart"] = json!(false);
                if let Some(object) = value.as_object_mut() {
                    object.remove("runRecovery");
                }
            }
        }
        value
    }

    pub async fn configuration(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        if self.deletion_operation_status(id)?["active"] != true {
            self.require_not_removing(id)?;
        }
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    pub async fn save(&self, id: &str, input: SaveConfigurationRequest) -> Result<Value> {
        let _permit = self.admit_operation(id, "保存配置")?;
        let _operation = self.runtime_operation(id)?.lock_owned().await;
        let inner = self.inner.lock().await;
        let record = self.registration(id)?;
        let manifest = self.manifest(&record)?;
        if input.manifest_digest.as_deref() != Some(manifest.source_digest.as_str()) {
            return Err(ApiError::new(
                "NODE_MANIFEST_CHANGED",
                "应用说明已变化，请重新读取配置后再保存。",
            ));
        }
        self.store(id)?
            .save(&manifest, input)
            .map_err(|e| fail(e.to_string()))?;
        self.detail(&record, &manifest, &inner)
    }

    pub(super) async fn forget_deleted_instance(
        &self,
        id: &str,
        permit: &NodeDeletionPermit,
    ) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.running.contains_key(id) {
            return Err(fail("应用写入尚未结束，不能清理内存状态。"));
        }
        inner.markers.remove(id);
        inner.errors.remove(id);
        drop(inner);
        self.runtime_operations
            .lock()
            .map_err(|_| fail("运行操作锁不可用。"))?
            .remove(id);
        self.operations.forget_deleting_instance(id, permit)?;
        Ok(())
    }

    fn runtime_operation(&self, id: &str) -> Result<Arc<Mutex<()>>> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用标识无效。"));
        }
        Ok(self
            .runtime_operations
            .lock()
            .map_err(|_| fail("应用操作锁不可用。"))?
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    /// The detached worker owns its permit and entry even if the caller stops
    /// waiting. It never carries the global table lock through resource waits.
    pub async fn start(&self, id: &str) -> Result<Value> {
        let permit = self.admit_operation(id, "启动应用")?;
        {
            let inner = self.inner.lock().await;
            if inner.shutting_down {
                return Err(fail("平台正在关闭，不能启动应用。"));
            }
            if inner
                .running
                .get(id)
                .is_some_and(|entry| entry.cleanup_pending())
            {
                return Err(fail(
                    "此应用尚未确认完整收尾，请先重试停止，不能启动第二个实例。",
                ));
            }
        }
        let apps = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let _operation = apps.runtime_operation(&id)?.lock_owned().await;
            apps.start_admitted(&id).await
        })
        .await
        .map_err(|_| fail("应用启动任务异常结束，已保留运行记录供核对。"))?
    }

    async fn start_admitted(&self, id: &str) -> Result<Value> {
        let (entry, record, manifest, slot, config) = {
            let mut inner = self.inner.lock().await;
            if inner.shutting_down {
                return Err(fail("平台正在关闭，不能启动应用。"));
            }
            if let Some(entry) = inner.running.get(id) {
                if entry.phase() != RUNTIME_RUNNING {
                    return Err(fail(
                        "此应用尚未确认完成启动或收尾，请先重试停止，不能启动第二个实例。",
                    ));
                }
                let record = self.registration(id)?;
                return self.detail(&record, &self.manifest(&record)?, &inner);
            }
            let mut record = self.registration(id)?;
            let manifest = Arc::new(self.manifest(&record)?);
            let slot = self.root.join(id);
            if has_start_intent_cleanup(&slot)? {
                return Err(fail(
                    "上次启动标记清理尚未确认完成，已阻止再次启动。请保留清理记录以便恢复。",
                ));
            }
            if fs::symlink_metadata(slot.join("active.json")).is_ok() {
                return Err(fail(
                    "上次运行尚未核对完成，已阻止再次启动。请查看运行状态。",
                ));
            }
            if record.manifest_digest != manifest.source_digest {
                let current = self.registration(id)?;
                if current.source_path != record.source_path
                    || current.manifest_digest != record.manifest_digest
                {
                    return Err(fail("应用登记已变化，请重新进入。"));
                }
                let updated = ExternalRegistration {
                    version: DIRECT_NODE_REGISTRATION_VERSION,
                    id: id.into(),
                    address: record.address,
                    source_path: record.source_path.clone(),
                    manifest_digest: manifest.source_digest.clone(),
                    provenance: record.provenance.clone(),
                };
                crate::builds::atomic_json(&slot.join("registration.json"), &updated)?;
                record.manifest_digest = manifest.source_digest.clone();
            }
            let store = self.store(id)?;
            let view = store.view(&manifest).map_err(|e| fail(e.to_string()))?;
            if !view.can_start {
                return Err(ApiError::new(
                    "NODE_CONFIGURATION_REQUIRED",
                    "应用说明已更新，请先完成配置后再打开。",
                )
                .detail("missingRequired", json!(view.missing_required))
                .detail("invalidFields", json!(view.invalid_fields)));
            }
            let snapshot = store
                .launch_snapshot(&manifest)
                .map_err(|e| fail(e.to_string()))?;
            let (backend, cwd) = manifest
                .resolve_backend(&record.source_path)
                .map_err(|e| fail(e.to_string()))?;
            let trial_root = slot
                .join("runs")
                .join(format!("node-runtime-trial-{}", Uuid::new_v4()));
            private_dir(&trial_root)?;
            let config = RuntimeLaunchConfig {
                node_path: self.node_path.clone(),
                expected_node_sha256: self.digest.clone(),
                entry_path: backend,
                working_directory: cwd,
                trial_root,
                business_env: snapshot.environment,
                control_directory: slot.clone(),
                redaction_values: snapshot.redaction_values,
            };
            let mut entry = RuntimeEntry::new(
                snapshot.revision,
                node_registration::registration_digest(&slot)?
                    .ok_or_else(|| fail("应用登记已失联。"))?,
                manifest.clone(),
            );
            entry.workspace_root = Some(self.deletion_workspace());
            entry.source_root = Some(record.source_path.clone());
            let entry = Arc::new(entry);
            inner.running.insert(id.into(), entry.clone());
            (entry, record, manifest, slot, config)
        };
        let _phase = RuntimePhaseGuard(entry.clone());
        let mut resources = entry.resources.lock().await;
        let launched = self
            .launch_entry(
                id,
                &entry,
                PreparedRuntimeStart {
                    address: record.address,
                    record,
                    manifest,
                    slot,
                    config,
                },
                &mut resources,
            )
            .await;
        if let Err(error) = launched {
            entry.set_phase(RUNTIME_FAILED);
            if resources.is_none() {
                let mut inner = self.inner.lock().await;
                if inner
                    .running
                    .get(id)
                    .is_some_and(|current| Arc::ptr_eq(current, &entry))
                {
                    inner.running.remove(id);
                }
            }
            return Err(error);
        }
        entry.set_phase(RUNTIME_RUNNING);
        let result = {
            let inner = self.inner.lock().await;
            self.registration(id).and_then(|record| {
                let current = self.manifest(&record)?;
                if current.source_digest != entry.manifest.source_digest {
                    return Err(ApiError::new(
                        "NODE_MANIFEST_CHANGED",
                        "应用说明在启动期间已变化，请重新进入。",
                    ));
                }
                self.detail(&record, &entry.manifest, &inner)
            })
        };
        if result.is_err() {
            // Failed first open must not leave a windowless backend. A reused
            // entry returned above and is never stopped by this rollback.
            let _ = self
                .stop_entry(
                    id,
                    &entry,
                    &mut resources,
                    RuntimeStopReason::StartupRollback,
                )
                .await;
        }
        result
    }

    async fn launch_entry(
        &self,
        id: &str,
        entry: &Arc<RuntimeEntry>,
        plan: PreparedRuntimeStart,
        resources: &mut Option<Running>,
    ) -> Result<()> {
        let PreparedRuntimeStart {
            record,
            manifest,
            slot,
            config,
            address,
        } = plan;
        // The native manager outlives a public workspace switch. Probe owners
        // must belong to the currently installed runtime, not its constructor.
        let processes = self.current_processes();
        NodeRuntimeTrial::preflight_project_with_processes(
            &config,
            &record.source_path,
            &slot.join("cache"),
            &processes,
        )
        .await?;
        let marker = slot.join("active.json");
        let mut run_record = run_marker::RunMarker::launching(id, config.trial_root.clone());
        {
            let mut inner = self.inner.lock().await;
            if inner.shutting_down {
                return Err(fail("平台正在关闭，不能启动应用。"));
            }
            // Existing admitted work may finish, but publication still uses its
            // original registration and exact intent; it never adopts a swap.
            if node_registration::registration_digest(&slot)?.as_deref()
                != Some(entry.registration_digest.as_str())
            {
                return Err(fail("应用登记已变化，不能继续启动。"));
            }
            let intent = StartIntentOwnership::create(&marker, &run_record)?;
            inner.markers.insert(id.into(), intent);
        }
        #[cfg(test)]
        {
            let pause = self.start_intent_pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                entered
                    .send(())
                    .expect("fixture observes owned start intent");
                release.await.expect("fixture releases owned start intent");
            }
        }
        let (launched, pending, cleanup) = NodeRuntimeTrial::launch_project_retaining_failed(
            config,
            record.source_path.clone(),
            slot.join("cache"),
            &processes,
        )
        .await;
        let runtime = match launched {
            Ok(runtime) => runtime,
            Err(error) => {
                let mut inner = self.inner.lock().await;
                if let Some(runtime) = pending {
                    if let Ok(identity) = runtime.process_identity() {
                        run_record.bind(identity);
                        let _ = inner
                            .markers
                            .get_mut(id)
                            .unwrap()
                            .publish(&marker, &run_record);
                    }
                    *resources = Some(Running {
                        runtime,
                        proxy: None,
                    });
                }
                if cleanup.permits_intent_removal() {
                    if let Err(ownership) = self.retire_owned_marker(id, &mut inner) {
                        inner.errors.insert(
                            id.into(),
                            "启动标记归属发生变化，已保留记录并阻止再次启动。".into(),
                        );
                        return Err(ownership.detail("launchCauseCode", error.code));
                    }
                    inner.errors.insert(
                        id.into(),
                        format!(
                            "启动未完成（{}），本次启动资源已确认关闭，可重试。",
                            error.code
                        ),
                    );
                    return Err(fail(
                        "应用启动未完成，本次启动资源已确认关闭。请检查运行环境和应用入口后重试。",
                    )
                    .detail("launchCauseCode", error.code)
                    .detail("launchCleanupConfirmed", true));
                }
                inner.errors.insert(
                    id.into(),
                    format!("启动未完成（{}），请保留运行记录。", error.code),
                );
                return Err(fail(
                    "应用启动未完成，已保留运行记录。请检查内置运行环境和应用入口。",
                ));
            }
        };
        // Persist the owner before proxy creation or any subsequent cleanup wait.
        *resources = Some(Running {
            runtime,
            proxy: None,
        });
        let binding = {
            let runtime = &resources.as_ref().unwrap().runtime;
            let mut inner = self.inner.lock().await;
            runtime.process_identity().and_then(|identity| {
                run_record.bind(identity);
                inner
                    .markers
                    .get_mut(id)
                    .unwrap()
                    .publish(&marker, &run_record)
            })
        };
        if let Err(error) = binding {
            let stopped = self
                .stop_entry(id, entry, resources, RuntimeStopReason::StartupRollback)
                .await
                .is_ok();
            return Err(ApiError::new(
                "NODE_APP_RUN_IDENTITY_UNCONFIRMED",
                "应用运行身份未能完整保存，本次启动已停止或保留收尾句柄，不能报告启动成功。",
            )
            .detail("identityCauseCode", error.code)
            .detail("launchCleanupConfirmed", stopped));
        }
        let target = resources.as_ref().unwrap().runtime.proxy_target();
        let (port, token) = match target {
            Ok(target) => target,
            Err(error) => {
                let _ = self
                    .stop_entry(id, entry, resources, RuntimeStopReason::StartupRollback)
                    .await;
                return Err(error);
            }
        };
        match NodeAppProxy::start_project_retaining_failed(
            record.source_path.clone(),
            record.source_path.join(&manifest.web.root),
            manifest.web.entry.clone(),
            port,
            token,
            address,
        )
        .await
        {
            Ok(proxy) => resources.as_mut().unwrap().proxy = Some(proxy),
            Err(failure) => {
                resources.as_mut().unwrap().proxy = Some(failure.proxy);
                let cleanup = self
                    .stop_entry(id, entry, resources, RuntimeStopReason::StartupRollback)
                    .await
                    .err();
                let mut error = failure.error;
                if let Some(cleanup) = cleanup {
                    error = error
                        .detail("shutdownIssues", cleanup.details["shutdownIssues"].clone())
                        .detail("proxyShutdown", json!(cleanup));
                }
                return Err(error);
            }
        }
        self.inner.lock().await.errors.remove(id);
        Ok(())
    }

    async fn stop_resources(
        running: &mut Running,
        reason: RuntimeStopReason,
    ) -> (Result<()>, Result<TrialStatus>) {
        let Running { runtime, proxy } = running;
        if let Some(proxy) = proxy.as_ref() {
            proxy.request_stop();
        }
        tokio::join!(
            async {
                match proxy {
                    Some(proxy) => proxy.stop().await,
                    None => Ok(()),
                }
            },
            runtime.stop_with_reason(reason)
        )
    }

    /// Caller owns this instance's runtime-operation guard. No global lock is
    /// held while waiting for its original resource owner or orphan stop.
    async fn stop_admitted(&self, id: &str, reason: RuntimeStopReason) -> Result<()> {
        let entry = self.inner.lock().await.running.get(id).cloned();
        if let Some(entry) = entry {
            let mut resources = entry.resources.lock().await;
            return self.stop_entry(id, &entry, &mut resources, reason).await;
        }
        let result =
            run_marker::inspect(&self.root.join(id), id, run_marker::boot_time_or_unknown())
                .map_err(|_| run_marker::unknown())?
                .stop_and_retire(&self.root.join(id))
                .await;
        if result.is_ok() {
            let mut inner = self.inner.lock().await;
            inner.errors.remove(id);
            inner.markers.remove(id);
        }
        result
    }

    async fn stop_entry(
        &self,
        id: &str,
        entry: &Arc<RuntimeEntry>,
        resources: &mut Option<Running>,
        reason: RuntimeStopReason,
    ) -> Result<()> {
        entry.set_phase(RUNTIME_STOPPING);
        let _phase = RuntimePhaseGuard(entry.clone());
        let result = match resources {
            Some(running) => {
                let (proxy, process) = Self::stop_resources(running, reason).await;
                let mut inner = self.inner.lock().await;
                self.finish_stop(id, entry, &mut inner, proxy, process)
            }
            None => {
                let result = run_marker::inspect(
                    &self.root.join(id),
                    id,
                    run_marker::boot_time_or_unknown(),
                )
                .map_err(|_| run_marker::unknown())?
                .stop_and_retire(&self.root.join(id))
                .await;
                if result.is_ok() {
                    let mut inner = self.inner.lock().await;
                    if inner
                        .running
                        .get(id)
                        .is_some_and(|current| Arc::ptr_eq(current, entry))
                    {
                        inner.running.remove(id);
                    }
                    inner.errors.remove(id);
                    inner.markers.remove(id);
                }
                result
            }
        };
        if result.is_ok() {
            resources.take();
        } else {
            entry.set_phase(RUNTIME_FAILED);
        }
        result
    }

    fn finish_stop(
        &self,
        id: &str,
        entry: &Arc<RuntimeEntry>,
        inner: &mut Inner,
        proxy: Result<()>,
        process: Result<TrialStatus>,
    ) -> Result<()> {
        if !inner
            .running
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            return Err(fail("应用运行归属已变化，未清理替代实例。"));
        }
        let status = match process {
            Ok(status) if status.process_group_exited => status,
            result => {
                inner.errors.insert(id.into(), "应用进程尚未确认全部关闭，已保留受管句柄。请重试停止；平台暂不允许退出或启动第二个实例。".into());
                return Err(result
                    .err()
                    .unwrap_or_else(|| fail("无法确认应用进程组已关闭。")));
            }
        };
        if let Err(error) = proxy {
            inner.errors.insert(
                id.into(),
                "应用进程已停止，网页连接或文件工作仍未确认完整关闭，已保留句柄。请重试停止。"
                    .into(),
            );
            return Err(error);
        }
        if !clean_shutdown(&status) {
            eprintln!("Node shutdown diagnostics: app={id}, state={}, forced={}, exitCode={:?}; process group and owned I/O closed", status.state, status.forced, status.exit_code);
        }
        if let Err(error) = self.retire_owned_marker(id, inner) {
            inner.errors.insert(
                id.into(),
                "应用进程已停止，但运行标记尚未确认清理完成。请重试停止；当前不能再次启动。".into(),
            );
            return Err(error);
        }
        inner.running.remove(id);
        inner.errors.remove(id);
        Ok(())
    }

    fn retire_owned_marker(&self, id: &str, inner: &mut Inner) -> Result<()> {
        let marker = inner.markers.get(id).ok_or_else(run_marker::unknown)?;
        marker.remove_if_owned(&self.root.join(id).join("active.json"))?;
        inner.markers.remove(id);
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<Value> {
        self.stop_runtime(id).await?;
        let inner = self.inner.lock().await;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    /// Closing is always admitted, including after the deletion gate seals.
    pub async fn stop_runtime(&self, id: &str) -> Result<()> {
        self.stop_runtime_with_reason(id, RuntimeStopReason::UserRequest)
            .await
    }

    pub async fn stop_runtime_with_reason(
        &self,
        id: &str,
        reason: RuntimeStopReason,
    ) -> Result<()> {
        let apps = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _operation = apps.runtime_operation(&id)?.lock_owned().await;
            apps.stop_admitted(&id, reason).await
        })
        .await
        .map_err(|_| fail("应用停止任务异常结束，已保留运行记录供核对。"))?
    }

    pub async fn running_ids(&self) -> Vec<String> {
        self.inner
            .lock()
            .await
            .running
            .iter()
            .filter(|(_, run)| run.phase() == RUNTIME_RUNNING)
            .map(|(id, _)| id.clone())
            .collect()
    }
    pub async fn owned_process_ids(&self) -> Vec<String> {
        self.inner.lock().await.running.keys().cloned().collect()
    }

    pub async fn restart(&self, id: &str) -> Result<Value> {
        let permit = self.admit_operation(id, "重启应用")?;
        let apps = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let _operation = apps.runtime_operation(&id)?.lock_owned().await;
            apps.stop_admitted(&id, RuntimeStopReason::Restart).await?;
            apps.start_admitted(&id).await
        })
        .await
        .map_err(|_| fail("应用重启任务异常结束，已保留运行记录供核对。"))?
    }

    async fn monitor(&self) {
        let entries = self
            .inner
            .lock()
            .await
            .running
            .iter()
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect::<Vec<_>>();
        for (id, entry) in entries {
            if entry.phase() != RUNTIME_RUNNING {
                continue;
            }
            let Ok(operation) = self.runtime_operation(&id) else {
                continue;
            };
            let Ok(_operation) = operation.try_lock_owned() else {
                continue;
            };
            let mut resources = entry.resources.lock().await;
            let healthy = match resources.as_mut() {
                Some(run) => {
                    let status = run.runtime.status();
                    if let Ok(status) = &status {
                        if let Some(error) = &status.log_error {
                            self.inner
                                .lock()
                                .await
                                .errors
                                .insert(id.clone(), error.clone());
                        }
                    }
                    status.is_ok_and(|s| s.state == "running")
                        && run.runtime.verify_health().await.is_ok()
                }
                None => false,
            };
            if !healthy
                && self
                    .stop_entry(
                        &id,
                        &entry,
                        &mut resources,
                        RuntimeStopReason::HealthCheckFailed,
                    )
                    .await
                    .is_ok()
            {
                self.inner.lock().await.errors.insert(
                    id,
                    "应用运行状态异常，已关闭页面入口并确认后台进程已停止。".into(),
                );
            }
        }
    }

    pub async fn status(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    pub async fn open_url(&self, id: &str) -> Result<String> {
        let _permit = self.admit_operation(id, "打开应用页面")?;
        let _operation = self.runtime_operation(id)?.lock_owned().await;
        let entry = self
            .inner
            .lock()
            .await
            .running
            .get(id)
            .cloned()
            .ok_or_else(|| fail("请先启动应用，再打开页面。"))?;
        if entry.phase() != RUNTIME_RUNNING {
            return Err(fail("应用正在启动或停止，暂时不能打开页面。"));
        }
        let mut resources = entry.resources.lock().await;
        resources
            .as_mut()
            .and_then(|r| r.proxy.as_mut())
            .map(|proxy| proxy.open_url())
            .ok_or_else(|| fail("请先启动应用，再打开页面。"))
    }

    pub async fn help_url(&self, id: &str, field: &str) -> Result<String> {
        let _inner = self.inner.lock().await;
        let record = self.registration(id)?;
        let manifest = self.manifest(&record)?;
        manifest
            .configuration
            .as_ref()
            .and_then(|c| c.fields.iter().find(|f| f.name == field))
            .and_then(|f| f.help_url.clone())
            .ok_or_else(|| fail("此配置没有声明帮助链接。"))
    }

    pub async fn stop_all(&self) -> Result<()> {
        let apps = self.clone();
        tokio::spawn(async move { apps.stop_all_owned().await })
            .await
            .map_err(|_| fail("平台停止任务异常结束，运行归属仍保留。"))?
    }
    async fn stop_all_owned(&self) -> Result<()> {
        self.directory_stopping.store(true, Ordering::Release);
        let ids = {
            let mut inner = self.inner.lock().await;
            inner.shutting_down = true;
            inner.running.keys().cloned().collect::<Vec<_>>()
        };
        {
            let _directory = self
                .directory_changes
                .lock()
                .map_err(|_| fail("目录初始化锁不可用。"))?;
        }
        let results = futures_util::future::join_all(
            ids.iter()
                .map(|id| self.stop_runtime_with_reason(id, RuntimeStopReason::PlatformExit)),
        )
        .await;
        results.into_iter().collect::<Result<Vec<_>>>()?;
        self.wait_deletion_workers().await
    }

    pub async fn monitor_tick(&self) {
        let apps = self.clone();
        let _ = tokio::spawn(async move { apps.monitor().await }).await;
    }

    #[cfg(test)]
    async fn fixture_running(
        &self,
        id: &str,
    ) -> tokio::sync::OwnedMappedMutexGuard<Option<Running>, Running> {
        let resources = self.inner.lock().await.running[id].resources.clone();
        tokio::sync::OwnedMutexGuard::map(resources.lock_owned().await, |value| {
            value.as_mut().unwrap()
        })
    }
}

fn digest_manifest(source: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(read_small(&source.join("panda.json"))?)
    ))
}

/// Same-boot cleanup evidence is a fail-closed gate. Earlier-boot evidence is
/// retained on disk but cannot represent a currently running worker.
pub(crate) fn has_start_intent_cleanup(slot: &Path) -> Result<bool> {
    run_marker::cleanup_pending(slot, run_marker::boot_time_or_unknown())
}

#[cfg(test)]
mod native_directory_io_tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn ordinary_path_rejects_a_real_junction_and_its_children_without_changing_the_target() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-node-path-junction-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let target = root.join("disposable-target");
        let link = root.join("disposable-junction");
        fs::create_dir(&target).unwrap();
        let payload = target.join("record.json");
        fs::write(&payload, b"disposable retained bytes").unwrap();
        let identity = crate::file_identity::PersistedIdentity::capture(&target).unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not create disposable junction"
        );
        assert!(ordinary_path(&link, true).is_err());
        assert!(ordinary_path(&link.join("record.json"), false).is_err());
        assert!(identity.matches_path(&target).unwrap());
        assert_eq!(fs::read(&payload).unwrap(), b"disposable retained bytes");
        fs::remove_dir(&link).unwrap();
        ordinary_path(&target, true).unwrap();
        assert_eq!(fs::read(&payload).unwrap(), b"disposable retained bytes");
    }

    #[test]
    fn canonical_directory_observation_keeps_file_types_and_missing_paths_distinct() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-node-native-directory-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let file = root.join("record.json");
        fs::write(&file, b"disposable original").unwrap();
        ordinary_path(&root, true).unwrap();
        ordinary_path(&file, false).unwrap();
        assert!(ordinary_path(&root, false).is_err());
        assert!(ordinary_path(&file, true).is_err());
        let missing = root.join("missing");
        assert!(ordinary_path(&missing, true).is_err());
        assert!(!missing.exists());
        assert_eq!(fs::read(&file).unwrap(), b"disposable original");
    }

    #[test]
    fn json_and_start_intent_keep_file_writes_and_directory_observation() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-node-native-write-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let record = root.join("record.json");
        let value = json!({"disposable": "中文 persisted bytes"});
        write_new_json(&record, &value).unwrap();
        let bytes = serde_json::to_vec_pretty(&value).unwrap();
        assert_eq!(fs::read(&record).unwrap(), bytes);
        assert!(write_new_json(&record, &json!({"replacement": true})).is_err());
        assert_eq!(fs::read(&record).unwrap(), bytes);

        let intent = root.join("start-intent.json");
        let guard = StartIntentOwnership::create(&intent, &value).unwrap();
        assert_eq!(guard.contents, bytes);
        assert_eq!(fs::read(&intent).unwrap(), bytes);
        drop(guard);

        let missing = root.join("missing").join("record.json");
        assert!(write_new_json(&missing, &value).is_err());
        assert!(!missing.parent().unwrap().exists());
        let error =
            platform_native::filesystem::sync_directory(missing.parent().unwrap()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!missing.parent().unwrap().exists());
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "node_apps_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "node_apps_manifest_refresh_tests.rs"]
mod manifest_refresh_tests;

#[cfg(test)]
mod dependency_hint_tests {
    use super::*;

    #[tokio::test]
    async fn detail_refreshes_dependency_advice_without_changing_registration_or_configuration() {
        let (temporary, paths) = crate::RuntimePaths::for_test();
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-dependency-advice");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('must not execute');",
        )
        .unwrap();
        let manifest = json!({
            "manifestVersion":1,"runtime":"node",
            "app":{"name":"Disposable advice","version":"1.0.0"},
            "web":{"root":"public","entry":"index.html"},
            "backend":{"root":"server","entry":"main.mjs","protocolVersion":1,"nodeMajor":24}
        });
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let package = br#"{"dependencies":{"example":"1.0.0"}}"#;
        fs::write(source.join("package.json"), package).unwrap();
        let apps = NodeApps::new(
            paths.data_root().into(),
            source.join("disposable-missing-runtime-node"),
        )
        .unwrap();
        let added = apps.register(source.clone()).await.unwrap();
        assert_eq!(added["dependencyWarnings"].as_array().unwrap().len(), 1);
        assert_eq!(added["state"], "stopped");
        assert_eq!(added["configuration"]["canStart"], true);
        let id = added["id"].as_str().unwrap();
        let registration = apps.root.join(id).join("registration.json");
        let registration_before = fs::read(&registration).unwrap();
        let before = apps.configuration(id).await.unwrap();
        fs::create_dir(source.join("node_modules")).unwrap();
        let refreshed = apps.configuration(id).await.unwrap();
        assert_eq!(refreshed["dependencyWarnings"], json!([]));
        assert_eq!(refreshed["configuration"], before["configuration"]);
        assert_eq!(refreshed["state"], before["state"]);
        assert_eq!(fs::read(&registration).unwrap(), registration_before);
        assert_eq!(fs::read(source.join("package.json")).unwrap(), package);
    }
}

#[cfg(all(test, windows))]
#[path = "node_apps/windows_runtime_tests.rs"]
mod windows_runtime_tests;
