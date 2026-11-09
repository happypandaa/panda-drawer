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
    routing::{any, get},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
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
    io::AsyncWriteExt,
    net::TcpListener,
    sync::{broadcast, oneshot, watch, Notify, Semaphore},
    task::JoinHandle,
};
use tower::ServiceExt;
use uuid::Uuid;

mod unified;
#[path = "service/web_runtime.rs"]
mod web_runtime;
pub use unified::{ManagementRequest, NativeAppReload};

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
    project: ProjectHandle,
    project_id: String,
    epoch: String,
    expires: Instant,
}
struct Session {
    project: ProjectHandle,
    project_id: String,
    document_epoch: String,
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
    io_lifetime: crate::io_lifetime::IoLifetime,
    shared: Weak<RuntimeSwitch>,
    lifecycle: Arc<crate::web_project_deletion::WebProjectDeletionManager>,
    storage: Arc<crate::storage::StorageManager>,
    info: BootstrapInfo,
    token: String,
    expected_host: String,
    store: Arc<ProjectStore>,
    builds: Arc<BuildManager>,
    git_environment: crate::git_environment::GitEnvironmentManager,
    git_remote: crate::git_remote::GitRemoteChecker,
    node_git_import: Arc<crate::node_git_import::NodeGitImportManager>,
    maintenance: Arc<crate::maintenance::MaintenanceManager>,
    resources: Option<Arc<ProjectResources>>,
    tickets: Mutex<HashMap<String, Ticket>>,
    sessions: Mutex<HashMap<String, Session>>,
    events: broadcast::Sender<Change>,
    uploads: Arc<Semaphore>,
    project_uploads: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    web_runtime: Option<Arc<web_runtime::WebRuntimeManager>>,
    backend: Option<Arc<crate::node_app_proxy::BackendForwarder>>,
    stopping: AtomicBool,
    active: AtomicUsize,
    drained: Notify,
    stop: watch::Sender<bool>,
}
impl HttpState {
    fn web_child(parent: &Arc<Self>, origin: &str) -> Self {
        let mut info = parent.info.clone();
        info.base_url = origin.into();
        Self {
            paths: parent.paths.clone(),
            io_lifetime: crate::io_lifetime::IoLifetime::default(),
            shared: parent.shared.clone(),
            lifecycle: parent.lifecycle.clone(),
            storage: parent.storage.clone(),
            info,
            token: secret(),
            expected_host: origin.trim_start_matches("http://").into(),
            store: parent.store.clone(),
            builds: parent.builds.clone(),
            git_environment: parent.git_environment.clone(),
            git_remote: parent.git_remote.clone(),
            node_git_import: parent.node_git_import.clone(),
            maintenance: parent.maintenance.clone(),
            resources: parent.resources.clone(),
            tickets: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            events: parent.events.clone(),
            uploads: parent.uploads.clone(),
            project_uploads: parent.project_uploads.clone(),
            web_runtime: None,
            backend: None,
            stopping: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
            stop: watch::channel(false).0,
        }
    }
}
struct RuntimeSwitch {
    native_startup: StdRwLock<Option<ApiError>>,
    project_cleanup: Arc<crate::project_cleanup::ProjectCleanupCoordinator>,
    processes: crate::owned_process::OwnedProcessRegistry,
    shutdown_issues: Mutex<Vec<crate::shutdown::ShutdownIssue>>,
    io_lifetime: crate::io_lifetime::IoLifetime,
    current: StdRwLock<Option<Arc<HttpState>>>,
    router: StdRwLock<Router>,
    migration_runner: Mutex<Option<JoinHandle<()>>>,
    migration_previous: Mutex<Vec<JoinHandle<()>>>,
    migration_submission: Mutex<()>,
    native_migration: StdRwLock<Option<NativeWorkspaceMigration>>,
    migration_retained_state: Mutex<Option<Arc<HttpState>>>,
    #[cfg(test)]
    migration_reopen_failure: Mutex<Option<ApiError>>,
    #[cfg(test)]
    migration_registration_pause:
        Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    closing: AtomicBool,
    migration_active: AtomicBool,
    /// Requests from app pages to open the user's AI tool; the desktop relays
    /// them to the manager window. Nothing here launches a program by itself.
    ai_open_requests: broadcast::Sender<AiOpenRequest>,
    ai_open_times: Mutex<HashMap<String, Instant>>,
    /// The desktop's app registry, so the command line can check and add folders.
    app_reload_hook: StdRwLock<Option<NativeAppReload>>,
    reload_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    management_requests: broadcast::Sender<ManagementRequest>,
    management_pending: Mutex<HashMap<String, unified::PendingConfirmation>>,
}
/// An app page asked to hand a short text to the user's own AI tool.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiOpenRequest {
    pub project_id: String,
    pub app_name: String,
    pub text: String,
}
const AI_OPEN_MAX_CHARS: usize = 2000;
const AI_OPEN_INTERVAL: Duration = Duration::from_secs(3);
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
/// Keeps the resolved project in this workspace while a native editor is opened.
/// This lease ends after dispatch; it does not track subsequent editing in the AI.
pub struct WebWindowLease {
    _token: web_runtime::WindowToken,
}

