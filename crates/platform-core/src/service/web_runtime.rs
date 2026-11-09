//! One fixed-origin HTTP service and owned close coordinator per web instance.
use super::*;
use crate::{projects::ProjectTaskLease, shutdown::ShutdownIssue};
use futures_util::FutureExt;
use std::{collections::BTreeMap, panic::AssertUnwindSafe};

#[path = "web_runtime/http.rs"]
mod http;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeStatus {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
    pub resource_closed: bool,
}
impl RuntimeStatus {
    fn closed() -> Self {
        Self {
            state: "closed",
            error: None,
            resource_closed: true,
        }
    }
}
#[derive(Default)]
struct Coordination {
    windows: usize,
    start: Option<JoinHandle<()>>,
    close: Option<JoinHandle<()>>,
    health: Option<JoinHandle<()>>,
}
#[derive(Default)]
struct Resources {
    child: Option<Arc<HttpState>>,
    project: Option<ProjectHandle>,
    store: Option<Arc<ProjectStore>>,
    lease: Option<ProjectTaskLease>,
    server: Option<JoinHandle<std::io::Result<()>>>,
    issues: Vec<ShutdownIssue>,
    backend: Option<crate::node_runtime_trial::NodeRuntimeTrial>,
    forwarder: Option<Arc<crate::node_app_proxy::BackendForwarder>>,
    run_guard: Option<crate::backend_run_marker::RunGuard>,
}
struct Instance {
    id: String,
    executor: tokio::runtime::Handle,
    report: watch::Sender<RuntimeStatus>,
    resources: tokio::sync::Mutex<Resources>,
    coordination: Mutex<Coordination>,
    close_requested: AtomicBool,
    retry: Notify,
    policy: Mutex<Option<crate::runtime_policy::RuntimePolicy>>,
    failure: Mutex<Option<ApiError>>,
}
pub(super) struct WindowToken {
    instance: Arc<Instance>,
}
impl Drop for WindowToken {
    fn drop(&mut self) {
        let last = {
            let mut state = self
                .instance
                .coordination
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.windows = state.windows.saturating_sub(1);
            if state.windows == 0 {
                self.instance.close_requested.store(true, Ordering::Release);
                true
            } else {
                false
            }
        };
        if last {
            self.instance.request_close();
        }
    }
}
struct LaunchWaiter {
    instance: Arc<Instance>,
    armed: bool,
}
impl Drop for LaunchWaiter {
    fn drop(&mut self) {
        if self.armed && !self.instance.status().resource_closed {
            self.instance.request_close();
        }
    }
}
pub(super) struct WebRuntimeManager {
    entries: Mutex<BTreeMap<String, Arc<Instance>>>,
    gates: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    paused: AtomicBool,
    stopping: AtomicBool,
}
impl WebRuntimeManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(BTreeMap::new()),
            gates: Mutex::new(BTreeMap::new()),
            paused: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        })
    }
    fn require_open(&self, parent: &HttpState) -> std::result::Result<(), ApiError> {
        if self.stopping.load(Ordering::Acquire) || parent.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "平台正在停止，无法打开应用。",
            ));
        }
        if self.paused.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "WORKSPACE_MIGRATING",
                "工作区正在切换，暂时无法打开应用。",
            ));
        }
        parent.store.check_workspace_available()
    }
    pub async fn open(
        self: &Arc<Self>,
        parent: Arc<HttpState>,
        id: &str,
        window: bool,
    ) -> std::result::Result<(LaunchInfo, Option<WebWindowLease>), ApiError> {
        self.require_open(&parent)?;
        let gate = self
            .gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _gate = gate.lock().await;
        self.require_open(&parent)?;
        let existing = self.entries.lock().unwrap().get(id).cloned();
        let (instance, created) = if let Some(instance) = existing {
            let report = instance.status();
            if report.state == "closing" {
                instance.wait_closed().await?;
                instance.join_close().await?;
            }
            let report = instance.status();
            if report.state == "running" || report.state == "starting" {
                (instance, false)
            } else if !report.resource_closed {
                return Err(instance.close_error());
            } else {
                instance.request_close();
                instance.wait_closed().await?;
                instance.join_close().await?;
                (self.spawn(parent.clone(), id), true)
            }
        } else {
            (self.spawn(parent.clone(), id), true)
        };
        let mut waiter = LaunchWaiter {
            instance: instance.clone(),
            armed: created,
        };
        instance.wait_running().await?;
        self.require_open(&parent)?;
        let launch = instance.launch(window).await?;
        waiter.armed = false;
        Ok(launch)
    }
    fn spawn(&self, parent: Arc<HttpState>, id: &str) -> Arc<Instance> {
        let (report, _) = watch::channel(RuntimeStatus {
            state: "starting",
            error: None,
            resource_closed: false,
        });
        let instance = Arc::new(Instance {
            id: id.into(),
            executor: tokio::runtime::Handle::current(),
            report,
            resources: tokio::sync::Mutex::new(Resources::default()),
            coordination: Mutex::new(Coordination::default()),
            close_requested: AtomicBool::new(false),
            retry: Notify::new(),
            policy: Mutex::new(None),
            failure: Mutex::new(None),
        });
        // Publish acceptance and the real startup handle under one admission
        // lock. A concurrent stop cannot miss an already accepted startup.
        let mut coordination = instance.coordination.lock().unwrap();
        self.entries
            .lock()
            .unwrap()
            .insert(id.into(), instance.clone());
        let owner = instance.clone();
        let start = tokio::spawn(async move {
            let result = AssertUnwindSafe(owner.start(parent))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(ApiError::new(
                        "WEB_RUNTIME_START_PANICKED",
                        "网页服务启动中断，正在核对资源。",
                    ))
                });
            if let Err(error) = result {
                let cleanup = AssertUnwindSafe(owner.close_resources())
                    .catch_unwind()
                    .await;
                let closed = matches!(cleanup, Ok(Ok(())));
                let error = match cleanup {
                    Ok(Err(issue)) => error.detail("closeError", json!(issue)),
                    Err(_) => error.detail("closePanicked", true),
                    _ => error,
                };
                owner.report.send_replace(RuntimeStatus {
                    state: "failed",
                    error: Some(error.detail("resourceClosed", closed)),
                    resource_closed: closed,
                });
            } else if owner.close_requested.load(Ordering::Acquire) {
                owner.request_close();
            }
        });
        coordination.start = Some(start);
        drop(coordination);
        instance
    }
    pub fn device_declared(&self, id: &str, device: &str) -> bool {
        let entry = self.entries.lock().unwrap().get(id).cloned();
        entry.is_some_and(|entry| {
            if entry.status().state != "running" || entry.close_requested.load(Ordering::Acquire) {
                return false;
            }
            let policy = entry.policy.lock().unwrap();
            policy.as_ref().is_some_and(|policy| match device {
                "camera" => policy.devices().camera.is_some(),
                "microphone" => policy.devices().microphone.is_some(),
                _ => false,
            })
        })
    }
    #[cfg(test)]
    pub async fn fixture_http(&self, id: &str) -> Arc<HttpState> {
        let instance = self.entries.lock().unwrap().get(id).cloned().unwrap();
        let child = instance
            .resources
            .lock()
            .await
            .child
            .as_ref()
            .unwrap()
            .clone();
        child
    }
    pub fn status(&self, id: &str) -> Value {
        json!(self
            .entries
            .lock()
            .unwrap()
            .get(id)
            .map(|entry| entry.status())
            .unwrap_or_else(RuntimeStatus::closed))
    }
    pub fn mark_failed(&self, id: &str, error: ApiError) {
        if let Some(instance) = self.entries.lock().unwrap().get(id).cloned() {
            let closed = instance.status().resource_closed;
            *instance.failure.lock().unwrap() = Some(error.clone());
            instance.report.send_replace(RuntimeStatus {
                state: "failed",
                error: Some(error),
                resource_closed: closed,
            });
        }
    }
    pub async fn close(&self, id: &str) -> std::result::Result<Value, ApiError> {
        let entry = self.entries.lock().unwrap().get(id).cloned();
        match entry {
            None => Ok(json!(RuntimeStatus::closed())),
            Some(entry) => {
                entry.request_close();
                entry.wait_closed().await?;
                entry.join_close().await?;
                Ok(json!(entry.status()))
            }
        }
    }
    pub fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        for entry in self.entries.lock().unwrap().values() {
            entry.request_close();
        }
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }
    pub fn resume(&self) {
        if !self.stopping.load(Ordering::Acquire) {
            self.paused.store(false, Ordering::Release);
        }
    }
    pub async fn close_all(&self) -> std::result::Result<(), ApiError> {
        let ids = self
            .entries
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut errors = vec![];
        for id in ids {
            if let Err(issue) = self.close(&id).await {
                errors.push(issue);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(
                ApiError::new("WEB_RUNTIME_CLOSE_FAILED", "部分网页服务尚未完成关闭。")
                    .detail("issues", json!(errors))
                    .detail("resourceClosed", false),
            )
        }
    }
    pub async fn shutdown(&self) -> Vec<ShutdownIssue> {
        self.request_stop();
        self.close_all()
            .await
            .err()
            .map(|issue| {
                vec![ShutdownIssue::new(
                    "webRuntime",
                    &issue.code,
                    issue.message,
                    true,
                    false,
                )]
            })
            .unwrap_or_default()
    }
    pub fn forget(&self, id: &str) {
        if let Some(entry) = self.entries.lock().unwrap().remove(id) {
            entry.request_close();
        }
        self.gates.lock().unwrap().remove(id);
    }
}
impl Drop for WebRuntimeManager {
    fn drop(&mut self) {
        self.request_stop();
    }
}
impl Instance {
    fn status(&self) -> RuntimeStatus {
        self.report.borrow().clone()
    }
    fn close_error(&self) -> ApiError {
        let status = self.status();
        status
            .error
            .clone()
            .unwrap_or_else(|| {
                ApiError::new(
                    "WEB_RUNTIME_CLOSING",
                    "网页应用仍在关闭，请等待资源释放后重试。",
                )
            })
            .detail("webRuntime", json!(status))
            .detail("resourceClosed", status.resource_closed)
    }
    async fn wait_running(&self) -> std::result::Result<(), ApiError> {
        let mut changes = self.report.subscribe();
        loop {
            let status = changes.borrow().clone();
            match status.state {
                "running" => return Ok(()),
                "failed" | "closed" => return Err(self.close_error()),
                _ => {}
            }
            changes
                .changed()
                .await
                .map_err(|_| ApiError::new("WEB_RUNTIME_UNAVAILABLE", "网页服务状态已中断。"))?;
        }
    }
    async fn wait_closed(&self) -> std::result::Result<(), ApiError> {
        let mut changes = self.report.subscribe();
        loop {
            let status = changes.borrow().clone();
            match status.state {
                "closed" => return Ok(()),
                "failed" if status.resource_closed => return Ok(()),
                "failed" => return Err(self.close_error()),
                _ => {}
            }
            changes.changed().await.map_err(|_| {
                ApiError::new("WEB_RUNTIME_UNAVAILABLE", "网页服务关闭状态已中断。")
            })?;
        }
    }
    async fn start(self: &Arc<Self>, parent: Arc<HttpState>) -> std::result::Result<(), ApiError> {
        let mut resources = self.resources.lock().await;
        let mut project = parent.store.project(&self.id)?;
        parent.store.require_web_configuration_ready(&project)?;
        *self.policy.lock().unwrap() = Some(crate::runtime_policy::RuntimePolicy::parse(&json!(
            project.manifest.runtime_requirements
        ))?);
        validate_entry_format(&project.manifest.web.entry)?;
        project
            .manifest
            .resolve_web_entry(&project.root)
            .map_err(|error| ApiError::new("WEB_ENTRY_MISSING", error.to_string()))?;
        crate::web_files::open(
            &project.root,
            &project.manifest,
            &project.manifest.web.entry,
        )?;
        resources.lease = Some(parent.store.reserve_web_open(&self.id)?);
        resources.store = Some(parent.store.clone());
        resources.project = Some(project.clone());
        crate::backend_run_marker::recover(&project.private_dir, &self.id).await?;
        let listener = project.address.bind().await?;
        let origin = project.address.origin();
        let mut child = HttpState::web_child(&parent, &origin);
        if project.manifest.has_backend() {
            let resources_owner = parent.resources.as_ref().ok_or_else(|| {
                ApiError::new(
                    "INSTALLATION_RESOURCES_MISSING",
                    "应用后台运行资源不可用，请修复安装。",
                )
            })?;
            let snapshot = crate::web_app_configuration::store(&project)?
                .launch_snapshot(&project.manifest)
                .map_err(|error| ApiError::new("APP_CONFIGURATION_INVALID", error.to_string()))?;
            let (entry_path, working_directory) =
                project
                    .manifest
                    .resolve_backend(&project.root)
                    .map_err(|error| ApiError::new("APP_BACKEND_INVALID", error.to_string()))?;
            let runs = project.private_dir.join("runs");
            crate::project_file_io::ensure_directory(&runs)?;
            let trial_root = runs.join(format!("node-runtime-trial-{}", Uuid::new_v4()));
            crate::project_file_io::ensure_directory(&trial_root)?;
            let cache = project.private_dir.join("cache");
            crate::project_file_io::ensure_directory(&cache)?;
            let digest = if crate::build_profile::PRODUCTION_PROFILE {
                crate::production_resources::runtime_node_digest()?
            } else {
                serde_json::from_str::<Value>(crate::node_tool_layout::RUNTIME_RELEASE)?["binary"]
                    ["sha256"]
                    .as_str()
                    .ok_or_else(|| {
                        ApiError::new("APP_RUNTIME_INVALID", "后台 Node 校验信息缺失。")
                    })?
                    .to_owned()
            };
            let config = crate::node_runtime_trial::RuntimeLaunchConfig {
                node_path: resources_owner.runtime_node(),
                expected_node_sha256: digest,
                entry_path,
                working_directory,
                trial_root: trial_root.clone(),
                business_env: snapshot.environment,
                control_directory: project.private_dir.clone(),
                redaction_values: snapshot.redaction_values,
            };
            resources.run_guard = Some(crate::backend_run_marker::RunGuard::begin(
                &project.private_dir,
                &self.id,
                trial_root,
            )?);
            let (result, pending, cleanup) =
                crate::node_runtime_trial::NodeRuntimeTrial::launch_project_with_progress(
                    config,
                    project.root.clone(),
                    cache,
                    &parent.store.project_processes(&self.id)?,
                    crate::node_runtime_trial::RuntimeStartupProgress::new(),
                )
                .await;
            resources.backend = pending;
            if result.is_err() && cleanup.permits_intent_removal() {
                if let Some(guard) = resources.run_guard.take() {
                    guard.retire()?;
                }
            }
            let runtime = result.map_err(|mut error| {
                error.message = crate::node_runtime_trial::startup_failure_explanation(&error);
                error.detail(
                    "logPath",
                    json!(project.private_dir.join("logs/current.log")),
                )
            })?;
            let (port, token) = runtime.proxy_target()?;
            resources.backend = Some(runtime);
            let Resources {
                run_guard, backend, ..
            } = &mut *resources;
            if let (Some(guard), Some(runtime)) = (run_guard.as_mut(), backend.as_ref()) {
                guard.bind(runtime)?;
            }
            let forwarder = Arc::new(crate::node_app_proxy::BackendForwarder::new(
                &project, port, token,
            )?);
            child.backend = Some(forwarder.clone());
            resources.forwarder = Some(forwarder);
        }
        project = parent
            .store
            .commit_started_manifest(&self.id, &project.manifest)?;
        resources.project = Some(project.clone());
        let child = Arc::new(child);
        let router = http::router(child.clone(), project.id.clone());
        let mut stop = child.stop.subscribe();
        resources.child = Some(child);
        resources.server = Some(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    if !*stop.borrow() {
                        let _ = stop.changed().await;
                    }
                })
                .await
        }));
        self.report.send_if_modified(|report| {
            if self.close_requested.load(Ordering::Acquire) {
                return false;
            }
            *report = RuntimeStatus {
                state: "running",
                error: None,
                resource_closed: false,
            };
            true
        });
        if resources.backend.is_some() {
            let owner = self.clone();
            self.coordination.lock().unwrap().health = Some(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if owner.close_requested.load(Ordering::Acquire) {
                        return;
                    }
                    let failure = {
                        let mut resources = owner.resources.lock().await;
                        if owner.close_requested.load(Ordering::Acquire) {
                            return;
                        }
                        match resources.backend.as_mut() {
                            Some(runtime) => match runtime.status() {
                                Ok(status) if status.state == "running" => {
                                    runtime.verify_health().await.err()
                                }
                                Ok(_) => Some(ApiError::new(
                                    "APP_BACKEND_EXITED",
                                    "后台意外退出，请查看日志后重新加载。",
                                )),
                                Err(error) => Some(error),
                            },
                            None => return,
                        }
                    };
                    if let Some(error) = failure {
                        *owner.failure.lock().unwrap() = Some(error.clone());
                        owner.report.send_replace(RuntimeStatus {
                            state: "failed",
                            error: Some(error),
                            resource_closed: false,
                        });
                        owner.request_close();
                        return;
                    }
                }
            }));
        }
        Ok(())
    }
    async fn launch(
        self: &Arc<Self>,
        window: bool,
    ) -> std::result::Result<(LaunchInfo, Option<WebWindowLease>), ApiError> {
        let resources = self.resources.lock().await;
        let child = resources.child.as_ref().ok_or_else(|| self.close_error())?;
        let project = resources
            .project
            .as_ref()
            .ok_or_else(|| self.close_error())?;
        project.validate_epoch(Some(&project.document_epoch))?;
        let token = secret();
        let lease = child
            .store
            .with_registered_epoch(&self.id, &project.document_epoch, || {
                // Match identity-removal callback lock ordering: registration first,
                // then runtime admission. No late ticket can revive a removed id.
                let mut coordination = self.coordination.lock().unwrap_or_else(|e| e.into_inner());
                if self.close_requested.load(Ordering::Acquire) || self.status().state != "running"
                {
                    return Err(self.close_error());
                }
                let mut tickets = child.tickets.lock().unwrap();
                tickets.retain(|_, ticket| ticket.expires > Instant::now());
                if tickets.len() >= 1024 {
                    return Err(ApiError::new(
                        "SERVICE_BUSY",
                        "未使用的网页入口过多，请稍后重试。",
                    ));
                }
                tickets.insert(
                    token.clone(),
                    Ticket {
                        project: project.clone(),
                        project_id: self.id.clone(),
                        epoch: project.document_epoch.clone(),
                        expires: Instant::now() + Duration::from_secs(60),
                    },
                );
                Ok(if window {
                    coordination.windows += 1;
                    Some(WebWindowLease {
                        _token: WindowToken {
                            instance: self.clone(),
                        },
                    })
                } else {
                    None
                })
            })?
            .ok_or_else(|| {
                ApiError::new("PROJECT_REMOVED", "应用登记已移除，不能创建新的入口。")
            })?;
        Ok((
            LaunchInfo {
                project_id: self.id.clone(),
                url: format!("{}/", child.info.base_url),
                launch_url: format!("{}/_platform/bootstrap?token={token}", child.info.base_url),
                expires_at: millis() + 60000,
                instance_id: child.info.instance_id.clone(),
                document_epoch: project.document_epoch.clone(),
            },
            lease,
        ))
    }
    fn signal_child(resources: &Resources) {
        if let Some(forwarder) = &resources.forwarder {
            forwarder.request_stop();
        }
        if let Some(child) = &resources.child {
            child.stopping.store(true, Ordering::Release);
            child.io_lifetime.seal();
            let _ = child.stop.send(true);
            child
                .tickets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            child
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
    }
    fn request_close(self: &Arc<Self>) {
        self.close_requested.store(true, Ordering::Release);
        if self.status().resource_closed {
            return;
        }
        if let Ok(resources) = self.resources.try_lock() {
            Self::signal_child(&resources);
        }
        self.report.send_replace(RuntimeStatus {
            state: "closing",
            error: None,
            resource_closed: false,
        });
        let mut coordination = self.coordination.lock().unwrap_or_else(|e| e.into_inner());
        if coordination.close.is_none() {
            let owner = self.clone();
            // Native window callbacks can release the last lease outside Tokio.
            coordination.close = Some(self.executor.spawn(async move {
                owner.close_loop().await;
            }));
        } else {
            self.retry.notify_one();
        }
    }
    async fn close_loop(self: Arc<Self>) {
        let start = self
            .coordination
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .start
            .take();
        if let Some(start) = start {
            if let Err(issue) = start.await {
                self.resources
                    .lock()
                    .await
                    .issues
                    .push(ShutdownIssue::join("webRuntimeStart", issue));
            }
        }
        let health = self
            .coordination
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .health
            .take();
        if let Some(health) = health {
            let _ = health.await;
        }
        loop {
            let result = AssertUnwindSafe(self.close_resources())
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(ApiError::new(
                        "WEB_RUNTIME_CLOSE_PANICKED",
                        "网页服务收尾中断，资源已保留。",
                    ))
                });
            match result {
                Ok(()) => {
                    let failure = self.failure.lock().unwrap().clone();
                    self.report.send_replace(if let Some(error) = failure {
                        RuntimeStatus {
                            state: "failed",
                            error: Some(error),
                            resource_closed: true,
                        }
                    } else {
                        RuntimeStatus::closed()
                    });
                    return;
                }
                Err(error) => {
                    self.report.send_replace(RuntimeStatus {
                        state: "failed",
                        error: Some(error.detail("resourceClosed", false)),
                        resource_closed: false,
                    });
                }
            }
            self.retry.notified().await;
            self.report.send_replace(RuntimeStatus {
                state: "closing",
                error: None,
                resource_closed: false,
            });
        }
    }
    async fn close_resources(&self) -> std::result::Result<(), ApiError> {
        let mut resources = self.resources.lock().await;
        Self::signal_child(&resources);
        if let Some(server) = resources.server.take() {
            match server.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => resources.issues.push(ShutdownIssue::new(
                    "webHttp",
                    "WEB_HTTP_FAILED",
                    error.to_string(),
                    false,
                    false,
                )),
                Err(error) => resources.issues.push(ShutdownIssue::join("webHttp", error)),
            }
        }
        if let Some(child) = &resources.child {
            child.io_lifetime.drain().await;
            let issues = child.io_lifetime.shutdown_issues();
            resources.issues.extend(issues);
        }
        if !resources.issues.is_empty() {
            return Err(ApiError::new(
                "WEB_RUNTIME_CLOSE_FAILED",
                "网页请求或文件资源尚未确认关闭。",
            )
            .detail("issues", json!(resources.issues))
            .detail("resourceClosed", false));
        }
        if let Some(forwarder) = &resources.forwarder {
            forwarder.stop().await?;
        }
        resources.forwarder = None;
        if let Some(backend) = resources.backend.as_mut() {
            let stopped = backend
                .stop_with_reason(crate::node_runtime_trial::RuntimeStopReason::WindowClosed)
                .await?;
            if !stopped.process_group_exited {
                return Err(ApiError::new(
                    "APP_BACKEND_STOP_FAILED",
                    "尚未确认后台进程已停止，已保留运行资源。",
                ));
            }
        }
        if let Some(guard) = resources.run_guard.take() {
            guard.retire()?;
        }
        if let Some(project) = &resources.project {
            crate::backend_run_marker::recover(&project.private_dir, &self.id).await?;
        }
        resources.backend = None;
        // The writer barrier also covers CLI SQL. Release has_web_open while
        // still holding it, so no later CLI admission can retain a new idle DB
        // in the gap between closing the old worker and declaring Closed.
        if let Some(store) = resources.store.clone() {
            let gate = store.gate(&self.id);
            let _drain = gate.write().await;
            store.close_database(&self.id).await?;
            resources.lease = None;
        }
        *self.policy.lock().unwrap() = None;
        resources.child = None;
        resources.project = None;
        resources.store = None;
        resources.lease = None;
        Ok(())
    }
    async fn join_close(&self) -> std::result::Result<(), ApiError> {
        let close = self
            .coordination
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .close
            .take();
        if let Some(close) = close {
            close.await.map_err(|issue| {
                ApiError::new("WEB_RUNTIME_CLOSE_PANICKED", issue.to_string())
                    .detail("resourceClosed", false)
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "web_runtime/tests.rs"]
mod tests;
