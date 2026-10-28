//! Native-manager-only source applications, separate from project-v1 storage.
//! Registration reads declarations; only an explicit start executes trusted code.

use crate::{
    node_app_configuration::{LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest},
    node_app_install::{self, InstallTargetSnapshot, ManagedNodeRegistration},
    node_app_manifest::{TrialManifest, TrialRuntime},
    node_app_proxy::NodeAppProxy,
    node_runtime_trial::{NodeRuntimeTrial, RuntimeLaunchConfig, RuntimeStopReason, TrialStatus},
    ApiError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;
use uuid::Uuid;

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
#[path = "node_run_marker.rs"]
mod run_marker;
pub use directory::DirectoryInitializeInput;
#[path = "node_app_development.rs"]
pub(crate) mod development;
pub use development::{NodeDevelopmentSource, NodeDevelopmentSourceLease};
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
struct ExternalRegistration {
    version: u32,
    id: String,
    source_path: PathBuf,
    manifest_digest: String,
}

struct Registration {
    id: String,
    source_path: PathBuf,
    manifest_digest: String,
    managed: Option<ManagedNodeRegistration>,
}
impl From<ExternalRegistration> for Registration {
    fn from(record: ExternalRegistration) -> Self {
        Self {
            id: record.id,
            source_path: record.source_path,
            manifest_digest: record.manifest_digest,
            managed: None,
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
    phase: AtomicU8,
}
impl RuntimeEntry {
    fn new(revision: u64, registration_digest: String) -> Self {
        Self {
            resources: Arc::new(Mutex::new(None)),
            revision,
            registration_digest,
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
    manifest: TrialManifest,
    slot: PathBuf,
    config: RuntimeLaunchConfig,
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
    development_active: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
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
            let release: Value =
                serde_json::from_str(include_str!("../../../scripts/node-runtime-release.json"))?;
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
            development_active: Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new())),
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

    fn load_source(&self, source: &Path) -> Result<TrialManifest> {
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
        let manifest = TrialManifest::load(source).map_err(|e| fail(e.to_string()))?;
        if manifest.runtime != TrialRuntime::NodeService {
            return Err(fail(
                "此入口只登记带 Node 服务的应用。网页项目请继续使用我的项目。",
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
            let version = value["version"].as_u64();
            let record = match version {
                Some(1) => {
                    let record: ExternalRegistration = serde_json::from_value(value)?;
                    Registration::from(record)
                }
                Some(2) => {
                    let record: ManagedNodeRegistration = serde_json::from_value(value)?;
                    let source_path = node_app_install::managed_program_path(&self.root, &record)?;
                    Registration {
                        id: record.id.clone(),
                        source_path,
                        manifest_digest: record.manifest_digest.clone(),
                        managed: Some(record),
                    }
                }
                _ => return Err(fail("Node 应用登记版本无效，请保留现场。")),
            };
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

    fn manifest(&self, record: &Registration) -> Result<TrialManifest> {
        let manifest = if let Some(managed) = &record.managed {
            let program = node_app_install::verify_managed_program(&self.root, managed)?;
            let manifest =
                TrialManifest::load(&program).map_err(|error| fail(error.to_string()))?;
            if manifest.runtime != TrialRuntime::NodeService {
                return Err(fail("受管安装必须为 Node 服务应用。"));
            }
            manifest
                .inspect(&program)
                .map_err(|error| fail(error.to_string()))?;
            manifest
        } else {
            self.load_source(&record.source_path)?
        };
        if digest_manifest(&record.source_path)? != record.manifest_digest {
            return Err(fail(
                "应用说明已更改，当前登记无法核对。请让 AI 检查并恢复已登记的说明后再进入项目。",
            ));
        }
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
        manifest: &TrialManifest,
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
        // Ordinary detail/configuration reads do not establish a writable
        // development association. That requires the explicit development entry.
        let development_source = self
            .read_development_binding(&record.id)
            .ok()
            .flatten()
            .or_else(|| {
                record
                    .managed
                    .as_ref()
                    .and_then(|managed| managed.development_source.clone())
            })
            .filter(|binding| binding.validate_record().is_ok());
        let mut object_layout = crate::object_layout::node_layout(
            &record.source_path,
            &self.root.join(&record.id),
            Some(manifest),
        );
        if record.managed.is_some() {
            object_layout["source"] = json!({"state":"unverified","path":null,"reason":"当前程序是平台独立安装的运行副本；来源候选记录另行保留，不将该副本视为已确认的开发源码。"});
            object_layout["runtime"]["external"] = json!(false);
            object_layout["build"] = json!({"state":"unavailable","reason":"此程序由独立安装候选准备；打开应用不会安装依赖，普通更新须重新准备并核验候选。"});
        }
        if let Some(source) = &development_source {
            let available = source
                .source(&record.id, record.managed.is_none())
                .validate()
                .is_ok();
            object_layout["source"] = json!({"state":if available {"available"} else {"missing"},"path":source.source_root,"reason":if available {""} else {"源码目录已变化，请重新绑定。"}});
            object_layout["build"] = development::build_capability(source, available);
            if available {
                let source_manifest = TrialManifest::load(&source.source_root).ok();
                object_layout["versioning"] = crate::object_layout::node_layout(
                    &source.source_root,
                    &self.root.join(&record.id),
                    source_manifest.as_ref(),
                )["versioning"]
                    .clone();
                object_layout["versioning"]["state"] = json!("source-git");
                object_layout["versioning"]["reason"] =
                    json!("在“源码版本”中查看，保存版本和分支由外部 AI 管理；尚未核实远端同步。");
            }
        }
        Ok(json!({
            "id":record.id,"name":manifest.app.name,"version":manifest.app.version,"sourcePath":record.source_path,"runtime":"node-service","window":manifest.window,
            "dataContract":manifest.data_contract,
            "objectLayout":object_layout,
            "development":development_source.as_ref().map(|source|json!({"sourcePath":source.source_root,"runtimeIsSource":record.managed.is_none(),"manifestId":source.manifest_id})),
            "installation":record.managed.as_ref().map(|managed| json!({"kind":"managed","generation":managed.generation,"sourceId":managed.provenance.source_id,"sourceDigest":managed.provenance.source_digest})),
            "state":state,"restartRequired":running.is_some_and(|r|r.revision != view.revision),
            "runningRevision":running.map(|r|r.revision),"lastError":error,
            "configuration":{"revision":view.revision,"values":view.saved_values,"secretStatus":view.secrets,
                "missingRequired":view.missing_required,"canStart":view.can_start && runtime_phase != Some(RUNTIME_STARTING) && !cleanup_pending && (!marker || running.is_some()) && !inner.shutting_down},
            "fields":inspected["configuration"]["fields"]
        }))
    }

    pub async fn register(&self, source: PathBuf) -> Result<Value> {
        let inner = self.inner.lock().await;
        self.register_locked(source, &inner)
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
        let manifest = if let Some(expected) = expected_manifest {
            let bytes = read_small(&source.join("local-app.json"))?;
            if format!("{:x}", Sha256::digest(&bytes)) != expected {
                return Err(directory::stale());
            }
            // Parse the declaration whose bytes the user actually previewed.
            // A reload followed by a later matching digest could observe an ABA
            // swap and attach an unpreviewed declaration to the registration.
            let manifest = TrialManifest::parse(&bytes).map_err(|e| fail(e.to_string()))?;
            if manifest.runtime != TrialRuntime::NodeService {
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
            for name in ["configuration", "data", "cache", "runs"] {
                private_dir(&slot.join(name))?;
            }
            let manifest_digest = digest_manifest(&source)?;
            if expected_manifest.is_some_and(|expected| expected != manifest_digest) {
                return Err(ApiError::new(
                    "NODE_DIRECTORY_CHANGED",
                    "应用声明已变化，请重新检查后登记。",
                )
                .outcome("not_started"));
            }
            let record = ExternalRegistration {
                version: 1,
                id: id.clone(),
                source_path: source.clone(),
                manifest_digest,
            };
            // Establish exclusive source ownership with the initial registration.
            // Detail reads no longer upgrade missing legacy bindings.
            self.initialize_development_binding(&record)?;
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
            fs::File::open(self.root.join(id))?.sync_all()?;
            fs::File::open(&self.root)?.sync_all()?;
            fs::File::open(&self.data_root)?.sync_all()?;
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

    /// Preparation reads only this public snapshot. It never obtains credential
    /// values or starts a runtime, and cannot adopt an external registration.
    pub(crate) async fn install_target_snapshot(
        &self,
        id: Option<&str>,
    ) -> Result<InstallTargetSnapshot> {
        let _permit = id
            .map(|id| self.admit_operation(id, "准备更新"))
            .transpose()?;
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能准备安装。"));
        }
        id.map_or_else(
            || Ok(InstallTargetSnapshot::fresh()),
            |id| self.install_target_locked(id, &inner),
        )
    }

    fn require_install_stopped(&self, id: &str, inner: &Inner) -> Result<()> {
        self.require_not_removing(id)?;
        self.require_no_source_restore(id)?;
        self.require_no_development_operation(id)?;
        if inner.shutting_down || inner.running.contains_key(id) {
            return Err(ApiError::new(
                "NODE_INSTALL_BUSY",
                "请先正常关闭应用，待运行进程完成收尾后再更新。",
            ));
        }
        let marker = self.root.join(id).join("active.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if marker.try_exists()? || has_start_intent_cleanup(&self.root.join(id))? {
            return Err(ApiError::new(
                "NODE_INSTALL_BUSY",
                "运行记录尚未完成收尾，不能更新；请保留现场。",
            ));
        }
        Ok(())
    }

    fn install_target_locked(&self, id: &str, inner: &Inner) -> Result<InstallTargetSnapshot> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用实例标识无效。"));
        }
        self.require_install_stopped(id, inner)?;
        let record = self.registration(id)?;
        let binding = self.development_binding(&record)?;
        if record.managed.is_none() {
            binding
                .as_ref()
                .ok_or_else(|| fail("请先确认应用源码目录。"))?
                .source(id, true)
                .validate()?;
        }
        let manifest_value = if record.managed.is_some() {
            serde_json::from_slice(&read_small(&record.source_path.join("local-app.json"))?)?
        } else {
            binding
                .as_ref()
                .ok_or_else(|| fail("请先确认应用源码目录。"))?
                .registered_manifest()?
        };
        let manifest = TrialManifest::parse(&serde_json::to_vec(&manifest_value)?)
            .map_err(|error| fail(error.to_string()))?;
        if record.managed.is_some() {
            self.manifest(&record)?;
        }
        let revision = self
            .store(id)?
            .view(&manifest)
            .map_err(|error| fail(error.to_string()))?
            .revision;
        Ok(InstallTargetSnapshot {
            app_id: id.into(),
            registration_digest: node_app_install::registration_digest(&self.root.join(id))?,
            generation: record
                .managed
                .as_ref()
                .map(|managed| managed.generation.clone()),
            manifest: Some(manifest_value),
            configuration_revision: revision,
            external_registration: record
                .managed
                .is_none()
                .then(|| read_small(&self.root.join(id).join("registration.json")))
                .transpose()?
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()?,
            development_source: binding,
        })
    }

    /// Native callers hold the window-operation lock outside this method. All
    /// NodeApps start/restart/save/register calls share `inner`, so the fresh
    /// snapshot/configuration check and pointer publication cannot interleave.
    /// The closure must be synchronous, publish only the prepared candidate,
    /// and report its actual commit outcome even if final bookkeeping fails.
    pub(crate) async fn with_install_commit<T>(
        &self,
        expected: &InstallTargetSnapshot,
        candidate_manifest: &Value,
        commit: impl FnOnce(&InstallTargetSnapshot) -> Result<T>,
    ) -> Result<T> {
        expected.validate()?;
        let _operation = self.runtime_operation(&expected.app_id)?.lock_owned().await;
        let mut inner = self.inner.lock().await;
        self.require_install_stopped(&expected.app_id, &inner)?;
        let fresh = if expected.registration_digest.is_some() {
            self.install_target_locked(&expected.app_id, &inner)?
        } else {
            let slot = self.root.join(&expected.app_id);
            crate::projects::check_no_symlinks(&slot, true)?;
            if slot.try_exists()? {
                return Err(fail("新安装目标目录已存在，请保留现场并重新准备。"));
            }
            expected.clone()
        };
        if fresh != *expected {
            return Err(ApiError::new(
                "NODE_INSTALL_CHANGED",
                "应用或配置已变化，请重新准备安装候选。",
            ));
        }
        node_app_install::compatible(expected, candidate_manifest)?;
        let manifest = TrialManifest::parse(&serde_json::to_vec(candidate_manifest)?)
            .map_err(|error| fail(error.to_string()))?;
        if manifest.runtime != TrialRuntime::NodeService {
            return Err(fail("受管安装只支持 Node 服务应用。"));
        }
        if expected.registration_digest.is_some() {
            // view checks bindings by metadata only. Missing required values do
            // not prevent a code update; only explicit start may read secrets.
            let view = self
                .store(&expected.app_id)?
                .view(&manifest)
                .map_err(|error| fail(error.to_string()))?;
            if view.revision != expected.configuration_revision {
                return Err(ApiError::new(
                    "NODE_INSTALL_CHANGED",
                    "配置已变化，请重新准备。",
                ));
            }
        }
        let result = commit(&fresh)?;
        inner.errors.remove(&expected.app_id);
        Ok(result)
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
                // A developer may edit the external declaration before its
                // first managed publication. Show the registered configuration
                // snapshot, while keeping launch blocked until explicit apply.
                let previous = (|| -> Result<Value> {
                    self.require_not_removing(&record.id)?;
                    if record.managed.is_some() {
                        return Err(error.clone());
                    }
                    let binding = self
                        .read_development_binding(&record.id)?
                        .ok_or_else(|| error.clone())?;
                    binding.validate_record()?;
                    if binding.source_root != record.source_path
                        || binding.registered_manifest_digest != record.manifest_digest
                    {
                        return Err(error.clone());
                    }
                    let manifest =
                        TrialManifest::parse(&serde_json::to_vec(&binding.registered_manifest()?)?)
                            .map_err(|_| error.clone())?;
                    let mut detail = self.detail(record, &manifest, inner)?;
                    detail["state"] = json!("failed");
                    detail["lastError"] = json!(error.message);
                    detail["configuration"]["canStart"] = json!(false);
                    Ok(detail)
                })();
                previous.unwrap_or_else(|_| json!({
                    "id":record.id,"name":record.source_path.file_name().and_then(|n|n.to_str()).unwrap_or("不可用的 Node 应用"),
                    "sourcePath":record.source_path,"runtime":"node-service","state":"failed",
                    "installation":record.managed.as_ref().map(|managed| json!({"kind":"managed","generation":managed.generation,"sourceId":managed.provenance.source_id,"sourceDigest":managed.provenance.source_digest})),
                    "objectLayout":null,"dataContract":null,
                    "restartRequired":false,"runningRevision":null,"lastError":error.message,
                    "configuration":{"unavailable":true,"revision":0,"values":{},"secretStatus":{},"missingRequired":[],"canStart":false},"fields":[]
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
        self.require_no_development_operation(id)?;
        let record = self.registration(id)?;
        let manifest = self.manifest(&record)?;
        self.store(id)?
            .save(&manifest, input)
            .map_err(|e| fail(e.to_string()))?;
        self.detail(&record, &manifest, &inner)
    }

    pub(super) async fn forget_deleted_instance(&self, id: &str) -> Result<()> {
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
        self.operations.forget_instance(id)?;
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
            self.require_no_development_operation(id)?;
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
            let record = self.registration(id)?;
            let manifest = self.manifest(&record)?;
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
            let snapshot = self
                .store(id)?
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
            let entry = Arc::new(RuntimeEntry::new(
                snapshot.revision,
                node_app_install::registration_digest(&slot)?
                    .ok_or_else(|| fail("应用登记已失联。"))?,
            ));
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
            self.registration(id)
                .and_then(|record| self.detail(&record, &self.manifest(&record)?, &inner))
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
        } = plan;
        NodeRuntimeTrial::preflight_with_processes(
            &config,
            &slot.join("data"),
            &slot.join("cache"),
            &self.processes,
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
            if node_app_install::registration_digest(&slot)?.as_deref()
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
        let (launched, pending, cleanup) = NodeRuntimeTrial::launch_retaining_failed(
            config,
            slot.join("data"),
            slot.join("cache"),
            &self.processes,
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
        match NodeAppProxy::start_retaining_failed(
            record.source_path.join(&manifest.web.root),
            manifest.web.entry.clone(),
            port,
            token,
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
        Sha256::digest(read_small(&source.join("local-app.json"))?)
    ))
}

/// Same-boot cleanup evidence is a fail-closed gate. Earlier-boot evidence is
/// retained on disk but cannot represent a currently running worker.
pub(crate) fn has_start_intent_cleanup(slot: &Path) -> Result<bool> {
    run_marker::cleanup_pending(slot, run_marker::boot_time_or_unknown())
}

fn ordinary_path(path: &Path, directory: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(fail("应用路径必须为规范的绝对路径。"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(fail("应用路径不能经过符号链接。"));
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(fail("应用文件类型不符。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !directory && metadata.nlink() != 1 {
            return Err(fail("应用记录不能是硬链接。"));
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    ordinary_path(path.parent().ok_or_else(|| fail("应用目录无效。"))?, true)?;
    if !path.try_exists()? {
        fs::create_dir(path)?;
    }
    ordinary_path(path, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_small(path: &Path) -> Result<Vec<u8>> {
    ordinary_path(path, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut bytes = Vec::new();
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > 256 * 1024 {
            return Err(fail("应用记录必须为大小受限且不共享硬链接的普通文件。"));
        }
    }
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Err(fail("应用记录超出大小限制。"));
    }
    Ok(bytes)
}

#[derive(Clone, Copy)]
enum IntentCleanupPoint {
    BeforeCapture,
    Captured,
}

/// This open file reserves the identity of precisely the intent we created.
/// It has no automatic cleanup: uncertainty and host crashes stay fail-closed.
struct StartIntentOwnership {
    file: fs::File,
    contents: Vec<u8>,
}

impl StartIntentOwnership {
    #[cfg(unix)]
    fn open_existing(path: &Path) -> Result<Self> {
        ordinary_path(path, false)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        #[cfg(unix)]
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.len() > 256 * 1024
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(run_marker::unknown());
        }
        let mut contents = Vec::new();
        (&mut file)
            .take(256 * 1024 + 1)
            .read_to_end(&mut contents)?;
        if contents.len() > 256 * 1024 {
            return Err(run_marker::unknown());
        }
        let current = fs::symlink_metadata(path)?;
        if current.dev() != metadata.dev()
            || current.ino() != metadata.ino()
            || current.nlink() != 1
        {
            return Err(run_marker::unknown());
        }
        Ok(Self { file, contents })
    }

    #[cfg(not(unix))]
    fn open_existing(_path: &Path) -> Result<Self> {
        Err(run_marker::unknown())
    }

    #[cfg(unix)]
    fn publish(&mut self, path: &Path, value: &impl Serialize) -> Result<()> {
        use std::io::{Seek, SeekFrom};
        let changed = || fail("运行标记的归属或内容变化，已保留记录；请核对后再启动。");
        let owned = self.file.metadata()?;
        let before = fs::symlink_metadata(path)?;
        if before.dev() != owned.dev()
            || before.ino() != owned.ino()
            || before.nlink() != 1
            || read_small(path)? != self.contents
        {
            return Err(changed());
        }
        let contents = serde_json::to_vec_pretty(value)?;
        // Keep the original inode and creation time. A crash during this second
        // phase leaves an invalid/incomplete present marker, hence unconfirmed.
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(&contents)?;
        self.file.sync_all()?;
        self.contents = contents;
        let after = fs::symlink_metadata(path)?;
        if after.dev() != owned.dev()
            || after.ino() != owned.ino()
            || after.nlink() != 1
            || read_small(path)? != self.contents
        {
            return Err(changed());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn publish(&mut self, _path: &Path, _value: &impl Serialize) -> Result<()> {
        Err(run_marker::unknown())
    }

    fn create(path: &Path, value: &impl Serialize) -> Result<Self> {
        ordinary_path(path.parent().ok_or_else(|| fail("记录目录无效。"))?, true)?;
        let contents = serde_json::to_vec_pretty(value)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(Self { file, contents })
    }

    fn remove_if_owned(&self, path: &Path) -> Result<()> {
        self.remove_if_owned_with(path, |_| ())
    }

    fn remove_if_owned_with(
        &self,
        path: &Path,
        mut observe: impl FnMut(IntentCleanupPoint),
    ) -> Result<()> {
        let changed = || fail("启动标记归属或内容发生变化，已保留记录，不能解除启动保护。");
        ordinary_path(path, false).map_err(|_| changed())?;
        let owned = self.file.metadata()?;
        let check = |current: &fs::Metadata| -> Result<()> {
            if !owned.is_file() || !current.is_file() || current.file_type().is_symlink() {
                return Err(changed());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if owned.dev() != current.dev()
                    || owned.ino() != current.ino()
                    || owned.nlink() != 1
                    || current.nlink() != 1
                {
                    return Err(changed());
                }
                Ok(())
            }
            #[cfg(not(unix))]
            Err(changed())
        };
        check(&fs::symlink_metadata(path)?)?;
        if read_small(path).map_err(|_| changed())? != self.contents {
            return Err(changed());
        }
        check(&fs::symlink_metadata(path)?)?;
        observe(IntentCleanupPoint::BeforeCapture);
        let parent = path.parent().ok_or_else(changed)?;
        let cleanup = parent.join(format!("{START_INTENT_CLEANUP_PREFIX}{}", Uuid::new_v4()));
        let mut directory = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        // Persist the fail-closed gate before removing anything from active.
        // A crash or any subsequent error leaves this evidence for recovery.
        directory.create(&cleanup)?;
        fs::File::open(&cleanup)?.sync_all()?;
        fs::File::open(parent)?.sync_all()?;
        let captured = cleanup.join("captured-intent");
        move_start_intent_exclusive(path, &captured).map_err(|_| changed())?;
        observe(IntentCleanupPoint::Captured);
        let captured_is_owned = (|| -> Result<()> {
            ordinary_path(&captured, false).map_err(|_| changed())?;
            check(&fs::symlink_metadata(&captured)?)?;
            if read_small(&captured).map_err(|_| changed())? != self.contents {
                return Err(changed());
            }
            check(&fs::symlink_metadata(&captured)?)
        })();
        if captured_is_owned.is_err() {
            // Restore only if active is still absent. A newer active marker is
            // never overwritten; either way the captured foreign object and/or
            // its restored active path survive, with the cleanup gate retained.
            let _ = move_start_intent_exclusive(&captured, path);
            return Err(changed());
        }
        // Unlink only the verified object inside our exclusive private slot.
        // Never unlink the mutable public active path after a check.
        fs::remove_file(&captured)?;
        fs::File::open(&cleanup)?.sync_all()?;
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(changed()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        fs::remove_dir(&cleanup)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

/// The same dirfd + exclusive rename primitive used by restoration transactions.
/// Paths may be replaced at the active name; displaced objects are never overwritten.
fn move_start_intent_exclusive(from: &Path, to: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            ffi::CString,
            os::{
                fd::AsRawFd,
                unix::{
                    ffi::OsStrExt,
                    fs::{MetadataExt, OpenOptionsExt},
                },
            },
        };
        let open_parent = |path: &Path| -> Result<fs::File> {
            let parent = path.parent().ok_or_else(|| fail("启动标记目录无效。"))?;
            ordinary_path(parent, true)?;
            Ok(OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(parent)?)
        };
        let left = open_parent(from)?;
        let right = open_parent(to)?;
        if left.metadata()?.dev() != right.metadata()?.dev() {
            return Err(fail("启动标记清理必须留在原有数据卷。"));
        }
        let name = |path: &Path| -> Result<CString> {
            CString::new(
                path.file_name()
                    .ok_or_else(|| fail("启动标记名称无效。"))?
                    .as_bytes(),
            )
            .map_err(|_| fail("启动标记名称无效。"))
        };
        let from = name(from)?;
        let to = name(to)?;
        if unsafe {
            libc::renameatx_np(
                left.as_raw_fd(),
                from.as_ptr(),
                right.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(fail("启动标记原子转移未完成，现有标记与清理记录均保留。"));
        }
        left.sync_all()?;
        right.sync_all()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(fail("启动标记清理目前仅验证 macOS。"))
    }
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    ordinary_path(path.parent().ok_or_else(|| fail("记录目录无效。"))?, true)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "node_apps_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "node_apps_install_tests.rs"]
mod install_tests;
