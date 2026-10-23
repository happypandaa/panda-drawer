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

/// Explicitly re-trust an external application's current declaration. These
/// digests are public metadata, never configuration values or credential IDs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclarationUpdateRequest {
    pub expected_registered_digest: String,
    pub expected_manifest_digest: String,
    pub configuration_revision: u64,
    pub confirmed: bool,
}

struct DeclarationUpdateCandidate {
    record: Registration,
    manifest: TrialManifest,
    preview: Value,
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
    revision: u64,
    cleanup_pending: bool,
}
#[derive(Default)]
struct Inner {
    running: BTreeMap<String, Running>,
    errors: BTreeMap<String, String>,
    shutting_down: bool,
}

pub struct NodeApps {
    root: PathBuf,
    data_root: PathBuf,
    node_path: PathBuf,
    digest: String,
    inner: Mutex<Inner>,
}

impl NodeApps {
    pub fn new(data_root: PathBuf, node_path: PathBuf) -> Result<Self> {
        ordinary_path(&data_root, true)?;
        let root = data_root.join("node-apps");
        let release: Value =
            serde_json::from_str(include_str!("../../../scripts/node-runtime-release.json"))?;
        let digest = release["binary"]["sha256"]
            .as_str()
            .ok_or_else(|| fail("内置 Node 校验信息无效。"))?
            .to_owned();
        Ok(Self {
            root,
            data_root,
            node_path,
            digest,
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn inspect(&self, source: &Path) -> Result<Value> {
        let manifest = self.load_source(source)?;
        Ok(serde_json::to_value(
            manifest.inspect(source).map_err(|e| fail(e.to_string()))?,
        )?)
    }

    fn load_source(&self, source: &Path) -> Result<TrialManifest> {
        self.load_source_snapshot(source)
            .map(|(manifest, _)| manifest)
    }

    /// Parse and hash the same bounded bytes when preparing a declaration
    /// update, rather than pairing one read's fields with another read's hash.
    fn load_source_snapshot(&self, source: &Path) -> Result<(TrialManifest, Vec<u8>)> {
        ordinary_path(source, true)?;
        if source.starts_with(&self.data_root) || self.data_root.starts_with(source) {
            return Err(fail(
                "请选择独立的应用源码目录，不能使用平台的数据目录或其父目录。",
            ));
        }
        let bytes = read_small(&source.join("local-app.json"))?;
        let manifest = TrialManifest::parse(&bytes).map_err(|e| fail(e.to_string()))?;
        if manifest.runtime != TrialRuntime::NodeService {
            return Err(fail(
                "此入口只登记带 Node 服务的应用。网页项目请继续使用我的项目。",
            ));
        }
        manifest.inspect(source).map_err(|e| fail(e.to_string()))?;
        Ok((manifest, bytes))
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
            let version = serde_json::from_slice::<Value>(&bytes)?["version"].as_u64();
            let record = match version {
                Some(1) => {
                    let record: ExternalRegistration = serde_json::from_slice(&bytes)?;
                    Registration::from(record)
                }
                Some(2) => {
                    let record: ManagedNodeRegistration = serde_json::from_slice(&bytes)?;
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
                "应用说明文件已更改，当前已暂停启动。请先关闭应用，再核对并明确重新信任新版应用说明；受管安装请使用应用更新入口。",
            ));
        }
        Ok(manifest)
    }

    fn store(&self, id: &str) -> Result<NodeConfigurationStore> {
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
        let error = inner.errors.get(&record.id).cloned().or_else(|| {
            (marker && running.is_none()).then(|| {
                "上次运行未完成正常关闭，已阻止再次启动。请保留应用数据和运行记录以便恢复。".into()
            })
        });
        let state = if running.is_some() {
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
        if record.managed.is_some() {
            object_layout["source"] = json!({"state":"unverified","path":null,"reason":"当前程序是平台独立安装的运行副本；来源候选记录另行保留，不将该副本视为已确认的开发源码。"});
            object_layout["runtime"]["external"] = json!(false);
            object_layout["build"] = json!({"state":"unavailable","reason":"此程序由独立安装候选准备；打开应用不会安装依赖，普通更新须重新准备并核验候选。"});
        }
        Ok(json!({
            "id":record.id,"name":manifest.app.name,"sourcePath":record.source_path,"runtime":"node-service","window":manifest.window,
            "objectLayout":object_layout,
            "declarationUpdateAvailable":record.managed.is_none(),
            "installation":record.managed.as_ref().map(|managed| json!({"kind":"managed","generation":managed.generation,"sourceId":managed.provenance.source_id,"sourceDigest":managed.provenance.source_digest})),
            "state":state,"restartRequired":running.is_some_and(|r|r.revision != view.revision),
            "runningRevision":running.map(|r|r.revision),"lastError":error,
            "configuration":{"revision":view.revision,"values":view.saved_values,"secretStatus":view.secrets,
                "missingRequired":view.missing_required,"canStart":view.can_start && (!marker || running.is_some()) && !inner.shutting_down},
            "fields":inspected["configuration"]["fields"]
        }))
    }

    pub async fn register(&self, source: PathBuf) -> Result<Value> {
        let inner = self.inner.lock().await;
        let manifest = self.load_source(&source)?;
        private_dir(&self.root)?;
        for record in self.registrations()? {
            if record.source_path == source {
                return self.detail(&record, &self.manifest(&record)?, &inner);
            }
        }
        let id = Uuid::new_v4().to_string();
        let slot = self.root.join(&id);
        private_dir(&slot)?;
        for name in ["configuration", "data", "cache", "runs"] {
            private_dir(&slot.join(name))?;
        }
        let manifest_digest = digest_manifest(&source)?;
        let record = ExternalRegistration {
            version: 1,
            id,
            source_path: source,
            manifest_digest,
        };
        // The digest is computed from the explicit source, not a cached declaration.
        write_new_json(&slot.join("registration.json"), &record)?;
        self.detail(&record.into(), &manifest, &inner)
    }

    fn require_declaration_update_stopped(&self, id: &str, inner: &Inner) -> Result<()> {
        if inner.shutting_down || inner.running.contains_key(id) {
            return Err(ApiError::new(
                "NODE_DECLARATION_UPDATE_BUSY",
                "请先正常关闭应用，待服务完成收尾后再核对应用说明。",
            ));
        }
        let marker = self.root.join(id).join("active.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if marker.try_exists()? {
            return Err(ApiError::new(
                "NODE_DECLARATION_UPDATE_BUSY",
                "运行记录尚未完成收尾，不能更新应用说明；请保留现场。",
            ));
        }
        Ok(())
    }

    fn declaration_update_candidate(
        &self,
        id: &str,
        inner: &Inner,
    ) -> Result<DeclarationUpdateCandidate> {
        if Uuid::parse_str(id).is_err() {
            return Err(fail("应用实例标识无效。"));
        }
        self.require_declaration_update_stopped(id, inner)?;
        let bytes = read_small(&self.root.join(id).join("registration.json"))?;
        let version = serde_json::from_slice::<Value>(&bytes)?["version"].as_u64();
        if version == Some(2) {
            return Err(ApiError::new(
                "NODE_DECLARATION_UPDATE_MANAGED",
                "受管安装不能重新登记外部应用说明，请使用应用更新入口。",
            ));
        }
        let external: ExternalRegistration = serde_json::from_slice(&bytes)?;
        if external.version != 1 || external.id != id {
            return Err(fail("外部应用登记记录无效，请保留现场。"));
        }
        let registered_digest = format!("{:x}", Sha256::digest(&bytes));
        let record = Registration::from(external);
        let (manifest, manifest_bytes) = self.load_source_snapshot(&record.source_path)?;
        let manifest_digest = format!("{:x}", Sha256::digest(&manifest_bytes));
        // view checks saved field types, endpoint bindings and private-file
        // metadata. It does not retrieve or deserialize any credential value.
        let view = self
            .store(id)?
            .view(&manifest)
            .map_err(|error| fail(error.to_string()))?;
        let inspected = serde_json::to_value(
            manifest
                .inspect(&record.source_path)
                .map_err(|error| fail(error.to_string()))?,
        )?;
        let preview = json!({
            "appId":id,
            "expectedRegisteredDigest":registered_digest,
            "registeredManifestDigest":record.manifest_digest,
            "expectedManifestDigest":manifest_digest,
            "configurationRevision":view.revision,
            "sourcePath":record.source_path,
            "app":inspected["app"],
            "backend":manifest.backend.as_ref().map(|backend| json!({
                "protocolVersion":backend.protocol_version,"root":backend.root,
                "entry":backend.entry,"nodeMajor":backend.node_major
            })),
            "fields":inspected["configuration"]["fields"],
            "configuration":{"revision":view.revision,"values":view.saved_values,
                "secretStatus":view.secrets,"missingRequired":view.missing_required,
                "canStart":view.can_start},
            "requiresRetrust":true,
            "notice":"旧外部登记只保存应用说明摘要，没有可供比较的旧声明。此操作表示你明确重新信任当前源码目录中的新版应用说明，平台不能自动确认它只是新增选填项或证明源码安全。原数据和已保存配置保持；服务地址变化的密钥仍需重新确认绑定。保存后不会启动应用。"
        });
        Ok(DeclarationUpdateCandidate {
            record,
            manifest,
            preview,
        })
    }

    pub async fn declaration_update_preview(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        Ok(self.declaration_update_candidate(id, &inner)?.preview)
    }

    pub async fn declaration_update_apply(
        &self,
        id: &str,
        input: DeclarationUpdateRequest,
    ) -> Result<Value> {
        if !input.confirmed {
            return Err(ApiError::new(
                "NODE_DECLARATION_UPDATE_CONFIRMATION_REQUIRED",
                "请先核对新版应用说明，并明确确认重新信任。",
            ));
        }
        let mut inner = self.inner.lock().await;
        let candidate = self.declaration_update_candidate(id, &inner)?;
        let changed = || {
            ApiError::new(
                "NODE_DECLARATION_UPDATE_CHANGED",
                "应用登记、说明文件或配置已变化，请重新预览并核对。",
            )
        };
        if candidate.preview["expectedRegisteredDigest"] != input.expected_registered_digest
            || candidate.preview["expectedManifestDigest"] != input.expected_manifest_digest
            || candidate.preview["configurationRevision"] != input.configuration_revision
        {
            return Err(changed());
        }
        let replacement = ExternalRegistration {
            version: 1,
            id: candidate.record.id.clone(),
            source_path: candidate.record.source_path.clone(),
            manifest_digest: input.expected_manifest_digest,
        };
        let record = Registration::from(replacement);
        // Prepare the response before publication. A late failure must not be
        // reported as an uncommitted update after the registration was changed.
        let mut detail = self.detail(&record, &candidate.manifest, &inner)?;
        detail["state"] = json!("stopped");
        detail["lastError"] = Value::Null;
        self.require_declaration_update_stopped(id, &inner)?;
        let fresh = self.declaration_update_candidate(id, &inner)?;
        if fresh.preview["expectedRegisteredDigest"] != input.expected_registered_digest
            || fresh.preview["expectedManifestDigest"] != record.manifest_digest
            || fresh.preview["configurationRevision"] != input.configuration_revision
        {
            return Err(changed());
        }
        let path = self.root.join(id).join("registration.json");
        replace_external_registration(&path, &record, &input.expected_registered_digest)?;
        inner.errors.remove(id);
        Ok(detail)
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
        if inner.shutting_down || inner.running.contains_key(id) {
            return Err(ApiError::new(
                "NODE_INSTALL_BUSY",
                "请先正常关闭应用，待运行进程完成收尾后再更新。",
            ));
        }
        let marker = self.root.join(id).join("active.json");
        crate::projects::check_no_symlinks(&marker, true)?;
        if marker.try_exists()? {
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
        let managed = record.managed.as_ref().ok_or_else(|| {
            fail("外部登记应用不能原地转为受管安装，请保留现有应用并新建安装实例。")
        })?;
        let manifest = self.manifest(&record)?;
        let revision = self
            .store(id)?
            .view(&manifest)
            .map_err(|error| fail(error.to_string()))?
            .revision;
        Ok(InstallTargetSnapshot {
            app_id: id.into(),
            registration_digest: node_app_install::registration_digest(&self.root.join(id))?,
            generation: Some(managed.generation.clone()),
            manifest: Some(serde_json::from_slice(&read_small(
                &record.source_path.join("local-app.json"),
            )?)?),
            configuration_revision: revision,
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
        self.manifest(record).and_then(|manifest|self.detail(record,&manifest,inner))
            .unwrap_or_else(|error|json!({
                "id":record.id,"name":record.source_path.file_name().and_then(|n|n.to_str()).unwrap_or("不可用的 Node 应用"),
                "sourcePath":record.source_path,"runtime":"node-service","state":"failed",
                "declarationUpdateAvailable":record.managed.is_none(),
                "installation":record.managed.as_ref().map(|managed| json!({"kind":"managed","generation":managed.generation,"sourceId":managed.provenance.source_id,"sourceDigest":managed.provenance.source_digest})),
                "objectLayout":null,
                "restartRequired":false,"runningRevision":null,"lastError":error.message,
                "configuration":{"revision":0,"values":{},"secretStatus":{},"missingRequired":[],"canStart":false},"fields":[]
            }))
    }

    pub async fn configuration(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().await;
        let record = self.registration(id)?;
        Ok(self.available_detail(&record, &inner))
    }

    pub async fn save(&self, id: &str, input: SaveConfigurationRequest) -> Result<Value> {
        let inner = self.inner.lock().await;
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
        NodeRuntimeTrial::preflight(&config, &slot.join("data"), &slot.join("cache")).await?;
        // Write intent before spawning. A host hard crash fails closed; no PID guessing.
        write_new_json(
            &marker,
            &json!({"version":1,"hostPid":std::process::id(),"runDirectory":trial_root}),
        )?;
        let (launched, pending) = NodeRuntimeTrial::launch_retaining_failed(
            config,
            slot.join("data"),
            slot.join("cache"),
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
                // launch has awaited owned-child cleanup before returning an error.
                // Preserve intent on uncertainty rather than starting a second writer.
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
                self.finish_failed_start(id, runtime, snapshot.revision, inner)
                    .await;
                return Err(error);
            }
        };
        let proxy = match NodeAppProxy::start(
            record.source_path.join(&manifest.web.root),
            manifest.web.entry.clone(),
            port,
            token,
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(error) => {
                self.finish_failed_start(id, runtime, snapshot.revision, inner)
                    .await;
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
        mut runtime: NodeRuntimeTrial,
        revision: u64,
        inner: &mut Inner,
    ) {
        match runtime.stop().await {
            Ok(status) if status.process_group_exited => {
                if clean_shutdown(&status) {
                    let _ = fs::remove_file(self.root.join(id).join("active.json"));
                }
            }
            _ => {
                inner.running.insert(
                    id.into(),
                    Running {
                        runtime,
                        proxy: None,
                        revision,
                        cleanup_pending: true,
                    },
                );
                inner.errors.insert(
                    id.into(),
                    "启动失败后的进程尚未确认全部关闭，请重试停止。".into(),
                );
            }
        }
    }

    async fn stop_locked(&self, id: &str, inner: &mut Inner) -> Result<()> {
        if let Some(running) = inner.running.get_mut(id) {
            running.cleanup_pending = true;
            if let Some(proxy) = &mut running.proxy {
                proxy.stop().await;
            }
            let status = match running.runtime.stop().await {
                Ok(status) if status.process_group_exited => status,
                result => {
                    inner.errors.insert(id.into(), "应用进程尚未确认全部关闭，已保留受管句柄。请重试停止；平台暂不允许退出或启动第二个实例。".into());
                    return Err(result
                        .err()
                        .unwrap_or_else(|| fail("无法确认应用进程组已关闭。")));
                }
            };
            // Keep ownership until stop has positively confirmed group cleanup.
            // A forced but fully cleaned process is removed here; its business
            // failure and recovery marker are still preserved below.
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
        }
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
        let mut inner = self.inner.lock().await;
        inner.shutting_down = true;
        let ids: Vec<_> = inner.running.keys().cloned().collect();
        let mut error = None;
        for id in ids {
            if let Err(e) = self.stop_locked(&id, &mut inner).await {
                error = Some(e);
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
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options.open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(fail("应用记录超出大小限制。"));
    }
    Ok(bytes)
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

/// Publish only the external registration. Rename is the commit point; no
/// configuration, credential or business-data file is rewritten here.
fn replace_external_registration(
    path: &Path,
    record: &Registration,
    expected_digest: &str,
) -> Result<()> {
    let parent = path.parent().ok_or_else(|| fail("登记目录无效。"))?;
    ordinary_path(parent, true)?;
    let temporary = parent.join(format!(".registration-{}.tmp", Uuid::new_v4()));
    let replacement = ExternalRegistration {
        version: 1,
        id: record.id.clone(),
        source_path: record.source_path.clone(),
        manifest_digest: record.manifest_digest.clone(),
    };
    let result = (|| {
        write_new_json(&temporary, &replacement)?;
        if format!("{:x}", Sha256::digest(read_small(path)?)) != expected_digest {
            return Err(ApiError::new(
                "NODE_DECLARATION_UPDATE_CHANGED",
                "应用登记已变化，请重新预览并核对。",
            ));
        }
        fs::rename(&temporary, path)?;
        // Publication succeeded. A directory-sync failure must not invite an
        // unsafe duplicate apply by reporting that nothing was committed.
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "node_apps_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "node_apps_install_tests.rs"]
mod install_tests;

#[cfg(test)]
#[path = "node_apps_declaration_update_tests.rs"]
mod declaration_update_tests;
