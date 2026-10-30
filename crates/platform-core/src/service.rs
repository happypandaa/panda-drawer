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
    artifact_url: Option<String>,
}
struct Session {
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
    lifecycle: Arc<crate::lifecycle::LifecycleManager>,
    metadata: crate::metadata::MetadataManager,
    storage: Arc<crate::storage::StorageManager>,
    history_cleanup: Arc<crate::history_cleanup::CleanupManager>,
    info: BootstrapInfo,
    token: String,
    expected_host: String,
    store: Arc<ProjectStore>,
    builds: Arc<BuildManager>,
    git_environment: crate::git_environment::GitEnvironmentManager,
    git_remote: crate::git_remote::GitRemoteChecker,
    project_git: std::result::Result<Arc<crate::project_git::ProjectGitManager>, ApiError>,
    node_git_import: Arc<crate::node_git_import::NodeGitImportManager>,
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
/// Keeps the resolved project in this workspace while a native editor is opened.
/// This lease ends after dispatch; it does not track subsequent editing in the AI.
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
    pub fn store(&self) -> Arc<ProjectStore> {
        self.current().store.clone()
    }
    pub(crate) fn node_task_manager(&self) -> Arc<BuildManager> {
        self.current().builds.clone()
    }
    pub async fn launch(&self, id: &str) -> std::result::Result<LaunchInfo, ApiError> {
        launch(&self.current(), id)
    }
    pub async fn launch_window(
        &self,
        id: &str,
    ) -> std::result::Result<(LaunchInfo, Option<crate::build_artifacts::ArtifactLease>), ApiError>
    {
        launch_inner(&self.current(), id, true)
    }
    pub fn artifact_access(&self) -> crate::build_artifacts::ArtifactAccess {
        self.current().builds.artifact_access()
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
        // Layout diagnostics use the registered, validated handle, never an
        // untrusted manifest retained by the error summary. They cannot change
        // runtime readiness or initialize a repository.
        value["objectLayout"] = state
            .store
            .project(id)
            .map(|project| crate::object_layout::project_layout(&project.root, &project.manifest))
            .unwrap_or(Value::Null);
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
        crate::node_apps::development::require_export_location_unclaimed(
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
    pub async fn project_git_status(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .status(id, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_versions(
        &self,
        id: &str,
        limit: u32,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .versions(id, limit, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_source_restore_preview(
        &self,
        id: &str,
        input: crate::project_git::GitSourceRestorePreviewInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .source_restore_preview(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_source_restore_apply(
        &self,
        id: &str,
        input: crate::project_git::GitSourceRestoreApplyInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .source_restore_apply(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_source_restore_status(
        &self,
        id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .source_restore_status(id, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_source_restore_recover(
        &self,
        id: &str,
        input: crate::project_git::GitSourceRestoreRecoverInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .source_restore_recover(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_data_restore_preview(
        &self,
        id: &str,
        input: crate::project_git::GitDataRestorePreviewInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .data_restore_preview(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_data_restore_apply(
        &self,
        id: &str,
        input: crate::project_git::GitDataRestoreApplyInput,
    ) -> Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .data_restore_apply(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_data_restore_recover(
        &self,
        id: &str,
        input: crate::project_git::GitDataRestoreRecoverInput,
    ) -> Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .data_restore_recover(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_data_restore_status(&self, id: &str) -> Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .data_restore_status(id, state.stop.subscribe())
                .await?
        ))
    }

    pub async fn project_git_data_save_status(
        &self,
        id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        state
            .project_git
            .as_ref()
            .map_err(Clone::clone)?
            .data_save_status(id, state.stop.subscribe())
            .await
    }
    pub async fn project_git_data_save_finalize(
        &self,
        id: &str,
        input: crate::project_git::GitDataSaveFinalizeInput,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "Platform is shutting down",
            ));
        }
        Ok(json!(
            state
                .project_git
                .as_ref()
                .map_err(Clone::clone)?
                .finalize_data_save(id, input, state.stop.subscribe())
                .await?
        ))
    }
    pub async fn project_git_cancel(
        &self,
        id: &str,
        task_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        if state.builds.task(task_id)?.project_id.as_deref() != Some(id) {
            return Err(ApiError::new(
                "PROJECT_GIT_INPUT_INVALID",
                "任务不属于当前项目。",
            ));
        }
        Ok(json!(state
            .project_git
            .as_ref()
            .map_err(Clone::clone)?
            .cancel(task_id)?))
    }
    pub async fn node_git_import_start(
        &self,
        apps: Arc<crate::node_apps::NodeApps>,
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
                .start(apps, input, state.stop.subscribe())
                .await?
        ))
    }
    pub fn node_git_import_status(&self, import_id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().node_git_import.status(import_id)?))
    }
    pub fn node_git_import_cancel(&self, task_id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().node_git_import.cancel(task_id)?))
    }
    pub async fn node_git_import_retry_cleanup(
        &self,
        apps: Arc<crate::node_apps::NodeApps>,
        task_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        Ok(json!(
            state.node_git_import.retry_cleanup(apps, task_id).await?
        ))
    }
    pub async fn recover_source_imports(
        &self,
        apps: Arc<crate::node_apps::NodeApps>,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        let attempts = state.node_git_import.recover(apps).await?;
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
        let mut handoff = resources.handoff(&project.root, id);
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
    pub async fn project_import_cleanup(
        &self,
        task_id: String,
    ) -> std::result::Result<Value, ApiError> {
        self.current().lifecycle.cleanup_import(task_id).await
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
        value["cleanupTask"] = json!(state.history_cleanup.latest_task()?);
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
        let resources = Arc::new(ProjectResources::new(resource_root, cli_path)?);
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
        launch(&self.shared.snapshot(), id)
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
        None => Arc::new(ProjectStore::with_resources_and_processes(
            paths.workspace_root().into(),
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
    let builds = BuildManager::with_managed_node_and_git(
        paths.clone(),
        store.clone(),
        info.base_url.clone(),
        resources
            .as_ref()
            .map(|resources| resources.development_node_root()),
        resources
            .as_ref()
            .map(|resources| resources.development_git_root()),
    )?;
    let maintenance = crate::maintenance::MaintenanceManager::new(store.clone(), builds.clone());
    let lifecycle = crate::lifecycle::LifecycleManager::new(
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
        info.instance_id.clone(),
    )?;
    let cleanup = shared
        .upgrade()
        .expect("runtime owner exists")
        .project_cleanup
        .clone();
    cleanup.register_storage(&storage);
    cleanup.register_history(&history_cleanup);
    cleanup.register_artifacts(&builds.artifact_access());
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
    let project_git = crate::project_git::ProjectGitManager::new(
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
        project_git,
        node_git_import,
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
    launch_inner(state, id, false).map(|(info, _)| info)
}
fn launch_inner(
    state: &HttpState,
    id: &str,
    window: bool,
) -> std::result::Result<(LaunchInfo, Option<crate::build_artifacts::ArtifactLease>), ApiError> {
    if state.stopping.load(Ordering::SeqCst) {
        return Err(ApiError::new(
            "SERVICE_STOPPING",
            "Platform is shutting down",
        ));
    }
    let project = state.store.project(id)?;
    project.validate_epoch(Some(&project.document_epoch))?;
    validate_entry_format(&project.manifest.entry)?;
    let (generation, artifact_lease) = state.builds.artifact_access().current(&project, !window)?;
    let artifact_url = generation.as_ref().filter(|_| window).map(|generation| {
        format!(
            "/p/{id}/_builds/{}/{}",
            generation.build_id, generation.entry_relative
        )
    });
    let (output_path, entry) = match generation {
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
    state
        .store
        .with_registered_epoch(id, &project.document_epoch, || {
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
                    artifact_url,
                },
            );
            Ok(())
        })?
        .ok_or_else(|| {
            ApiError::new("PROJECT_REMOVED", "The launch project is no longer active")
        })?;
    Ok((
        LaunchInfo {
            project_id: id.into(),
            url: format!("{}/p/{id}/", state.info.base_url),
            launch_url: format!("{}/_platform/connect#ticket={ticket}", state.info.base_url),
            expires_at: millis() + 60000,
            instance_id: state.info.instance_id.clone(),
            document_epoch: project.document_epoch,
        },
        if window { artifact_lease } else { None },
    ))
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
            info["platform"] = crate::platform_info::info();
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
    let identity_absent = state.store.deleted_project_identity_absent(id)?;
    let mut paths = if owner_file.exists() {
        crate::web_project_cleanup::project_material_paths(&state.paths, id)?
    } else {
        let mut paths = crate::web_project_ownership::fixed_paths(state.paths.workspace_root(), id);
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
    paths.push(
        crate::webview_storage::StoreLedger::new(state.paths.data_root().into())
            .project_directory(crate::project_cleanup::ProjectKind::Web, id)?,
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
        paths.insert(0, root.clone());
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
    let already_absent = identity_absent
        && crate::web_project_cleanup::project_materials_absent(&state.paths, &state.builds, id)?;
    Ok(
        json!({"projectId":id,"instanceId":state.info.instance_id,"projectRoot":root,"expectedDocumentEpoch":epoch,"mode":mode,"maintenanceId":maintenance_id,"deletePaths":existing,"alreadyAbsent":already_absent,"cleanupOnly":identity_absent&&!already_absent}),
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
            ["projects", _, "builds" | "exports" | "copies" | "deletions"]
                | ["projects", "imports"]
                | ["workspace", "migrations" | "cleanup-plans" | "cleanups"]
                | ["workspace", "migrations", _, "cleanup"]
                | ["projects", _, "storage", "scans" | "cleanups"]
                | [
                    "projects",
                    _,
                    "git",
                    "initialize"
                        | "saves"
                        | "data-saves"
                        | "data-save-finalizations"
                        | "branches"
                        | "source-restores"
                        | "source-recoveries"
                        | "data-restores"
                        | "data-recoveries"
                ]
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
            (Method::GET, ["projects", id, "git"]) => handle.project_git_status(id).await,
            (Method::GET, ["projects", id, "git", "versions"]) => {
                let parameters = query(&request)?;
                if parameters.keys().any(|key| key != "limit") {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Only limit is accepted for local Git history.",
                    ));
                }
                let limit = parameters
                    .get("limit")
                    .map(|value| value.parse::<u32>())
                    .transpose()
                    .map_err(|_| {
                        ApiError::new("INVALID_INPUT", "limit must be an integer from 1 to 100")
                    })?
                    .unwrap_or(50);
                if !(1..=100).contains(&limit) {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "limit must be an integer from 1 to 100",
                    ));
                }
                handle.project_git_versions(id, limit).await
            }
            (Method::POST, ["projects", id, "git", "source-restore-preview"]) => {
                handle
                    .project_git_source_restore_preview(id, input(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "git", "source-restores"]) => {
                handle
                    .project_git_source_restore_apply(id, input(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "git", "source-recovery"]) => {
                if !query(&request)?.is_empty() {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Source recovery status does not accept query parameters.",
                    ));
                }
                handle.project_git_source_restore_status(id).await
            }
            (Method::POST, ["projects", id, "git", "source-recoveries"]) => {
                handle
                    .project_git_source_restore_recover(id, input(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "git", "data-restore-preview"]) => {
                handle
                    .project_git_data_restore_preview(id, input(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "git", "data-restores"]) => {
                handle
                    .project_git_data_restore_apply(id, input(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "git", "data-recovery"]) => {
                if !query(&request)?.is_empty() {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Data recovery status does not accept query parameters.",
                    ));
                }
                handle.project_git_data_restore_status(id).await
            }
            (Method::POST, ["projects", id, "git", "data-recoveries"]) => {
                handle
                    .project_git_data_restore_recover(id, input(request).await?)
                    .await
            }
            (Method::GET, ["projects", id, "git", "data-save-recovery"]) => {
                if !query(&request)?.is_empty() {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Data save recovery does not accept query parameters.",
                    ));
                }
                handle.project_git_data_save_status(id).await
            }
            (Method::POST, ["projects", id, "git", "data-save-finalizations"]) => {
                handle
                    .project_git_data_save_finalize(id, input(request).await?)
                    .await
            }
            (Method::POST, ["projects", id, "git", "tasks", task_id, "cancel"]) => {
                handle.project_git_cancel(id, task_id).await
            }
            (Method::GET, ["environment"]) => handle.environment().await,
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
                handle.set_git_path(path).await
            }
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
            (Method::POST, ["workspace", "migrations", id, "cleanup"]) => {
                if optional_json(request).await? != json!({}) {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Migration cleanup accepts only its registered migration ID",
                    ));
                }
                handle.workspace_migration_cleanup((*id).to_owned()).await
            }
            (Method::POST, ["workspace", "migrations"]) => {
                handle
                    .workspace_migrate(input::<Value>(request).await?)
                    .await
            }
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
            (Method::GET, ["projects", id, "deletion-preview"]) => {
                if !query(&request)?.is_empty() {
                    return Err(ApiError::new(
                        "INVALID_INPUT",
                        "Deletion preview does not accept query parameters.",
                    ));
                }
                web_delete_preview(&state, id)
            }
            (Method::POST, ["projects", id, "deletions"]) => {
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
            (Method::GET, ["projects", id, "maintenance"]) => handle.maintenance_status(id).await,
            (Method::GET, ["projects", id, "handoff"]) => handle.handoff(id).await,
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
    drop(sessions);
    if state.store.registered_epoch(id)?.as_deref() != Some(session_epoch.as_str()) {
        return Err(ApiError::new(
            "INVALID_SESSION",
            "This project identity is no longer active; reopen it from the platform",
        ));
    }
    let project = state.store.project(id)?;
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
        if state.store.registered_epoch(&ticket.project_id)?.as_deref()
            != Some(ticket.epoch.as_str())
        {
            return Err(ApiError::new(
                "INVALID_TICKET",
                "The launch project identity is no longer active; reopen it from the platform",
            ));
        }
        let project = state.store.project(&ticket.project_id)?;
        project.validate_epoch(Some(&ticket.epoch))?;
        let token = secret();
        state
            .store
            .with_registered_epoch(&ticket.project_id, &ticket.epoch, || {
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
                        document_epoch: ticket.epoch.clone(),
                    },
                );
                Ok(())
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "INVALID_TICKET",
                    "The launch project identity is no longer active",
                )
            })?;
        Ok((ticket.project_id, token, ticket.artifact_url))
    }
    .await;
    match outcome {
        Err(error) => result::<Value>(Err(error), None, &rid),
        Ok((id, token, artifact_url)) => {
            let path = artifact_url.unwrap_or_else(|| format!("/p/{id}/"));
            let mut response = result(Ok(json!({"projectId":id,"url":path})), Some(&id), &rid);
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
fn policy_fingerprint(requirements: &Value) -> String {
    use sha2::Digest;
    format!(
        "{:x}",
        sha2::Sha256::digest(serde_json::to_vec(requirements).unwrap_or_default())
    )
}
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
                return(StatusCode::UNAUTHORIZED,[(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; frame-ancestors 'none'")],"<!doctype html><html lang=zh-CN><meta charset=utf-8><title>需要从平台打开</title><h1>请从平台打开这个项目</h1><p>在熊猫小抽屉中打开项目，或让 AI 获取新的浏览器入口。平台重启后需要重新连接。</p></html>").into_response();
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
        let artifacts = state.builds.artifact_access();
        let (root_path, relative, base, artifact_lease) =
            if let Some(versioned) = path.as_deref().and_then(|p| p.strip_prefix("_builds/")) {
                let (build_id, relative) = versioned
                    .split_once('/')
                    .ok_or_else(|| ApiError::new("INVALID_BUILD_ID", "Missing artifact path"))?;
                let pin = artifacts.reader(&project, build_id)?;
                (
                    pin.root().to_path_buf(),
                    relative.to_owned(),
                    Some(format!("/p/{id}/_builds/{build_id}/")),
                    Some(pin),
                )
            } else {
                let (generation, pin) = artifacts.current(&project, true)?;
                if let Some(generation) = generation {
                    (generation.root, path.unwrap_or(generation.entry_relative),
                        Some(format!("/p/{id}/_builds/{}/", generation.build_id)), pin)
                } else {
                    (project.root.join(&project.manifest.output_dir),
                        path.unwrap_or_else(|| entry.to_owned()), None, pin)
                }
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
            stream_file(file, path, &headers, false, lease, artifact_lease, &state.io_lifetime).await?
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
    artifact_lease: Option<crate::build_artifacts::ArtifactLease>,
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
        let _artifact_lease = artifact_lease;
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
    let root = files::root(&project.root.join(&project.manifest.attachments_dir))?;
    stream_file(
        files::read(&root, path)?,
        path,
        headers,
        true,
        lease,
        None,
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
        .with_registered_epoch(&project.manifest.id, epoch, || {
            Ok(state
                .project_uploads
                .lock()
                .unwrap()
                .entry(project.manifest.id.clone())
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
                            Ok(change) if change.project_id==project.manifest.id=>{let data=if change.event=="project.state.changed"{let current=project.control.get();json!({"projectId":change.project_id,"mode":current.mode,"busy":project.control.frozen.load(Ordering::Acquire),"reloadRequired":project.document_epoch!=epoch||project.check().is_err_and(|e|matches!(e.code.as_str(),"PROJECT_RELOAD_REQUIRED"|"PROJECT_IDENTITY_CHANGED")),"workspaceMigrating":project.is_workspace_paused(),"removed":project.check().is_err_and(|e|e.code=="PROJECT_REMOVED")})}else{json!({"projectId":change.project_id,"requestId":change.request_id,"taskId":if change.source=="build" {Some(&change.request_id)} else {None},"source":change.source})};(change.event,data)},
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
        let admission = tokio::task::spawn_blocking(move || {
            admitted
                .store
                .with_registered_epoch(&admitted_id, &admitted_epoch, || {
                    admitted.tickets.lock().unwrap().insert(
                        "disposable-late-ticket".into(),
                        Ticket {
                            project_id: admitted_id.clone(),
                            epoch: admitted_epoch.clone(),
                            expires: Instant::now() + Duration::from_secs(60),
                            artifact_url: None,
                        },
                    );
                    admitted.sessions.lock().unwrap().insert(
                        "disposable-late-session".into(),
                        Session {
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
        let events_response = client
            .get(format!(
                "{}/p/{}/api/v1/events?documentEpoch={}",
                service.info().base_url,
                project.id,
                project.document_epoch
            ))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(events_response.status(), StatusCode::OK);
        let mut events = events_response.bytes_stream();
        let unused = service.launch(&project.id).await.unwrap();
        let ticket = unused
            .launch_url
            .split("#ticket=")
            .nth(1)
            .unwrap()
            .to_owned();
        let state = service.shared.snapshot();
        state
            .project_uploads
            .lock()
            .unwrap()
            .insert(project.id.clone(), Arc::new(Semaphore::new(1)));
        let retained_handle = service.store().project(&project.id).unwrap();
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
        let removed_event = tokio::time::timeout(Duration::from_secs(2), async {
            let mut received = String::new();
            loop {
                received.push_str(&String::from_utf8_lossy(
                    &events.next().await.unwrap().unwrap(),
                ));
                if received.contains("\"removed\":true") {
                    break received;
                }
            }
        })
        .await
        .unwrap();
        assert!(!removed_event.contains("\"trashed\""));
        drop(events);

        assert_eq!(retained_handle.check().unwrap_err().code, "PROJECT_REMOVED");
        assert!(state
            .sessions
            .lock()
            .unwrap()
            .values()
            .all(|session| session.project_id != project.id));
        assert!(state
            .tickets
            .lock()
            .unwrap()
            .values()
            .all(|ticket| ticket.project_id != project.id));
        assert!(!state
            .project_uploads
            .lock()
            .unwrap()
            .contains_key(&project.id));
        for kind in ["", "reservations", "retired", "deleting"] {
            assert!(!paths
                .workspace_root()
                .join(".platform/project-state")
                .join(kind)
                .join(format!("{}.json", project.id))
                .exists());
        }
        let base = service.info().base_url;
        assert_eq!(
            client
                .post(format!("{base}/_platform/session"))
                .header("origin", &base)
                .json(&json!({"ticket":ticket}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(format!("{base}/p/{}/api/v1/context", project.id))
                .header("cookie", &cookie)
                .header("x-platform-document-epoch", &project.document_epoch)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let recreated = create(&service, "Disposable browser deletion identity").await;
        assert_ne!(recreated.id, project.id);
        assert_eq!(
            client
                .get(format!("{base}/p/{}/api/v1/context", recreated.id))
                .header("cookie", cookie)
                .header("x-platform-document-epoch", recreated.document_epoch)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        service.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn cached_browser_session_never_reregisters_a_missing_project_identity() {
        let (_temp, paths, service, client) = start().await;
        let project = create(&service, "Disposable absent registration").await;
        let cookie = connect(&service, &client, &project.id).await;
        let registration = paths
            .workspace_root()
            .join(".platform/project-state")
            .join(format!("{}.json", project.id));
        std::fs::remove_file(&registration).unwrap();
        let base = service.info().base_url;
        let response = client
            .get(format!("{base}/p/{}/api/v1/context", project.id))
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

#[cfg(test)]
#[path = "service_object_layout_tests.rs"]
mod object_layout_tests;

#[cfg(test)]
#[path = "service_git_environment_tests.rs"]
mod git_environment_tests;

#[cfg(test)]
#[path = "service_git_remote_tests.rs"]
mod git_remote_tests;

#[cfg(test)]
#[path = "service_editor_handoff_tests.rs"]
mod editor_handoff_tests;

#[cfg(test)]
#[path = "service_source_tests.rs"]
mod source_tests;

#[cfg(test)]
#[path = "service_project_git_tests.rs"]
mod project_git_tests;

#[path = "service_node_development.rs"]
mod node_development;

#[cfg(test)]
#[path = "service_io_lifetime_tests.rs"]
mod io_lifetime_tests;

#[cfg(test)]
#[path = "service_workspace_migration_tests.rs"]
mod workspace_migration_tests;