pub struct EditorHandoffLease {
    pub project_root: PathBuf,
    project_id: String,
    _task: crate::projects::ProjectTaskLease,
    active: Active,
}
impl EditorHandoffLease {
    pub fn validate(&self) -> std::result::Result<(), ApiError> {
        let state = &self.active.0;
        if state.stopping.load(Ordering::SeqCst)
            || state
                .shared
                .upgrade()
                .is_none_or(|shared| shared.closing.load(Ordering::Acquire))
        {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        state.store.check_workspace_available()?;
        state.store.check_development_available(&self.project_id)?;
        let project = state.store.project(&self.project_id)?;
        project.validate_epoch(None)?;
        if project.root != self.project_root {
            return Err(ApiError::new(
                "HANDOFF_CHANGED",
                "项目位置已变化，请重新准备接手说明",
            ));
        }
        Ok(())
    }
}
impl RuntimeHandle {
    pub fn check_native_ready(&self) -> std::result::Result<(), ApiError> {
        self.shared
            .native_startup
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .map_or(Ok(()), Err)
    }
    pub fn set_workspace_migration_hook(&self, hook: NativeWorkspaceMigration) {
        *self.shared.native_migration.write().unwrap() = Some(hook);
    }
    pub fn complete_native_startup(&self) {
        *self
            .shared
            .native_startup
            .write()
            .unwrap_or_else(|p| p.into_inner()) = None;
        let _ = workspace_migration::submit_selected_cleanup(self.shared.clone());
    }
    pub fn fail_native_startup(&self, error: ApiError) {
        *self
            .shared
            .native_startup
            .write()
            .unwrap_or_else(|p| p.into_inner()) = Some(error);
    }
    pub(crate) fn source_registration_scope(
        &self,
    ) -> std::result::Result<tokio::sync::OwnedRwLockReadGuard<()>, ApiError> {
        self.shared.project_cleanup.source_registration_scope()
    }
    pub fn webview_stores(&self) -> crate::webview_storage::StoreLedger {
        self.shared.project_cleanup.stores()
    }
    pub fn native_project_io(
        &self,
        id: &str,
        epoch: &str,
        component: &str,
    ) -> std::result::Result<crate::native_project_io::NativeProjectIoGuard, ApiError> {
        let state = self.current();
        self.check_native_ready()?;
        crate::native_project_io::NativeProjectIoGuard::try_begin(
            &state.store,
            state.paths.data_root(),
            id,
            epoch,
            component,
        )
    }
    pub fn publish_native_file(
        &self,
        guard: &crate::native_project_io::NativeProjectIoGuard,
        source: std::fs::File,
        destination: &std::path::Path,
    ) -> std::result::Result<(), ApiError> {
        let state = self.current();
        guard.validate()?;
        let project = state.store.project(guard.project_id())?;
        crate::native_project_files::publish_file(
            state.paths.data_root(),
            state.paths.workspace_root(),
            &project,
            source,
            destination,
            |path| {
                guard.validate()?;
                self.validate_download_destination(path)
            },
        )
    }
    pub fn window_size_store(&self) -> Arc<crate::app_windows::WindowSizeStore> {
        self.shared.project_cleanup.windows()
    }
    pub fn set_project_cleanup_hook(&self, hook: crate::project_cleanup::NativeProjectCleanup) {
        self.shared.project_cleanup.set_native(hook);
    }
    pub(crate) fn project_cleanup(&self) -> Arc<crate::project_cleanup::ProjectCleanupCoordinator> {
        self.shared.project_cleanup.clone()
    }
    pub fn io_lifetime(&self) -> crate::io_lifetime::IoLifetime {
        self.shared.io_lifetime.clone()
    }
    fn current(&self) -> Arc<HttpState> {
        self.pinned
            .clone()
            .unwrap_or_else(|| self.shared.snapshot())
    }
    pub(crate) fn application_resources(&self) -> Option<Arc<ProjectResources>> {
        self.current().resources.clone()
    }
    pub fn store(&self) -> Arc<ProjectStore> {
        self.current().store.clone()
    }
    pub(crate) fn node_task_manager(&self) -> Arc<BuildManager> {
        self.current().builds.clone()
    }
    pub async fn launch(&self, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
        launch(&self.current(), id).await
    }
    pub async fn launch_window(
        &self,
        id: &str,
    ) -> std::result::Result<(LaunchInfo, Option<WebWindowLease>), ApiError> {
        launch_inner(&self.current(), id, true).await
    }
    pub fn web_status(&self, id: &str) -> Value {
        self.current()
            .web_runtime
            .as_ref()
            .map(|manager| manager.status(id))
            .unwrap_or_else(|| json!({"state":"closed","resourceClosed":true}))
    }
    pub async fn close_web(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        match &state.web_runtime {
            Some(manager) => manager.close(id).await,
            None => Ok(json!({"state":"closed","resourceClosed":true})),
        }
    }
    pub async fn web_configuration(&self, id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().store.web_configuration(id)?))
    }
    pub async fn save_web_configuration(
        &self,
        id: &str,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().store.save_web_configuration(
            id,
            serde_json::from_value(input)?
        )?))
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
                    info.build_info = json!({"declared":info.manifest.as_ref().is_some_and(|manifest|manifest.development.is_some()),"unavailable":true});
                    info.issues.push(error.message);
                }
            }
        }
        let mut value = json!(info);
        value["webRuntime"] = self.web_status(id);
        value["maintenance"] = self.current().maintenance.status(id)?;
        if value["maintenance"]["mode"] == "maintenance" {
            value["state"] = json!(if value["maintenance"]["recoveryRequired"] == true {
                "recoveryRequired"
            } else {
                "maintenance"
            });
        }
        let state = self.current();
        // Layout diagnostics use the registered, validated handle, never an
        // untrusted manifest retained by the error summary. They cannot change
        // runtime readiness or initialize a repository.
        value["objectLayout"] = state
            .store
            .project(id)
            .map(|project| {
                let mut layout =
                    crate::object_layout::project_layout(&project.root, &project.manifest);
                layout["controlPath"] = json!(project.private_dir);
                layout
            })
            .unwrap_or(Value::Null);
        value["storage"] = state.storage.info(id).await.unwrap_or_else(
            |error| json!({"currentProject":null,"issues":[error],"state":"unavailable"}),
        );
        Ok(value)
    }
    /// Lets the command line check and add local folders through the desktop's registry.
    pub async fn check_directory(&self, path: String) -> std::result::Result<Value, ApiError> {
        if !PathBuf::from(&path).is_absolute() {
            return Err(ApiError::new(
                "INVALID_INPUT",
                "请提供应用文件夹的完整路径。",
            ));
        }
        self.inspect_app_directory(path).await
    }
    pub async fn add_directory(&self, path: String) -> std::result::Result<Value, ApiError> {
        let inspection = self.check_directory(path.clone()).await?;
        if inspection["existingApp"].is_object() {
            return Ok(
                json!({"added":false,"alreadyAdded":true,"existingApp":inspection["existingApp"]}),
            );
        }
        if inspection["state"] != "ready" {
            return Err(ApiError::new(
                "LOCAL_DIRECTORY_NOT_READY",
                "这个文件夹还不能添加，请按检查结果修改后再试。",
            )
            .detail("inspection", inspection));
        }
        let revision = inspection["expectedRevision"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        Ok(json!({"added":true,"app":self.register_directory(path,revision).await?}))
    }
    /// Desktop hosts relay these to the manager window, which opens the AI tool.
    pub fn subscribe_ai_open_requests(&self) -> broadcast::Receiver<AiOpenRequest> {
        self.shared.ai_open_requests.subscribe()
    }
    pub fn platform_info(&self) -> Value {
        crate::platform_info::with_resources(self.current().resources.as_deref())
    }
    /// The installed platform command line, when installation resources are present.
    pub(crate) fn platform_cli_path(&self) -> Option<PathBuf> {
        self.current()
            .resources
            .as_ref()
            .map(|resources| resources.cli_path.clone())
    }
    /// The installed platform skill, when installation resources are present.
    pub(crate) fn platform_skill_path(&self) -> Option<PathBuf> {
        self.current()
            .resources
            .as_ref()
            .map(|resources| resources.skill_path())
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
        crate::installation_scope::require_destination(
            state
                .resources
                .as_ref()
                .and_then(|resources| resources.installation_scope())
                .as_deref(),
            destination,
        )?;
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
        crate::app_source_ownership::require_delivery_location_unclaimed(
            state.paths.data_root(),
            &parent,
        )
        .map_err(|_| {
            ApiError::new(
                "UNSAFE_DOWNLOAD_DESTINATION",
                "保存位置属于项目独占源码，请选择项目范围以外的位置。",
            )
        })?;
        crate::web_project_ownership::WebProjectOwnership::new(
            state.paths.data_root().into(),
            state.paths.workspace_root().into(),
        )
        .require_global_tool_unowned(&actual)
        .map_err(|_| {
            ApiError::new(
                "UNSAFE_DOWNLOAD_DESTINATION",
                "保存目标属于项目独占材料，请选择其他位置。",
            )
        })?;
        if let Ok(metadata) = std::fs::symlink_metadata(&actual) {
            if !metadata.is_file()
                || crate::project_file_io::observed_link_count(&actual, &metadata)? != 1
            {
                return Err(ApiError::new(
                    "INVALID_PATH",
                    "Download target must be an independent regular file",
                ));
            }
        }
        Ok(())
    }
    pub fn device_permission_declared(&self, id: &str, device: &str) -> bool {
        self.current()
            .web_runtime
            .as_ref()
            .is_some_and(|manager| manager.device_declared(id, device))
    }

    pub async fn project_location(
        &self,
        id: &str,
        target: &str,
    ) -> std::result::Result<PathBuf, ApiError> {
        let state = self.current();
        let project = state.store.project(id)?;
        let root = project.root;
        let manifest = project.manifest;
        match target {
            "root" => Ok(root),
            "readme" => {
                let readme = manifest
                    .documentation
                    .as_ref()
                    .and_then(|docs| docs.readme.as_deref())
                    .ok_or_else(|| {
                        ApiError::new("DOCUMENTATION_NOT_DECLARED", "这个项目没有声明使用说明。")
                    })?;
                crate::projects::checked_relative(&root, readme, false)
            }
            _ => Err(ApiError::new(
                "INVALID_INPUT",
                "Only project root and README locations are supported",
            )),
        }
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
        let root = files::root(&project.ensure_attachments_dir()?)?;
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
    pub async fn maintenance_status(&self, id: &str) -> std::result::Result<Value, ApiError> {
        self.current().maintenance.status(id)
    }
    pub async fn task(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if let Some(task) = crate::workspace::task(&state.paths, id)? {
            return Ok(json!(task));
        }
        let task = state.builds.task(id)?;
        if task.task_type == "delete" {
            if let Some(project) = task.project_id.as_deref() {
                return Ok(json!(state
                    .lifecycle
                    .task_for_project(id, project, "delete")?));
            }
        }
        Ok(json!(task))
    }
    pub async fn task_for_project(
        &self,
        id: &str,
        project_id: &str,
        task_type: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if let Some(task) = crate::workspace::task(&state.paths, id)? {
            if task.project_id.as_deref() != Some(project_id) || task.task_type != task_type {
                return Err(ApiError::new(
                    "TASK_IDENTITY_CHANGED",
                    "Task does not belong to the requested project and type.",
                ));
            }
            return Ok(json!(task));
        }
        Ok(json!(state
            .lifecycle
            .task_for_project(id, project_id, task_type)?))
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
    pub async fn git_environment(&self) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().git_environment.doctor().await))
    }
    pub async fn check_git_remote(
        &self,
        input: crate::git_remote::GitRemoteCheckInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        // Native commands also participate in shutdown drain. HTTP requests
        // retain their outer guard while this operation holds its own guard.
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        state.store.check_workspace_available()?;
        Ok(json!(
            state
                .git_remote
                .check(&state.git_environment, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn set_git_path(
        &self,
        git_path: Option<String>,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.store.check_workspace_available()?;
        Ok(json!(
            state
                .git_environment
                .configure_git(git_path.map(PathBuf::from))
                .await?
        ))
    }
    pub async fn node_git_import_start(
        &self,
        input: crate::node_git_import::StartNodeGitImportInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .node_git_import
                .start(state.store.clone(), input, state.stop.subscribe())
                .await?
        ))
    }
    pub fn node_git_import_status(&self, import_id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().node_git_import.status(import_id)?))
    }
    pub fn node_git_import_confirm_placement(
        &self,
        import_id: &str,
        destination: Option<crate::source_acquisition::SelectedDirectory>,
    ) -> std::result::Result<Value, ApiError> {
        Ok(json!(self
            .current()
            .node_git_import
            .confirm_placement(import_id, destination)?))
    }
    pub fn node_git_import_confirm_build(
        &self,
        import_id: &str,
        expected_revision: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.store.check_workspace_available()?;
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "平台正在停止，无法开始构建。",
            ));
        }
        Ok(json!(state
            .node_git_import
            .confirm_build(import_id, expected_revision)?))
    }
    pub fn node_git_import_cancel(&self, task_id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().node_git_import.cancel(task_id)?))
    }
    pub async fn node_git_import_retry_cleanup(
        &self,
        task_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        Ok(json!(
            state
                .node_git_import
                .retry_cleanup(state.store.clone(), task_id)
                .await?
        ))
    }
    pub async fn recover_source_imports(&self) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        let attempts = state.node_git_import.recover(state.store.clone()).await?;
        Ok(json!(attempts.into_iter().map(|attempt| {
            let committed = attempt.commit_state == crate::node_git_import::CommitState::Committed;
            let blocked = attempt.cleanup_pending || attempt.commit_state == crate::node_git_import::CommitState::Unknown;
            json!({"taskId":attempt.task.task_id,"appId":attempt.app_id,
                "status":if blocked {"blocked"} else if committed {"installed"} else {"cleaned"},
                "message":attempt.task.error.map(|error|error.message).unwrap_or_else(||if committed {"应用已添加，临时文件已清理。".into()} else {"本次添加已中断，临时材料已清理。".into()}),
                "cleanupPending":attempt.cleanup_pending,"remainingPaths":attempt.remaining_paths})
        }).collect::<Vec<_>>()))
    }
    pub fn forget_deleted_node_import(&self, app_id: &str) {
        self.current()
            .node_git_import
            .forget_deleted_project(app_id);
    }
    pub async fn handoff(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.store.check_development_available(id)?;
        let project = state.store.project(id)?;
        let resources = state.resources.as_ref().ok_or_else(|| {
            ApiError::new(
                "INSTALLATION_RESOURCES_MISSING",
                "This service has no installed project template or CLI resources",
            )
        })?;
        let mut handoff =
            resources.handoff(&project.root, id, &project.private_dir.join("handoff"));
        handoff["instanceId"] = json!(state.info.instance_id);
        Ok(handoff)
    }
    /// The first handoff for a newly created app asks the AI to plan with the user.
    pub async fn create_handoff(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.store.check_development_available(id)?;
        let project = state.store.project(id)?;
        let resources = state.resources.as_ref().ok_or_else(|| {
            ApiError::new(
                "INSTALLATION_RESOURCES_MISSING",
                "This service has no installed project template or CLI resources",
            )
        })?;
        let mut handoff =
            resources.create_handoff(&project.root, id, &project.private_dir.join("handoff"));
        handoff["instanceId"] = json!(state.info.instance_id);
        Ok(handoff)
    }
    pub fn editor_handoff_lease(
        &self,
        id: &str,
        expected_project_root: &std::path::Path,
        expected_instance_id: &str,
    ) -> std::result::Result<EditorHandoffLease, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(state.clone());
        if state.info.instance_id != expected_instance_id {
            return Err(ApiError::new(
                "HANDOFF_CHANGED",
                "平台已重启或工作区已变化，请重新准备接手说明",
            ));
        }
        let task = state
            .store
            .reserve_transient_operation(id, "editor-handoff")?;
        let project = state.store.project(id)?;
        if project.root != expected_project_root {
            return Err(ApiError::new(
                "HANDOFF_CHANGED",
                "项目位置已变化，请重新准备接手说明",
            ));
        }
        let lease = EditorHandoffLease {
            project_root: project.root,
            project_id: id.into(),
            _task: task,
            active,
        };
        lease.validate()?;
        Ok(lease)
    }
    pub async fn project_delete_preview(&self, id: &str) -> std::result::Result<Value, ApiError> {
        web_delete_preview(&self.current(), id)
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
    pub async fn workspace_info(&self) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        let mut value = crate::workspace::info(&state.paths, &state.store)?;
        value["migrationCleanupTask"] = json!(crate::workspace::cleanup_task_id(&state.paths)?
            .and_then(|id| state.builds.task(&id).ok()));
        Ok(value)
    }
    pub async fn workspace_migration_cleanup(
        &self,
        migration_id: String,
    ) -> std::result::Result<Value, ApiError> {
        self.check_native_ready()?;
        workspace_migration::submit_cleanup(self.shared.clone(), self.current(), &migration_id)
            .map(|task| json!(task))
    }
    pub async fn workspace_migrate(&self, input: Value) -> std::result::Result<Value, ApiError> {
        crate::platform_support::require_workspace_migration()?;
        self.workspace_migrate_after_platform_admission(input).await
    }
    // The original admission/order is shared unchanged with recovery fixtures.
    async fn workspace_migrate_after_platform_admission(
        &self,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        self.check_native_ready()?;
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
    #[cfg(test)]
    pub(crate) async fn workspace_migrate_backend_fixture(
        &self,
        input: Value,
    ) -> std::result::Result<Value, ApiError> {
        // Keep the existing lifecycle/recovery tests on the real engine. This
        // is compiled only into tests, never a runtime capability override.
        // Mac still takes its original public API path and all its checks.
        if crate::platform_support::HostPlatform::current()
            == crate::platform_support::HostPlatform::Windows
        {
            self.workspace_migrate_after_platform_admission(input).await
        } else {
            self.workspace_migrate(input).await
        }
    }
}
#[path = "service_workspace_migration.rs"]
mod workspace_migration;
use workspace_migration::submit_migration;
pub use workspace_migration::{
    NativeWorkspaceMigration, WorkspaceMigrationNotice, WorkspaceMigrationStage,
};
#[path = "service_shutdown.rs"]
mod shutdown_coordinator;
pub struct RuntimeService {
    shared: Arc<RuntimeSwitch>,
    shutdown: Arc<shutdown_coordinator::RuntimeShutdown>,
}

