//! Native-manager-only source applications, separate from project-v1 storage.
//! Registration reads declarations; only an explicit start executes trusted code.

use crate::{
    node_app_configuration::{LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest},
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
    path::{Path, PathBuf},
    sync::Arc,
};
#[cfg(not(windows))]
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Component,
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[path = "node_app_directory.rs"]
mod directory;
pub use directory::DirectoryInitializeInput;

type Result<T> = std::result::Result<T, ApiError>;
fn fail(message: impl Into<String>) -> ApiError {
    ApiError::new("NODE_APP_ERROR", message)
}

#[cfg(windows)]
fn preview_changed() -> ApiError {
    ApiError::new(
        "NODE_APP_PREVIEW_CHANGED",
        "应用目录或声明已变化，或无法继续验证。请重新读取预览后再登记。",
    )
}

#[cfg(windows)]
struct SourceSnapshot {
    // This retains the entire directory ancestry and the actual declaration
    // handle without write/delete sharing until registration has finished.
    source: platform_host::WindowsSourceDirectory,
    manifest_bytes: Vec<u8>,
    manifest_digest: String,
    expected_revision: String,
}

#[cfg(windows)]
impl SourceSnapshot {
    fn manifest(&self) -> Result<TrialManifest> {
        let manifest =
            TrialManifest::parse(&self.manifest_bytes).map_err(|e| fail(e.to_string()))?;
        if manifest.runtime != TrialRuntime::NodeService {
            return Err(fail(
                "此入口只登记带 Node 服务的应用。网页项目请继续使用我的项目。",
            ));
        }
        manifest
            .inspect(self.source.path())
            .map_err(|e| fail(e.to_string()))?;
        self.source
            .verify_binding()
            .map_err(crate::windows_fs::error)?;
        Ok(manifest)
    }
}

fn clean_shutdown(status: &TrialStatus) -> bool {
    status.process_group_exited && status.state == "stopped"
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    version: u32,
    id: String,
    source_path: PathBuf,
    manifest_digest: String,
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
    #[cfg(windows)]
    windows_root: platform_host::WorkspaceDirectory,
    #[cfg(windows)]
    markers: std::sync::Mutex<BTreeMap<PathBuf, platform_host::WindowsObjectIdentity>>,
    directory_changes: Mutex<()>,
    directory_stopping: std::sync::atomic::AtomicBool,
}

