use crate::{
    builds::{self, BuildManager},
    files,
    resources::ProjectResources,
    tasks::{TaskRecord, TaskStatus},
    ApiError, CreateProjectInput, Discovery, InstanceLock, ProjectHandle, ProjectStore,
    RuntimePaths, SqlCaller, SqlRequest, TransactionRequest, API_VERSION, APP_IDENTIFIER,
    DISPLAY_NAME, ENVIRONMENT,
};
use anyhow::{Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{any, get, post},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::HashMap,
    convert::Infallible,
    io::Read,
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, RwLock as StdRwLock, Weak,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::TcpListener,
    sync::{broadcast, oneshot, watch, Notify, Semaphore},
    task::JoinHandle,
};
use tower::ServiceExt;
use uuid::Uuid;

const JSON_MAX: usize = 2_000_000;
const FILE_MAX: u64 = 256_000_000;
const ENTRY_MAX: u64 = 8_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthInfo {
    pub api_version: u32,
    pub instance_id: String,
    pub identifier: String,
    pub environment: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapInfo {
    pub display_name: String,
    pub identifier: String,
    pub environment: String,
    pub api_version: u32,
    pub instance_id: String,
    pub base_url: String,
    pub workspace_root: PathBuf,
    pub data_root: PathBuf,
    pub sqlite_version: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    pub project_id: String,
    pub url: String,
    pub launch_url: String,
    pub expires_at: u64,
    pub instance_id: String,
    pub document_epoch: String,
}
struct Ticket {
    project_id: String,
    epoch: String,
    expires: Instant,
}
struct Session {
    project_id: String,
    lifecycle_generation: Option<String>,
}
#[derive(Clone)]
struct Change {
    project_id: String,
    request_id: String,
    source: String,
    event: String,
}
struct HttpState {
    paths: RuntimePaths,
    shared: Weak<RuntimeSwitch>,
    lifecycle: Arc<crate::lifecycle::LifecycleManager>,
    metadata: crate::metadata::MetadataManager,
    storage: Arc<crate::storage::StorageManager>,
    history_cleanup: Arc<crate::history_cleanup::CleanupManager>,
    info: BootstrapInfo,
    token: String,
    expected_host: String,
    store: Arc<ProjectStore>,
    builds: Arc<BuildManager>,
    maintenance: Arc<crate::maintenance::MaintenanceManager>,
    resources: Option<Arc<ProjectResources>>,
    tickets: Mutex<HashMap<String, Ticket>>,
    sessions: Mutex<HashMap<String, Session>>,
    events: broadcast::Sender<Change>,
    uploads: Arc<Semaphore>,
    project_uploads: Mutex<HashMap<String, Arc<Semaphore>>>,
    stopping: AtomicBool,
    active: AtomicUsize,
    drained: Notify,
    stop: watch::Sender<bool>,
}
struct RuntimeSwitch {
    current: StdRwLock<Option<Arc<HttpState>>>,
    router: StdRwLock<Router>,
    migration_runner: Mutex<Option<JoinHandle<()>>>,
    closing: AtomicBool,
    migration_active: AtomicBool,
}
impl RuntimeSwitch {
    fn snapshot(&self) -> Arc<HttpState> {
        self.current
            .read()
            .unwrap()
            .as_ref()
            .expect("Runtime initialized before serving")
            .clone()
    }
    fn install(&self, state: Arc<HttpState>) {
        *self.router.write().unwrap() = runtime_router(state.clone());
        *self.current.write().unwrap() = Some(state);
    }
}
#[derive(Clone)]
pub struct RuntimeHandle {
    shared: Arc<RuntimeSwitch>,
    pinned: Option<Arc<HttpState>>,
}
impl RuntimeHandle {
    fn current(&self) -> Arc<HttpState> {
        self.pinned
            .clone()
            .unwrap_or_else(|| self.shared.snapshot())
    }
    pub fn store(&self) -> Arc<ProjectStore> {
        self.current().store.clone()
    }
    pub async fn launch(&self, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
        launch(&self.current(), id)
    }
    pub fn info(&self) -> BootstrapInfo {
        self.current().info.clone()
    }
    pub async fn project(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let mut info = self.current().store.info(id).await?;
        if info.manifest.is_some() && !matches!(info.state.as_str(), "missing" | "error") {
            match self.current().builds.info(id).await {
                Ok(build_info) => info.build_info = build_info,
                Err(error) => {
                    info.build_info = json!({"sourceState":"unknown","checkedAt":millis(),"stale":true,"reason":error.message});
                    info.issues.push(error.message);
                    if error.code.starts_with("BUILD_ARTIFACT") {
                        info.state = "error".into();
                    }
                }
            }
        }
        let mut value = json!(info);
        value["maintenance"] = self.current().maintenance.status(id)?;
        if value["maintenance"]["mode"] == "maintenance" {
            value["state"] = json!(if value["maintenance"]["recoveryRequired"] == true {
                "recoveryRequired"
            } else {
                "maintenance"
            });
        }
        let state = self.current();
        match state.metadata.info(id).await {
            Ok(metadata) => {
                for key in [
                    "metadataRevision",
                    "iconStatus",
                    "iconDataUrl",
                    "metadataIssues",
                ] {
                    value[key] = metadata[key].clone();
                }
            }
            Err(error) => {
                value["metadataRevision"] = Value::Null;
                value["iconStatus"] = json!({"state":"default","message":error.message});
                value["iconDataUrl"] = Value::Null;
                value["metadataIssues"] = json!([error.message]);
            }
        }
        value["storage"] = state.storage.info(id).await.unwrap_or_else(
            |error| json!({"currentProject":null,"issues":[error],"state":"unavailable"}),
        );
        Ok(value)
    }
    pub fn platform_info(&self) -> Value {
        crate::platform_info::info()
    }
    pub fn validate_download_destination(
        &self,
        destination: &std::path::Path,
    ) -> std::result::Result<(), ApiError> {
        let state = self.current();
        if !destination.is_absolute() {
            return Err(ApiError::new(
                "INVALID_PATH",
                "Choose an absolute download destination",
            ));
        }
        crate::projects::check_no_symlinks(destination, true)?;
        let parent = destination
            .parent()
            .ok_or_else(|| ApiError::new("INVALID_PATH", "Download parent missing"))?;
        let parent = std::fs::canonicalize(parent)?;
        let actual = parent.join(
            destination
                .file_name()
                .ok_or_else(|| ApiError::new("INVALID_PATH", "Download filename missing"))?,
        );
        let retained = crate::workspace::info(&state.paths, &state.store)?;
        if retained["retainedRoots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["path"].as_str())
            .any(|path| actual.starts_with(path))
        {
            return Err(ApiError::new(
                "UNSAFE_DOWNLOAD_DESTINATION",
                "Save outside retained migration snapshots",
            ));
        }
        if [
            Some(state.paths.data_root().to_path_buf()),
            Some(state.paths.workspace_root().to_path_buf()),
            crate::platform_info::installation_root(),
        ]
        .into_iter()
        .flatten()
        .any(|root| actual.starts_with(root))
        {
            return Err(ApiError::new(
                "UNSAFE_DOWNLOAD_DESTINATION",
                "Save outside platform data, projects and the application bundle",
            ));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&actual) {
            use std::os::unix::fs::MetadataExt;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(ApiError::new(
                    "INVALID_PATH",
                    "Download target must be an independent regular file",
                ));
            }
        }
        Ok(())
    }
    pub fn device_permission_declared(&self, id: &str, device: &str) -> bool {
        let state = self.current();
        let Ok(project) = state.store.project(id) else {
            return false;
        };
        if project.validate_epoch(None).is_err() {
            return false;
        }
        let Ok(policy) =
            crate::runtime_policy::RuntimePolicy::parse(&project.manifest.runtime_requirements)
        else {
            return false;
        };
        match device {
            "camera" => policy.devices().camera.is_some(),
            "microphone" => policy.devices().microphone.is_some(),
            _ => false,
        }
    }
    pub async fn project_metadata(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().metadata.info(id).await
    }
    pub async fn project_location(
        &self,
        id: &str,
        target: &str,
    ) -> std::result::Result<PathBuf, ApiError> {
        let state = self.current();
        let (root, manifest) = state.store.metadata_root(id)?;
        match target {
            "root" => Ok(root),
            "readme" => {
                let readme = manifest
                    .as_ref()
                    .and_then(|m| m.docs["readme"].as_str())
                    .unwrap_or("README.md");
                crate::projects::checked_relative(&root, readme, false)
            }
            _ => Err(ApiError::new(
                "INVALID_INPUT",
                "Only project root and README locations are supported",
            )),
        }
    }
    pub async fn project_metadata_set(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        self.current().metadata.update(id, input).await
    }
    pub async fn project_icon_import(
        &self,
        id: &str,
        source: &std::path::Path,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        let expected = input["expectedRevision"]
            .as_str()
            .ok_or_else(|| ApiError::new("INVALID_INPUT", "expectedRevision required"))?;
        self.current()
            .metadata
            .import_icon(id, source, expected, input["maintenanceId"].as_str())
            .await
    }
    pub async fn storage_info(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().storage.info(id).await
    }
    pub async fn storage_scan(&self, id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().storage.submit_scan(id).await?))
    }
    pub async fn storage_files(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        self.current().storage.files(id, input).await
    }
    pub async fn storage_cleanup(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current().storage.submit_cleanup(id, input).await?
        ))
    }
    pub async fn workspace_storage(&self) -> std::result::Result<Value, ApiError> {
        self.current().storage.workspace_info().await
    }
    pub async fn workspace_cleanup_preview(&self) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if self.shared.closing.load(Ordering::Acquire)
            || !Arc::ptr_eq(&state, &self.shared.snapshot())
        {
            return Err(ApiError::new(
                "SERVICE_INSTANCE_CHANGED",
                "Refresh the current workspace before checking cleanup",
            ));
        }
        Ok(json!(state.history_cleanup.preview()?))
    }
    pub async fn workspace_cleanup(&self, input: Value) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if self.shared.closing.load(Ordering::Acquire)
            || !Arc::ptr_eq(&state, &self.shared.snapshot())
        {
            return Err(ApiError::new(
                "SERVICE_INSTANCE_CHANGED",
                "Refresh the current workspace before submitting cleanup",
            ));
        }
        Ok(json!(state.history_cleanup.submit(input)?))
    }
    /// Native manager attachment access keeps the gate until the descriptor has
    /// been consumed. It shares the browser's epoch and maintenance checks.
    pub async fn attachment_read(
        &self,
        id: &str,
        path: &str,
        epoch: &str,
    ) -> std::result::Result<(std::fs::File, tokio::sync::OwnedRwLockReadGuard<()>, Value), ApiError>
    {
        let project = self.current().store.project(id)?;
        let lease = project.operation_gate.clone().read_owned().await;
        project.validate_epoch(Some(epoch))?;
        let root = files::root(&project.root.join(&project.manifest.attachments_dir))?;
        let file = files::read(&root, path)?;
        let info = json!(files::info(path, &file)?);
        Ok((file, lease, info))
    }
    pub async fn projects(&self) -> std::result::Result<Value, ApiError> {
        let mut items = Vec::new();
        for info in self.current().store.list().await? {
            if crate::projects::valid_id(&info.id) {
                items.push(self.project(&info.id).await?);
            } else {
                items.push(json!(info));
            }
        }
        Ok(json!(items))
    }
    pub async fn create_project(
        &self,
        input: CreateProjectInput,
    ) -> std::result::Result<Value, ApiError> {
        let info = self.current().store.create(input).await?;
        self.project(&info.id).await
    }
    pub async fn build_project(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.build_project_with_input(id, json!({})).await
    }
    pub async fn build_project_with_input(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        let input: crate::maintenance::OperationInput = serde_json::from_value(input)?;
        Ok(json!(
            self.current()
                .builds
                .submit_with_maintenance(id, input.maintenance_id.as_deref())
                .await?
        ))
    }
    pub async fn backup_list(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().maintenance.list(id).await
    }
    pub async fn backup_info(
        &self,
        id: &str,
        backup_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        self.current().maintenance.info(id, backup_id).await
    }
    pub async fn maintenance_status(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().maintenance.status(id)
    }
    pub async fn backup_create(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .maintenance
                .submit(id, "backup", serde_json::from_value(input)?)
                .await?
        ))
    }
    pub async fn backup_restore(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .maintenance
                .submit(id, "restore", serde_json::from_value(input)?)
                .await?
        ))
    }
    pub async fn maintenance_begin(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .maintenance
                .submit(id, "maintenanceBegin", serde_json::from_value(input)?)
                .await?
        ))
    }
    pub async fn maintenance_end(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .maintenance
                .submit(id, "maintenanceEnd", serde_json::from_value(input)?)
                .await?
        ))
    }
    pub async fn task(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if let Some(task) = crate::workspace::task(&state.paths, id)? {
            return Ok(json!(task));
        }
        Ok(json!(state.builds.task(id)?))
    }
    pub async fn task_logs(&self, id: &str, cursor: u64) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if let Some(logs) = crate::workspace::logs(&state.paths, id, cursor)? {
            return Ok(logs);
        }
        Ok(json!(state.builds.logs(id, cursor)?))
    }
    pub async fn environment(&self) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().builds.environment().doctor().await))
    }
    pub async fn set_node_path(
        &self,
        node_path: Option<String>,
    ) -> std::result::Result<Value, ApiError> {
        self.current().store.check_workspace_available()?;
        Ok(json!(
            self.current()
                .builds
                .environment()
                .configure_node(node_path.map(PathBuf::from))
                .await?
        ))
    }
    pub async fn handoff(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        let project = state.store.project(id)?;
        let resources = state.resources.as_ref().ok_or_else(|| {
            ApiError::new(
                "INSTALLATION_RESOURCES_MISSING",
                "This service has no installed project template or CLI resources",
            )
        })?;
        Ok(resources.handoff(&project.root, id))
    }
    pub async fn project_export(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .lifecycle
                .submit_project(id, "export", input)
                .await?
        ))
    }
    pub async fn project_import(&self, input: Value) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().lifecycle.submit_import(input).await?))
    }
    pub async fn project_copy(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .lifecycle
                .submit_project(id, "copy", input)
                .await?
        ))
    }
    pub async fn project_delete(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current()
                .lifecycle
                .submit_project(id, "delete", input)
                .await?
        ))
    }
    pub async fn trash_list(&self) -> std::result::Result<Value, ApiError> {
        self.current().lifecycle.trash_list().await
    }
    pub async fn trash_info(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().lifecycle.trash_info(id).await
    }
    pub async fn trash_restore(&self, id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current().lifecycle.submit_trash(id, "restore").await?
        ))
    }
    pub async fn trash_purge(&self, id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(
            self.current().lifecycle.submit_trash(id, "purge").await?
        ))
    }
    pub async fn workspace_info(&self) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        let mut value = crate::workspace::info(&state.paths, &state.store)?;
        value["cleanupTask"] = json!(state.history_cleanup.latest_task()?);
        Ok(value)
    }
    pub async fn workspace_migrate(&self, input: Value) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if !Arc::ptr_eq(&state, &self.shared.snapshot()) {
            return Err(ApiError::new(
                "SERVICE_INSTANCE_CHANGED",
                "Refresh discovery before submitting another operation",
            ));
        }
        submit_migration(self.shared.clone(), state, serde_json::from_value(input)?)
            .await
            .map(|task| json!(task))
    }
}
pub struct RuntimeService {
    shared: Arc<RuntimeSwitch>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
    _lock: InstanceLock,
}
impl RuntimeService {
    pub async fn start(paths: RuntimePaths, lock: InstanceLock) -> Result<Self> {
        Self::start_inner(paths, lock, None).await
    }
    pub async fn start_with_resources(
        paths: RuntimePaths,
        lock: InstanceLock,
        resource_root: PathBuf,
        cli_path: PathBuf,
    ) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        let resources = Arc::new(ProjectResources::new(resource_root, cli_path)?);
        Self::start_inner(paths, lock, Some(resources)).await
    }
    async fn start_inner(
        paths: RuntimePaths,
        lock: InstanceLock,
        resources: Option<Arc<ProjectResources>>,
    ) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        paths.verify_lock(&lock)?;
        crate::workspace::recover_startup(&paths)?;
        let listener = bind_loopback(paths.last_port()?).await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(RuntimeSwitch {
            current: StdRwLock::new(None),
            router: StdRwLock::new(Router::new()),
            migration_runner: Mutex::new(None),
            closing: AtomicBool::new(false),
            migration_active: AtomicBool::new(false),
        });
        let state = make_state(
            paths.clone(),
            format!("http://127.0.0.1:{port}"),
            resources,
            Arc::downgrade(&shared),
            None,
        )?;
        let info = state.info.clone();
        let token = state.token.clone();
        shared.install(state);
        let router = Router::new()
            .fallback(any(dispatch))
            .with_state(shared.clone());
        let discovery = Discovery {
            api_version: API_VERSION,
            instance_id: info.instance_id.clone(),
            pid: std::process::id(),
            base_url: info.base_url.clone(),
            workspace_root: info.workspace_root.clone(),
            environment: ENVIRONMENT.into(),
            identifier: APP_IDENTIFIER.into(),
        };
        paths.save_port(port)?;
        paths.publish_runtime(&discovery, &token)?;
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Ok(Self {
            shared,
            shutdown: Some(shutdown),
            task: Some(task),
            _lock: lock,
        })
    }
    pub fn info(&self) -> BootstrapInfo {
        self.shared.snapshot().info.clone()
    }
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            shared: self.shared.clone(),
            pinned: None,
        }
    }
    pub fn store(&self) -> Arc<ProjectStore> {
        self.shared.snapshot().store.clone()
    }
    pub async fn launch(&self, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
        launch(&self.shared.snapshot(), id)
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.shared.closing.store(true, Ordering::Release);
        let migration = self.shared.migration_runner.lock().unwrap().take();
        if let Some(task) = migration {
            task.await.context("Workspace migration runner panicked")?;
        }
        let state = self.shared.snapshot();
        state.stopping.store(true, Ordering::SeqCst);
        let _ = state.stop.send(true);
        if let Some(signal) = self.shutdown.take() {
            let _ = signal.send(());
        }
        loop {
            let notified = state.drained.notified();
            if state.active.load(Ordering::SeqCst) == 0 {
                break;
            }
            notified.await;
        }
        state.storage.shutdown().await;
        state.history_cleanup.shutdown().await;
        state.lifecycle.shutdown().await;
        state.maintenance.shutdown().await;
        state.builds.shutdown().await;
        state.store.shutdown().await;
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
                Ok(result) => result
                    .context("Local HTTP service panicked")?
                    .context("Local HTTP service failed")?,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        state
            .paths
            .remove_runtime_if_owned(&state.info.instance_id, &state.token)
    }
}
impl Drop for RuntimeService {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        let state = self.shared.snapshot();
        state.stopping.store(true, Ordering::SeqCst);
        let _ = state.stop.send(true);
        if let Some(signal) = self.shutdown.take() {
            let _ = signal.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Err(error) = state
            .paths
            .remove_runtime_if_owned(&state.info.instance_id, &state.token)
        {
            eprintln!("Development runtime cleanup failed: {error}");
        }
    }
}
fn runtime_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/runtime", get(runtime))
        .route("/api/v1/{*tail}", any(management))
        .route("/_platform/connect", get(connect_page))
        .route("/_platform/session", post(exchange))
        .route("/p/{id}/api/v1/{*tail}", any(project_api))
        .route("/p/{id}/", get(project_entry))
        .route("/p/{id}/{*tail}", get(project_static))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), validate_host))
        .with_state(state)
}
async fn dispatch(State(shared): State<Arc<RuntimeSwitch>>, request: Request) -> Response {
    if shared.closing.load(Ordering::Acquire) {
        return failure(
            ApiError::new("SERVICE_STOPPING", "Platform is shutting down"),
            None,
        );
    }
    let router = shared.router.read().unwrap().clone();
    match router.oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    }
}
fn make_state(
    paths: RuntimePaths,
    base_url: String,
    resources: Option<Arc<ProjectResources>>,
    shared: Weak<RuntimeSwitch>,
    store: Option<Arc<ProjectStore>>,
) -> Result<Arc<HttpState>> {
    let store = match store {
        Some(store) => store,
        None => Arc::new(ProjectStore::with_resources(
            paths.workspace_root().into(),
            resources.clone(),
        )?),
    };
    let info = BootstrapInfo {
        display_name: DISPLAY_NAME.into(),
        identifier: APP_IDENTIFIER.into(),
        environment: ENVIRONMENT.into(),
        api_version: API_VERSION,
        instance_id: Uuid::new_v4().to_string(),
        base_url,
        workspace_root: paths.workspace_root().into(),
        data_root: paths.data_root().into(),
        sqlite_version: rusqlite::version().into(),
    };
    let builds = BuildManager::new(paths.clone(), store.clone(), info.base_url.clone())?;
    let maintenance =
        crate::maintenance::MaintenanceManager::new(paths.clone(), store.clone(), builds.clone())?;
    let lifecycle = crate::lifecycle::LifecycleManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        maintenance.clone(),
    )?;
    let metadata = crate::metadata::MetadataManager::new(store.clone(), info.instance_id.clone());
    let storage = crate::storage::StorageManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        maintenance.clone(),
    )?;
    let history_cleanup = crate::history_cleanup::CleanupManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        lifecycle.clone(),
        info.instance_id.clone(),
    )?;
    let (events, _) = broadcast::channel(128);
    let (stop, _) = watch::channel(false);
    let state = Arc::new(HttpState {
        paths,
        shared,
        info: info.clone(),
        token: secret(),
        expected_host: format!(
            "127.0.0.1:{}",
            url::Url::parse(&info.base_url)?.port().unwrap()
        ),
        store,
        builds,
        maintenance,
        lifecycle,
        metadata,
        storage,
        history_cleanup,
        resources,
        tickets: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        events,
        uploads: Arc::new(Semaphore::new(2)),
        project_uploads: Mutex::new(HashMap::new()),
        stopping: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        drained: Notify::new(),
        stop,
    });
    for mut receiver in [state.maintenance.subscribe(), state.lifecycle.subscribe()] {
        let weak = Arc::downgrade(&state);
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(id) => {
                        let Some(state) = weak.upgrade() else { break };
                        let _ = state.events.send(Change {
                            project_id: id,
                            request_id: Uuid::new_v4().to_string(),
                            source: "lifecycle".into(),
                            event: "project.state.changed".into(),
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    let mut receiver = state.builds.subscribe();
    let weak = Arc::downgrade(&state);
    let mut stop = state.stop.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _=stop.changed()=>break,
                event=receiver.recv()=>match event {
                    Ok(event) => {
                        let Some(state)=weak.upgrade() else {break};
                        let task_type=state.builds.task(&event.task_id).map(|t|t.task_type).unwrap_or_default();
                        if task_type=="build" && event.status==TaskStatus::Succeeded {
                            let _=state.events.send(Change{project_id:event.project_id,request_id:event.task_id,source:"build".into(),event:"project.updated".into()});
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_))=>(),Err(_)=>break
                }
            }
        }
    });
    Ok(state)
}
async fn submit_migration(
    shared: Arc<RuntimeSwitch>,
    state: Arc<HttpState>,
    input: crate::workspace::MigrationInput,
) -> std::result::Result<TaskRecord, ApiError> {
    if shared.closing.load(Ordering::Acquire) {
        return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
    }
    if shared.migration_active.swap(true, Ordering::AcqRel) {
        return Err(ApiError::new(
            "WORKSPACE_MIGRATING",
            "A migration is already finishing or running",
        ));
    }
    if let Err(error) = state.store.begin_workspace_migration() {
        shared.migration_active.store(false, Ordering::Release);
        return Err(error);
    }
    let submitted = match crate::workspace::create(&state.paths, &input.destination_path) {
        Ok(task) => task,
        Err(error) => {
            state.store.resume_workspace();
            state.storage.resume();
            shared.migration_active.store(false, Ordering::Release);
            return Err(error);
        }
    };
    let id = submitted.task_id.clone();
    let weak = Arc::downgrade(&shared);
    let migration_shared = shared.clone();
    let runner = tokio::spawn(async move {
        struct Lease(Arc<RuntimeSwitch>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.migration_active.store(false, Ordering::Release);
            }
        }
        let _lease = Lease(migration_shared);
        let mut committed = false;
        let outcome: std::result::Result<Value, ApiError> = async {
            crate::workspace::phase(&state.paths, &id, "waiting-for-project-tasks")?;
            while state.store.has_active_tasks() {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            // Reject unresolved recovery before any filesystem copying.
            for project in state.store.list().await? {
                if crate::projects::valid_id(&project.id)
                    && state.store.control(&project.id)?.get().recovery_required
                {
                    return Err(ApiError::new(
                        "RECOVERY_REQUIRED",
                        "Resolve pending project recovery before moving the workspace",
                    )
                    .detail("projectId", project.id));
                }
            }
            state.storage.quiesce().await;
            state.store.pause_workspace().await;
            let mut guards = vec![];
            for project in state.store.list().await? {
                if crate::projects::valid_id(&project.id) {
                    let _ = state.events.send(Change {
                        project_id: project.id.clone(),
                        request_id: id.clone(),
                        source: "migration".into(),
                        event: "project.state.changed".into(),
                    });
                    guards.push(state.store.gate(&project.id).write_owned().await);
                }
            }
            let paths = state.paths.clone();
            let store = state.store.clone();
            let task_id = id.clone();
            let (selected, mut journal) = tokio::task::spawn_blocking(move || {
                crate::workspace::copy_and_commit(&paths, &store, &task_id)
            })
            .await
            .map_err(|e| ApiError::new("WORKSPACE_MIGRATION_FAILED", e.to_string()))??;
            committed = true;
            let store = Arc::new(ProjectStore::with_resources(
                selected.workspace_root().into(),
                state.resources.clone(),
            )?);
            crate::workspace::initialize_selected(&selected, &store, &mut journal)?;
            let Some(shared) = weak.upgrade() else {
                return Err(ApiError::new(
                    "SERVICE_STOPPING",
                    "Runtime closed after committing the new workspace",
                ));
            };
            let new = make_state(
                selected.clone(),
                state.info.base_url.clone(),
                state.resources.clone(),
                Arc::downgrade(&shared),
                Some(store),
            )
            .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            let port = url::Url::parse(&new.info.base_url)
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?
                .port()
                .unwrap();
            selected
                .publish_runtime(
                    &Discovery {
                        api_version: API_VERSION,
                        instance_id: new.info.instance_id.clone(),
                        pid: std::process::id(),
                        base_url: new.info.base_url.clone(),
                        workspace_root: new.info.workspace_root.clone(),
                        environment: ENVIRONMENT.into(),
                        identifier: APP_IDENTIFIER.into(),
                    },
                    &new.token,
                )
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            selected
                .save_port(port)
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            shared.install(new.clone());
            let _ = state.stop.send(true);
            state.storage.shutdown().await;
            state.history_cleanup.shutdown().await;
            state.lifecycle.shutdown().await;
            state.maintenance.shutdown().await;
            state.builds.shutdown().await;
            crate::workspace::mark_old_retired(&selected, &journal)?;
            for project in new.store.list().await? {
                if let Some(resources) = &new.resources {
                    if let Ok(project) = new.store.project(&project.id) {
                        let handoff_path = project.root.join(".platform/handoff.md");
                        crate::projects::check_no_symlinks(&handoff_path, true)?;
                        if let Ok(text) = std::fs::read_to_string(&handoff_path) {
                            if text.starts_with("# Platform-managed project handoff\n") {
                                let updated = text
                                    .lines()
                                    .map(|line| {
                                        if line.starts_with("Project root: ") {
                                            format!("Project root: `{}`", project.root.display())
                                        } else {
                                            line.to_owned()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                                    + "\n";
                                let temp = handoff_path.with_file_name(format!(
                                    ".handoff-{}",
                                    Uuid::new_v4().simple()
                                ));
                                let mut file = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create_new(true)
                                    .mode(0o600)
                                    .open(&temp)?;
                                std::io::Write::write_all(&mut file, updated.as_bytes())?;
                                file.sync_all()?;
                                std::fs::rename(&temp, &handoff_path)?;
                            }
                        }
                        builds::atomic_json(
                            &project.root.join(".platform/current-handoff.json"),
                            &resources.handoff(&project.root, &project.manifest.id),
                        )?;
                    }
                }
            }
            drop(guards);
            Ok(crate::workspace::completed_result(&journal))
        }
        .await;
        if let Err(error) = &outcome {
            // If settings committed but a final journal write failed, selection
            // remains authoritative. Never reopen or write into the source.
            committed =
                committed || crate::workspace::is_committed(&state.paths, &id).unwrap_or(true);
            if !committed {
                state.store.resume_workspace();
                state.storage.resume();
                if let Ok(projects) = state.store.list().await {
                    for project in projects {
                        let _ = state.events.send(Change {
                            project_id: project.id,
                            request_id: id.clone(),
                            source: "migration".into(),
                            event: "project.state.changed".into(),
                        });
                    }
                }
            } else {
                state.store.pause_workspace().await;
            }
            let error = error
                .clone()
                .outcome(if committed {
                    "committed_needs_attention"
                } else {
                    "not_committed"
                })
                .detail("sourceRetained", true)
                .detail("sourcePath", json!(state.paths.workspace_root()))
                .detail("destinationPath", json!(input.destination_path));
            let _ = crate::workspace::finish(&state.paths, &id, Err(error));
        } else {
            let _ = crate::workspace::finish(&state.paths, &id, outcome);
        }
    });
    *shared.migration_runner.lock().unwrap() = Some(runner);
    Ok(submitted)
}

fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn is_html(path: &str) -> bool {
    files::mime(path).split(';').next() == Some("text/html")
}
fn validate_entry_format(path: &str) -> std::result::Result<(), ApiError> {
    if is_html(path) {
        Ok(())
    } else {
        Err(ApiError::new(
            "INVALID_PROJECT",
            "Project entry must be an HTML document",
        ))
    }
}
fn launch(state: &HttpState, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
    if state.stopping.load(Ordering::SeqCst) {
        return Err(ApiError::new(
            "SERVICE_STOPPING",
            "Platform is shutting down",
        ));
    }
    let project = state.store.project(id)?;
    project.validate_epoch(Some(&project.document_epoch))?;
    validate_entry_format(&project.manifest.entry)?;
    let (output_path, entry) = match builds::artifact_generation(&project)? {
        Some(generation) => (generation.root, generation.entry_relative),
        None => {
            let entry = project
                .manifest
                .entry
                .strip_prefix(&format!("{}/", project.manifest.output_dir))
                .ok_or_else(|| {
                    ApiError::new("INVALID_PROJECT", "Entry is outside output directory")
                })?;
            (
                project.root.join(&project.manifest.output_dir),
                entry.to_owned(),
            )
        }
    };
    let output = files::root(&output_path).map_err(|error| {
        if error.code == "FILE_NOT_FOUND" {
            ApiError::new("PROJECT_NOT_BUILT", "Project has no built output yet")
        } else {
            error
        }
    })?;
    files::read(&output, &entry).map_err(|_| {
        ApiError::new(
            "PROJECT_NOT_BUILT",
            "Project has no readable built entry yet",
        )
    })?;
    let ticket = secret();
    let mut tickets = state.tickets.lock().unwrap();
    tickets.retain(|_, value| value.expires > Instant::now());
    if tickets.len() >= 1024 {
        return Err(ApiError::new(
            "SERVICE_BUSY",
            "Too many unconsumed launch tickets",
        ));
    }
    tickets.insert(
        ticket.clone(),
        Ticket {
            project_id: id.into(),
            epoch: project.document_epoch.clone(),
            expires: Instant::now() + Duration::from_secs(60),
        },
    );
    Ok(LaunchInfo {
        project_id: id.into(),
        url: format!("{}/p/{id}/", state.info.base_url),
        launch_url: format!("{}/_platform/connect#ticket={ticket}", state.info.base_url),
        expires_at: millis() + 60000,
        instance_id: state.info.instance_id.clone(),
        document_epoch: project.document_epoch,
    })
}
async fn bind_loopback(preferred: Option<u16>) -> Result<TcpListener> {
    if let Some(port) = preferred {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(l) => return Ok(l),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(e) => return Err(e).context("Cannot bind development service"),
        }
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("Cannot bind development service")
}
struct Active(Arc<HttpState>);
impl Drop for Active {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.drained.notify_one();
        }
    }
}
async fn validate_host(
    State(state): State<Arc<HttpState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(error) = platform_project_format::http_protocol::host(request.headers(), &state.expected_host) { return failure(error, None); }
    state.active.fetch_add(1, Ordering::SeqCst);
    let _active = Active(state.clone());
    if state.stopping.load(Ordering::SeqCst) {
        return failure(
            ApiError::new("SERVICE_STOPPING", "Platform is shutting down"),
            None,
        );
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
}
fn status(error: &ApiError) -> StatusCode { StatusCode::from_u16(platform_project_format::http_protocol::status(error)).unwrap() }
fn failure(error: ApiError, project_id: Option<&str>) -> Response {
    let status = status(&error);
    (status,Json(platform_project_format::http_protocol::envelope::<Value>(Err(error), project_id, &Uuid::new_v4().to_string()))).into_response()
}
fn result<T: Serialize>(
    data: std::result::Result<T, ApiError>,
    project_id: Option<&str>,
    request_id: &str,
) -> Response {
    let status = data.as_ref().err().map(status).unwrap_or(StatusCode::OK); (status, Json(platform_project_format::http_protocol::envelope(data, project_id, request_id))).into_response()
}
async fn not_found() -> Response {
    failure(ApiError::new("NOT_FOUND", "Unknown platform route"), None)
}
async fn health(State(state): State<Arc<HttpState>>) -> Json<HealthInfo> {
    Json(HealthInfo {
        api_version: API_VERSION,
        instance_id: state.info.instance_id.clone(),
        identifier: APP_IDENTIFIER.into(),
        environment: ENVIRONMENT.into(),
    })
}
fn admin(state: &HttpState, headers: &HeaderMap) -> std::result::Result<(), ApiError> {
    if headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site") {
        return Err(ApiError::new(
            "BROWSER_MANAGEMENT_DENIED",
            "Browser pages cannot use management APIs",
        ));
    }
    let values = headers.get_all(header::AUTHORIZATION);
    if values.iter().count() != 1
        || values.iter().next().and_then(|v| v.to_str().ok())
            != Some(format!("Bearer {}", state.token).as_str())
    {
        return Err(ApiError::new(
            "UNAUTHENTICATED",
            "Management credential is missing or invalid",
        ));
    }
    Ok(())
}
async fn runtime(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    match admin(&state, &headers) {
        Ok(()) => {
            let mut info = json!(state.info);
            info["platform"] = crate::platform_info::info();
            Json(info).into_response()
        }
        Err(error) => failure(error, None),
    }
}
fn origin(state: &HttpState, headers: &HeaderMap, required: bool) -> std::result::Result<(), ApiError> { platform_project_format::http_protocol::origin(headers, &state.info.base_url, required) }
async fn input<T: DeserializeOwned>(request: Request) -> std::result::Result<T, ApiError> {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim())
        != Some("application/json")
    {
        return Err(ApiError::new("INVALID_INPUT", "Expected application/json"));
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(30),
        to_bytes(request.into_body(), JSON_MAX),
    )
    .await
    .map_err(|_| ApiError::new("REQUEST_TIMEOUT", "JSON request body timed out"))?
    .map_err(|_| {
        ApiError::new(
            "REQUEST_TOO_LARGE",
            "JSON request exceeds 2 MB or could not be read",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|e| ApiError::new("INVALID_INPUT", e.to_string()))
}
fn changed(state: &HttpState, id: &str, request_id: &str, source: &str) {
    if let Err(error) =
        crate::maintenance::MaintenanceManager::invalidate_storage(state.paths.workspace_root(), id)
    {
        eprintln!("Storage invalidation failed: {error}");
    }
    let _ = state.events.send(Change {
        project_id: id.into(),
        request_id: request_id.into(),
        source: source.into(),
        event: "data.changed".into(),
    });
}
async fn optional_json(request: Request) -> std::result::Result<Value, ApiError> {
    let bytes = to_bytes(request.into_body(), JSON_MAX)
        .await
        .map_err(|_| ApiError::new("INVALID_INPUT", "Request body too large"))?;
    if bytes.is_empty() {
        Ok(json!({}))
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}
async fn managed_input<T: DeserializeOwned>(
    request: Request,
) -> std::result::Result<(T, Option<String>), ApiError> {
    let mut value = input::<Value>(request).await?;
    let maintenance = match value
        .as_object_mut()
        .and_then(|v| v.remove("maintenanceId"))
    {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id),
        Some(_) => {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "maintenanceId must be a string",
            ))
        }
    };
    Ok((serde_json::from_value(value)?, maintenance))
}
async fn management(
    State(state): State<Arc<HttpState>>,
    Path(tail): Path<String>,
    request: Request,
) -> Response {
    let segments: Vec<_> = tail.split('/').collect();
    let method = request.method().clone();
    let is_build_submission = method == Method::POST
        && matches!(
            segments.as_slice(),
            [
                "projects",
                _,
                "builds" | "backups" | "restore" | "maintenance" | "exports" | "copies" | "trash"
            ] | ["projects", _, "maintenance", "end"]
                | ["projects", "imports"]
                | ["trash", _, "restore" | "purge"]
                | ["workspace", "migrations" | "cleanup-plans" | "cleanups"]
                | ["projects", _, "storage", "scans" | "cleanups"]
        );
    let rid = Uuid::new_v4().to_string();
    let id = if segments.len() > 1 && segments[0] == "projects" {
        Some(segments[1])
    } else {
        None
    };
    if let Err(error) = admin(&state, request.headers()) {
        return failure(error, id);
    }
    let Some(shared) = state.shared.upgrade() else {
        return failure(ApiError::new("SERVICE_STOPPING", "Runtime is stopping"), id);
    };
    if !Arc::ptr_eq(&state, &shared.snapshot()) {
        return failure(
            ApiError::new(
                "SERVICE_INSTANCE_CHANGED",
                "Refresh service discovery before retrying a query",
            ),
            id,
        );
    }
    if !matches!(method, Method::GET | Method::HEAD)
        && segments.as_slice() != ["workspace", "migrations"]
    {
        if let Err(error) = state.store.check_workspace_available() {
            return failure(error, id);
        }
    }
    let handle = RuntimeHandle {
        shared,
        pinned: Some(state.clone()),
    };
    let outcome: std::result::Result<Value, ApiError> = async {
        match (method, segments.as_slice()) {
            (Method::GET, ["environment"]) => handle.environment().await,
            (Method::PUT, ["environment", "node"]) => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct NodeConfig {
                    node_path: Option<String>,
                }
                handle
                    .set_node_path(input::<NodeConfig>(request).await?.node_path)
                    .await
            }
            (Method::GET, ["workspace", "storage"]) => handle.workspace_storage().await,
            (Method::POST, ["workspace", "cleanup-plans"]) => {
                handle.workspace_cleanup_preview().await
            }
            (Method::POST, ["workspace", "cleanups"]) => {
                handle
                    .workspace_cleanup(input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["workspace"]) => handle.workspace_info().await,
            (Method::GET, ["projects", id, "metadata"]) => handle.project_metadata(id).await,
            (Method::PATCH, ["projects", id, "metadata"]) => {
                handle
                    .project_metadata_set(id, input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "storage"]) => handle.storage_info(id).await,
            (Method::POST, ["projects", id, "storage", "scans"]) => {
                let body = optional_json(request).await?;
                if body != json!({}) {
                    return Err(ApiError::new("INVALID_INPUT", "Scan input must be empty"));
                }
                handle.storage_scan(id).await
            }
            (Method::POST, ["projects", id, "storage", "cleanups"]) => {
                handle
                    .storage_cleanup(id, input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "storage", "files"]) => {
                let mut body = serde_json::Map::new();
                for (key, value) in query(&request)? {
                    if key == "limit" {
                        body.insert(
                            key,
                            json!(value.parse::<u64>().map_err(|_| ApiError::new(
                                "INVALID_INPUT",
                                "limit must be an integer"
                            ))?),
                        );
                    } else {
                        body.insert(key, Value::String(value));
                    }
                }
                handle.storage_files(id, Value::Object(body)).await
            }
            (Method::POST, ["workspace", "migrations"]) => {
                handle
                    .workspace_migrate(input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["trash"]) => handle.trash_list().await,
            (Method::GET, ["trash", id]) => handle.trash_info(id).await,
            (Method::POST, ["trash", id, "restore"]) => handle.trash_restore(id).await,
            (Method::POST, ["trash", id, "purge"]) => handle.trash_purge(id).await,
            (Method::POST, ["projects", "imports"]) => {
                handle.project_import(input::<Value>(request).await?).await
            }
            (Method::POST, ["projects", id, "exports"]) => {
                handle
                    .project_export(id, input::<Value>(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "copies"]) => {
                handle.project_copy(id, optional_json(request).await?).await
            }
            (Method::POST, ["projects", id, "trash"]) => {
                handle
                    .project_delete(id, optional_json(request).await?)
                    .await
            }
            (Method::GET, ["projects"]) => handle.projects().await,
            (Method::POST, ["projects"]) => {
                handle
                    .create_project(input::<CreateProjectInput>(request).await?)
                    .await
            }
            (Method::GET, ["projects", id]) => handle.project(id).await,
            (Method::POST, ["projects", id, "builds"]) => {
                handle
                    .build_project_with_input(id, optional_json(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "backups"]) => handle.backup_list(id).await,
            (Method::POST, ["projects", id, "backups"]) => {
                handle
                    .backup_create(id, optional_json(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "backups", backup]) => {
                handle.backup_info(id, backup).await
            }
            (Method::POST, ["projects", id, "restore"]) => {
                handle
                    .backup_restore(id, input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "maintenance"]) => handle.maintenance_status(id).await,
            (Method::POST, ["projects", id, "maintenance"]) => {
                handle
                    .maintenance_begin(id, input::<Value>(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "maintenance", "end"]) => {
                handle
                    .maintenance_end(id, input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "handoff"]) => handle.handoff(id).await,
            (Method::GET, ["tasks", id]) => handle.task(id).await,
            (Method::GET, ["tasks", id, "logs"]) => {
                let query: HashMap<String, String> =
                    url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                        .into_owned()
                        .collect();
                let cursor = query
                    .get("offset")
                    .map(|s| s.parse::<u64>())
                    .transpose()
                    .map_err(|_| {
                        ApiError::new("INVALID_INPUT", "offset must be an unsigned byte cursor")
                    })?
                    .unwrap_or(0);
                handle.task_logs(id, cursor).await
            }
            (Method::GET, ["projects", id, "db", "schema"]) => {
                Ok(json!(state.store.schema(id, None).await?))
            }
            (Method::POST, ["projects", id, "db", "execute"]) => {
                let (request, maintenance) = managed_input::<SqlRequest>(request).await?;
                let value = state
                    .store
                    .execute_managed(
                        id,
                        request,
                        None,
                        SqlCaller::Management,
                        maintenance.as_deref(),
                    )
                    .await?;
                if value.changed {
                    changed(&state, id, &rid, "sql");
                }
                Ok(json!(value))
            }
            (Method::POST, ["projects", id, "db", "transaction"]) => {
                let (request, maintenance) = managed_input::<TransactionRequest>(request).await?;
                let value = state
                    .store
                    .transaction_managed(
                        id,
                        request,
                        None,
                        SqlCaller::Management,
                        maintenance.as_deref(),
                    )
                    .await?;
                if value.iter().any(|v| v.changed) {
                    changed(&state, id, &rid, "transaction");
                }
                Ok(json!(value))
            }
            (Method::POST, ["projects", id, "launch"]) => Ok(json!(launch(&state, id)?)),
            (Method::POST, ["projects", id, "refresh"]) => {
                let input: crate::maintenance::OperationInput =
                    serde_json::from_value(optional_json(request).await?)?;
                state
                    .store
                    .control(id)?
                    .check(false, input.maintenance_id.as_deref(), false)?;
                state.store.project(id)?.check()?;
                changed(&state, id, &rid, "external");
                Ok(json!({"refreshed":true,"projectId":id}))
            }
            _ => Err(ApiError::new(
                "NOT_FOUND",
                "Unknown management method or route",
            )),
        }
    }
    .await;
    let accepted = is_build_submission && outcome.is_ok();
    let mut response = result(outcome, id, &rid);
    if accepted {
        *response.status_mut() = StatusCode::ACCEPTED;
    }
    response
}
fn cookie_name(id: &str) -> String { platform_project_format::http_protocol::cookie_name(id) }
fn session(
    state: &HttpState,
    id: &str,
    headers: &HeaderMap,
    mutating: bool,
) -> std::result::Result<ProjectHandle, ApiError> {
    origin(state, headers, mutating)?;
    let cookie = platform_project_format::http_protocol::session_cookie(headers, id)?;
    let sessions = state.sessions.lock().unwrap();
    let _session = sessions
        .get(cookie)
        .filter(|s| s.project_id == id)
        .ok_or_else(|| {
            ApiError::new(
                "INVALID_SESSION",
                "Project session has expired; reopen from the platform",
            )
        })?;
    if state.store.is_retired(id)?.is_some() {
        return Err(ApiError::new("PROJECT_TRASHED","Project has been moved to the recycle bin; reopen it through the platform after restoring"));
    }
    if _session.lifecycle_generation != state.store.session_generation(id)? {
        return Err(ApiError::new(
            "INVALID_SESSION",
            "This project was re-registered; obtain a new launch session from the platform",
        ));
    }
    let project = state.store.project(id)?;
    project.check()?;
    Ok(project)
}
fn epoch<'a>(headers: &'a HeaderMap, query: &'a HashMap<String, String>) -> std::result::Result<&'a str, ApiError> { platform_project_format::http_protocol::epoch(headers, query.get("documentEpoch").map(String::as_str)) }
fn query(request: &Request) -> std::result::Result<HashMap<String, String>, ApiError> {
    axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|q| q.0)
        .map_err(|_| ApiError::new("INVALID_INPUT", "Invalid query parameters"))
}
async fn project_api(
    State(state): State<Arc<HttpState>>,
    Path((id, tail)): Path<(String, String)>,
    request: Request,
) -> Response {
    let rid = Uuid::new_v4().to_string();
    let method = request.method().clone();
    let setup = (|| {
        let project = session(
            &state,
            &id,
            request.headers(),
            method != Method::GET && method != Method::HEAD,
        )?;
        let query = query(&request)?;
        let epoch = epoch(request.headers(), &query)?.to_string();
        if !matches!(tail.as_str(), "context" | "events") {
            project.validate_epoch(Some(&epoch))?;
        }
        Ok::<_, ApiError>((project, query, epoch))
    })();
    let (project, query, epoch) = match setup {
        Ok(v) => v,
        Err(error) => return result::<Value>(Err(error), Some(&id), &rid),
    };
    if tail == "events" && method == Method::GET {
        return events(state, project, epoch).await;
    }
    if tail == "file" {
        let path = match query.get("path") {
            Some(path) => path.clone(),
            None => {
                return failure(
                    ApiError::new("INVALID_INPUT", "Missing attachment path"),
                    Some(&id),
                )
            }
        };
        if method == Method::GET {
            return read_attachment(&project, &path, &epoch, request.headers())
                .await
                .unwrap_or_else(|e| failure(e, Some(&id)));
        }
        if method == Method::PUT {
            return result(
                upload(
                    &state,
                    &project,
                    &path,
                    &epoch,
                    query.get("overwrite").is_some_and(|v| v == "true"),
                    request,
                    &rid,
                )
                .await,
                Some(&id),
                &rid,
            );
        }
        if method == Method::DELETE {
            let outcome = async {
                let _lease = project.operation_gate.read().await;
                project.validate_epoch(Some(&epoch))?;
                let root = files::root(&project.root.join(&project.manifest.attachments_dir))?;
                files::remove(&root, &path)?;
                changed(&state, &id, &rid, "file");
                Ok(json!({"removed":true,"path":path}))
            }
            .await;
            return result(outcome, Some(&id), &rid);
        }
    }
    let outcome:std::result::Result<Value,ApiError>=async {
        match(method,tail.as_str()) {
            (Method::GET,"context")=>Ok(json!({"projectId":id,"name":project.manifest.name,"sdkApiVersion":API_VERSION,"instanceId":state.info.instance_id,"capabilities":["db.execute","db.transaction","db.schema","files","events"],"runtimeRequirements":project.manifest.runtime_requirements,"runtimePolicyFingerprint":policy_fingerprint(&project.manifest.runtime_requirements),"limits":browser_limits(),"state":state.maintenance.browser_status(&id,&epoch)})),
            (Method::GET,"db/schema")=>Ok(json!(state.store.schema(&id,Some(&epoch)).await?)),
            (Method::POST,"db/execute")=>{let value=state.store.execute(&id,input::<SqlRequest>(request).await?,Some(&epoch),SqlCaller::Browser).await?;if value.changed{changed(&state,&id,&rid,"sql");}Ok(json!(value))},
            (Method::POST,"db/transaction")=>{let value=state.store.transaction(&id,input::<TransactionRequest>(request).await?,Some(&epoch),SqlCaller::Browser).await?;if value.iter().any(|v|v.changed){changed(&state,&id,&rid,"transaction");}Ok(json!(value))},
            (Method::GET,"files")=>{let _lease=project.operation_gate.read().await;project.validate_epoch(Some(&epoch))?;Ok(json!(files::list(&project.root.join(&project.manifest.attachments_dir),query.get("prefix").map(String::as_str).unwrap_or(""))?))},
            _=>Err(ApiError::new("NOT_FOUND","Unknown project method or route")),
        }
    }.await;
    result(outcome, Some(&id), &rid)
}
async fn connect_page(State(_state): State<Arc<HttpState>>) -> Response {
    let nonce = secret();
    let html = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>连接项目</title><style nonce="{nonce}">body{{font:16px system-ui;max-width:38rem;margin:15vh auto;padding:2rem;line-height:1.8;color:#202020}}h1{{font-size:24px}}p{{color:#666}}</style></head><body><h1>正在连接项目</h1><p id="message">正在建立本机项目会话…</p><script nonce="{nonce}">const ticket = new URLSearchParams(location.hash.slice(1)).get('ticket'); history.replaceState(null,'',location.pathname); (async()=>{{if(!ticket)throw new Error('请从平台重新打开项目，或让 AI 获取新的项目入口。'); const response=await fetch('/_platform/session',{{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{ticket}})}}); const envelope=await response.json();if(!response.ok||!envelope.ok)throw new Error(envelope.error?.message||'连接失败，请重新打开项目。'); location.replace(envelope.data.url);}})().catch(error=>{{document.querySelector('h1').textContent='需要重新连接';document.querySelector('#message').textContent=error.message;}});</script></body></html>"#
    );
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    response.headers_mut().insert("content-security-policy",format!("default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'").parse().unwrap());
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    ticket: String,
}
async fn exchange(State(state): State<Arc<HttpState>>, request: Request) -> Response {
    let rid = Uuid::new_v4().to_string();
    let outcome: std::result::Result<_, ApiError> = async {
        origin(&state, request.headers(), true)?;
        let input = input::<Exchange>(request).await?;
        let ticket = state
            .tickets
            .lock()
            .unwrap()
            .remove(&input.ticket)
            .filter(|v| v.expires > Instant::now())
            .ok_or_else(|| {
                ApiError::new(
                    "INVALID_TICKET",
                    "Launch ticket expired or was already used; reopen the project",
                )
            })?;
        let project = state.store.project(&ticket.project_id)?;
        project.validate_epoch(Some(&ticket.epoch))?;
        let token = secret();
        let mut sessions = state.sessions.lock().unwrap();
        if sessions.len() >= 4096 {
            return Err(ApiError::new(
                "SERVICE_BUSY",
                "Too many project sessions; restart the platform",
            ));
        }
        sessions.insert(
            token.clone(),
            Session {
                project_id: ticket.project_id.clone(),
                lifecycle_generation: state.store.session_generation(&ticket.project_id)?,
            },
        );
        Ok((ticket.project_id, token))
    }
    .await;
    match outcome {
        Err(error) => result::<Value>(Err(error), None, &rid),
        Ok((id, token)) => {
            let mut response = result(
                Ok(json!({"projectId":id,"url":format!("/p/{id}/")})),
                Some(&id),
                &rid,
            );
            response.headers_mut().insert(
                header::SET_COOKIE,
                format!(
                    "{}={token}; Path=/p/{id}/; HttpOnly; SameSite=Strict",
                    cookie_name(&id)
                )
                .parse()
                .unwrap(),
            );
            response
        }
    }
}
#[cfg(test)]
fn network_origins(requirements: &Value, kind: &str) -> std::result::Result<String, ApiError> {
    let Some(entries) = requirements.get("network").and_then(|v| v.get(kind)) else {
        return Ok(String::new());
    };
    let entries = entries
        .as_array()
        .ok_or_else(|| ApiError::new("INVALID_PROJECT", "Network declarations must be arrays"))?;
    let mut output = String::new();
    for item in entries {
        let origin = item.get("origin").and_then(Value::as_str).ok_or_else(|| {
            ApiError::new("INVALID_PROJECT", "Network declaration requires an origin")
        })?;
        let uri = url::Url::parse(origin)
            .map_err(|_| ApiError::new("INVALID_PROJECT", "Invalid network origin"))?;
        let host = uri
            .host_str()
            .ok_or_else(|| ApiError::new("INVALID_PROJECT", "Invalid network origin"))?;
        if uri.scheme() != "https"
            || origin != uri.origin().ascii_serialization()
            || !uri.username().is_empty()
            || uri.password().is_some()
            || host.parse::<std::net::IpAddr>().is_ok()
            || !host.contains('.')
            || host.ends_with(".localhost")
            || host.ends_with(".local")
            || !host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
            || item
                .get("purpose")
                .and_then(Value::as_str)
                .is_none_or(|v| v.trim().is_empty())
            || item
                .get("dataSent")
                .and_then(Value::as_str)
                .is_none_or(|v| v.trim().is_empty())
        {
            return Err(ApiError::new(
                "INVALID_PROJECT",
                "Declare exact public HTTPS origins with purpose and dataSent",
            ));
        }
        output.push(' ');
        output.push_str(origin);
    }
    Ok(output)
}
fn policy(project: &ProjectHandle) -> std::result::Result<(String, String), ApiError> {
    let policy =
        crate::runtime_policy::RuntimePolicy::parse(&project.manifest.runtime_requirements)?;
    Ok((
        policy.content_security_policy(),
        policy.permissions_policy(),
    ))
}
fn browser_limits() -> Value {
    let mut limits = crate::sql_limits(SqlCaller::Browser);
    limits["fileUploadMaxBytes"] = json!(FILE_MAX);
    limits
}
fn policy_fingerprint(requirements: &Value) -> String { platform_project_format::http_protocol::policy_fingerprint(requirements) }
async fn project_entry(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_project(state, id, None, headers).await
}
async fn project_static(
    State(state): State<Arc<HttpState>>,
    Path((id, tail)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if tail == "api" || tail.starts_with("api/") {
        return failure(
            ApiError::new("NOT_FOUND", "Unknown project API route"),
            Some(&id),
        );
    }
    serve_project(state, id, Some(tail), headers).await
}
async fn serve_project(
    state: Arc<HttpState>,
    id: String,
    path: Option<String>,
    headers: HeaderMap,
) -> Response {
    let project = match session(&state, &id, &headers, false) {
        Ok(project) => project,
        Err(error) => {
            if error.code == "INVALID_SESSION" && path.is_none() {
                return(StatusCode::UNAUTHORIZED,[(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; frame-ancestors 'none'")],"<!doctype html><html lang=zh-CN><meta charset=utf-8><title>需要从平台打开</title><h1>请从平台打开这个项目</h1><p>在 Local Projects Dev 中打开项目，或让 AI 获取新的浏览器入口。平台重启后需要重新连接。</p></html>").into_response();
            }
            return failure(error, Some(&id));
        }
    };
    let outcome: std::result::Result<Response, ApiError> = async {
        let lease = project.operation_gate.clone().read_owned().await;
        project.validate_epoch(Some(&project.document_epoch))?;
        let entry = project
            .manifest
            .entry
            .strip_prefix(&format!("{}/", project.manifest.output_dir))
            .ok_or_else(|| ApiError::new("INVALID_PROJECT", "Entry is outside output directory"))?;
        validate_entry_format(entry)?;
        let generation = builds::artifact_generation(&project)?;
        let (root_path, relative, base) =
            if let Some(versioned) = path.as_deref().and_then(|p| p.strip_prefix("_builds/")) {
                let (build_id, relative) = versioned
                    .split_once('/')
                    .ok_or_else(|| ApiError::new("INVALID_BUILD_ID", "Missing artifact path"))?;
                (
                    builds::version_root(&project, build_id)?,
                    relative.to_owned(),
                    Some(format!("/p/{id}/_builds/{build_id}/")),
                )
            } else if let Some(generation) = generation {
                (
                    generation.root,
                    path.unwrap_or(generation.entry_relative),
                    Some(format!("/p/{id}/_builds/{}/", generation.build_id)),
                )
            } else {
                (
                    project.root.join(&project.manifest.output_dir),
                    path.unwrap_or_else(|| entry.to_owned()),
                    None,
                )
            };
        let path = relative.as_str();
        let root = files::root(&root_path)?;
        let mut file = files::read(&root, path)?;
        let (mut csp, permissions) = policy(&project)?;
        let mut response = if is_html(path) {
            if file.metadata().map_err(files::error)?.len() > ENTRY_MAX {
                return Err(ApiError::new(
                    "REQUEST_TOO_LARGE",
                    "Project HTML exceeds the 8 MB test limit",
                ));
            }
            let mut bytes = Vec::new();
            file.by_ref()
                .take(ENTRY_MAX + 1)
                .read_to_end(&mut bytes)
                .map_err(files::error)?;
            if bytes.len() as u64 > ENTRY_MAX {
                return Err(ApiError::new(
                    "REQUEST_TOO_LARGE",
                    "Project HTML exceeds the 8 MB test limit",
                ));
            }
            let mut html = String::from_utf8(bytes)
                .map_err(|_| ApiError::new("INVALID_PROJECT", "Project HTML must be UTF-8"))?;
            let safe_epoch = project
                .document_epoch
                .replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let base = base.map(|prefix| {
                let parent = std::path::Path::new(path)
                    .parent()
                    .unwrap_or(std::path::Path::new(""));
                let suffix = parent.to_string_lossy();
                let mut url =
                    url::Url::parse(&state.info.base_url).expect("validated loopback URL");
                url.set_path(&if suffix.is_empty() {
                    prefix
                } else {
                    format!("{prefix}{suffix}/")
                });
                url.path().to_owned()
            });
            let nonce=secret();
            csp=csp.replace("script-src 'self'",&format!("script-src 'self' 'nonce-{nonce}'"));
            let meta = format!(
                "{}<meta name=\"platform-document-epoch\" content=\"{}\"><meta name=\"platform-runtime-policy\" content=\"{}\"><script nonce=\"{nonce}\">{}</script>",
                base.map(|base| format!("<base href=\"{base}\">"))
                    .unwrap_or_default(),
                safe_epoch,
                policy_fingerprint(&project.manifest.runtime_requirements),
                crate::runtime_notice::script()
            );
            let lower = html.to_ascii_lowercase();
            let position = lower
                .find("<head")
                .and_then(|start| lower[start..].find('>').map(|end| start + end + 1))
                .unwrap_or(0);
            html.insert_str(position, &meta);
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
        } else {
            stream_file(file, path, &headers, false, lease).await?
        };
        // Apply the same restrictions to every resource, including SVG/XHTML
        // opened as a top-level document. MIME, not extension spelling, decides
        // which documents also receive their immutable epoch.
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            csp.parse()
                .map_err(|_| ApiError::new("INVALID_PROJECT", "Invalid content policy"))?,
        );
        response
            .headers_mut()
            .insert("permissions-policy", permissions.parse().unwrap());
        Ok(response)
    }
    .await;
    outcome.unwrap_or_else(|e| failure(e, Some(&id)))
}
async fn stream_file(
    file: std::fs::File,
    path: &str,
    headers: &HeaderMap,
    attachment: bool,
    lease: tokio::sync::OwnedRwLockReadGuard<()>,
) -> std::result::Result<Response, ApiError> {
    let size = file.metadata().map_err(files::error)?.len();
    let mut start = 0;
    let mut end = size.saturating_sub(1);
    let mut partial = false;
    if let Some(range) = headers.get(header::RANGE) {
        let value = range
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("bytes="))
            .filter(|v| !v.contains(','));
        let parsed = value.and_then(|v| v.split_once('-')).and_then(|(a, b)| {
            if a.is_empty() {
                let suffix = b.parse::<u64>().ok()?.min(size);
                if suffix == 0 {
                    return None;
                }
                Some((size - suffix, size - 1))
            } else {
                let from = a.parse::<u64>().ok()?;
                let to = if b.is_empty() {
                    size.saturating_sub(1)
                } else {
                    b.parse::<u64>().ok()?.min(size.saturating_sub(1))
                };
                if from >= size || from > to {
                    return None;
                }
                Some((from, to))
            }
        });
        match parsed {
            Some((a, b)) => {
                start = a;
                end = b;
                partial = true;
            }
            None => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{size}"))],
                )
                    .into_response())
            }
        }
    }
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut file = tokio::fs::File::from_std(file);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(files::error)?;
    let reader = file.take(length);
    let body = Body::from_stream(stream::try_unfold(
        (reader, lease),
        |(mut reader, lease)| async move {
            let mut buffer = vec![0; 65536];
            let count = reader.read(&mut buffer).await?;
            if count == 0 {
                Ok::<_, std::io::Error>(None)
            } else {
                buffer.truncate(count);
                Ok(Some((buffer, (reader, lease))))
            }
        },
    ));
    let mut response = body.into_response();
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, files::mime(path).parse().unwrap());
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    if partial {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    if attachment {
        // An HTML/SVG attachment opened as a document gets no executable origin.
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            "sandbox; default-src 'none'; frame-ancestors 'none'"
                .parse()
                .unwrap(),
        );
        if !files::mime(path).starts_with("image/")
            && !files::mime(path).starts_with("audio/")
            && !files::mime(path).starts_with("video/")
        {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, "attachment".parse().unwrap());
        }
    }
    Ok(response)
}
async fn read_attachment(
    project: &ProjectHandle,
    path: &str,
    epoch: &str,
    headers: &HeaderMap,
) -> std::result::Result<Response, ApiError> {
    let lease = project.operation_gate.clone().read_owned().await;
    project.validate_epoch(Some(epoch))?;
    let root = files::root(&project.root.join(&project.manifest.attachments_dir))?;
    stream_file(files::read(&root, path)?, path, headers, true, lease).await
}
struct Temporary {
    root: std::fs::File,
    name: String,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        files::discard_temporary(&self.root, &self.name);
    }
}
async fn upload(
    state: &Arc<HttpState>,
    project: &ProjectHandle,
    path: &str,
    epoch: &str,
    overwrite: bool,
    request: Request,
    rid: &str,
) -> std::result::Result<files::FileInfo, ApiError> {
    files::parts(path)?;
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|v| v > FILE_MAX)
    {
        return Err(ApiError::new(
            "FILE_TOO_LARGE",
            "Attachment exceeds the 256 MB internal test limit",
        ));
    }
    let _global = state.uploads.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            "SERVICE_BUSY",
            "Two uploads are already active; try again after completion",
        )
    })?;
    let semaphore = state
        .project_uploads
        .lock()
        .unwrap()
        .entry(project.manifest.id.clone())
        .or_insert_with(|| Arc::new(Semaphore::new(1)))
        .clone();
    let _local = semaphore.try_acquire_owned().map_err(|_| {
        ApiError::new(
            "SERVICE_BUSY",
            "An upload is already active for this project",
        )
    })?;
    let _lease = project.operation_gate.read().await;
    project.validate_epoch(Some(epoch))?;
    let projectfd = files::root(&project.root)?;
    // Descriptor-based mkdir/open prevents staging links from escaping a project.
    let temporary_name = format!(".upload-{}", Uuid::new_v4().simple());
    let staging_path = format!(".platform/uploads/{temporary_name}");
    let staged = files::create_internal(&projectfd, &staging_path)?;
    let temporary = Temporary {
        root: files::root(&project.root.join(".platform/uploads"))?,
        name: temporary_name,
    };
    let attachments = files::root(&project.root.join(&project.manifest.attachments_dir))?;
    let mut file = tokio::fs::File::from_std(staged);
    let mut body = request.into_body().into_data_stream();
    let mut size = 0u64;
    let mut stop = state.stop.subscribe();
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        if state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Upload cancelled before publication",
            ));
        }
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(30));
        if remaining.is_zero() {
            return Err(ApiError::new(
                "UPLOAD_TIMEOUT",
                "Upload took too long; original file was preserved",
            ));
        }
        let chunk = tokio::select! {
            biased;
            _=stop.changed()=>return Err(ApiError::new("SERVICE_STOPPING","Upload cancelled before publication")),
            chunk=tokio::time::timeout(remaining,body.next())=>chunk.map_err(|_|ApiError::new("UPLOAD_TIMEOUT","Upload stalled; original file was preserved"))?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|_| {
            ApiError::new(
                "UPLOAD_INTERRUPTED",
                "Upload disconnected before publication",
            )
        })?;
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| ApiError::new("FILE_TOO_LARGE", "Attachment exceeds test limit"))?;
        if size > FILE_MAX {
            return Err(ApiError::new(
                "FILE_TOO_LARGE",
                "Attachment exceeds the 256 MB internal test limit",
            ));
        }
        file.write_all(&chunk).await.map_err(files::error)?;
    }
    file.sync_all().await.map_err(files::error)?;
    drop(file);
    project.validate_epoch(Some(epoch))?;
    if state.stopping.load(Ordering::SeqCst) {
        return Err(ApiError::new(
            "SERVICE_STOPPING",
            "Upload cancelled before publication",
        ));
    }
    if let Err(error) = files::publish(
        &temporary.root,
        &temporary.name,
        &attachments,
        path,
        overwrite,
    ) {
        if error
            .details
            .get("operationOutcome")
            .and_then(Value::as_str)
            == Some("committed")
        {
            changed(state, &project.manifest.id, rid, "file");
        }
        return Err(error);
    }
    changed(state, &project.manifest.id, rid, "file");
    let file = files::read(&attachments, path).map_err(|e| e.outcome("committed"))?;
    files::info(path, &file).map_err(|e| e.outcome("committed"))
}
async fn events(state: Arc<HttpState>, project: ProjectHandle, epoch: String) -> Response {
    let receiver = state.events.subscribe();
    let stop = state.stop.subscribe();
    let event_stream = stream::unfold(
        (receiver, stop, project, epoch),
        |(mut receiver, mut stop, project, epoch)| async move {
            loop {
                if *stop.borrow() {
                    return None;
                }
                tokio::select! {
                    _=stop.changed()=>return None,
                    value=receiver.recv()=>{
                        let (event, data)=match value{
                            Ok(change) if change.project_id==project.manifest.id=>{let data=if change.event=="project.state.changed"{let current=project.control.get();json!({"projectId":change.project_id,"mode":current.mode,"busy":project.control.frozen.load(Ordering::Acquire),"reloadRequired":project.document_epoch!=epoch||project.check().is_err_and(|e|matches!(e.code.as_str(),"PROJECT_RELOAD_REQUIRED"|"PROJECT_IDENTITY_CHANGED")),"workspaceMigrating":project.is_workspace_paused(),"trashed":project.check().is_err_and(|e|e.code=="PROJECT_TRASHED")})}else{json!({"projectId":change.project_id,"requestId":change.request_id,"taskId":if change.source=="build" {Some(&change.request_id)} else {None},"source":change.source})};(change.event,data)},
                            Ok(_)=>continue,
                            Err(broadcast::error::RecvError::Lagged(_))=>("data.changed".into(),json!({"projectId":project.manifest.id,"source":"resync"})),
                            Err(broadcast::error::RecvError::Closed)=>return None,
                        };
                        return Some((Ok::<_,Infallible>(Event::default().event(event).data(data.to_string())),(receiver,stop,project,epoch)));
                    }
                }
            }
        },
    );
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::{Client, Response as ClientResponse};
    fn make_client() -> Client {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }
    pub(super) async fn start() -> (tempfile::TempDir, RuntimePaths, RuntimeService, Client) {
        let (temp, paths) = RuntimePaths::for_test();
        let lock = paths.acquire_lock().unwrap();
        let service = RuntimeService::start(paths.clone(), lock).await.unwrap();
        (temp, paths, service, make_client())
    }
    pub(super) async fn create(service: &RuntimeService, name: &str) -> crate::ProjectInfo {
        let project = service
            .store()
            .create(CreateProjectInput {
                name: name.into(),
                description: None,
            })
            .await
            .unwrap();
        std::fs::create_dir(project.project_root.join("dist")).unwrap();
        std::fs::write(project.project_root.join("dist/index.html"),"<!doctype html><html><head><title>Test</title></head><body><script src=app.js></script></body></html>").unwrap();
        std::fs::write(
            project.project_root.join("dist/app.js"),
            "document.body.dataset.ready='true';",
        )
        .unwrap();
        project
    }
    pub(super) async fn connect(service: &RuntimeService, client: &Client, id: &str) -> String {
        let launch = service.launch(id).await.unwrap();
        let ticket = launch.launch_url.split("#ticket=").nth(1).unwrap();
        let response = client
            .post(format!("{}/_platform/session", service.info().base_url))
            .header("origin", &service.info().base_url)
            .json(&json!({"ticket":ticket}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        cookie.split(';').next().unwrap().into()
    }
    async fn sql(
        service: &RuntimeService,
        client: &Client,
        id: &str,
        body: Value,
    ) -> ClientResponse {
        client
            .post(format!(
                "{}/api/v1/projects/{id}/db/execute",
                service.info().base_url
            ))
            .bearer_auth(&service.shared.snapshot().token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn health_admin_host_and_cleanup() {
        let (_temp, paths, service, client) = start().await;
        let base = &service.info().base_url;
        let health = client
            .get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(health["instanceId"], service.info().instance_id);
        assert!(health.get("workspaceRoot").is_none());
        assert_eq!(
            client
                .get(format!("{base}/api/v1/runtime"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(format!("{base}/api/v1/runtime"))
                .bearer_auth("wrong")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(format!("{base}/api/v1/runtime"))
                .bearer_auth(&service.shared.snapshot().token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .get(format!("{base}/healthz"))
                .header("host", "evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        for header in ["origin", "sec-fetch-site"] {
            assert_eq!(
                client
                    .get(format!("{base}/api/v1/runtime"))
                    .bearer_auth(&service.shared.snapshot().token)
                    .header(header, base)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let url = base.clone();
        service.shutdown().await.unwrap();
        assert!(!paths.connection_path().exists());
        assert!(!paths.token_path().exists());
        assert!(paths.acquire_lock().is_ok());
        assert!(client.get(format!("{url}/healthz")).send().await.is_err());
    }
    #[tokio::test]
    async fn project_sessions_epochs_origins_and_database_isolation() {
        let (_temp, _paths, service, client) = start().await;
        let a = create(&service, "A").await;
        let b = create(&service, "B").await;
        for (project, value) in [(&a, "only-a"), (&b, "only-b")] {
            assert!(sql(
                &service,
                &client,
                &project.id,
                json!({"sql":"CREATE TABLE records(value TEXT NOT NULL)"})
            )
            .await
            .status()
            .is_success());
            assert!(sql(
                &service,
                &client,
                &project.id,
                json!({"sql":"INSERT INTO records VALUES (?)","params":[value]})
            )
            .await
            .status()
            .is_success());
        }
        let cookie = connect(&service, &client, &a.id).await;
        let base = &service.info().base_url;
        let body = json!({"sql":"SELECT value FROM records"});
        let request = || {
            client
                .post(format!("{base}/p/{}/api/v1/db/execute", a.id))
                .header("cookie", &cookie)
                .header("origin", base)
                .header("x-platform-document-epoch", &a.document_epoch)
                .json(&body)
        };
        let result = request()
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(result["data"]["rows"][0]["value"], "only-a");
        assert_eq!(
            request()
                .header("origin", "https://evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let old = request()
            .header("x-platform-document-epoch", "old")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(old["error"]["code"], "PROJECT_RELOAD_REQUIRED");
        let wrong = client
            .post(format!("{base}/p/{}/api/v1/db/execute", b.id))
            .header("cookie", &cookie)
            .header("origin", base)
            .header("x-platform-document-epoch", &b.document_epoch)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let no_origin = client
            .post(format!("{base}/p/{}/api/v1/db/execute", a.id))
            .header("cookie", &cookie)
            .header("x-platform-document-epoch", &a.document_epoch)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);
        let context = client
            .get(format!("{base}/p/{}/api/v1/context", a.id))
            .header("cookie", &cookie)
            .header("x-platform-document-epoch", &a.document_epoch)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(context["data"]["projectId"], a.id);
        assert_eq!(context["data"]["limits"]["limitsStatus"], "internalTest");
        assert!(context["data"].get("documentEpoch").is_none());
        assert!(context["data"].get("workspaceRoot").is_none());
        let html = client
            .get(format!("{base}/p/{}/", a.id))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert!(html.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'"));
        assert!(html.text().await.unwrap().contains(&format!(
            "name=\"platform-document-epoch\" content=\"{}\"",
            a.document_epoch
        )));
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn tickets_are_single_use_and_restart_invalidates_all_browser_identity() {
        let (_temp, paths, service, client) = start().await;
        let project = create(&service, "A").await;
        let launch = service.launch(&project.id).await.unwrap();
        let ticket = launch.launch_url.split("#ticket=").nth(1).unwrap();
        let base = service.info().base_url.clone();
        let exchange = || {
            client
                .post(format!("{base}/_platform/session"))
                .header("origin", &base)
                .json(&json!({"ticket":ticket}))
        };
        let first = exchange().send().await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let cookie = first.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(
            exchange().send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let expired = service.launch(&project.id).await.unwrap();
        let expired_ticket = expired.launch_url.split("#ticket=").nth(1).unwrap();
        service
            .shared
            .snapshot()
            .tickets
            .lock()
            .unwrap()
            .get_mut(expired_ticket)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            client
                .post(format!("{base}/_platform/session"))
                .header("origin", &base)
                .json(&json!({"ticket":expired_ticket}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let unused = service.launch(&project.id).await.unwrap();
        let unused_ticket = unused
            .launch_url
            .split("#ticket=")
            .nth(1)
            .unwrap()
            .to_string();
        let instance = service.info().instance_id.clone();
        service.shutdown().await.unwrap();
        let lock = paths.acquire_lock().unwrap();
        let restarted = RuntimeService::start(paths.clone(), lock).await.unwrap();
        assert_ne!(restarted.info().instance_id, instance);
        let base = &restarted.info().base_url;
        let client = make_client();
        assert_eq!(
            client
                .get(format!("{base}/p/{}/api/v1/context", project.id))
                .header("cookie", cookie)
                .header("x-platform-document-epoch", project.document_epoch)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(format!("{base}/_platform/session"))
                .header("origin", base)
                .json(&json!({"ticket":unused_ticket}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        restarted.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn attachment_publication_conflicts_ranges_and_static_boundaries() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Files").await;
        let cookie = connect(&service, &client, &project.id).await;
        let base = &service.info().base_url;
        let file_url = format!("{base}/p/{}/api/v1/file?path=notes/example.txt", project.id);
        let put = || {
            client
                .put(&file_url)
                .header("cookie", &cookie)
                .header("origin", base)
                .header("x-platform-document-epoch", &project.document_epoch)
        };
        assert_eq!(
            put().body("original").send().await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            put().body("replace").send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
        let read = || {
            client
                .get(&file_url)
                .header("cookie", &cookie)
                .header("x-platform-document-epoch", &project.document_epoch)
        };
        assert_eq!(
            read().send().await.unwrap().text().await.unwrap(),
            "original"
        );
        let range = read().header("range", "bytes=1-3").send().await.unwrap();
        assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(range.text().await.unwrap(), "rig");
        assert_eq!(
            put()
                .header("content-length", FILE_MAX + 1)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            client
                .put(format!("{file_url}&overwrite=true"))
                .header("cookie", &cookie)
                .header("origin", base)
                .header("x-platform-document-epoch", &project.document_epoch)
                .body("new")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(read().send().await.unwrap().text().await.unwrap(), "new");
        for tail in [
            "project.json",
            "data/main.sqlite",
            "api/no-such-route",
            "%2e%2e/project.json",
        ] {
            let response = client
                .get(format!("{base}/p/{}/{tail}", project.id))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success(), "{tail}");
        }
        std::os::unix::fs::symlink(
            project.project_root.join("data/main.sqlite"),
            project.project_root.join("dist/leak"),
        )
        .unwrap();
        assert!(!client
            .get(format!("{base}/p/{}/leak", project.id))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        assert!(!client
            .put(format!(
                "{base}/p/{}/api/v1/file?path=../escape",
                project.id
            ))
            .header("cookie", &cookie)
            .header("origin", base)
            .header("x-platform-document-epoch", &project.document_epoch)
            .body("bad")
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        let listing = client
            .get(format!("{base}/p/{}/api/v1/files", project.id))
            .header("cookie", &cookie)
            .header("x-platform-document-epoch", &project.document_epoch)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(listing["data"].as_array().unwrap().len(), 1);
        assert_eq!(listing["data"][0]["size"], 3);
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn aborted_upload_preserves_existing_file_and_cleans_staging() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Abort").await;
        let cookie = connect(&service, &client, &project.id).await;
        let attachment = project.project_root.join("data/attachments/original.txt");
        std::fs::write(&attachment, b"original").unwrap();
        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(vec![b'x'; 1024]),
            Err(std::io::Error::other("simulate disconnect")),
        ]);
        let response = client
            .put(format!(
                "{}/p/{}/api/v1/file?path=original.txt&overwrite=true",
                service.info().base_url,
                project.id
            ))
            .header("cookie", cookie)
            .header("origin", &service.info().base_url)
            .header("x-platform-document-epoch", project.document_epoch)
            .body(reqwest::Body::wrap_stream(chunks))
            .send()
            .await;
        assert!(response.is_err());
        service.shutdown().await.unwrap();
        assert_eq!(std::fs::read(attachment).unwrap(), b"original");
        let staging = project.project_root.join(".platform/uploads");
        assert!(!staging.exists() || std::fs::read_dir(staging).unwrap().count() == 0);
    }
    #[tokio::test]
    async fn sse_notifies_only_committed_writes_and_refresh() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Events").await;
        let cookie = connect(&service, &client, &project.id).await;
        sql(
            &service,
            &client,
            &project.id,
            json!({"sql":"CREATE TABLE notes(value TEXT UNIQUE)"}),
        )
        .await;
        let response = client
            .get(format!(
                "{}/p/{}/api/v1/events?documentEpoch={}",
                service.info().base_url,
                project.id,
                project.document_epoch
            ))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut events = response.bytes_stream();
        sql(
            &service,
            &client,
            &project.id,
            json!({"sql":"SELECT * FROM notes"}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.next())
                .await
                .is_err()
        );
        sql(
            &service,
            &client,
            &project.id,
            json!({"sql":"INSERT INTO notes VALUES ('one')"}),
        )
        .await;
        let event = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&event).contains("event: data.changed"));
        let failed=client.post(format!("{}/api/v1/projects/{}/db/transaction",service.info().base_url,project.id)).bearer_auth(&service.shared.snapshot().token).json(&json!({"statements":[{"sql":"INSERT INTO notes VALUES ('two')"},{"sql":"INSERT INTO notes VALUES ('one')"}]})).send().await.unwrap();
        assert!(!failed.status().is_success());
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.next())
                .await
                .is_err()
        );
        client
            .post(format!(
                "{}/api/v1/projects/{}/refresh",
                service.info().base_url,
                project.id
            ))
            .bearer_auth(&service.shared.snapshot().token)
            .send()
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&event).contains("external"));
        drop(events);
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn unbuilt_project_is_explicit_and_shutdown_cancels_partial_upload() {
        let (_temp, _paths, service, client) = start().await;
        let empty = service
            .store()
            .create(CreateProjectInput {
                name: "Unbuilt".into(),
                description: None,
            })
            .await
            .unwrap();
        assert_eq!(
            service.launch(&empty.id).await.unwrap_err().code,
            "PROJECT_NOT_BUILT"
        );
        let project = create(&service, "Partial upload").await;
        let cookie = connect(&service, &client, &project.id).await;
        let attachment = project.project_root.join("data/attachments/original.txt");
        std::fs::write(&attachment, b"original").unwrap();
        let info = service.info();
        let host = info.base_url.strip_prefix("http://").unwrap();
        let mut socket = tokio::net::TcpStream::connect(host).await.unwrap();
        let headers = format!("PUT /p/{}/api/v1/file?path=original.txt&overwrite=true HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nCookie: {}\r\nX-Platform-Document-Epoch: {}\r\nContent-Length: 1000000\r\n\r\npartial", project.id,host,service.info().base_url,cookie,project.document_epoch);
        socket.write_all(headers.as_bytes()).await.unwrap();
        let staging = project.project_root.join(".platform/uploads");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if staging.exists() && std::fs::read_dir(&staging).unwrap().count() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), service.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(attachment).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn html_mime_controls_epoch_and_every_static_resource_receives_policy() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Alternate HTML").await;
        let manifest_path = project.project_root.join("project.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        for entry in ["index.HTML", "index.htm"] {
            std::fs::write(project.project_root.join("dist").join(entry), "<!doctype html><html><head><title>Alternate</title></head><body>HTML</body></html>").unwrap();
            manifest["entry"] = json!(format!("dist/{entry}"));
            std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let cookie = connect(&service, &client, &project.id).await;
            for tail in ["", entry] {
                let response = client
                    .get(format!(
                        "{}/p/{}/{tail}",
                        service.info().base_url,
                        project.id
                    ))
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert!(response.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("text/html"));
                assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("script-src 'self'"));
                assert!(response.headers()["permissions-policy"]
                    .to_str()
                    .unwrap()
                    .contains("microphone=()"));
                assert!(response.text().await.unwrap().contains(&format!(
                    "name=\"platform-document-epoch\" content=\"{}\"",
                    project.document_epoch
                )));
            }
        }
        let cookie = connect(&service, &client, &project.id).await;
        std::fs::write(
            project.project_root.join("dist/image.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert('inline')</script></svg>",
        )
        .unwrap();
        let response = client
            .get(format!(
                "{}/p/{}/image.svg",
                service.info().base_url,
                project.id
            ))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self'"));
        assert!(response.headers().contains_key("permissions-policy"));
        manifest["entry"] = json!("dist/image.svg");
        std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(
            service.launch(&project.id).await.unwrap_err().code,
            "INVALID_PROJECT"
        );
        let response = client
            .get(format!("{}/p/{}/", service.info().base_url, project.id))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
        service.shutdown().await.unwrap();
    }

    #[test]
    fn csp_origins_reject_browser_normalized_loopback_and_paths() {
        for origin in [
            "http://api.example.com",
            "https://127.1",
            "https://0x7f000001",
            "https://localhost",
            "https://api.example.com/path",
            "https://*.example.com",
            "https://user@api.example.com",
        ] {
            assert!(network_origins(&json!({"network":{"connect":[{"origin":origin,"purpose":"test","dataSent":"test"}]}}), "connect").is_err(), "{origin}");
        }
        assert_eq!(network_origins(&json!({"network":{"connect":[{"origin":"https://api.example.com","purpose":"test","dataSent":"text"}]}}), "connect").unwrap(), " https://api.example.com");
    }

    #[tokio::test]
    async fn occupied_preferred_port_gets_a_different_loopback_port() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let listener = bind_loopback(Some(occupied.local_addr().unwrap().port()))
            .await
            .unwrap();
        assert_ne!(
            listener.local_addr().unwrap().port(),
            occupied.local_addr().unwrap().port()
        );
        assert_eq!(listener.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
    }
}

#[cfg(test)]
#[path = "service_m2_tests.rs"]
mod m2_tests;

#[cfg(test)]
#[path = "service_m3_tests.rs"]
mod m3_tests;

#[cfg(test)]
#[path = "service_m4_tests.rs"]
mod m4_tests;

#[cfg(test)]
#[path = "service_m5_tests.rs"]
mod m5_tests;

#[cfg(test)]
#[path = "service_history_cleanup_tests.rs"]
mod history_cleanup_tests;
