//! W2.1 host: fresh sample creation and authenticated read-only API v1.
//! It never constructs the full service or any recovery/background manager.
use crate::windows_projects::{BundledProject, OpenedProject, WindowsProjectStore};
use crate::{API_VERSION, APP_IDENTIFIER, ENVIRONMENT};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{any, get, post},
    Json, Router,
};
use platform_project_format::{
    http_protocol as protocol, runtime_policy::RuntimePolicy, ApiError, CreateProjectInput,
    ProjectInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpListener,
    sync::{watch, Notify},
};
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub stage: &'static str,
    pub root_status: &'static str,
    pub message: String,
    pub data_root: String,
    pub projects: Vec<ProjectInfo>,
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
    epoch: String,
}
#[derive(Default)]
struct Operations {
    stopping: bool,
    active: usize,
}
#[derive(Default)]
struct OperationState {
    counts: Mutex<Operations>,
    drained: Notify,
}
struct ActiveOperation(Arc<OperationState>);
impl Drop for ActiveOperation {
    fn drop(&mut self) {
        let mut counts = self.0.counts.lock().unwrap();
        counts.active -= 1;
        if counts.active == 0 {
            self.0.drained.notify_one();
        }
    }
}
pub struct WindowsProjectRuntime {
    store: Arc<WindowsProjectStore>,
    base_url: String,
    expected_host: String,
    instance_id: String,
    tickets: Mutex<HashMap<String, Ticket>>,
    sessions: Mutex<HashMap<String, Session>>,
    stop: watch::Sender<bool>,
    operations: Arc<OperationState>,
}