impl RuntimeService {
    pub async fn start(paths: RuntimePaths, lock: InstanceLock) -> Result<Self> {
        Self::start_inner(paths, lock, None, false).await
    }
    pub async fn start_with_resources(
        paths: RuntimePaths,
        lock: InstanceLock,
        resource_root: PathBuf,
        cli_path: PathBuf,
    ) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        let resources = Arc::new(ProjectResources::new(resource_root, cli_path)?);
        Self::start_inner(paths, lock, Some(resources), false).await
    }
    pub async fn start_desktop_with_resources(
        paths: RuntimePaths,
        lock: InstanceLock,
        resource_root: PathBuf,
        cli_path: PathBuf,
    ) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        let resources = Arc::new(ProjectResources::new(resource_root, cli_path)?.for_desktop());
        Self::start_inner(paths, lock, Some(resources), true).await
    }
    async fn start_inner(
        paths: RuntimePaths,
        lock: InstanceLock,
        resources: Option<Arc<ProjectResources>>,
        desktop_pending: bool,
    ) -> Result<Self> {
        crate::platform_support::require_runtime()?;
        paths.verify_lock(&lock)?;
        crate::workspace::recover_startup(&paths)?;
        let listener = bind_loopback(paths.last_port()?).await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(RuntimeSwitch {
            native_startup: StdRwLock::new(desktop_pending.then(|| {
                ApiError::new(
                    "WEBVIEW_STORAGE_STARTING",
                    "正在核对浏览器存储隔离，请稍后重试。",
                )
            })),
            project_cleanup: crate::project_cleanup::ProjectCleanupCoordinator::new(
                paths.data_root(),
            ),
            processes: crate::owned_process::OwnedProcessRegistry::default().with_writer_owner(
                crate::durable_writer::WriterOwner::platform(paths.data_root())?,
            ),
            shutdown_issues: Mutex::new(Vec::new()),
            io_lifetime: crate::io_lifetime::IoLifetime::default(),
            current: StdRwLock::new(None),
            router: StdRwLock::new(Router::new()),
            migration_runner: Mutex::new(None),
            migration_previous: Mutex::new(Vec::new()),
            migration_submission: Mutex::new(()),
            native_migration: StdRwLock::new(None),
            migration_retained_state: Mutex::new(None),
            #[cfg(test)]
            migration_reopen_failure: Mutex::new(None),
            #[cfg(test)]
            migration_registration_pause: Mutex::new(None),
            closing: AtomicBool::new(false),
            migration_active: AtomicBool::new(false),
            ai_open_requests: broadcast::channel(8).0,
            ai_open_times: Mutex::new(HashMap::new()),
            app_reload_hook: StdRwLock::new(None),
            reload_gates: Mutex::new(HashMap::new()),
            management_requests: broadcast::channel(16).0,
            management_pending: Mutex::new(HashMap::new()),
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
        let shutdown =
            shutdown_coordinator::RuntimeShutdown::new(shared.clone(), shutdown, task, lock);
        if !desktop_pending {
            let _ = workspace_migration::submit_selected_cleanup(shared.clone());
        }
        Ok(Self { shared, shutdown })
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
        launch(&self.shared.snapshot(), id).await
    }
    pub fn shutdown_status(&self) -> crate::shutdown::ShutdownReport {
        self.shutdown.status()
    }
    pub fn begin_quiesce(&self) -> crate::shutdown::ShutdownReport {
        self.shutdown.request()
    }
    pub async fn quiesce(&self) -> crate::shutdown::ShutdownReport {
        self.shutdown.wait().await
    }
    pub async fn shutdown(self) -> Result<()> {
        self.quiesce().await.into_result()
    }
}
impl Drop for RuntimeService {
    fn drop(&mut self) {
        self.shutdown.on_service_drop();
    }
}

fn runtime_router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/runtime", get(runtime))
        .route("/api/v1/{*tail}", any(management))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), validate_host))
        .with_state(state)
}
async fn dispatch(State(shared): State<Arc<RuntimeSwitch>>, request: Request) -> Response {
    let startup_error = shared
        .native_startup
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(error) = startup_error {
        return failure(error, None);
    }
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
    let processes = shared
        .upgrade()
        .expect("runtime owner exists")
        .processes
        .clone();
    let store = match store {
        Some(store) => store,
        None => Arc::new(ProjectStore::with_paths(
            paths.workspace_root().into(),
            paths.data_root().into(),
            resources.clone(),
            processes.clone(),
        )?),
    };
    store.set_node_source_data_root(paths.data_root().to_path_buf());
    anyhow::ensure!(
        store.processes().shares_registry(&processes),
        "Project store must share the runtime process registry"
    );
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
    let builds = BuildManager::with_managed_node(
        paths.clone(),
        store.clone(),
        resources
            .as_ref()
            .map(|resources| resources.development_node_root()),
    )?;
    let maintenance = crate::maintenance::MaintenanceManager::new(store.clone(), builds.clone());
    let lifecycle = crate::web_project_deletion::WebProjectDeletionManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        maintenance.clone(),
    )?;
    lifecycle.set_project_cleanup(
        shared
            .upgrade()
            .expect("runtime owner exists")
            .project_cleanup
            .clone(),
    );
    let storage = crate::storage::StorageManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        maintenance.clone(),
    )?;
    let installation_scope = resources
        .as_ref()
        .and_then(|resources| resources.installation_scope());
    crate::storage::configure_installation_scope(&storage, installation_scope.clone());
    let cleanup = shared
        .upgrade()
        .expect("runtime owner exists")
        .project_cleanup
        .clone();
    cleanup.register_storage(&storage);
    let (events, _) = broadcast::channel(128);
    let (stop, _) = watch::channel(false);
    let git_environment = crate::git_environment::GitEnvironmentManager::with_processes(
        paths.data_root().into(),
        resources
            .as_ref()
            .map(|resources| resources.development_git_root()),
        processes.clone(),
    )
    .with_project_boundary(paths.workspace_root().into());
    // Corrupt source evidence must not prevent existing applications opening.
    // Keep the feature error and preserve its files for explicit inspection.
    let source_acquisition = crate::source_acquisition::SourceAcquisitionManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        git_environment.clone(),
    );
    let node_git_import = crate::node_git_import::NodeGitImportManager::new(
        paths.clone(),
        store.clone(),
        builds.clone(),
        source_acquisition.clone(),
    )?;
    let state = Arc::new(HttpState {
        io_lifetime: shared
            .upgrade()
            .expect("runtime owner exists")
            .io_lifetime
            .clone(),
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
        git_environment,
        git_remote: crate::git_remote::GitRemoteChecker::with_processes(processes),
        node_git_import,
        maintenance,
        lifecycle,
        storage,
        resources,
        tickets: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        events,
        uploads: Arc::new(Semaphore::new(2)),
        project_uploads: Arc::new(Mutex::new(HashMap::new())),
        web_runtime: Some(web_runtime::WebRuntimeManager::new()),
        backend: None,
        stopping: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        drained: Notify::new(),
        stop,
    });
    if let Some(manager) = &state.web_runtime {
        let runtime = Arc::downgrade(manager);
        cleanup.set_web_close(Arc::new(move |id| {
            let runtime = runtime.clone();
            Box::pin(async move {
                if let Some(runtime) = runtime.upgrade() {
                    runtime.close(&id).await?;
                }
                Ok(())
            })
        }));
    }
    let removed_state = Arc::downgrade(&state);
    state.store.on_project_identity_removed(Arc::new(move |id| {
        if let Some(state) = removed_state.upgrade() {
            state
                .sessions
                .lock()
                .unwrap()
                .retain(|_, session| session.project_id != id);
            state
                .tickets
                .lock()
                .unwrap()
                .retain(|_, ticket| ticket.project_id != id);
            state.project_uploads.lock().unwrap().remove(id);
            state.node_git_import.forget_deleted_project(id);
            if let Some(manager) = &state.web_runtime {
                manager.forget(id);
            }
        }
    }));
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

fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
pub(crate) fn millis() -> u64 {
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
async fn launch(state: &Arc<HttpState>, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
    launch_inner(state, id, false).await.map(|(info, _)| info)
}
async fn launch_inner(
    state: &Arc<HttpState>,
    id: &str,
    window: bool,
) -> std::result::Result<(LaunchInfo, Option<WebWindowLease>), ApiError> {
    let manager = state
        .web_runtime
        .as_ref()
        .ok_or_else(|| ApiError::new("WEB_RUNTIME_UNAVAILABLE", "网页服务管理器不存在。"))?;
    manager.open(state.clone(), id, window).await
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
    let hosts = request.headers().get_all(header::HOST);
    if hosts.iter().count() != 1
        || hosts.iter().next().and_then(|v| v.to_str().ok()) != Some(state.expected_host.as_str())
    {
        return failure(
            ApiError::new("INVALID_HOST", "Use the discovered loopback address"),
            None,
        );
    }
    state.active.fetch_add(1, Ordering::SeqCst);
    let active = Active(state.clone());
    if state.stopping.load(Ordering::SeqCst) {
        return failure(
            ApiError::new("SERVICE_STOPPING", "Platform is shutting down"),
            None,
        );
    }
    let io_lease = match state.io_lifetime.admit() {
        Ok(lease) => lease,
        Err(error) => return failure(error, None),
    };
    // Losing a connection/request must not detach a blocking handler from the
    // service's drain. The response body has a separate producer lifetime.
    let mut response = match io_lease
        .run(async move {
            let _active = active;
            if _active.0.stopping.load(Ordering::SeqCst) {
                return failure(
                    ApiError::new("SERVICE_STOPPING", "The previous workspace has stopped"),
                    None,
                );
            }
            next.run(request).await
        })
        .await
    {
        Ok(response) => response,
        Err(error) => return failure(error, None),
    };
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
fn status(error: &ApiError) -> StatusCode {
    match error.code.as_str() {
        "UNAUTHENTICATED" | "INVALID_SESSION" | "INVALID_TICKET" => StatusCode::UNAUTHORIZED,
        "INVALID_HOST" | "INVALID_ORIGIN" | "BROWSER_MANAGEMENT_DENIED" | "FILE_ACCESS_DENIED" => {
            StatusCode::FORBIDDEN
        }
        "PROJECT_NOT_FOUND" | "FILE_NOT_FOUND" | "NOT_FOUND" | "TRASH_NOT_FOUND" => {
            StatusCode::NOT_FOUND
        }
        "FILE_EXISTS"
        | "METADATA_CHANGED"
        | "STORAGE_CURSOR_INVALID"
        | "PROJECT_RELOAD_REQUIRED"
        | "PROJECT_ID_CONFLICT"
        | "PROJECT_REMOVED"
        | "PROJECT_DELETED"
        | "PROJECT_DELETE_INCOMPLETE"
        | "PROJECT_DELETE_CONFIRMATION_CHANGED"
        | "PROJECT_REGISTRATION_PENDING"
        | "PROJECT_DATA_RESTORE_PENDING"
        | "PROJECT_DATA_RESTORE_BLOCKED"
        | "WORKSPACE_MIGRATING"
        | "SERVICE_INSTANCE_CHANGED"
        | "PROJECT_NOT_BUILT"
        | "DATABASE_MISSING"
        | "PROJECT_CHANGED"
        | "PROJECT_BUSY"
        | "PROJECT_MAINTENANCE"
        | "MAINTENANCE_ID_MISMATCH"
        | "RECOVERY_REQUIRED"
        | "APP_PORT_IN_USE"
        | "WEB_CONFIGURATION_REQUIRED"
        | "WEB_RUNTIME_CLOSING"
        | "WEB_RUNTIME_CLOSE_FAILED"
        | "BUILD_REQUIRED" => StatusCode::CONFLICT,
        "SERVICE_BUSY" | "SERVICE_STOPPING" | "SQL_QUEUE_FULL" | "SQL_BUSY" => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        "REQUEST_TOO_LARGE" | "FILE_TOO_LARGE" => StatusCode::PAYLOAD_TOO_LARGE,
        "FILE_IO_ERROR" | "INTERNAL_ERROR" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}
fn failure(error: ApiError, project_id: Option<&str>) -> Response {
    let status = status(&error);
    (status,Json(json!({"apiVersion":API_VERSION,"ok":false,"requestId":Uuid::new_v4().to_string(),"projectId":project_id,"error":error}))).into_response()
}
fn result<T: Serialize>(
    data: std::result::Result<T, ApiError>,
    project_id: Option<&str>,
    request_id: &str,
) -> Response {
    match data {Ok(data)=>Json(json!({"apiVersion":API_VERSION,"ok":true,"requestId":request_id,"projectId":project_id,"data":data})).into_response(),Err(error)=>(status(&error),Json(json!({"apiVersion":API_VERSION,"ok":false,"requestId":request_id,"projectId":project_id,"error":error}))).into_response()}
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
            info["platform"] = crate::platform_info::with_resources(state.resources.as_deref());
            Json(info).into_response()
        }
        Err(error) => failure(error, None),
    }
}
fn origin(
    state: &HttpState,
    headers: &HeaderMap,
    required: bool,
) -> std::result::Result<(), ApiError> {
    let values = headers.get_all(header::ORIGIN);
    let count = values.iter().count();
    if (required && count == 0)
        || count > 1
        || (count == 1
            && values.iter().next().and_then(|v| v.to_str().ok())
                != Some(state.info.base_url.as_str()))
    {
        return Err(ApiError::new(
            "INVALID_ORIGIN",
            "Request must originate from this platform instance",
        ));
    }
    Ok(())
}
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
/// Read only private identity and ownership; a broken business database must not
/// prevent the user from reviewing permanent deletion.
fn web_delete_preview(state: &HttpState, id: &str) -> std::result::Result<Value, ApiError> {
    let ownership = crate::web_project_ownership::WebProjectOwnership::new(
        state.paths.data_root().into(),
        state.paths.workspace_root().into(),
    );
    let owner_directory = ownership.directory(id)?;
    let owner_file = ownership.path(id)?;
    crate::projects::check_no_symlinks(&owner_file, true)?;
    // An unreadable registration is previewed as a cleanup of platform data
    // only; its folder is kept.
    let unreadable =
        state.store.registration_unreadable(id)? && state.store.pending_deletion(id)?.is_none();
    let identity_absent = unreadable || state.store.deleted_project_identity_absent(id)?;
    let project_ownership = if identity_absent {
        None
    } else {
        Some(state.store.project_ownership(id)?)
    };
    let recorded = if owner_file.exists() {
        crate::web_project_cleanup::project_material_paths(&state.paths, id).ok()
    } else {
        None
    };
    let mut paths = if let Some(paths) = recorded {
        paths
    } else {
        let mut paths = crate::web_project_ownership::fixed_paths(
            state.paths.data_root(),
            state.paths.workspace_root(),
            id,
        );
        for task in state.builds.project_task_ids(id) {
            paths.push(
                state
                    .paths
                    .workspace_root()
                    .join(".platform/tasks")
                    .join(&task),
            );
            paths.push(
                state
                    .paths
                    .workspace_root()
                    .join(".platform/operations")
                    .join(task),
            );
        }
        paths
    };
    paths.push(owner_directory);
    if unreadable {
        paths.push(crate::projects::registration_path(
            state.paths.data_root(),
            id,
        ));
    }
    paths.push(
        crate::webview_storage::StoreLedger::new(state.paths.data_root().into())
            .project_directory(crate::project_cleanup::ProjectKind::App, id)?,
    );
    let (root, epoch, mode, maintenance_id) = if identity_absent {
        (None, None, "ready".to_string(), None)
    } else {
        let (root, _) = state.store.deletion_root(id)?;
        let pending = state.store.pending_deletion(id)?;
        let epoch = state
            .store
            .registered_epoch(id)?
            .or_else(|| pending.map(|marker| marker.source_epoch.unwrap_or(marker.target_epoch)))
            .ok_or_else(|| {
                ApiError::new(
                    "PROJECT_DELETE_CONFIRMATION_REQUIRED",
                    "项目登记身份无法核实，未授权删除。",
                )
            })?;
        if project_ownership == Some(crate::projects::ProjectOwnership::Platform) {
            paths.insert(0, root.clone());
        }
        let (mode, maintenance_id) = match state.store.control_for_deletion(id) {
            Ok(control) => {
                let value = control.get();
                (value.mode, value.maintenance_id)
            }
            Err(_) => ("unknown".into(), None),
        };
        (Some(root), Some(epoch), mode, maintenance_id)
    };
    let mut existing = Vec::new();
    for path in paths {
        crate::projects::check_no_symlinks(&path, true)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) if !existing.contains(&path) => existing.push(path),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    let already_absent = !unreadable
        && identity_absent
        && crate::web_project_cleanup::project_materials_absent(&state.paths, &state.builds, id)?;
    Ok(
        json!({"projectId":id,"instanceId":state.info.instance_id,"projectRoot":root,"expectedDocumentEpoch":epoch,"mode":mode,"maintenanceId":maintenance_id,"deletePaths":existing,"ownership":project_ownership,"sourcePreserved":project_ownership==Some(crate::projects::ProjectOwnership::User),"alreadyAbsent":already_absent,"cleanupOnly":identity_absent&&!already_absent}),
    )
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
            ["apps", _, "builds" | "deletions"]
                | ["workspace", "migrations"]
                | ["workspace", "migrations", _, "cleanup"]
                | ["apps", _, "storage", "scans" | "cleanups"]
        );
    let rid = Uuid::new_v4().to_string();
    let id = if segments.len() > 1 && segments[0] == "apps" {
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
            (Method::POST, ["apps", "check"]) => {
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Directory { path: String }
                handle.check_directory(input::<Directory>(request).await?.path).await
            }
            (Method::POST, ["apps", "add"]) => {
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Add { path: Option<String>, reservation: Option<String> }
                let value = input::<Add>(request).await?;
                match (value.path, value.reservation) {
                    (Some(path), None) => handle.add_directory(path).await,
                    (None, Some(id)) => Ok(json!({"added":true,"app":state.store.add_reserved_app(&id).await?})),
                    _ => Err(ApiError::new("INVALID_INPUT", "请提供文件夹路径或预留标识之一。")),
                }
            }
            (Method::POST, ["apps", "reserve"]) => {
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Reserve { name: Option<String> }
                let value: Reserve = serde_json::from_value(optional_json(request).await?)?;
                state.store.reserve_app(value.name.as_deref())
            }
            (Method::POST, ["apps", id, "reload"]) => {
                if optional_json(request).await? != json!({}) { return Err(ApiError::new("INVALID_INPUT", "重新加载不接受额外参数。")); }
                handle.reload_app(id).await
            }
            (Method::GET, ["apps", id, "logs"]) => handle.app_logs(id),
            (Method::GET, ["environment"]) => handle.environment().await,
            (Method::POST, ["directories", action @ ("check" | "add")]) => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct DirectoryInput {
                    path: String,
                }
                let path = input::<DirectoryInput>(request).await?.path;
                if *action == "add" {
                    handle.add_directory(path).await
                } else {
                    handle.check_directory(path).await
                }
            }
            (Method::GET, ["environment", "git"]) => handle.git_environment().await,
            (Method::POST, ["environment", "git", "remote-check"]) => {
                handle.check_git_remote(input(request).await?).await
            }
            (Method::PUT, ["environment", "git"]) => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct GitConfig {
                    git_path: Value,
                }
                let value = input::<GitConfig>(request).await?.git_path;
                let path = match value {
                    Value::Null => None,
                    Value::String(path) => Some(path),
                    _ => {
                        return Err(ApiError::new(
                            "INVALID_GIT_PATH",
                            "gitPath must be an absolute path string or null",
                        ))
                    }
                };
                if let Some(result) = handle.confirm_management("更改 Git 开发环境", format!("将 Git 路径设为：{}。这会影响后续下载和 Git 操作。", path.as_deref().unwrap_or("平台默认"))).await? { return Ok(result); }
                handle.set_git_path(path).await
            }
            (Method::PUT, ["environment", "node"]) => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct NodeConfig {
                    node_path: Option<String>,
                }
                let path = input::<NodeConfig>(request).await?.node_path;
                if let Some(result) = handle.confirm_management("更改 Node 开发环境", format!("将 Node 路径设为：{}。这会影响后续构建。", path.as_deref().unwrap_or("平台默认"))).await? { return Ok(result); }
                handle.set_node_path(path).await
            }
            (Method::GET, ["workspace", "storage"]) => handle.workspace_storage().await,
            (Method::GET, ["workspace"]) => handle.workspace_info().await,
            (Method::GET, ["apps", id, "storage"]) => handle.storage_info(id).await,
            (Method::POST, ["apps", id, "storage", "scans"]) => {
                let body = optional_json(request).await?;
                if body != json!({}) {
                    return Err(ApiError::new("INVALID_INPUT", "Scan input must be empty"));
                }
                handle.storage_scan(id).await
            }
            (Method::POST, ["apps", id, "storage", "cleanups"]) => {
                let body = input::<Value>(request).await?;
                if let Some(result) = handle.confirm_management("清理应用存储", format!("应用：{id}。将按当前清理计划移除选中的受管材料。请核对清理范围：{body}")).await? { return Ok(result); }
                handle.storage_cleanup(id, body).await
            }
            (Method::GET, ["apps", id, "storage", "files"]) => {
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
            (Method::POST, ["workspace", "migrations", id, "cleanup"]) => {
                let _ = id;
                Err(ApiError::new("MANAGEMENT_UI_REQUIRED", "请在平台界面操作工作区收尾。"))
            }
            (Method::POST, ["workspace", "migrations"]) => {
                Err(ApiError::new("MANAGEMENT_UI_REQUIRED", "请在平台界面迁移工作区。"))
            }
            (Method::GET, ["apps", id, "deletion-preview"]) => {
                if !query(&request)?.is_empty() {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Deletion preview does not accept query parameters.",
                    ));
                }
                web_delete_preview(&state, id)
            }
            (Method::POST, ["apps", id, "deletions"]) => {
                let body = optional_json(request).await?;
                let preview = web_delete_preview(&state, id)?;
                let name = state.store.info(id).await.map(|info|info.name).unwrap_or_else(|_|id.to_string());
                let impact = if preview["sourcePreserved"] == true {
                    format!("删除应用“{name}”的平台登记、配置、密钥、日志和浏览器数据；保留用户文件夹 {} 及其中的数据。", preview["projectRoot"])
                } else { format!("永久删除应用“{name}”，包括平台归属的文件夹 {}、代码、Git、数据、配置、密钥、日志和浏览器数据。", preview["projectRoot"]) };
                if let Some(result) = handle.confirm_management("删除应用", impact).await? { return Ok(result); }
                handle.project_delete(id, body).await
            }
            (Method::GET, ["apps"]) => handle.projects().await,
            (Method::POST, ["apps"]) => {
                handle
                    .create_project(input::<CreateProjectInput>(request).await?)
                    .await
            }
            (Method::GET, ["apps", id]) => handle.project(id).await,
            (Method::GET, ["apps", id, "configuration"]) => handle.web_configuration(id).await,
            (Method::PUT, ["apps", id, "configuration"]) => {
                handle
                    .save_web_configuration(id, input::<Value>(request).await?)
                    .await
            }
            (Method::GET, ["apps", id, "web-runtime"]) => Ok(handle.web_status(id)),
            (Method::POST, ["apps", id, "web-runtime", "close"]) => {
                if optional_json(request).await? != json!({}) {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "关闭网页服务不接受额外参数。",
                    ));
                }
                handle.close_web(id).await
            }
            (Method::POST, ["apps", id, "builds"]) => {
                handle
                    .build_project_with_input(id, optional_json(request).await?)
                    .await
            }
            (Method::GET, ["apps", id, "maintenance"]) => handle.maintenance_status(id).await,
            (Method::GET, ["apps", id, "handoff"]) => handle.handoff(id).await,
            (Method::GET, ["tasks", id]) => {
                let parameters = query(&request)?;
                if parameters
                    .keys()
                    .any(|key| !matches!(key.as_str(), "projectId" | "taskType"))
                {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Only projectId and taskType are accepted for task context.",
                    ));
                }
                match (parameters.get("projectId"), parameters.get("taskType")) {
                    (Some(project), Some(kind)) => handle.task_for_project(id, project, kind).await,
                    (None, None) => handle.task(id).await,
                    _ => Err(ApiError::new(
                        "INVALID_INPUT",
                        "projectId and taskType must be supplied together.",
                    )),
                }
            }
            (Method::GET, ["apps", id, "db", "schema"]) => {
                Ok(json!(state.store.schema(id, None).await?))
            }
            (Method::POST, ["apps", id, "db", "execute"]) => {
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
            (Method::POST, ["apps", id, "db", "transaction"]) => {
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
            (Method::POST, ["apps", id, "launch"]) => Ok(json!(launch(&state, id).await?)),
            (Method::POST, ["apps", id, "refresh"]) => {
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
fn cookie_name(id: &str) -> String {
    format!("lp_{id}")
}
fn session(
    state: &HttpState,
    id: &str,
    headers: &HeaderMap,
    mutating: bool,
) -> std::result::Result<ProjectHandle, ApiError> {
    origin(state, headers, mutating)?;
    let name = cookie_name(id);
    let mut found = Vec::new();
    for line in headers.get_all(header::COOKIE).iter() {
        if let Ok(line) = line.to_str() {
            for pair in line.split(';') {
                if let Some((key, value)) = pair.trim().split_once('=') {
                    if key == name {
                        found.push(value);
                    }
                }
            }
        }
    }
    if found.len() != 1 {
        return Err(ApiError::new(
            "INVALID_SESSION",
            "Open this project from the platform to connect",
        ));
    }
    let sessions = state.sessions.lock().unwrap();
    let _session = sessions
        .get(found[0])
        .filter(|s| s.project_id == id)
        .ok_or_else(|| {
            ApiError::new(
                "INVALID_SESSION",
                "Project session has expired; reopen from the platform",
            )
        })?;
    let session_epoch = _session.document_epoch.clone();
    let project = _session.project.clone();
    drop(sessions);
    if state.store.registered_epoch(id)?.as_deref() != Some(session_epoch.as_str()) {
        return Err(ApiError::new(
            "INVALID_SESSION",
            "This project identity is no longer active; reopen it from the platform",
        ));
    }
    project.check()?;
    Ok(project)
}
fn epoch<'a>(
    headers: &'a HeaderMap,
    query: &'a HashMap<String, String>,
) -> std::result::Result<&'a str, ApiError> {
    let values = headers.get_all("x-platform-document-epoch");
    if values.iter().count() > 1 {
        return Err(ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Provide one document epoch",
        ));
    }
    let header = values.iter().next().and_then(|v| v.to_str().ok());
    let param = query.get("documentEpoch").map(String::as_str);
    if header.is_some() && param.is_some() && header != param {
        return Err(ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Conflicting document epochs",
        ));
    }
    header.or(param).filter(|v| !v.is_empty()).ok_or_else(|| {
        ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Reload this project to establish its document epoch",
        )
    })
}
fn query(request: &Request) -> std::result::Result<HashMap<String, String>, ApiError> {
    if url::form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
        .filter(|(name, _)| name == "documentEpoch")
        .count()
        > 1
    {
        return Err(ApiError::new(
            "PROJECT_RELOAD_REQUIRED",
            "Provide one document epoch",
        ));
    }
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
        project.require_sdk()?;
        let query = query(&request)?;
        let epoch = if method == Method::GET
            && tail == "context"
            && !request.headers().contains_key("x-platform-document-epoch")
            && !query.contains_key("documentEpoch")
        {
            project.document_epoch.clone()
        } else {
            epoch(request.headers(), &query)?.to_string()
        };
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
            return read_attachment(&state, &project, &path, &epoch, request.headers())
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
                let root = files::root(&project.ensure_attachments_dir()?)?;
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
            (Method::GET,"context")=>Ok(json!({"projectId":id,"projectRoot":project.root,"name":project.manifest.app.name,"sdkApiVersion":API_VERSION,"documentEpoch":project.document_epoch,"instanceId":state.info.instance_id,"capabilities":["db.execute","db.transaction","db.schema","files","events","ai.open"],"runtimeRequirements":project.manifest.runtime_requirements,"runtimePolicyFingerprint":policy_fingerprint(&project.manifest.runtime_requirements),"limits":browser_limits(),"state":state.maintenance.browser_status(&id,&epoch)})),
            (Method::GET,"db/schema")=>Ok(json!(state.store.schema_snapshot(&project,Some(&epoch),SqlCaller::Browser).await?)),
            (Method::POST,"db/execute")=>{let value=state.store.execute_snapshot(&project,input::<SqlRequest>(request).await?,Some(&epoch),SqlCaller::Browser).await?;if value.changed{changed(&state,&id,&rid,"sql");}Ok(json!(value))},
            (Method::POST,"db/transaction")=>{let value=state.store.transaction_snapshot(&project,input::<TransactionRequest>(request).await?,Some(&epoch),SqlCaller::Browser).await?;if value.iter().any(|v|v.changed){changed(&state,&id,&rid,"transaction");}Ok(json!(value))},
            (Method::POST,"ai/open")=>ai_open(&state,&id,&project.manifest.app.name,input::<AiOpenInput>(request).await?),
            (Method::GET,"files")=>{let _lease=project.operation_gate.read().await;project.validate_epoch(Some(&epoch))?;Ok(json!(files::list(&project.ensure_attachments_dir()?,query.get("prefix").map(String::as_str).unwrap_or(""))?))},
            _=>Err(ApiError::new("NOT_FOUND","Unknown project method or route")),
        }
    }.await;
    result(outcome, Some(&id), &rid)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AiOpenInput {
    text: String,
}
/// Hand a short, app-written text to the user's AI tool. The folder is always
/// this app's own registered root; the manager never sends anything by itself.
fn ai_open(
    state: &HttpState,
    id: &str,
    app_name: &str,
    input: AiOpenInput,
) -> std::result::Result<Value, ApiError> {
    let text = input.text.trim();
    if text.is_empty() || text.chars().count() > AI_OPEN_MAX_CHARS {
        return Err(ApiError::new(
            "INVALID_INPUT",
            "交给 AI 的文字不能为空，也不能超过 2000 字。",
        ));
    }
    let shared = state
        .shared
        .upgrade()
        .ok_or_else(|| ApiError::new("SERVICE_STOPPING", "平台正在退出，请稍后重试。"))?;
    {
        let mut times = shared
            .ai_open_times
            .lock()
            .map_err(|_| ApiError::new("AI_OPEN_UNAVAILABLE", "暂时无法打开 AI 工具。"))?;
        let now = Instant::now();
        if times
            .get(id)
            .is_some_and(|last| now.duration_since(*last) < AI_OPEN_INTERVAL)
        {
            return Err(ApiError::new(
                "AI_OPEN_TOO_FREQUENT",
                "刚刚已经请求打开 AI 工具，请稍等几秒。",
            ));
        }
        times.insert(id.to_owned(), now);
    }
    let delivered = shared
        .ai_open_requests
        .send(AiOpenRequest {
            project_id: id.to_owned(),
            app_name: app_name.to_owned(),
            text: text.to_owned(),
        })
        .is_ok();
    if !delivered {
        return Err(ApiError::new(
            "AI_OPEN_UNAVAILABLE",
            "这个窗口暂时不能打开 AI 工具，请复制文字后发给你的 AI。",
        ));
    }
    Ok(json!({"requested": true}))
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
        crate::runtime_policy::RuntimePolicy::parse(&json!(project.manifest.runtime_requirements))?;
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
fn policy_fingerprint(requirements: &impl Serialize) -> String {
    use sha2::Digest;
    format!(
        "{:x}",
        sha2::Sha256::digest(serde_json::to_vec(requirements).unwrap_or_default())
    )
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
                return(StatusCode::UNAUTHORIZED,[(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; frame-ancestors 'none'")],"<!doctype html><html lang=zh-CN><meta charset=utf-8><title>需要从平台打开</title><h1>请从平台打开这个项目</h1><p>在熊猫小抽屉中打开项目，或让 AI 获取新的浏览器入口。平台重启后需要重新连接。</p></html>").into_response();
            }
            return failure(error, Some(&id));
        }
    };
    let outcome: std::result::Result<Response, ApiError> = async {
        let lease = project.operation_gate.clone().read_owned().await;
        project.validate_epoch(Some(&project.document_epoch))?;
        let relative = path.unwrap_or_else(|| project.manifest.web.entry.clone());
        let path = relative.as_str();
        let mut file = crate::web_files::open(&project.root, &project.manifest, path)?;
        let (csp, permissions) = policy(&project)?;
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
            let nonce=secret();

            let meta = format!(
                "<meta name=\"platform-project-id\" content=\"{}\"><meta name=\"platform-document-epoch\" content=\"{}\"><meta name=\"platform-runtime-policy\" content=\"{}\"><script nonce=\"{nonce}\">{}</script>",
                project.id,
                safe_epoch,
                policy_fingerprint(&project.manifest.runtime_requirements),
                if project.manifest.platform.is_some() { crate::runtime_notice::script() } else { "" }
            );
            let lower = html.to_ascii_lowercase();
            let position = lower
                .find("<head")
                .and_then(|start| lower[start..].find('>').map(|end| start + end + 1))
                .unwrap_or(0);
            html.insert_str(position, &meta);
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
        } else {
            stream_file(file, path, &headers, false, lease, &state.io_lifetime).await?
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
    lifetime: &crate::io_lifetime::IoLifetime,
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
    // The producer owns the actual descriptor and project gate, rather than a
    // response future that can disappear while a blocking read is still alive.
    let io_lease = lifetime.admit()?;
    let lifetime = lifetime.clone();
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let work = io_lease.hold(move || {
        use std::io::{Read, Seek};
        let _project_lease = lease;
        let mut file = file;
        let result = (|| -> std::io::Result<()> {
            file.seek(std::io::SeekFrom::Start(start))?;
            let mut reader = file.take(length);
            loop {
                if lifetime.check_open().is_err() || sender.is_closed() {
                    return Ok(());
                }
                let mut buffer = vec![0; 65536];
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    return Ok(());
                }
                buffer.truncate(count);
                let mut item = Ok(buffer);
                loop {
                    match sender.try_send(item) {
                        Ok(()) => break,
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return Ok(()),
                        Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => {
                            if lifetime.check_open().is_err() {
                                return Ok(());
                            }
                            item = value;
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                }
            }
        })();
        if let Err(error) = result {
            let _ = sender.try_send(Err(error));
        }
    });
    // This owned blocking task is deliberately not aborted with a response.
    tokio::task::spawn_blocking(move || work.run_blocking());
    let body = Body::from_stream(stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    }));
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
    state: &Arc<HttpState>,
    project: &ProjectHandle,
    path: &str,
    epoch: &str,
    headers: &HeaderMap,
) -> std::result::Result<Response, ApiError> {
    let lease = project.operation_gate.clone().read_owned().await;
    project.validate_epoch(Some(epoch))?;
    let root = files::root(&project.ensure_attachments_dir()?)?;
    stream_file(
        files::read(&root, path)?,
        path,
        headers,
        true,
        lease,
        &state.io_lifetime,
    )
    .await
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
    let _lease = project.operation_gate.read().await;
    project.validate_epoch(Some(epoch))?;
    let semaphore = state
        .store
        .with_registered_epoch(&project.id, epoch, || {
            Ok(state
                .project_uploads
                .lock()
                .unwrap()
                .entry(project.id.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(1)))
                .clone())
        })?
        .ok_or_else(|| {
            ApiError::new("PROJECT_REMOVED", "The upload project is no longer active")
        })?;
    let _local = semaphore.try_acquire_owned().map_err(|_| {
        ApiError::new(
            "SERVICE_BUSY",
            "An upload is already active for this project",
        )
    })?;
    let private_uploads = project.private_dir.join("uploads");
    builds::ensure_dir(&private_uploads)?;
    let temporary_name = format!(".upload-{}", Uuid::new_v4().simple());
    let staged = files::create(&files::root(&private_uploads)?, &temporary_name)?;
    let record_path = private_uploads.join(format!("publication-{}.json", Uuid::new_v4().simple()));
    let temporary = Temporary {
        root: files::root(&private_uploads)?,
        name: temporary_name,
    };
    let attachments_path = project.ensure_attachments_dir()?;
    let attachments = files::root(&attachments_path)?;
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
    if let Err(error) = files::publish_tracked(
        &temporary.root,
        &temporary.name,
        &attachments,
        path,
        overwrite,
        &attachments_path,
        &record_path,
    ) {
        if error
            .details
            .get("operationOutcome")
            .and_then(Value::as_str)
            == Some("committed")
        {
            changed(state, &project.id, rid, "file");
        }
        return Err(error);
    }
    changed(state, &project.id, rid, "file");
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
                            Ok(change) if change.project_id==project.id=>{let data=if change.event=="project.state.changed"{let current=project.control.get();json!({"projectId":change.project_id,"mode":current.mode,"busy":project.control.frozen.load(Ordering::Acquire),"reloadRequired":project.document_epoch!=epoch||project.check().is_err_and(|e|matches!(e.code.as_str(),"PROJECT_RELOAD_REQUIRED"|"PROJECT_IDENTITY_CHANGED")),"workspaceMigrating":project.is_workspace_paused(),"removed":project.check().is_err_and(|e|e.code=="PROJECT_REMOVED")})}else{json!({"projectId":change.project_id,"requestId":change.request_id,"taskId":if change.source=="build" {Some(&change.request_id)} else {None},"source":change.source})};(change.event,data)},
                            Ok(_)=>continue,
                            Err(broadcast::error::RecvError::Lagged(_))=>("data.changed".into(),json!({"projectId":project.id,"source":"resync"})),
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
    #[tokio::test]
    async fn desktop_http_stays_closed_until_native_storage_initialization_completes() {
        let (_temp, paths) = RuntimePaths::for_test();
        let lock = paths.acquire_lock().unwrap();
        let service = RuntimeService::start_inner(paths, lock, None, true)
            .await
            .unwrap();
        let client = make_client();
        let endpoint = format!("{}/healthz", service.info().base_url);
        assert!(client
            .get(&endpoint)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .contains("WEBVIEW_STORAGE_STARTING"));
        let handle = service.handle();
        handle.fail_native_startup(ApiError::new(
            "DISPOSABLE_NATIVE_FAILURE",
            "disposable test",
        ));
        assert!(client
            .get(&endpoint)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .contains("DISPOSABLE_NATIVE_FAILURE"));
        assert!(handle.check_native_ready().is_err());
        handle.complete_native_startup();
        assert!(handle.check_native_ready().is_ok());
        assert!(client
            .get(&endpoint)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        service.shutdown().await.unwrap();
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
        std::fs::create_dir_all(project.project_root.join("dist")).unwrap();
        std::fs::write(project.project_root.join("dist/index.html"),"<!doctype html><html><head><title>Test</title></head><body><script src=app.js></script></body></html>").unwrap();
        std::fs::write(
            project.project_root.join("dist/app.js"),
            "document.body.dataset.ready='true';",
        )
        .unwrap();
        let manifest_path = project.project_root.join("panda.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["web"] = json!({"root":"dist","entry":"index.html"});
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        service.store().info(&project.id).await.unwrap()
    }
    pub(super) async fn connect(service: &RuntimeService, client: &Client, id: &str) -> String {
        let launch = service.launch(id).await.unwrap();
        let response = client.get(&launch.launch_url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        cookie.split(';').next().unwrap().into()
    }
    pub(super) fn app_base(service: &RuntimeService, id: &str) -> String {
        service.store().web_address(id).unwrap().origin()
    }
    pub(super) async fn app_state(service: &RuntimeService, id: &str) -> Arc<HttpState> {
        service
            .shared
            .snapshot()
            .web_runtime
            .as_ref()
            .unwrap()
            .fixture_http(id)
            .await
    }
    async fn sql(
        service: &RuntimeService,
        client: &Client,
        id: &str,
        body: Value,
    ) -> ClientResponse {
        client
            .post(format!(
                "{}/api/v1/apps/{id}/db/execute",
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
        let cookie_b = connect(&service, &client, &b.id).await;
        let base = &app_base(&service, &a.id);
        let body = json!({"sql":"SELECT value FROM records"});
        let request = || {
            client
                .post(format!(
                    "{}/_platform/api/v1/db/execute",
                    app_base(&service, &a.id)
                ))
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
            .post(format!(
                "{}/_platform/api/v1/db/execute",
                app_base(&service, &b.id)
            ))
            .header("cookie", &cookie)
            .header("origin", app_base(&service, &b.id))
            .header("x-platform-document-epoch", &b.document_epoch)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let no_origin = client
            .post(format!(
                "{}/_platform/api/v1/db/execute",
                app_base(&service, &a.id)
            ))
            .header("cookie", &cookie)
            .header("x-platform-document-epoch", &a.document_epoch)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);
        let context = client
            .get(format!(
                "{}/_platform/api/v1/context",
                app_base(&service, &a.id)
            ))
            .header("cookie", &cookie)
            .header("x-platform-document-epoch", &a.document_epoch)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(context["data"]["projectId"], a.id);
        assert_eq!(context["data"]["projectRoot"], json!(a.project_root));
        assert_eq!(context["data"]["limits"]["limitsStatus"], "internalTest");
        assert_eq!(context["data"]["documentEpoch"], a.document_epoch);
        assert!(context["data"].get("workspaceRoot").is_none());
        assert!(!context
            .to_string()
            .contains(&serde_json::to_string(&b.project_root).unwrap()));
        let other_context = client
            .get(format!(
                "{}/_platform/api/v1/context",
                app_base(&service, &b.id)
            ))
            .header("cookie", &cookie_b)
            .header("x-platform-document-epoch", &b.document_epoch)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(other_context["data"]["projectId"], b.id);
        assert_eq!(other_context["data"]["projectRoot"], json!(b.project_root));
        assert!(other_context["data"].get("workspaceRoot").is_none());
        assert!(!other_context
            .to_string()
            .contains(&serde_json::to_string(&a.project_root).unwrap()));
        assert_eq!(
            client
                .get(format!(
                    "{}/_platform/api/v1/context",
                    app_base(&service, &b.id)
                ))
                .header("cookie", &cookie)
                .header("x-platform-document-epoch", &b.document_epoch)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let html = client
            .get(format!("{}/", app_base(&service, &a.id)))
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
        let first = client.get(&launch.launch_url).send().await.unwrap();
        assert_eq!(first.status(), StatusCode::SEE_OTHER);
        let cookie = first.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(
            client
                .get(&launch.launch_url)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let expired = service.launch(&project.id).await.unwrap();
        let token = url::Url::parse(&expired.launch_url)
            .unwrap()
            .query_pairs()
            .find(|(name, _)| name == "token")
            .unwrap()
            .1
            .into_owned();
        app_state(&service, &project.id)
            .await
            .tickets
            .lock()
            .unwrap()
            .get_mut(&token)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            client
                .get(&expired.launch_url)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let unused = service.launch(&project.id).await.unwrap();
        let instance = service.info().instance_id;
        let original_origin = app_base(&service, &project.id);
        service.shutdown().await.unwrap();
        let restarted = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
            .await
            .unwrap();
        assert_ne!(restarted.info().instance_id, instance);
        let reopened = restarted.launch(&project.id).await.unwrap();
        assert_eq!(reopened.url, format!("{original_origin}/"));
        assert_eq!(
            client
                .get(format!("{original_origin}/_platform/api/v1/context"))
                .header("cookie", cookie)
                .header("x-platform-document-epoch", &project.document_epoch)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&unused.launch_url)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        restarted.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn browser_cache_admission_is_ordered_with_permanent_identity_removal() {
        let (_temporary, _paths, service, _client) = start().await;
        let project = create(&service, "Disposable ordered browser cache").await;
        let state = service.shared.snapshot();
        let id = project.id.clone();
        let epoch = project.document_epoch.clone();
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let admitted = state.clone();
        let admitted_id = id.clone();
        let admitted_epoch = epoch.clone();
        let admitted_project = state.store.project(&id).unwrap();
        let admission = tokio::task::spawn_blocking(move || {
            admitted
                .store
                .with_registered_epoch(&admitted_id, &admitted_epoch, || {
                    admitted.tickets.lock().unwrap().insert(
                        "disposable-late-ticket".into(),
                        Ticket {
                            project: admitted_project.clone(),
                            project_id: admitted_id.clone(),
                            epoch: admitted_epoch.clone(),
                            expires: Instant::now() + Duration::from_secs(60),
                        },
                    );
                    admitted.sessions.lock().unwrap().insert(
                        "disposable-late-session".into(),
                        Session {
                            project: admitted_project.clone(),
                            project_id: admitted_id.clone(),
                            document_epoch: admitted_epoch.clone(),
                        },
                    );
                    admitted
                        .project_uploads
                        .lock()
                        .unwrap()
                        .insert(admitted_id.clone(), Arc::new(Semaphore::new(1)));
                    entered.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
        });
        entered_rx.await.unwrap();
        let (removing, removing_rx) = oneshot::channel();
        let removed = state.clone();
        let removed_id = id.clone();
        let root = project.project_root.clone();
        let deletion = tokio::task::spawn_blocking(move || {
            removing.send(()).unwrap();
            let marker = removed
                .store
                .begin_deletion(
                    &removed_id,
                    "task_disposable_ordered_cache_delete",
                    &Uuid::new_v4().to_string(),
                    &crate::file_identity::PersistedIdentity::capture(&root).unwrap(),
                )
                .unwrap();
            std::fs::remove_dir_all(root).unwrap();
            removed
                .store
                .complete_deleted_project_identity(&marker)
                .unwrap();
        });
        removing_rx.await.unwrap();
        assert!(!deletion.is_finished());
        release.send(()).unwrap();
        assert_eq!(admission.await.unwrap().unwrap(), Some(()));
        deletion.await.unwrap();
        assert_eq!(
            state
                .store
                .with_registered_epoch(&id, &epoch, || {
                    panic!("A delayed cache insertion must not run after identity removal")
                })
                .unwrap(),
            None::<()>
        );
        assert!(state
            .tickets
            .lock()
            .unwrap()
            .values()
            .all(|ticket| ticket.project_id != id));
        assert!(state
            .sessions
            .lock()
            .unwrap()
            .values()
            .all(|session| session.project_id != id));
        assert!(!state.project_uploads.lock().unwrap().contains_key(&id));
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn project_deletion_revokes_browser_identity_and_recreation_uses_a_fresh_id() {
        let (_temp, paths, service, client) = start().await;
        let project = create(&service, "Disposable browser deletion identity").await;
        let cookie = connect(&service, &client, &project.id).await;
        let old_child = app_state(&service, &project.id).await;
        let old_origin = app_base(&service, &project.id);
        let response = client
            .get(format!(
                "{old_origin}/_platform/api/v1/events?documentEpoch={}",
                project.document_epoch
            ))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut events = response.bytes_stream();
        let unused = service.launch(&project.id).await.unwrap();
        let retained_handle = service.store().project(&project.id).unwrap();
        old_child
            .project_uploads
            .lock()
            .unwrap()
            .insert(project.id.clone(), Arc::new(Semaphore::new(1)));
        let handle = service.handle();
        let task = handle
            .project_delete(
                &project.id,
                json!({"expectedDocumentEpoch":project.document_epoch}),
            )
            .await
            .unwrap();
        let deleted = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let record = handle
                    .task_for_project(task["taskId"].as_str().unwrap(), &project.id, "delete")
                    .await
                    .unwrap();
                if record["final"] == true {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(deleted["status"], "succeeded", "{deleted}");
        // Lifecycle events published before revocation may already be queued
        // in the transport. Require actual stream closure after draining them.
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(Ok(_)) = events.next().await {}
        })
        .await
        .expect("the deleted project's event stream must close after queued events drain");
        drop(events);
        assert_eq!(retained_handle.check().unwrap_err().code, "PROJECT_REMOVED");
        assert!(old_child.sessions.lock().unwrap().is_empty());
        assert!(old_child.tickets.lock().unwrap().is_empty());
        assert!(!old_child
            .project_uploads
            .lock()
            .unwrap()
            .contains_key(&project.id));
        assert!(!project.project_root.exists());
        assert!(!paths.data_root().join("apps").join(&project.id).exists());
        assert!(client.get(&unused.launch_url).send().await.is_err());
        let recreated = create(&service, "Disposable browser deletion identity").await;
        assert_ne!(recreated.id, project.id);
        let _ = service.launch(&recreated.id).await.unwrap();
        let stale = client
            .get(format!(
                "{}/_platform/api/v1/context",
                app_base(&service, &recreated.id)
            ))
            .header("cookie", cookie)
            .header("x-platform-document-epoch", &recreated.document_epoch)
            .send()
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
        assert!(!recreated.project_root.join("data").exists());
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn cached_browser_session_never_reregisters_a_missing_project_identity() {
        let (_temp, paths, service, client) = start().await;
        let project = create(&service, "Disposable absent registration").await;
        let cookie = connect(&service, &client, &project.id).await;
        let registration = paths
            .data_root()
            .join("apps")
            .join(&project.id)
            .join("registration.json");
        let base = app_base(&service, &project.id);
        std::fs::remove_file(&registration).unwrap();
        let response = client
            .get(format!("{base}/_platform/api/v1/context"))
            .header("cookie", cookie)
            .header("x-platform-document-epoch", project.document_epoch)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!registration.exists());
        assert!(project.project_root.is_dir());
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn task_http_context_preserves_deleted_receipts_and_rejects_identity_mismatch() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Disposable task context").await;
        let state = service.shared.snapshot();
        let task = state
            .builds
            .create_readonly_operation(&project.id, "storageScan")
            .unwrap();
        state.builds.finish(&task.task_id, Ok(json!({}))).unwrap();
        let base = service.info().base_url;
        let route = format!("{base}/api/v1/tasks/{}", task.task_id);
        for (query, code) in [
            (
                format!("?projectId={}&taskType=delete", project.id),
                "TASK_IDENTITY_CHANGED",
            ),
            (format!("?projectId={}", project.id), "INVALID_INPUT"),
            ("?other=1".into(), "INVALID_INPUT"),
        ] {
            let response: Value = client
                .get(format!("{route}{query}"))
                .bearer_auth(&state.token)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(response["error"]["code"], code, "{response}");
        }
        let handle = service.handle();
        let submitted = handle
            .project_delete(
                &project.id,
                json!({"expectedDocumentEpoch":project.document_epoch}),
            )
            .await
            .unwrap();
        let task_id = submitted["taskId"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let value = handle
                    .task_for_project(task_id, &project.id, "delete")
                    .await
                    .unwrap();
                if value["final"] == true {
                    assert_eq!(value["status"], "succeeded", "{value}");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let response: Value = client
            .get(format!(
                "{base}/api/v1/tasks/{task_id}?projectId={}&taskType=delete",
                project.id
            ))
            .bearer_auth(&state.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["data"]["result"]["alreadyAbsent"], true);
        let unscoped: Value = client
            .get(format!("{base}/api/v1/tasks/{task_id}"))
            .bearer_auth(&state.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(unscoped["error"]["code"], "TASK_NOT_FOUND");
        let other: Value = client
            .get(format!(
                "{base}/api/v1/tasks/{task_id}?projectId={}&taskType=build",
                project.id
            ))
            .bearer_auth(&state.token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(other["error"]["code"], "TASK_NOT_FOUND");
        service.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_publication_conflicts_ranges_and_static_boundaries() {
        let (_temp, _paths, service, client) = start().await;
        let project = create(&service, "Files").await;
        let cookie = connect(&service, &client, &project.id).await;
        let base = &app_base(&service, &project.id);
        let file_url = format!(
            "{}/_platform/api/v1/file?path=notes/example.txt",
            app_base(&service, &project.id)
        );
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
            "panda.json",
            "data/main.sqlite",
            "api/no-such-route",
            "%2e%2e/panda.json",
        ] {
            let response = client
                .get(format!("{}/{tail}", app_base(&service, &project.id)))
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
            .get(format!("{}/leak", app_base(&service, &project.id)))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        assert!(!client
            .put(format!(
                "{}/_platform/api/v1/file?path=../escape",
                app_base(&service, &project.id)
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
            .get(format!(
                "{}/_platform/api/v1/files",
                app_base(&service, &project.id)
            ))
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
        std::fs::create_dir_all(attachment.parent().unwrap()).unwrap();
        std::fs::write(&attachment, b"original").unwrap();
        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(vec![b'x'; 1024]),
            Err(std::io::Error::other("simulate disconnect")),
        ]);
        let response = client
            .put(format!(
                "{}/_platform/api/v1/file?path=original.txt&overwrite=true",
                app_base(&service, &project.id)
            ))
            .header("cookie", cookie)
            .header("origin", app_base(&service, &project.id))
            .header("x-platform-document-epoch", project.document_epoch)
            .body(reqwest::Body::wrap_stream(chunks))
            .send()
            .await;
        assert!(response.is_err());
        let staging = service
            .store()
            .project(&project.id)
            .unwrap()
            .private_dir
            .join("uploads");
        service.shutdown().await.unwrap();
        assert_eq!(std::fs::read(attachment).unwrap(), b"original");
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
                "{}/_platform/api/v1/events?documentEpoch={}",
                app_base(&service, &project.id),
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
        let failed=client.post(format!("{}/api/v1/apps/{}/db/transaction",service.info().base_url,project.id)).bearer_auth(&service.shared.snapshot().token).json(&json!({"statements":[{"sql":"INSERT INTO notes VALUES ('two')"},{"sql":"INSERT INTO notes VALUES ('one')"}]})).send().await.unwrap();
        assert!(!failed.status().is_success());
        assert!(
            tokio::time::timeout(Duration::from_millis(80), events.next())
                .await
                .is_err()
        );
        client
            .post(format!(
                "{}/api/v1/apps/{}/refresh",
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
        std::fs::remove_file(empty.project_root.join("index.html")).unwrap();
        assert_eq!(
            service.launch(&empty.id).await.unwrap_err().code,
            "WEB_ENTRY_MISSING"
        );
        let project = create(&service, "Partial upload").await;
        let cookie = connect(&service, &client, &project.id).await;
        let attachment = project.project_root.join("data/attachments/original.txt");
        std::fs::create_dir_all(attachment.parent().unwrap()).unwrap();
        std::fs::write(&attachment, b"original").unwrap();
        let application = app_base(&service, &project.id);
        let host = application.strip_prefix("http://").unwrap();
        let mut socket = tokio::net::TcpStream::connect(host).await.unwrap();
        let headers = format!("PUT /_platform/api/v1/file?path=original.txt&overwrite=true HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nCookie: {}\r\nX-Platform-Document-Epoch: {}\r\nContent-Length: 1000000\r\n\r\npartial", host,application,cookie,project.document_epoch);
        socket.write_all(headers.as_bytes()).await.unwrap();
        let staging = service
            .store()
            .project(&project.id)
            .unwrap()
            .private_dir
            .join("uploads");
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
        let manifest_path = project.project_root.join("panda.json");
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        for entry in ["index.HTML", "index.htm"] {
            std::fs::write(project.project_root.join("dist").join(entry), "<!doctype html><html><head><title>Alternate</title></head><body>HTML</body></html>").unwrap();
            manifest["web"]["entry"] = json!(entry);
            std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            service.handle().close_web(&project.id).await.unwrap();
            let cookie = connect(&service, &client, &project.id).await;
            let current_epoch = service.store().project(&project.id).unwrap().document_epoch;
            for tail in ["", entry] {
                let response = client
                    .get(format!("{}/{tail}", app_base(&service, &project.id)))
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
                    current_epoch
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
            .get(format!("{}/image.svg", app_base(&service, &project.id)))
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
        manifest["web"]["entry"] = json!("image.svg");
        std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let response = client
            .get(format!("{}/", app_base(&service, &project.id)))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("platform-document-epoch"));
        service.handle().close_web(&project.id).await.unwrap();
        assert_eq!(
            service.launch(&project.id).await.unwrap_err().code,
            "INVALID_PROJECT"
        );
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
#[path = "service_m5_tests.rs"]
mod m5_tests;

#[cfg(test)]
#[path = "service_object_layout_tests.rs"]
mod object_layout_tests;

#[cfg(test)]
#[path = "service_git_environment_tests.rs"]
mod git_environment_tests;

#[cfg(all(test, unix))]
#[path = "service_git_remote_tests.rs"]
mod git_remote_tests;

#[cfg(test)]
#[path = "service_editor_handoff_tests.rs"]
mod editor_handoff_tests;

#[cfg(test)]
#[path = "service_source_tests.rs"]
mod source_tests;

#[cfg(test)]
#[path = "service_flow_prompts_tests.rs"]
mod flow_prompts_tests;

#[cfg(test)]
#[path = "service_io_lifetime_tests.rs"]
mod io_lifetime_tests;

#[cfg(test)]
#[path = "service_workspace_migration_tests.rs"]
mod workspace_migration_tests;

#[cfg(all(test, windows))]
#[path = "service_workspace_migration_windows_tests.rs"]
mod workspace_migration_windows_tests;

#[cfg(all(test, windows))]
#[path = "installation_scope/windows/runtime_tests.rs"]
mod installation_scope_tests;
