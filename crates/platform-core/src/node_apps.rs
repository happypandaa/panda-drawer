//! Native-manager-only source applications, separate from project-v1 storage.
//! Registration reads declarations; only an explicit start executes trusted code.

use crate::{
    node_app_configuration::{LocalFileVault, NodeConfigurationStore, SaveConfigurationRequest},
    node_app_manifest::{TrialManifest, TrialRuntime},
    node_app_proxy::NodeAppProxy,
    node_runtime_trial::{NodeRuntimeTrial, RuntimeLaunchConfig},
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
    proxy: NodeAppProxy,
    revision: u64,
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
        let running = inner.running.get(&record.id);
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
        Ok(json!({
            "id":record.id,"name":manifest.app.name,"sourcePath":record.source_path,"runtime":"node-service",
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
        let record = Registration {
            version: 1,
            id,
            source_path: source,
            manifest_digest,
        };
        // The digest is computed from the explicit source, not a cached declaration.
        write_new_json(&slot.join("registration.json"), &record)?;
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
        let launched =
            NodeRuntimeTrial::launch_with_storage(config, slot.join("data"), slot.join("cache"))
                .await;
        let mut runtime = match launched {
            Ok(value) => value,
            Err(error) => {
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
        let (port, token) = runtime.proxy_target()?;
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
                let stopped = runtime.stop().await;
                if stopped
                    .as_ref()
                    .is_ok_and(|s| !s.forced && s.stopping_observed && s.exit_code == Some(0))
                {
                    fs::remove_file(&marker)?;
                }
                return Err(error);
            }
        };
        inner.errors.remove(id);
        inner.running.insert(
            id.into(),
            Running {
                runtime,
                proxy,
                revision: snapshot.revision,
            },
        );
        Ok(())
    }

    async fn stop_locked(&self, id: &str, inner: &mut Inner) -> Result<()> {
        if let Some(mut running) = inner.running.remove(id) {
            running.proxy.stop().await;
            let status = running.runtime.stop().await?;
            if status.forced || !status.stopping_observed || status.exit_code != Some(0) {
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
            if !healthy {
                let _ = self.stop_locked(&id, inner).await;
                inner
                    .errors
                    .insert(id, "应用运行状态异常，已关闭页面入口并停止此应用。".into());
            }
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
            .map(|r| r.proxy.open_url())
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

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
#[path = "node_apps_lifecycle_tests.rs"]
mod lifecycle_tests;