impl WindowsProjectRuntime {
    /// Opens the loopback listener only. A missing data root stays missing.
    pub async fn start(bundle: BundledProject) -> Result<Arc<Self>, ApiError> {
        let store = Arc::new(WindowsProjectStore::new(bundle)?);
        Self::start_store(store).await
    }
    async fn start_store(store: Arc<WindowsProjectStore>) -> Result<Arc<Self>, ApiError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let (stop, mut stopped) = watch::channel(false);
        let state = Arc::new(Self {
            store,
            base_url: format!("http://127.0.0.1:{port}"),
            expected_host: format!("127.0.0.1:{port}"),
            instance_id: Uuid::new_v4().to_string(),
            tickets: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            stop,
            operations: Arc::default(),
        });
        let router = Router::new()
            .route("/_platform/health", get(health))
            .route("/_platform/connect", get(connect_page))
            .route("/_platform/session", post(exchange))
            .route("/p/{id}/api/v1/{*tail}", any(project_api))
            .route("/p/{id}/", get(project_entry))
            .route("/p/{id}/{*tail}", get(project_static))
            .fallback(unavailable)
            .layer(middleware::from_fn_with_state(state.clone(), boundary))
            .with_state(state.clone());
        let stopped_on_failure = state.stop.clone();
        tokio::spawn(async move {
            if axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    while !*stopped.borrow() && stopped.changed().await.is_ok() {}
                })
                .await
                .is_err()
            {
                let _ = stopped_on_failure.send(true);
            }
        });
        Ok(state)
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    pub fn stop(&self) {
        let mut counts = self.operations.counts.lock().unwrap();
        counts.stopping = true;
        let _ = self.stop.send(true);
        self.tickets.lock().unwrap().clear();
        self.sessions.lock().unwrap().clear();
    }
    pub async fn shutdown(&self) {
        self.stop();
        loop {
            let notified = self.operations.drained.notified();
            if self.operations.counts.lock().unwrap().active == 0 {
                break;
            }
            notified.await;
        }
        // No accepted operation remains and the admission gate is closed.
        // Release the Windows instance and ancestry handles without deleting
        // the lock file or changing any retained project/control bytes.
        let _ = self.store.close();
    }
    async fn with_store<T: Send + 'static>(
        &self,
        f: impl FnOnce(&WindowsProjectStore) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let store = self.store.clone();
        {
            let mut counts = self.operations.counts.lock().unwrap();
            if counts.stopping {
                return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping")
                    .outcome("not_started"));
            }
            counts.active += 1;
        }
        let operation = ActiveOperation(self.operations.clone());
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            f(&store)
        })
        .await
        .map_err(|_| {
            ApiError::new("INTERNAL_ERROR", "Project operation did not complete").outcome("unknown")
        })?
    }
    pub async fn status(&self) -> ProjectStatus {
        let data_root = crate::fixed_workspace_data_root()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if *self.stop.borrow() {
            return ProjectStatus {
                stage: "createReadOnly",
                root_status: "blocked",
                message: "本地服务已停止，请重新启动应用。".into(),
                data_root,
                projects: vec![],
            };
        }
        let (root_status, message, projects) = match self.with_store(|s| s.list()).await {
            Ok(projects) => (
                "ready",
                "可创建并只读打开本轮 Windows 样本。".into(),
                projects,
            ),
            Err(error) if error.code == "FIXED_ROOT_MISSING" => {
                ("missing", "首次创建会初始化固定默认工作区。".into(), vec![])
            }
            Err(error) => (
                "blocked",
                format!("{} ({})", error.message, error.code),
                vec![],
            ),
        };
        ProjectStatus {
            stage: "createReadOnly",
            root_status,
            message,
            data_root,
            projects,
        }
    }
    pub async fn create(&self, input: CreateProjectInput) -> Result<ProjectInfo, ApiError> {
        if *self.stop.borrow() {
            return Err(
                ApiError::new("SERVICE_STOPPING", "Platform is stopping").outcome("not_started")
            );
        }
        self.with_store(move |store| store.create(input)).await
    }
    pub async fn launch(&self, id: &str) -> Result<LaunchInfo, ApiError> {
        if *self.stop.borrow() {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let id_owned = id.to_owned();
        let project = self.with_store(move |store| store.open(&id_owned)).await?;
        let counts = self.operations.counts.lock().unwrap();
        if counts.stopping {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let ticket = secret();
        let mut tickets = self.tickets.lock().unwrap();
        tickets.retain(|_, v| v.expires > Instant::now());
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
            url: format!("{}/p/{id}/", self.base_url),
            launch_url: format!("{}/_platform/connect#ticket={ticket}", self.base_url),
            expires_at: millis() + 60_000,
            instance_id: self.instance_id.clone(),
            document_epoch: project.document_epoch,
        })
    }
    async fn session(&self, id: &str, headers: &HeaderMap) -> Result<OpenedProject, ApiError> {
        protocol::origin(headers, &self.base_url, false)?;
        let cookie = protocol::session_cookie(headers, id)?;
        let epoch = self
            .sessions
            .lock()
            .unwrap()
            .get(cookie)
            .filter(|s| s.project_id == id)
            .map(|s| s.epoch.clone())
            .ok_or_else(|| {
                ApiError::new(
                    "INVALID_SESSION",
                    "Project session has expired; reopen from the platform",
                )
            })?;
        let id = id.to_owned();
        let project = self.with_store(move |s| s.open(&id)).await?;
        if project.document_epoch != epoch {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "Project identity changed; reopen the project",
            ));
        }
        Ok(project)
    }
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn response(result: Result<Value, ApiError>, id: Option<&str>) -> Response {
    let status = result.as_ref().err().map(protocol::status).unwrap_or(200);
    (
        StatusCode::from_u16(status).unwrap(),
        Json(protocol::envelope(result, id, &Uuid::new_v4().to_string())),
    )
        .into_response()
}
fn denied() -> ApiError {
    ApiError::new(
        "PLATFORM_STAGE_UNAVAILABLE",
        "此 Windows 阶段尚未开放此操作。",
    )
    .outcome("not_started")
}
async fn unavailable() -> Response {
    response(Err(denied()), None)
}
async fn health(State(s): State<Arc<WindowsProjectRuntime>>) -> Response {
    Json(json!({"apiVersion":API_VERSION,"instanceId":s.instance_id,"identifier":APP_IDENTIFIER,"environment":ENVIRONMENT,"stage":"createReadOnly"})).into_response()
}
async fn boundary(
    State(s): State<Arc<WindowsProjectRuntime>>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(e) = protocol::host(request.headers(), &s.expected_host) {
        return response(Err(e), None);
    }
    if *s.stop.borrow() {
        return response(
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping")),
            None,
        );
    }
    let mut result = next.run(request).await;
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
    ] {
        result.headers_mut().insert(key, value.parse().unwrap());
    }
    result
}
async fn connect_page() -> Response {
    let nonce = secret();
    let html = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>连接项目</title></head><body><h1>正在连接项目</h1><p id="message">正在建立本机项目会话…</p><script nonce="{nonce}">const ticket=new URLSearchParams(location.hash.slice(1)).get('ticket');history.replaceState(null,'',location.pathname);(async()=>{{if(!ticket)throw new Error('请从平台重新打开项目。');const response=await fetch('/_platform/session',{{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{ticket}})}});const envelope=await response.json();if(!response.ok||!envelope.ok)throw new Error(envelope.error?.message||'连接失败');location.replace(envelope.data.url);}})().catch(error=>{{document.querySelector('h1').textContent='需要重新连接';document.querySelector('#message').textContent=error.message;}});</script></body></html>"#
    );
    let mut r = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,format!("default-src 'none'; script-src 'nonce-{nonce}'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'").parse().unwrap());
    r
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    ticket: String,
}
async fn exchange(State(s): State<Arc<WindowsProjectRuntime>>, request: Request) -> Response {
    let outcome = async {
        protocol::origin(request.headers(), &s.base_url, true)?;
        if request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/json")
        {
            return Err(ApiError::new("INVALID_INPUT", "Expected application/json"));
        }
        let bytes =
            tokio::time::timeout(Duration::from_secs(10), to_bytes(request.into_body(), 4096))
                .await
                .map_err(|_| ApiError::new("REQUEST_TIMEOUT", "Session request timed out"))?
                .map_err(|_| ApiError::new("REQUEST_TOO_LARGE", "Session request exceeds limit"))?;
        let input: Exchange = platform_project_format::strict_json::from_slice(&bytes)?;
        let ticket = s
            .tickets
            .lock()
            .unwrap()
            .remove(&input.ticket)
            .filter(|v| v.expires > Instant::now())
            .ok_or_else(|| {
                ApiError::new(
                    "INVALID_TICKET",
                    "Launch ticket expired or was already used",
                )
            })?;
        let id = ticket.project_id.clone();
        let project = s.with_store(move |store| store.open(&id)).await?;
        if project.document_epoch != ticket.epoch {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "Project identity changed",
            ));
        }
        let token = secret();
        let counts = s.operations.counts.lock().unwrap();
        if counts.stopping {
            return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
        }
        let mut sessions = s.sessions.lock().unwrap();
        if sessions.len() >= 4096 {
            return Err(ApiError::new(
                "SERVICE_BUSY",
                "Too many sessions; restart platform",
            ));
        }
        sessions.insert(
            token.clone(),
            Session {
                project_id: ticket.project_id.clone(),
                epoch: ticket.epoch,
            },
        );
        Ok((ticket.project_id, token))
    }
    .await;
    match outcome {
        Err(e) => response(Err(e), None),
        Ok((id, token)) => {
            let mut r = response(
                Ok(json!({"projectId":id,"url":format!("/p/{id}/")})),
                Some(&id),
            );
            r.headers_mut().insert(
                header::SET_COOKIE,
                format!(
                    "{}={token}; Path=/p/{id}/; HttpOnly; SameSite=Strict",
                    protocol::cookie_name(&id)
                )
                .parse()
                .unwrap(),
            );
            r
        }
    }
}
async fn project_api(
    State(s): State<Arc<WindowsProjectRuntime>>,
    Path((id, tail)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    request: Request,
) -> Response {
    if request.method() != Method::GET {
        return response(Err(denied()), Some(&id));
    }
    let headers = request.headers().clone();
    drop(request);
    let setup = async {
        let project = s.session(&id, &headers).await?;
        let epoch = protocol::epoch(&headers, query.get("documentEpoch").map(String::as_str))?;
        if project.document_epoch != epoch {
            return Err(ApiError::new(
                "PROJECT_RELOAD_REQUIRED",
                "Reopen this project document",
            ));
        }
        Ok(project)
    }
    .await;
    let project = match setup {
        Ok(p) => p,
        Err(e) => return response(Err(e), Some(&id)),
    };
    match tail.as_str() {
        "context" => response(
            Ok(
                json!({"projectId":id,"name":project.manifest.name,"sdkApiVersion":API_VERSION,"instanceId":s.instance_id,"capabilities":["db.schema","events"],"runtimeRequirements":project.manifest.runtime_requirements,"runtimePolicyFingerprint":protocol::policy_fingerprint(&project.manifest.runtime_requirements),"limits":{"limitsStatus":"internalTest","fileUploadMaxBytes":0},"state":{"mode":"normal","busy":false,"reloadRequired":false,"readOnly":true}}),
            ),
            Some(&id),
        ),
        "db/schema" => {
            let key = id.clone();
            response(
                s.with_store(move |store| store.schema(&key)).await,
                Some(&id),
            )
        }
        "events" => {
            // No mutable capabilities exist yet. Heartbeats revalidate the real
            // registration and close on shutdown or identity/file changes.
            let stopped = s.stop.subscribe();
            let epoch = project.document_epoch;
            let stream = futures_util::stream::unfold(
                (s, id, epoch, stopped),
                |(s, id, epoch, mut stopped)| async move {
                    if *stopped.borrow() {
                        return None;
                    }
                    tokio::select! {
                        _=stopped.changed()=>None,
                        _=tokio::time::sleep(Duration::from_secs(3))=>{
                            let key=id.clone();
                            let observed=s.with_store(move |store|store.open(&key)).await;
                            let ok=observed.is_ok_and(|p|p.document_epoch==epoch);
                            if ok {Some((Ok::<_,Infallible>(Event::default().comment("registration checked")),(s,id,epoch,stopped)))} else {
                                // Next iteration terminates instead of reconnecting a
                                // stale page as though it still had a healthy project.
                                None
                            }
                        }
                    }
                },
            );
            Sse::new(stream)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
        _ => response(Err(denied()), Some(&id)),
    }
}
async fn project_entry(
    State(s): State<Arc<WindowsProjectRuntime>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_static(s, id, None, headers).await
}
async fn project_static(
    State(s): State<Arc<WindowsProjectRuntime>>,
    Path((id, tail)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    serve_static(s, id, Some(tail), headers).await
}
async fn serve_static(
    s: Arc<WindowsProjectRuntime>,
    id: String,
    tail: Option<String>,
    headers: HeaderMap,
) -> Response {
    let result=async {
        let project=s.session(&id,&headers).await?;
        let build_id=&project.starter.build_id;
        let path=tail.as_deref().unwrap_or("index.html");
        if path=="api" || path.starts_with("api/") {return Err(ApiError::new("NOT_FOUND","Unknown API route"));}
        let relative=if let Some(versioned)=path.strip_prefix("_builds/") {
            let (version,relative)=versioned.split_once('/').ok_or_else(||ApiError::new("NOT_FOUND","Missing artifact path"))?;
            if version!=build_id {return Err(ApiError::new("NOT_FOUND","Unknown artifact generation"));}
            relative
        } else {path};
        let key=id.clone();let logical=relative.to_owned();
        let bytes=s.with_store(move |store|store.read_static(&key,&logical)).await?;
        let policy=RuntimePolicy::parse(&project.manifest.runtime_requirements)?;
        let mut csp=policy.content_security_policy();
        let mime=mime_guess::from_path(relative).first_or_octet_stream();
        let mut r=if mime.essence_str()=="text/html" {
            let mut html=String::from_utf8(bytes).map_err(|_|ApiError::new("INVALID_PROJECT","HTML must be UTF-8"))?;
            let nonce=secret();
            csp=csp.replace("script-src 'self'",&format!("script-src 'self' 'nonce-{nonce}'"));
            let meta=format!("<base href=\"/p/{id}/_builds/{build_id}/\"><meta name=\"platform-document-epoch\" content=\"{}\"><meta name=\"platform-runtime-policy\" content=\"{}\"><script nonce=\"{nonce}\">{}</script>",project.document_epoch,protocol::policy_fingerprint(&project.manifest.runtime_requirements),platform_project_format::runtime_notice::script());
            let lower=html.to_ascii_lowercase();
            let position=lower.find("<head").and_then(|start|lower[start..].find('>').map(|end|start+end+1)).unwrap_or(0);
            html.insert_str(position,&meta);
            ([(header::CONTENT_TYPE,"text/html; charset=utf-8")],html).into_response()
        } else {( [(header::CONTENT_TYPE,mime.as_ref())],Body::from(bytes)).into_response()};
        r.headers_mut().insert(header::CONTENT_SECURITY_POLICY,csp.parse().unwrap());
        r.headers_mut().insert("permissions-policy", "camera=(), microphone=(), geolocation=(), display-capture=()".parse().unwrap());
        Ok(r)
    }.await;
    result.unwrap_or_else(|e| response(Err(e), Some(&id)))
}

/// The caller passes only freshly issued runtime launch URLs. Revalidate here
/// before invoking the system browser; no command shell parses the URL.
pub fn open_project_browser(value: &str) -> Result<(), ApiError> {
    use std::os::windows::ffi::OsStrExt;
    let url = url::Url::parse(value)
        .map_err(|_| ApiError::new("INVALID_INPUT", "Invalid project launch URL"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/_platform/connect"
        || url.query().is_some()
        || !url.fragment().is_some_and(|v| {
            v.starts_with("ticket=")
                && v.len() == 71
                && v[7..].bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(ApiError::new("INVALID_INPUT", "Invalid project launch URL"));
    }
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let outcome = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if outcome as isize <= 32 {
        return Err(ApiError::new(
            "BROWSER_OPEN_FAILED",
            "系统浏览器未能打开项目，请重试。",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn fixture() -> PathBuf {
        let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let parent = artifacts.join(format!(
            "windows-workspace-disposable-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&parent).unwrap();
        parent
    }
    async fn connect(
        runtime: &WindowsProjectRuntime,
        client: &reqwest::Client,
        id: &str,
    ) -> (String, String) {
        let launch = runtime.launch(id).await.unwrap();
        let ticket = launch
            .launch_url
            .split("#ticket=")
            .nth(1)
            .unwrap()
            .to_owned();
        let response = client
            .post(format!("{}/_platform/session", runtime.base_url))
            .header("origin", &runtime.base_url)
            .json(&json!({"ticket":ticket}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        (cookie.split(';').next().unwrap().to_owned(), ticket)
    }

    #[test]
    fn real_loopback_sessions_schema_static_denials_and_restart() {
        let _serial = crate::WINDOWS_FILESYSTEM_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let parent = fixture();
                let store = Arc::new(
                    WindowsProjectStore::for_test(
                        crate::windows_projects::test_bundle(),
                        parent.clone(),
                    )
                    .unwrap(),
                );
                let runtime = WindowsProjectRuntime::start_store(store).await.unwrap();
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(10))
                    .build()
                    .unwrap();
                assert_eq!(runtime.status().await.root_status, "missing");
                assert!(!parent.join(APP_IDENTIFIER).exists());
                let a = runtime
                    .create(CreateProjectInput {
                        name: "Disposable HTTP sample A".into(),
                        description: None,
                    })
                    .await
                    .unwrap();
                let b = runtime
                    .create(CreateProjectInput {
                        name: "Disposable HTTP sample B".into(),
                        description: None,
                    })
                    .await
                    .unwrap();
                let base = &runtime.base_url;
                assert_eq!(
                    client
                        .get(format!("{base}/_platform/health"))
                        .header("host", "localhost:1")
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    403
                );
                let (cookie, used) = connect(&runtime, &client, &a.id).await;
                let repeat: Value = client
                    .post(format!("{base}/_platform/session"))
                    .header("origin", base)
                    .json(&json!({"ticket":used}))
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(repeat["error"]["code"], "INVALID_TICKET");
                let alien = runtime.launch(&a.id).await.unwrap();
                let alien_ticket = alien.launch_url.split("#ticket=").nth(1).unwrap();
                assert_eq!(
                    client
                        .post(format!("{base}/_platform/session"))
                        .header("origin", "https://example.com")
                        .json(&json!({"ticket":alien_ticket}))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    403
                );
                let endpoint = format!("{base}/p/{}/api/v1", a.id);
                let context: Value = client
                    .get(format!("{endpoint}/context"))
                    .header("cookie", &cookie)
                    .header("x-platform-document-epoch", &a.document_epoch)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(context["apiVersion"], 1);
                assert_eq!(
                    context["data"]["capabilities"],
                    json!(["db.schema", "events"])
                );
                assert_eq!(context["data"]["state"]["readOnly"], true);
                assert!(context["data"].get("projectRoot").is_none());
                let wrong: Value = client
                    .get(format!("{endpoint}/db/schema"))
                    .header("cookie", &cookie)
                    .header("x-platform-document-epoch", "old")
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(wrong["error"]["code"], "PROJECT_RELOAD_REQUIRED");
                let schema: Value = client
                    .get(format!("{endpoint}/db/schema"))
                    .header("cookie", &cookie)
                    .header("x-platform-document-epoch", &a.document_epoch)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(schema["data"]["tables"], json!([]));
                let cross: Value = client
                    .get(format!("{base}/p/{}/api/v1/context", b.id))
                    .header("cookie", &cookie)
                    .header("x-platform-document-epoch", &b.document_epoch)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(cross["error"]["code"], "INVALID_SESSION");
                let duplicated: Value = client
                    .get(format!("{endpoint}/context"))
                    .header("cookie", format!("{cookie}; {cookie}"))
                    .header("x-platform-document-epoch", &a.document_epoch)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(duplicated["error"]["code"], "INVALID_SESSION");
                let html = client
                    .get(format!("{base}/p/{}/", a.id))
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(html.status(), 200);
                assert!(html.headers()["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .contains("frame-ancestors 'none'"));
                assert_eq!(
                    html.headers()["permissions-policy"],
                    "camera=(), microphone=(), geolocation=(), display-capture=()"
                );
                assert_eq!(html.headers()["cache-control"], "no-store");
                let html = html.text().await.unwrap();
                assert!(html.contains(&format!("content=\"{}\"", a.document_epoch)));
                assert!(html.contains("/_builds/starter_"));
                let db_path = a.project_root.join("data/main.sqlite");
                let before = fs::read(&db_path).unwrap();
                for path in [
                    format!("{endpoint}/db/execute"),
                    format!("{endpoint}/db/transaction"),
                    format!("{endpoint}/file?path=a.txt"),
                    format!("{base}/api/v1/projects"),
                    format!("{base}/api/v1/projects/{}/build", a.id),
                ] {
                    let result: Value = client
                        .post(path)
                        .header("cookie", &cookie)
                        .json(&json!({"sql":"CREATE TABLE forbidden(x)"}))
                        .send()
                        .await
                        .unwrap()
                        .json()
                        .await
                        .unwrap();
                    assert_eq!(result["error"]["code"], "PLATFORM_STAGE_UNAVAILABLE");
                    assert_eq!(
                        result["error"]["details"]["operationOutcome"],
                        "not_started"
                    );
                }
                assert_eq!(fs::read(&db_path).unwrap(), before);
                assert!(!a.project_root.join("data/main.sqlite-wal").exists());
                assert!(!a.project_root.join("data/main.sqlite-shm").exists());
                let sse = client
                    .get(format!(
                        "{endpoint}/events?documentEpoch={}",
                        a.document_epoch
                    ))
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(sse.headers()["content-type"], "text/event-stream");
                drop(sse);
                runtime.shutdown().await;
                assert_eq!(runtime.status().await.root_status, "blocked");
                assert!(runtime.launch(&a.id).await.is_err());
                let next = WindowsProjectRuntime::start_store(Arc::new(
                    WindowsProjectStore::for_test(crate::windows_projects::test_bundle(), parent)
                        .unwrap(),
                ))
                .await
                .unwrap();
                assert_eq!(next.status().await.projects.len(), 2);
                let stale: Value = client
                    .get(format!("{}/p/{}/api/v1/context", next.base_url, a.id))
                    .header("cookie", &cookie)
                    .header("x-platform-document-epoch", &a.document_epoch)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert_eq!(stale["error"]["code"], "INVALID_SESSION");
                let (fresh, _) = connect(&next, &client, &a.id).await;
                assert_ne!(cookie, fresh);
                next.shutdown().await;
            });
    }

    #[tokio::test]
    async fn shutdown_waits_for_an_accepted_operation_and_refuses_new_work() {
        let runtime = WindowsProjectRuntime::start(crate::windows_projects::test_bundle())
            .await
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = runtime.clone();
        let task = tokio::spawn(async move {
            worker
                .with_store(move |_| {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        let closing = runtime.clone();
        let closing = tokio::spawn(async move { closing.shutdown().await });
        tokio::task::yield_now().await;
        assert!(!closing.is_finished());
        assert_eq!(
            runtime.with_store(|_| Ok(())).await.unwrap_err().code,
            "SERVICE_STOPPING"
        );
        release_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
        closing.await.unwrap();
    }
}
