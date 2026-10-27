//! Native-manager-only source applications, separate from project-v1 storage.
//! Registration reads declarations; only an explicit start executes trusted code.

use crate::{
    node_app_configuration::{LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest},
    node_app_install::{self, InstallTargetSnapshot, ManagedNodeRegistration},
    node_app_manifest::{TrialManifest, TrialRuntime},
    node_app_proxy::NodeAppProxy,
    node_runtime_trial::{NodeRuntimeTrial, RuntimeLaunchConfig, TrialStatus},
    ApiError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[path = "node_app_delete.rs"]
mod deletion;
pub use deletion::DeleteNodeAppInput;
#[path = "node_app_directory.rs"]
mod directory;
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
    removal_pending: bool,
}
impl From<ExternalRegistration> for Registration {
    fn from(record: ExternalRegistration) -> Self {
        Self {
            id: record.id,
            source_path: record.source_path,
            manifest_digest: record.manifest_digest,
            managed: None,
            removal_pending: false,
        }
    }
}

struct Running {
    runtime: NodeRuntimeTrial,
    proxy: Option<NodeAppProxy>,
    revision: u64,
    cleanup_pending: bool,
}
#[derive(Default)]
struct Inner {
    running: BTreeMap<String, Running>,
    errors: BTreeMap<String, String>,
    shutting_down: bool,
}

#[cfg(test)]
type StartIntentPause = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