impl NodeApps {
    pub fn new(data_root: PathBuf, node_path: PathBuf) -> Result<Self> {
        #[cfg(windows)]
        {
            let _ = (data_root, node_path);
            Err(fail("Windows Node 必须接收宿主已准入并持锁的固定数据根。"))
        }
        #[cfg(not(windows))]
        {
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
    }
    #[cfg(windows)]
    pub fn new_windows(
        root: platform_host::WorkspaceDirectory,
        node_path: PathBuf,
    ) -> Result<Self> {
        root.verify_binding().map_err(crate::windows_fs::error)?;
        platform_host::windows_node_root::node_apps_directory(&root, false)
            .map_err(crate::windows_fs::error)?;
        let release: Value = serde_json::from_str(include_str!(
            "../../../scripts/node-runtime-release-windows.json"
        ))?;
        let digest = release["binary"]["sha256"]
            .as_str()
            .ok_or_else(|| fail("Windows私有Node校验记录无效。"))?
            .to_owned();
        Ok(Self {
            data_root: root.path().to_owned(),
            root: root.path().join("node-apps"),
            node_path,
            digest,
            inner: Mutex::new(Inner::default()),
            windows_root: root,
            markers: std::sync::Mutex::new(BTreeMap::new()),
            directory_changes: Mutex::new(()),
            directory_stopping: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn inspect(&self, source: &Path) -> Result<Value> {
        #[cfg(windows)]
        {
            let snapshot = self.source_snapshot(source)?;
            let manifest = snapshot.manifest()?;
            let mut inspected = serde_json::to_value(
                manifest
                    .inspect(snapshot.source.path())
                    .map_err(|e| fail(e.to_string()))?,
            )?;
            snapshot
                .source
                .verify_binding()
                .map_err(crate::windows_fs::error)?;
            inspected["expectedRevision"] = json!(snapshot.expected_revision);
            Ok(inspected)
        }
        #[cfg(not(windows))]
        {
            let manifest = self.load_source(source)?;
            Ok(serde_json::to_value(
                manifest.inspect(source).map_err(|e| fail(e.to_string()))?,
            )?)
        }
    }

    #[cfg(windows)]
    fn source_snapshot(&self, source: &Path) -> Result<SourceSnapshot> {
        let source = platform_host::WindowsSourceDirectory::open(source)
            .map_err(crate::windows_fs::error)?;
        let data = platform_host::WindowsSourceDirectory::from_workspace(&self.windows_root)
            .map_err(crate::windows_fs::error)?;
        if source.overlaps(&data)
            || source.path().starts_with(&self.data_root)
            || self.data_root.starts_with(source.path())
        {
            return Err(fail("应用源码必须与平台数据目录及祖先目录相互独立。"));
        }
        let source = source
            .pin_file("local-app.json")
            .map_err(crate::windows_fs::error)?;
        let manifest_bytes = source
            .read_bounded("local-app.json", 65536)
            .map_err(crate::windows_fs::error)?
            .bytes;
        let digest = Sha256::digest(&manifest_bytes);
        let mut revision = Sha256::new();
        revision.update(b"windows-node-source-preview-v1\0");
        revision.update(source.identity().volume_serial.to_le_bytes());
        revision.update(source.identity().file_id128);
        revision.update(digest);
        Ok(SourceSnapshot {
            source,
            manifest_bytes,
            manifest_digest: format!("{digest:x}"),
            expected_revision: format!("{:x}", revision.finalize()),
        })
    }

    fn load_source(&self, source: &Path) -> Result<TrialManifest> {
        #[cfg(windows)]
        {
            self.source_snapshot(source)?.manifest()
        }
        #[cfg(not(windows))]
        {
            ordinary_path(source, true)?;
            if source.starts_with(&self.data_root) || self.data_root.starts_with(source) {
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
    }

    #[cfg(not(windows))]
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
            let record: Registration = serde_json::from_slice(&read_small(&path)?)?;
            if record.version != 1 || record.id != name {
                return Err(fail("Node 应用登记记录无效，请保留现场。"));
            }
            result.push(record);
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
    #[cfg(windows)]
    fn registrations(&self) -> Result<Vec<Registration>> {
        let Some(root) =
            platform_host::windows_node_root::node_apps_directory(&self.windows_root, false)
                .map_err(crate::windows_fs::error)?
        else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for entry in root.list().map_err(crate::windows_fs::error)? {
            if entry.name == ".node-apps-root.json" {
                continue;
            }
            let slot = root
                .open_directory(&entry.name)
                .map_err(crate::windows_fs::error)?;
            let record: Registration = serde_json::from_slice(
                &slot
                    .read_bounded("registration.json", 65536)
                    .map_err(crate::windows_fs::error)?
                    .bytes,
            )?;
            if record.version != 1 || record.id != entry.name {
                return Err(fail("Node登记记录无效，已保留现场。"));
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
        let manifest = self.load_source(&record.source_path)?;
        if digest_manifest(&record.source_path)? != record.manifest_digest {
            return Err(fail(
                "应用说明文件已更改，当前已暂停启动。请保留原配置和源码以便核对；此版本尚未提供声明更新入口。",
            ));
        }
        Ok(manifest)
    }

    fn store(&self, id: &str) -> Result<NodeConfigurationStore> {
        let directory = self.root.join(id).join("configuration");
        let instance = Uuid::parse_str(id).map_err(|_| fail("应用标识无效。"))?;
        #[cfg(windows)]
        let vault = LocalFileVault::open_existing(directory.join("credentials"), instance)
            .map_err(|e| fail(e.to_string()))?;
        #[cfg(not(windows))]
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
        let marker = self.marker_exists(&self.root.join(&record.id).join("active.json"))?;
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
        Ok(json!({
            "id":record.id,"name":manifest.app.name,"sourcePath":record.source_path,"runtime":"node-service","window":manifest.window,"dataContract":manifest.data_contract,
            "state":state,"restartRequired":running.is_some_and(|r|r.revision != view.revision),
            "runningRevision":running.map(|r|r.revision),"lastError":error,
            "configuration":{"revision":view.revision,"values":view.saved_values,"secretStatus":view.secrets,
                "missingRequired":view.missing_required,"canStart":view.can_start && (!marker || running.is_some()) && !inner.shutting_down},
            "fields":inspected["configuration"]["fields"]
        }))
    }

    pub async fn register(&self, source: PathBuf, expected_revision: &str) -> Result<Value> {
        self.register_checked(source, expected_revision, || Ok(()))
            .await
    }

    async fn register_checked(
        &self,
        source: PathBuf,
        expected_revision: &str,
        mut check: impl FnMut() -> Result<()> + Send,
    ) -> Result<Value> {
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能登记应用。"));
        }
        #[cfg(windows)]
        let snapshot = {
            if expected_revision.len() != 64
                || !expected_revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(preview_changed());
            }
            let snapshot = self
                .source_snapshot(&source)
                .map_err(|_| preview_changed())?;
            if snapshot.expected_revision != expected_revision {
                return Err(preview_changed());
            }
            snapshot
        };
        #[cfg(windows)]
        let source = snapshot.source.path().to_owned();
        #[cfg(windows)]
        let manifest = snapshot.manifest().map_err(|_| preview_changed())?;
        check()?;
        #[cfg(not(windows))]
        let manifest = self.load_source(&source)?;
        #[cfg(windows)]
        snapshot
            .source
            .verify_binding()
            .map_err(|_| preview_changed())?;
        #[cfg(not(windows))]
        private_dir(&self.root)?;
        for record in self.registrations()? {
            #[cfg(windows)]
            let same_source = platform_host::WindowsSourceDirectory::open(&record.source_path)
                .map(|guard| guard.identity() == snapshot.source.identity())
                .unwrap_or(false);
            #[cfg(not(windows))]
            let same_source = record.source_path == source;
            if same_source {
                check()?;
                return self.detail(&record, &self.manifest(&record)?, &inner);
            }
        }
        #[cfg(windows)]
        {
            snapshot
                .source
                .verify_binding()
                .map_err(|_| preview_changed())?;
            platform_host::windows_node_root::node_apps_directory(&self.windows_root, true)
                .map_err(crate::windows_fs::error)?;
        }
        let id = Uuid::new_v4().to_string();
        let slot = self.root.join(&id);
        private_dir(&slot)?;
        for name in ["configuration", "data", "cache", "runs"] {
            private_dir(&slot.join(name))?;
        }
        #[cfg(windows)]
        private_dir(&slot.join("configuration/credentials"))?;
        #[cfg(windows)]
        let manifest_digest = snapshot.manifest_digest.clone();
        #[cfg(not(windows))]
        let manifest_digest = digest_manifest(&source)?;
        let record = Registration {
            version: 1,
            id,
            source_path: source,
            manifest_digest,
        };
        // On Windows this is the same pinned byte snapshot whose revision was
        // compared before any write; never reread a newer declaration here.
        check()?;
        self.write_record(&slot.join("registration.json"), &record)?;
        #[cfg(windows)]
        {
            let root = self
                .windows_root
                .open_directory("node-apps")
                .map_err(crate::windows_fs::error)?;
            platform_host::windows_node_root::seal_slot(&root, &record.id)
                .map_err(crate::windows_fs::error)?;
        }
        self.detail(&record, &manifest, &inner)
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
            .map_err(crate::node_app_configuration::ConfigurationError::into_api_error)?;
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
        if self.marker_exists(&marker)? {
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
        self.write_record(
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
                    let _ = self.remove_marker(&self.root.join(id).join("active.json"));
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
            self.remove_marker(&self.root.join(id).join("active.json"))?;
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
        self.directory_stopping
            .store(true, std::sync::atomic::Ordering::Release);
        // Reject runtime launches immediately while an accepted initializer
        // finishes; do not hold the runtime lock while waiting for its gate.
        self.inner.lock().await.shutting_down = true;
        // Accepted initializations run on blocking workers. Await their lock
        // without blocking the async runtime or abandoning partial files.
        let _directory = self.directory_changes.lock().await;
        let mut inner = self.inner.lock().await;
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
    fn write_record(&self, path: &Path, value: &impl Serialize) -> Result<()> {
        #[cfg(windows)]
        {
            let parent = path.parent().ok_or_else(|| fail("记录没有父目录。"))?;
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(|| fail("记录名无效。"))?;
            let directory = crate::windows_fs::managed_directory(&self.windows_root, parent)?;
            let identity = directory
                .write_new(name, &serde_json::to_vec_pretty(value)?)
                .map_err(crate::windows_fs::error)?;
            if name == "active.json" {
                self.markers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(path.to_owned(), identity);
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            write_new_json(path, value)
        }
    }
    fn marker_exists(&self, path: &Path) -> Result<bool> {
        #[cfg(windows)]
        {
            let directory = crate::windows_fs::managed_directory(
                &self.windows_root,
                path.parent().ok_or_else(|| fail("运行标记路径无效。"))?,
            )?;
            match directory.read_bounded("active.json", 65536) {
                Ok(_) => Ok(true),
                Err(error) if error.code == "WORKSPACE_NAME_MISSING" => Ok(false),
                Err(error) => Err(crate::windows_fs::error(error)),
            }
        }
        #[cfg(not(windows))]
        {
            Ok(path.try_exists()?)
        }
    }
    fn remove_marker(&self, path: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            let mut markers = self.markers.lock().unwrap_or_else(|p| p.into_inner());
            let identity = markers
                .get(path)
                .ok_or_else(|| fail("当前宿主不拥有此运行标记，已保留。"))?;
            let directory = crate::windows_fs::managed_directory(
                &self.windows_root,
                path.parent().ok_or_else(|| fail("运行标记路径无效。"))?,
            )?;
            directory
                .remove_file_checked("active.json", identity)
                .map_err(crate::windows_fs::error)?;
            markers.remove(path);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Ok(fs::remove_file(path)?)
        }
    }
}

fn digest_manifest(source: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(read_small(&source.join("local-app.json"))?)
    ))
}

#[cfg(not(windows))]
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

#[cfg(windows)]
fn private_dir(path: &Path) -> Result<()> {
    crate::windows_fs::create_private_runtime_dir(path).map(|_| ())
}
#[cfg(not(windows))]
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

#[cfg(windows)]
fn read_small(path: &Path) -> Result<Vec<u8>> {
    let directory = platform_host::WindowsSourceDirectory::open(
        path.parent().ok_or_else(|| fail("记录路径无效。"))?,
    )
    .map_err(crate::windows_fs::error)?;
    directory
        .read_bounded(
            path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| fail("记录名无效。"))?,
            65536,
        )
        .map(|file| file.bytes)
        .map_err(crate::windows_fs::error)
}
#[cfg(not(windows))]
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

#[cfg(not(windows))]
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

#[cfg(all(test, windows))]
#[path = "windows_node_apps_tests.rs"]
mod windows_tests;