pub struct NodeApps {
    root: PathBuf,
    data_root: PathBuf,
    node_path: PathBuf,
    digest: String,
    processes: crate::owned_process::OwnedProcessRegistry,
    inner: Arc<Mutex<Inner>>,
    development_active: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    directory_changes: std::sync::Mutex<()>,
    directory_stopping: std::sync::atomic::AtomicBool,
    source_workspace: std::sync::RwLock<Option<PathBuf>>,
    #[cfg(test)]
    start_intent_pause: std::sync::Mutex<Option<StartIntentPause>>,
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
        let apps = Self::with_processes(data_root, node_path, runtime.store().processes())?;
        apps.set_source_workspace(runtime.info().workspace_root)?;
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
        Ok(Self {
            root,
            data_root,
            node_path,
            digest,
            processes,
            inner: Arc::new(Mutex::new(Inner::default())),
            development_active: Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new())),
            directory_changes: std::sync::Mutex::new(()),
            directory_stopping: std::sync::atomic::AtomicBool::new(false),
            source_workspace: std::sync::RwLock::new(None),
            #[cfg(test)]
            start_intent_pause: std::sync::Mutex::new(None),
        })
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
        self.source_workspace
            .read()
            .ok()
            .and_then(|w| w.as_ref().map(|w| w.join("project-sources")))
            .is_some_and(|root| {
                source.strip_prefix(root).is_ok_and(|relative| {
                    relative.components().count() == 1
                        && matches!(relative.components().next(), Some(Component::Normal(_)))
                })
            })
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
            let mut value: Value = serde_json::from_slice(&bytes)?;
            let removal_pending = match value
                .as_object_mut()
                .and_then(|record| record.remove("removalPending"))
            {
                None => false,
                Some(Value::Bool(value)) => value,
                Some(_) => return Err(fail("项目当前移除状态无效。")),
            };
            // Older releases used a separate deletion receipt. Never interpret
            // its retained slot as a fresh app after an interrupted deletion.
            // This only checks the old marker; new deletions never create one.
            let legacy_deletion = self.root.join(".deletions").join(format!("{name}.json"));
            crate::projects::check_no_symlinks(&legacy_deletion, true)?;
            let removal_pending = removal_pending || legacy_deletion.try_exists()?;
            let version = value["version"].as_u64();
            let mut record = match version {
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
                        removal_pending,
                    }
                }
                _ => return Err(fail("Node 应用登记版本无效，请保留现场。")),
            };
            record.removal_pending = removal_pending;
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
        if self.registration(id)?.removal_pending {
            return Err(ApiError::new(
                "NODE_APP_REMOVAL_PENDING",
                "项目已开始移除，不能运行或重新创建配置。请重新核对范围后完成移除。",
            ));
        }
        Ok(())
    }

    fn manifest(&self, record: &Registration) -> Result<TrialManifest> {
        if record.removal_pending {
            return Err(ApiError::new(
                "NODE_APP_REMOVAL_PENDING",
                "项目已开始移除，请核对范围后完成移除。",
            ));
        }
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
            return Err(fail("应用说明已更改，请从开发工具构建并更新后再打开。"));
        }
        Ok(manifest)
    }

    fn store(&self, id: &str) -> Result<NodeConfigurationStore> {
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
            .filter(|run| !run.cleanup_pending);
        let marker = self
            .root
            .join(&record.id)
            .join("active.json")
            .try_exists()?;
        let cleanup_pending = has_start_intent_cleanup(&self.root.join(&record.id))?;
        let error = if cleanup_pending {
            Some(if running.is_some() {
                "应用仍在运行，启动收尾记录需要核对；已阻止再次启动，请保留运行记录。".into()
            } else {
                "上次启动标记清理尚未确认完成，已阻止再次启动。请保留清理记录以便恢复。".into()
            })
        } else {
            inner.errors.get(&record.id).cloned().or_else(|| {
                (marker && running.is_none()).then(|| {
                    "上次运行未完成正常关闭，已阻止再次启动。请保留应用数据和运行记录以便恢复。"
                        .into()
                })
            })
        };
        let state = if running.is_some() {
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
                "missingRequired":view.missing_required,"canStart":view.can_start && !cleanup_pending && (!marker || running.is_some()) && !inner.shutting_down},
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
        let id = Uuid::new_v4().to_string();
        let slot = self.root.join(&id);
        private_dir(&slot)?;
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
            id,
            source_path: source,
            manifest_digest,
        };
        // The digest is computed from the explicit source, not a cached declaration.
        check()?;
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
        let mut inner = self.inner.lock().await;
        self.monitor(&mut inner).await;
        Ok(self
            .registrations()?
            .iter()
            .map(|record| self.available_detail(record, &inner))
            .collect())
    }

    fn available_detail(&self, record: &Registration, inner: &Inner) -> Value {
        let result = self
            .manifest(record)
            .and_then(|manifest| self.detail(record, &manifest, inner));
        match result {
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
        }
    }

    pub async fn configuration(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        self.require_not_removing(id)?;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    pub async fn save(&self, id: &str, input: SaveConfigurationRequest) -> Result<Value> {
        let inner = self.inner.lock().await;
        self.require_no_development_operation(id)?;
        let record = self.registration(id)?;
        let manifest = self.manifest(&record)?;
        self.store(id)?
            .save(&manifest, input)
            .map_err(|e| fail(e.to_string()))?;
        self.detail(&record, &manifest, &inner)
    }

    pub async fn start(&self, id: &str) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        let was_running = inner.running.contains_key(id);
        self.start_locked(id, &mut inner).await?;
        let result = (|| {
            let record = self.registration(id)?;
            self.detail(&record, &self.manifest(&record)?, &inner)
        })();
        if result.is_err() && !was_running {
            // Source/configuration can disappear after the process becomes
            // ready. A failed first open must not leave a windowless backend.
            // Never stop an older instance reused by this request. Cleanup
            // failure retains active.json so another writer cannot be started.
            let _ = self.stop_locked(id, &mut inner).await;
        }
        result
    }

    async fn start_locked(&self, id: &str, inner: &mut Inner) -> Result<()> {
        self.require_no_development_operation(id)?;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能启动应用。"));
        }
        if inner.running.get(id).is_some_and(|run| run.cleanup_pending) {
            return Err(fail(
                "此应用进程尚未确认全部关闭，请先重试停止，不能启动第二个实例。",
            ));
        }
        if inner.running.contains_key(id) {
            return Ok(());
        }
        let record = self.registration(id)?;
        let manifest = self.manifest(&record)?;
        let slot = self.root.join(id);
        let marker = slot.join("active.json");
        // A failed/crashed intent cleanup is a persistent gate, including a
        // malformed file or symlink in place of the private cleanup directory.
        if has_start_intent_cleanup(&slot)? {
            return Err(fail(
                "上次启动标记清理尚未确认完成，已阻止再次启动。请保留清理记录以便恢复。",
            ));
        }
        if marker.try_exists()? {
            return Err(fail(
                "上次运行没有正常关闭，已阻止再次启动。请保留运行记录以便恢复。",
            ));
        }
        let snapshot = self
            .store(id)?
            .launch_snapshot(&manifest)
            .map_err(|e| fail(e.to_string()))?;
        let (entry, cwd) = manifest
            .resolve_backend(&record.source_path)
            .map_err(|e| fail(e.to_string()))?;
        let trial_root = slot
            .join("runs")
            .join(format!("node-runtime-trial-{}", Uuid::new_v4()));
        private_dir(&trial_root)?;
        let config = RuntimeLaunchConfig {
            node_path: self.node_path.clone(),
            expected_node_sha256: self.digest.clone(),
            entry_path: entry,
            working_directory: cwd,
            trial_root: trial_root.clone(),
            business_env: snapshot.environment,
        };
        NodeRuntimeTrial::preflight_with_processes(
            &config,
            &slot.join("data"),
            &slot.join("cache"),
            &self.processes,
        )
        .await?;
        // Write intent before spawning. A host hard crash fails closed; no PID guessing.
        let intent = StartIntentOwnership::create(
            &marker,
            &json!({"version":1,"hostPid":std::process::id(),"runDirectory":trial_root}),
        )?;
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
            Ok(value) => value,
            Err(error) => {
                if let Some(runtime) = pending {
                    inner.running.insert(
                        id.into(),
                        Running {
                            runtime,
                            proxy: None,
                            revision: snapshot.revision,
                            cleanup_pending: true,
                        },
                    );
                }
                // Only a positive launch receipt and this exact intent's file
                // identity/content permit removal. Never infer safety from an
                // empty pending handle, an empty directory or an absent PID.
                if cleanup.permits_intent_removal() {
                    if let Err(ownership_error) = intent.remove_if_owned(&marker) {
                        inner.errors.insert(
                            id.into(),
                            "启动标记归属发生变化，已保留记录并阻止再次启动。".into(),
                        );
                        return Err(ownership_error.detail("launchCauseCode", error.code));
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
        let (port, token) = match runtime.proxy_target() {
            Ok(target) => target,
            Err(error) => {
                let _ = self
                    .finish_failed_start(id, runtime, None, snapshot.revision, inner)
                    .await;
                return Err(error);
            }
        };
        let proxy = match NodeAppProxy::start_retaining_failed(
            record.source_path.join(&manifest.web.root),
            manifest.web.entry.clone(),
            port,
            token,
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(failure) => {
                let shutdown = self
                    .finish_failed_start(id, runtime, Some(failure.proxy), snapshot.revision, inner)
                    .await;
                let mut error = failure.error;
                if let Some(shutdown) = shutdown {
                    error = error
                        .detail("shutdownIssues", shutdown.details["shutdownIssues"].clone())
                        .detail("proxyShutdown", json!(shutdown));
                }
                return Err(error);
            }
        };
        inner.errors.remove(id);
        inner.running.insert(
            id.into(),
            Running {
                runtime,
                proxy: Some(proxy),
                revision: snapshot.revision,
                cleanup_pending: false,
            },
        );
        Ok(())
    }

    async fn finish_failed_start(
        &self,
        id: &str,
        runtime: NodeRuntimeTrial,
        proxy: Option<NodeAppProxy>,
        revision: u64,
        inner: &mut Inner,
    ) -> Option<ApiError> {
        let mut running = Running {
            runtime,
            proxy,
            revision,
            cleanup_pending: true,
        };
        let (proxy, process) = Self::stop_resources(&mut running).await;
        let shutdown = proxy.as_ref().err().cloned();
        match process {
            Ok(status) if status.process_group_exited && proxy.is_ok() => {
                if clean_shutdown(&status) {
                    let _ = fs::remove_file(self.root.join(id).join("active.json"));
                }
            }
            _ => {
                inner.running.insert(id.into(), running);
                inner.errors.insert(
                    id.into(),
                    "启动失败后的进程或网页连接尚未确认完整关闭，请重试停止。".into(),
                );
            }
        }
        shutdown
    }
    async fn stop_resources(running: &mut Running) -> (Result<()>, Result<TrialStatus>) {
        let Running { runtime, proxy, .. } = running;
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
            runtime.stop()
        )
    }
    async fn stop_locked(&self, id: &str, inner: &mut Inner) -> Result<()> {
        if let Some(running) = inner.running.get_mut(id) {
            running.cleanup_pending = true;
            // Proxy connections/files and the runtime drain independently. A
            // failed proxy cannot suppress process stop or release its owner.
            let (proxy, process) = Self::stop_resources(running).await;
            return self.finish_stop(id, inner, proxy, process);
        }
        Ok(())
    }
    fn finish_stop(
        &self,
        id: &str,
        inner: &mut Inner,
        proxy: Result<()>,
        process: Result<TrialStatus>,
    ) -> Result<()> {
        let status = match process {
            Ok(status) if status.process_group_exited => status,
            result => {
                inner.errors.insert(id.into(),"应用进程尚未确认全部关闭，已保留受管句柄。请重试停止；平台暂不允许退出或启动第二个实例。".into());
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
        inner.running.remove(id);
        if !clean_shutdown(&status) {
            inner.errors.insert(
                id.into(),
                "应用未完成正常关闭，已阻止再次启动并保留数据。".into(),
            );
            return Err(fail("应用未正常关闭，请保留运行记录以便恢复。"));
        }
        fs::remove_file(self.root.join(id).join("active.json"))?;
        inner.errors.remove(id);
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        self.stop_locked(id, &mut inner).await?;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    /// Window teardown must remain possible even when source files disappear.
    pub async fn stop_runtime(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        self.stop_locked(id, &mut inner).await
    }

    pub async fn running_ids(&self) -> Vec<String> {
        self.inner
            .lock()
            .await
            .running
            .iter()
            .filter(|(_, run)| !run.cleanup_pending)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Includes failed/closing handles whose exact process group still needs
    /// cleanup. Normal platform exit is blocked while this list is nonempty.
    pub async fn owned_process_ids(&self) -> Vec<String> {
        self.inner.lock().await.running.keys().cloned().collect()
    }

    pub async fn restart(&self, id: &str) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        self.stop_locked(id, &mut inner).await?;
        self.start_locked(id, &mut inner).await?;
        let record = self.registration(id)?;
        self.detail(&record, &self.manifest(&record)?, &inner)
    }

    async fn monitor(&self, inner: &mut Inner) {
        let ids: Vec<_> = inner.running.keys().cloned().collect();
        for id in ids {
            let run = inner.running.get_mut(&id).unwrap();
            let healthy = run.runtime.status().is_ok_and(|s| s.state == "running")
                && run.runtime.verify_health().await.is_ok();
            if !healthy && self.stop_locked(&id, inner).await.is_ok() {
                inner.errors.insert(
                    id,
                    "应用运行状态异常，已关闭页面入口并确认后台进程已停止。".into(),
                );
            }
            // A failed cleanup keeps its precise pending-ownership error.
            // A forced but confirmed cleanup keeps its business/recovery
            // failure. Neither may be replaced with a false clean message.
        }
    }

    pub async fn status(&self, id: &str) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        self.monitor(&mut inner).await;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    pub async fn open_url(&self, id: &str) -> Result<String> {
        let mut inner = self.inner.lock().await;
        self.monitor(&mut inner).await;
        inner
            .running
            .get_mut(id)
            .filter(|r| !r.cleanup_pending)
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
        self.directory_stopping
            .store(true, std::sync::atomic::Ordering::Release);
        let mut inner = self.inner.lock().await;
        inner.shutting_down = true;
        {
            // Finish accepted local initialization before completing shutdown;
            // queued initialization checks directory_stopping after admission.
            let _directory = self
                .directory_changes
                .lock()
                .map_err(|_| fail("目录初始化锁不可用。"))?;
        }
        for running in inner.running.values() {
            if let Some(proxy) = &running.proxy {
                proxy.request_stop();
            }
        }
        let drains = inner.running.iter_mut().map(|(id, running)| {
            running.cleanup_pending = true;
            let id = id.clone();
            async move { (id, Self::stop_resources(running).await) }
        });
        // Poll every application's runtime and proxy before awaiting any one
        // blocked file/connection. Keep each Running in the owned map throughout.
        let results = futures_util::future::join_all(drains).await;
        let mut error = None;
        for (id, (proxy, process)) in results {
            if let Err(failure) = self.finish_stop(&id, &mut inner, proxy, process) {
                error = Some(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }

    pub async fn monitor_tick(&self) {
        let mut inner = self.inner.lock().await;
        self.monitor(&mut inner).await;
    }
}

fn digest_manifest(source: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(read_small(&source.join("local-app.json"))?)
    ))
}

/// Presence alone is a fail-closed gate; never follow or classify its entries.
/// Fresh installation IDs legitimately have no slot yet.
pub(crate) fn has_start_intent_cleanup(slot: &Path) -> Result<bool> {
    match fs::symlink_metadata(slot) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => ordinary_path(slot, true)?,
    }
    for entry in fs::read_dir(slot)? {
        if entry?
            .file_name()
            .to_string_lossy()
            .starts_with(START_INTENT_CLEANUP_PREFIX)
        {
            return Ok(true);
        }
    }
    Ok(false)
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
