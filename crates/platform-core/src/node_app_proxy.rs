//! Private per-application loopback web adapter. It exposes no platform management
//! API, never forwards the browser cookie, and keeps the backend bearer in Rust.
use crate::{io_lifetime::IoLease, ApiError};
#[cfg(test)]
#[path = "node_app_proxy_api_tests.rs"]
mod api_tests;
#[cfg(all(test, unix))]
#[path = "node_app_proxy/unix_files.rs"]
mod file_io;
#[cfg(all(test, windows))]
#[path = "node_app_proxy/windows_files.rs"]
mod file_io;
#[path = "node_app_proxy_shutdown.rs"]
mod shutdown;
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
#[cfg(test)]
use file_io::{open_file, open_root};
use futures_util::{stream, StreamExt};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use hyper_util::rt::TokioIo;
use shutdown::ProxyShutdown;
use std::{
    fs::File,
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpListener,
    sync::watch,
    task::JoinHandle,
};
use uuid::Uuid;

#[cfg(test)]
const BOOTSTRAP_PATH: &str = "/_platform/bootstrap";
const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const BOOTSTRAP_LIFETIME: Duration = Duration::from_secs(60);

#[cfg(test)]
#[derive(Default)]
struct ProxySource {
    project_root: Option<PathBuf>,
    fixed_address: Option<crate::app_address::FixedAppAddress>,
    api_prefixes: Option<Vec<String>>,
}

#[cfg(test)]
struct Bootstrap {
    token: String,
    issued: Instant,
}
#[cfg(test)]
struct Authorization {
    bootstrap: Option<Bootstrap>,
    session: String,
}
struct ProxyState {
    #[cfg(test)]
    root: Arc<File>,
    #[cfg(test)]
    root_path: PathBuf,
    #[cfg(test)]
    project_root: PathBuf,
    #[cfg(test)]
    entry: Vec<String>,
    #[cfg(test)]
    authority: String,
    #[cfg(test)]
    cookie_name: String,
    #[cfg(test)]
    origin: String,
    backend: String,
    api_prefixes: Vec<Vec<String>>,
    bearer: HeaderValue,
    #[cfg(test)]
    authorization: Arc<Mutex<Authorization>>,
    stopping: watch::Receiver<bool>,
    owner: Arc<ProxyShutdown>,
}

/// Backend forwarding is a capability of the unified application listener.
/// Authentication and SDK routing are performed by the application router;
/// the original bounded streaming driver and its owned shutdown stay here.
pub(crate) struct BackendForwarder(Arc<ProxyState>);
impl BackendForwarder {
    pub(crate) fn new(
        project: &crate::ProjectHandle,
        backend_port: u16,
        token: String,
    ) -> Result<Self, ApiError> {
        let owner = ProxyShutdown::new();
        #[cfg(test)]
        let root_path = project
            .root
            .join(&project.manifest.web.root)
            .canonicalize()?;
        #[cfg(test)]
        let root = open_root(&root_path)?;
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| proxy_error("APP_BACKEND_INVALID", "后台连接凭据无效。"))?;
        bearer.set_sensitive(true);
        #[cfg(test)]
        let origin = project.address.origin();
        let prefixes = project
            .manifest
            .backend
            .as_ref()
            .ok_or_else(|| proxy_error("APP_BACKEND_MISSING", "应用未声明后台。"))?
            .effective_api_prefixes();
        Ok(Self(Arc::new(ProxyState {
            #[cfg(test)]
            root: Arc::new(root),
            #[cfg(test)]
            root_path,
            #[cfg(test)]
            project_root: project.root.clone(),
            #[cfg(test)]
            entry: project
                .manifest
                .web
                .entry
                .split('/')
                .map(str::to_owned)
                .collect(),
            #[cfg(test)]
            authority: origin.trim_start_matches("http://").into(),
            #[cfg(test)]
            cookie_name: String::new(),
            #[cfg(test)]
            origin,
            backend: format!("http://127.0.0.1:{backend_port}"),
            api_prefixes: prefixes
                .into_iter()
                .map(|value| {
                    value
                        .trim_start_matches('/')
                        .split('/')
                        .map(str::to_owned)
                        .collect()
                })
                .collect(),
            bearer,
            #[cfg(test)]
            authorization: owner.authorization.clone(),
            stopping: owner.stopping.subscribe(),
            owner,
        })))
    }
    pub(crate) fn matches(&self, path: &str) -> bool {
        safe_path(path).is_some_and(|parts| {
            parts.first().is_none_or(|part| part != "_platform")
                && self
                    .0
                    .api_prefixes
                    .iter()
                    .any(|prefix| parts.starts_with(prefix))
        })
    }
    pub(crate) async fn forward(&self, mut request: Request) -> Response {
        let Some(parts) = safe_path(request.uri().path()) else {
            return refusal(StatusCode::BAD_REQUEST, "请求路径无效。");
        };
        let state = self.0.clone();
        let (cancel, cancelled) = watch::channel(false);
        let mut cancellation = PendingRequestCancel(Some(cancel));
        request
            .extensions_mut()
            .insert(ClientCancellation(cancelled));
        let worker = state
            .owner
            .clone()
            .request(move |lease| forward(state, request, parts, lease));
        let response = match worker {
            Ok(worker) => worker
                .await
                .unwrap_or_else(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "后台请求已中断。")),
            Err(_) => refusal(StatusCode::SERVICE_UNAVAILABLE, "后台正在停止。"),
        };
        cancellation.0.take();
        response
    }
    pub(crate) fn request_stop(&self) {
        self.0.owner.request_stop();
    }
    pub(crate) async fn stop(&self) -> Result<(), ApiError> {
        self.0.owner.wait().await
    }
}

/// A per-application endpoint whose stop waiter never owns the real drain.
#[cfg(test)]
pub struct NodeAppProxy {
    origin: String,
    authorization: Arc<Mutex<Authorization>>,
    owner: Arc<ProxyShutdown>,
}
#[cfg(test)]
pub(crate) struct ProxyStartFailure {
    pub error: ApiError,
    pub proxy: NodeAppProxy,
}
#[cfg(test)]
struct StartupWaiter(Option<Arc<ProxyShutdown>>);
#[cfg(test)]
impl Drop for StartupWaiter {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            owner.begin();
        }
    }
}
#[cfg(test)]
impl NodeAppProxy {
    // Product callers retain failed startup ownership via start_retaining_failed.
    #[cfg(test)]
    pub async fn start(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
    ) -> Result<Self, ApiError> {
        Self::finish_public_start(
            Self::start_retaining_failed(web_root, entry, backend_port, backend_token).await,
        )
        .await
    }
    #[cfg(test)]
    async fn finish_public_start(
        result: Result<Self, Box<ProxyStartFailure>>,
    ) -> Result<Self, ApiError> {
        match result {
            Ok(proxy) => Ok(proxy),
            Err(failure) => {
                let mut error = failure.error;
                if let Err(shutdown) = failure.proxy.stop().await {
                    error = error
                        .detail("shutdownIssues", shutdown.details["shutdownIssues"].clone())
                        .detail("proxyShutdown", serde_json::json!(shutdown));
                }
                Err(error)
            }
        }
    }
    #[cfg(test)]
    pub(crate) async fn start_retaining_failed(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
    ) -> Result<Self, Box<ProxyStartFailure>> {
        Self::start_with_owner(
            web_root,
            entry,
            backend_port,
            backend_token,
            ProxyShutdown::new(),
        )
        .await
    }
    pub(crate) async fn start_project_retaining_failed(
        project_root: PathBuf,
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
        address: crate::app_address::FixedAppAddress,
        api_prefixes: Vec<String>,
    ) -> Result<Self, Box<ProxyStartFailure>> {
        Self::start_with_project_owner(
            web_root,
            entry,
            backend_port,
            backend_token,
            ProxyShutdown::new(),
            ProxySource {
                project_root: Some(project_root),
                fixed_address: Some(address),
                api_prefixes: Some(api_prefixes),
            },
        )
        .await
    }
    #[cfg(test)]
    async fn start_with_owner(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
        owner: Arc<ProxyShutdown>,
    ) -> Result<Self, Box<ProxyStartFailure>> {
        Self::start_with_project_owner(
            web_root,
            entry,
            backend_port,
            backend_token,
            owner,
            ProxySource::default(),
        )
        .await
    }
    async fn start_with_project_owner(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
        owner: Arc<ProxyShutdown>,
        source: ProxySource,
    ) -> Result<Self, Box<ProxyStartFailure>> {
        let mut waiter = StartupWaiter(Some(owner.clone()));
        let worker = owner.clone();
        let result = match owner.request(move |lease| async move {
            Self::start_work(
                web_root,
                entry,
                backend_port,
                backend_token,
                worker,
                lease,
                source,
            )
            .await
        }) {
            Ok(receive) => receive.await.unwrap_or_else(|_| {
                Err(proxy_error(
                    "NODE_APP_PROXY_UNAVAILABLE",
                    "应用连接初始化未返回结果，句柄已保留。",
                ))
            }),
            Err(error) => Err(error),
        };
        waiter.0 = None;
        let mut proxy = Self {
            origin: String::new(),
            authorization: owner.authorization.clone(),
            owner,
        };
        match result {
            Ok(origin) => {
                proxy.origin = origin;
                Ok(proxy)
            }
            Err(error) => Err(Box::new(ProxyStartFailure { error, proxy })),
        }
    }
    async fn start_work(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
        owner: Arc<ProxyShutdown>,
        lease: IoLease,
        source: ProxySource,
    ) -> Result<String, ApiError> {
        let _lease = lease;
        let ProxySource {
            project_root,
            fixed_address,
            api_prefixes,
        } = source;
        let api_prefixes =
            api_prefixes.unwrap_or_else(crate::panda_manifest::default_backend_api_prefixes);
        crate::panda_manifest::validate_backend_api_prefixes(&api_prefixes)
            .map_err(|_| proxy_error("NODE_APP_PROXY_INVALID", "应用后端路径声明无效。"))?;
        let api_prefixes = api_prefixes
            .into_iter()
            .map(|prefix| prefix[1..].split('/').map(str::to_owned).collect())
            .collect();
        if backend_port == 0 || backend_token.is_empty() || backend_token.len() > 4096 {
            return Err(proxy_error("NODE_APP_PROXY_INVALID", "应用连接信息无效。"));
        }
        let mut bearer = HeaderValue::from_str(&format!("Bearer {backend_token}"))
            .map_err(|_| proxy_error("NODE_APP_PROXY_INVALID", "应用连接信息无效。"))?;
        bearer.set_sensitive(true);
        // Author paths may use './'; HTTP request paths still use the strict
        // traversal-rejecting decoder unchanged.
        let entry = entry
            .split('/')
            .filter(|part| *part != ".")
            .collect::<Vec<_>>()
            .join("/");
        let entry = safe_path(&format!("/{entry}"))
            .filter(|parts| !parts.is_empty())
            .ok_or_else(|| proxy_error("NODE_APP_WEB_INVALID", "应用网页入口无效。"))?;
        let checked_entry = entry.clone();
        let opener = owner.clone();
        let (root, root_path, project_root) = owner
            .blocking(move || {
                #[cfg(test)]
                if let Some((entered, release)) = opener.hooks.open.lock().unwrap().take() {
                    let _ = entered.send(());
                    let _ = release.recv();
                }
                let project_root = project_root
                    .unwrap_or_else(|| web_root.clone())
                    .canonicalize()?;
                let root_path = web_root.canonicalize()?;
                if !root_path.starts_with(&project_root)
                    || protected_web_path(&project_root, &root_path)
                {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                let root = open_root(&root_path)?;
                let checked_entry = resolved_web_parts(&project_root, &root_path, &checked_entry)?;
                let _entry = open_file(&root, &checked_entry)?;
                #[cfg(test)]
                if opener
                    .hooks
                    .panic_open
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    panic!("controlled artificial proxy startup worker panic");
                }
                drop(opener);
                Ok::<_, io::Error>((root, root_path, project_root))
            })
            .await
            .map_err(|_| proxy_error("NODE_APP_WEB_INVALID", "应用网页检查未返回结果。"))??
            .map_err(|_| {
                proxy_error("NODE_APP_WEB_INVALID", "应用网页缺失，或包含不支持的链接。")
            })?;
        owner.lifetime.check_open()?;
        let listener = if let Some(address) = fixed_address {
            address.bind().await?
        } else {
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "无法打开本机应用端口。"))?
        };
        let port = listener
            .local_addr()
            .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "无法读取本机应用端口。"))?
            .port();
        let origin = format!("http://127.0.0.1:{port}");
        let state = Arc::new(ProxyState {
            root: Arc::new(root),
            root_path,
            project_root,
            entry,
            authority: format!("127.0.0.1:{port}"),
            cookie_name: format!("node_app_session_{port}"),
            origin: origin.clone(),
            backend: format!("http://127.0.0.1:{backend_port}"),
            api_prefixes,
            bearer,
            authorization: owner.authorization.clone(),
            stopping: owner.stopping.subscribe(),
            owner: owner.clone(),
        });
        #[cfg(test)]
        {
            *owner.hooks.state.lock().unwrap() = Arc::downgrade(&state);
        }
        owner.install(listener, Router::new().fallback(handle).with_state(state))?;
        Ok(origin)
    }
    pub fn open_url(&mut self) -> String {
        let token = Uuid::new_v4().simple().to_string();
        let mut authorization = self.authorization.lock().unwrap_or_else(|p| p.into_inner());
        authorization.bootstrap = Some(Bootstrap {
            token: token.clone(),
            issued: Instant::now(),
        });
        format!("{}{BOOTSTRAP_PATH}?token={token}", self.origin)
    }
    pub fn request_stop(&self) {
        self.owner.request_stop();
    }
    pub async fn stop(&self) -> Result<(), ApiError> {
        self.owner.wait().await
    }
    #[cfg(test)]
    pub(crate) fn fixture_read_pause(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.owner.hooks.read.lock().unwrap() = Some((entered, release));
    }
    #[cfg(test)]
    pub(crate) fn fixture_serve_panic(&self) {
        self.owner
            .hooks
            .panic_serve
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
#[cfg(test)]
impl Drop for NodeAppProxy {
    fn drop(&mut self) {
        self.owner.begin();
    }
}

fn proxy_error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
fn refusal(status: StatusCode, message: &'static str) -> Response {
    protect((status, message).into_response())
}
fn protect(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(
            &crate::runtime_policy::RuntimePolicy::default().content_security_policy(),
        )
        .expect("Validated platform policy is a valid HTTP header"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}
#[cfg(test)]
fn exact_header(headers: &HeaderMap, name: header::HeaderName, value: &str) -> bool {
    let mut values = headers.get_all(name).iter();
    values
        .next()
        .is_some_and(|entry| entry.as_bytes() == value.as_bytes())
        && values.next().is_none()
}
#[cfg(test)]
fn constant_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |different, (a, b)| different | (a ^ b))
        == 0
}
#[cfg(test)]
fn authorized(headers: &HeaderMap, cookie_name: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let mut matched = false;
    for line in headers.get_all(header::COOKIE) {
        let Ok(line) = line.to_str() else {
            return false;
        };
        for pair in line.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == cookie_name {
                if matched || !constant_equal(value, expected) {
                    return false;
                }
                matched = true;
            }
        }
    }
    matched
}

#[derive(Clone)]
struct ClientCancellation(watch::Receiver<bool>);

struct PendingRequestCancel(Option<watch::Sender<bool>>);
impl Drop for PendingRequestCancel {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel.send_replace(true);
        }
    }
}

#[cfg(test)]
async fn handle(State(state): State<Arc<ProxyState>>, mut request: Request) -> Response {
    let (cancel, cancelled) = watch::channel(false);
    let mut cancellation = PendingRequestCancel(Some(cancel));
    request
        .extensions_mut()
        .insert(ClientCancellation(cancelled));
    let owner = state.owner.clone();
    let receive = match owner.request(move |lease| handle_owned(state, request, lease)) {
        Ok(receive) => receive,
        Err(_) => return refusal(StatusCode::SERVICE_UNAVAILABLE, "应用已停止。"),
    };
    let response = receive
        .await
        .unwrap_or_else(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "应用连接未正常结束。"));
    // Once response ownership transfers to Axum, its Body receiver is the
    // cancellation signal. Returning headers must not cancel a live stream.
    cancellation.0.take();
    response
}
#[cfg(test)]
async fn handle_owned(state: Arc<ProxyState>, request: Request, lease: IoLease) -> Response {
    if *state.stopping.borrow() {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "应用已停止，请从平台重新打开。",
        );
    }
    let headers = request.headers();
    if !exact_header(headers, header::HOST, &state.authority) {
        return refusal(StatusCode::FORBIDDEN, "只允许访问此应用的本机地址。");
    }
    let safe_method = request.method() == Method::GET || request.method() == Method::HEAD;
    if (headers.contains_key(header::ORIGIN)
        && !exact_header(headers, header::ORIGIN, &state.origin))
        || (!safe_method && !headers.contains_key(header::ORIGIN))
    {
        return refusal(StatusCode::FORBIDDEN, "请从应用自己的页面发起请求。");
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|value| value != "same-origin" && value != "none")
    {
        return refusal(StatusCode::FORBIDDEN, "不允许其他网站访问此应用。");
    }
    let Some(parts) = safe_path(request.uri().path()) else {
        return refusal(StatusCode::BAD_REQUEST, "网页路径无效。");
    };
    if request.uri().path() == BOOTSTRAP_PATH {
        if request.method() != Method::GET {
            return refusal(StatusCode::METHOD_NOT_ALLOWED, "请使用平台提供的打开入口。");
        }
        let query = request.uri().query().unwrap_or_default();
        let fields: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
        let token = match fields.as_slice() {
            [(name, token)] if name == "token" => token.as_ref(),
            _ => {
                return refusal(
                    StatusCode::UNAUTHORIZED,
                    "打开入口已失效，请从平台重新打开。",
                )
            }
        };
        let mut authorization = state
            .authorization
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let accepted = authorization.bootstrap.as_ref().is_some_and(|bootstrap| {
            bootstrap.issued.elapsed() <= BOOTSTRAP_LIFETIME
                && constant_equal(&bootstrap.token, token)
        });
        if !accepted {
            return refusal(
                StatusCode::UNAUTHORIZED,
                "打开入口已失效，请从平台重新打开。",
            );
        }
        authorization.bootstrap = None;
        let cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            state.cookie_name, authorization.session
        );
        let mut response = StatusCode::SEE_OTHER.into_response();
        response
            .headers_mut()
            .insert(header::LOCATION, HeaderValue::from_static("/"));
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("generated UUID cookie is a valid header"),
        );
        return protect(response);
    }
    {
        let authorization = state
            .authorization
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !authorized(headers, &state.cookie_name, &authorization.session) {
            return refusal(StatusCode::UNAUTHORIZED, "请先从平台打开此应用。");
        }
    }
    if parts.first().is_some_and(|part| part == "_platform") {
        if parts.get(1).is_some_and(|part| part == "api") {
            return protect((StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"ok":false,"apiVersion":1,"error":ApiError::new("SDK_NOT_ENABLED","这个应用没有启用平台 SDK。"),"requestId":null}))).into_response());
        }
        return refusal(StatusCode::NOT_FOUND, "没有这个平台接口。");
    }
    if state
        .api_prefixes
        .iter()
        .any(|prefix| parts.starts_with(prefix))
    {
        forward(state, request, parts, lease).await
    } else if safe_method {
        let page = request
            .headers()
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().split(';').next() == Some("text/html"))
            })
            && request
                .headers()
                .get("sec-fetch-dest")
                .is_none_or(|value| value == "document");
        static_file(state, request.method() == Method::HEAD, parts, page, lease).await
    } else {
        refusal(StatusCode::METHOD_NOT_ALLOWED, "此网页不接受该请求方式。")
    }
}

// A response/timeout owns cancellation until its producer really releases the
// incoming body. Only the separately registered driver owns the actual socket.
struct ConnectionCancel(watch::Sender<bool>);
impl Drop for ConnectionCancel {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    if !*stop.borrow() {
        let _ = stop.changed().await;
    }
}

async fn client_cancelled(cancel: &mut Option<watch::Receiver<bool>>) {
    let Some(cancel) = cancel else {
        return std::future::pending().await;
    };
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            // The handler disarms by closing a false channel after handing its
            // response off. Body ownership then supplies downstream cancellation.
            return std::future::pending().await;
        }
    }
}

// HTTP/1 framing belongs to Hyper. Copy end-to-end fields and their repeated
// values, but never propagate hop-by-hop fields or either side's credentials.
fn business_headers(source: &HeaderMap, response: bool) -> HeaderMap {
    let mut nominated = std::collections::HashSet::new();
    for line in source.get_all(header::CONNECTION) {
        if let Ok(line) = line.to_str() {
            for name in line.split(',') {
                if let Ok(name) = header::HeaderName::from_bytes(name.trim().as_bytes()) {
                    nominated.insert(name);
                }
            }
        }
    }
    let mut forwarded = HeaderMap::new();
    for (name, value) in source {
        let name_text = name.as_str();
        let restricted = matches!(
            name_text,
            "connection"
                | "proxy-connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "proxy-authentication-info"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "host"
                | "authorization"
                | "www-authenticate"
                | "authentication-info"
                | "cookie"
                | "cookie2"
                | "set-cookie"
                | "set-cookie2"
                | "origin"
                | "referer"
                | "forwarded"
                | "via"
                | "x-real-ip"
        ) || name_text.starts_with("sec-fetch-")
            || name_text.starts_with("sec-websocket-")
            || name_text.starts_with("x-platform-")
            || name_text.starts_with("x-panda-")
            || name_text.starts_with("x-forwarded-");
        let response_policy = response
            && (matches!(
                name_text,
                "location"
                    | "refresh"
                    | "alt-svc"
                    | "clear-site-data"
                    | "content-security-policy"
                    | "content-security-policy-report-only"
                    | "referrer-policy"
                    | "x-content-type-options"
                    | "permissions-policy"
            ) || name_text.starts_with("access-control-")
                || name_text.starts_with("cross-origin-"));
        if !restricted && !response_policy && !nominated.contains(name) {
            forwarded.append(name.clone(), value.clone());
        }
    }
    forwarded
}

// Hyper calls the underlying flush only after its own encoded write buffer has
// drained. Source exhaustion alone is insufficient: the last body frame may
// still be blocked on a full socket while an early response is already flowing.
struct UploadSocket {
    socket: tokio::net::TcpStream,
    source_complete: Arc<std::sync::atomic::AtomicBool>,
    sent_complete: watch::Sender<bool>,
    #[cfg(test)]
    write_budget: Option<usize>,
}

impl AsyncRead for UploadSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_read(context, buffer)
    }
}

impl AsyncWrite for UploadSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        #[cfg(test)]
        let bytes = match this.write_budget {
            Some(0) if !bytes.is_empty() => return Poll::Pending,
            Some(remaining) => &bytes[..bytes.len().min(remaining)],
            None => bytes,
        };
        let result = Pin::new(&mut this.socket).poll_write(context, bytes);
        #[cfg(test)]
        if let (Some(remaining), Poll::Ready(Ok(written))) = (&mut this.write_budget, &result) {
            *remaining -= *written;
        }
        result
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        #[cfg(test)]
        if self.write_budget.is_some() {
            return self.poll_write(
                context,
                buffers
                    .iter()
                    .find(|bytes| !bytes.is_empty())
                    .map_or(&[], |bytes| &bytes[..]),
            );
        }
        Pin::new(&mut self.get_mut().socket).poll_write_vectored(context, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.socket.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let flushed = Pin::new(&mut this.socket).poll_flush(context);
        if matches!(flushed, Poll::Ready(Ok(())))
            && this
                .source_complete
                .load(std::sync::atomic::Ordering::Acquire)
        {
            this.sent_complete.send_replace(true);
        }
        flushed
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_shutdown(context)
    }
}

fn upload_body(
    body: Body,
    stop: watch::Receiver<bool>,
    timed_out: Arc<std::sync::atomic::AtomicBool>,
    complete: Arc<std::sync::atomic::AtomicBool>,
) -> Body {
    if body.is_end_stream() {
        complete.store(true, std::sync::atomic::Ordering::Release);
        return body;
    }
    let remaining = body.size_hint().exact();
    let stream = stream::unfold(
        (
            Box::pin(body.into_data_stream()),
            stop,
            remaining,
            complete,
            false,
        ),
        move |(mut source, mut stop, mut remaining, complete, finished)| {
            let timed_out = timed_out.clone();
            async move {
                if finished {
                    return None;
                }
                // The timer runs only while Hyper asks for the next chunk. A
                // slow backend applies backpressure instead of growing a buffer.
                let next = tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => Some(Err(io::Error::other("应用已停止。"))),
                    next = tokio::time::timeout(UPLOAD_IDLE_TIMEOUT, source.next()) => match next {
                        Ok(next) => next.map(|item| item.map_err(|_| io::Error::other("读取请求失败。"))),
                        Err(_) => {
                            timed_out.store(true, std::sync::atomic::Ordering::Release);
                            Some(Err(io::Error::new(io::ErrorKind::TimedOut, "读取请求数据超时。")))
                        }
                    }
                };
                match next {
                    Some(item) => {
                        let finished = item.is_err();
                        if let (Some(left), Ok(bytes)) = (&mut remaining, &item) {
                            *left = left.saturating_sub(bytes.len() as u64);
                        }
                        if !finished && remaining == Some(0) {
                            complete.store(true, std::sync::atomic::Ordering::Release);
                        }
                        Some((item, (source, stop, remaining, complete, finished)))
                    }
                    None => {
                        complete.store(true, std::sync::atomic::Ordering::Release);
                        None
                    }
                }
            }
        },
    );
    Body::from_stream(stream)
}

async fn unfinished_upload_deadline(
    mut complete: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) {
    loop {
        if *complete.borrow() {
            return std::future::pending().await;
        }
        tokio::select! {
            biased;
            changed = complete.changed() => if changed.is_err() {
                return std::future::pending().await;
            },
            _ = tokio::time::sleep_until(deadline) => return,
        }
    }
}

async fn forward(
    state: Arc<ProxyState>,
    request: Request,
    parts: Vec<String>,
    lease: IoLease,
) -> Response {
    forward_with_budget(state, request, parts, lease, FORWARD_REQUEST_TIMEOUT).await
}

async fn forward_with_budget(
    state: Arc<ProxyState>,
    request: Request,
    parts: Vec<String>,
    lease: IoLease,
    request_timeout: Duration,
) -> Response {
    if request.headers().contains_key(header::UPGRADE) {
        return refusal(StatusCode::BAD_REQUEST, "不支持 WebSocket 或其他连接升级。");
    }
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return refusal(StatusCode::METHOD_NOT_ALLOWED, "不支持此请求方式。");
    }
    let mut client_cancellation = request
        .extensions()
        .get::<ClientCancellation>()
        .map(|cancel| cancel.0.clone());
    let (request, body) = request.into_parts();
    let mut stop = state.stopping.clone();
    let upload_timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let upload_budget_expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let source_complete = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (upload_sent, upload_complete) = watch::channel(body.is_end_stream());
    let body = upload_body(
        body,
        state.stopping.clone(),
        upload_timed_out.clone(),
        source_complete.clone(),
    );
    let mut url = url::Url::parse(&state.backend).expect("fixed loopback URL is valid");
    let mut backend_path = format!("/{}", parts.join("/"));
    if request.uri.path().ends_with('/') {
        backend_path.push('/');
    }
    url.set_path(&backend_path);
    url.set_query(request.uri.query());
    let target = &url[url::Position::BeforePath..];
    let mut upstream = Request::builder().method(request.method).uri(target);
    *upstream.headers_mut().expect("new request headers") =
        business_headers(&request.headers, false);
    let upstream = upstream
        .header(
            header::HOST,
            &url[url::Position::BeforeHost..url::Position::AfterPort],
        )
        .header(header::CONNECTION, "close")
        .header(header::AUTHORIZATION, state.bearer.clone())
        .body(body)
        .expect("validated loopback request");
    // This is a direct loopback HTTP/1 connection. There is no proxy, redirect
    // follower, pool, or hidden connection driver whose completion we cannot join.
    let connection = async {
        #[cfg(test)]
        let write_budget = *state.owner.hooks.socket_write_budget.lock().unwrap();
        let socket = tokio::net::TcpStream::connect((
            Ipv4Addr::LOCALHOST,
            url.port().expect("fixed loopback port"),
        ))
        .await?;
        hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(UploadSocket {
            socket,
            source_complete,
            sent_complete: upload_sent,
            #[cfg(test)]
            write_budget,
        }))
        .await
        .map_err(io::Error::other)
    };
    let (mut upstream_sender, connection) = tokio::select! {
        biased;
        _ = stopped(&mut stop) => return refusal(StatusCode::SERVICE_UNAVAILABLE, "应用已停止。"),
        _ = client_cancelled(&mut client_cancellation) => return refusal(StatusCode::REQUEST_TIMEOUT, "请求已取消。"),
        result = tokio::time::timeout(Duration::from_secs(3), connection) => match result {
            Ok(Ok(connection)) => connection,
            _ => return refusal(StatusCode::BAD_GATEWAY, "应用服务暂时无法连接。"),
        }
    };
    let (cancel, mut cancelled) = watch::channel(false);
    let cancellation = ConnectionCancel(cancel);
    let mut stopping = state.stopping.clone();
    let mut requester = client_cancellation.clone();
    let driver_owner = state.owner.clone();
    let deadline = tokio::time::Instant::now() + request_timeout;
    let upload_expired = upload_budget_expired.clone();
    let driver = state.owner.work(async move {
        // Dropping Connection here closes the owned TcpStream. The registry
        // observes this actual driver JoinHandle, not merely a stop notification.
        let result = {
            tokio::pin!(connection);
            tokio::select! {
                biased;
                _ = stopped(&mut stopping) => Ok(()),
                _ = stopped(&mut cancelled) => Ok(()),
                _ = client_cancelled(&mut requester) => Ok(()),
                _ = unfinished_upload_deadline(upload_complete, deadline) => {
                    upload_expired.store(true, std::sync::atomic::Ordering::Release);
                    Err(io::Error::new(io::ErrorKind::TimedOut, "上传请求超时。"))
                },
                result = &mut connection => result.map_err(|_| io::Error::other("应用连接已结束。")),
            }
        };
        #[cfg(test)]
        {
            let pause = driver_owner.hooks.driver_closed.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
            if driver_owner
                .hooks
                .panic_driver
                .load(std::sync::atomic::Ordering::Acquire)
            {
                panic!("controlled artificial proxy connection driver panic");
            }
        }
        drop(driver_owner);
        // A transport failure with the socket released is a business response
        // error, not unknown resource ownership. The registry still joins this
        // actual task and records any panic/JoinError as sticky shutdown failure.
        result
    });
    let response = tokio::select! {
        biased;
        _ = stopped(&mut stop) => return refusal(StatusCode::SERVICE_UNAVAILABLE, "应用已停止。"),
        _ = client_cancelled(&mut client_cancellation) => return refusal(StatusCode::REQUEST_TIMEOUT, "请求已取消。"),
        result = tokio::time::timeout_at(deadline, upstream_sender.send_request(upstream)) => match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) if upload_timed_out.load(std::sync::atomic::Ordering::Acquire) => return refusal(StatusCode::REQUEST_TIMEOUT, "等待下一段请求数据超时，请重试。"),
            Ok(Err(_)) if upload_budget_expired.load(std::sync::atomic::Ordering::Acquire) => return refusal(StatusCode::GATEWAY_TIMEOUT, "上传请求或等待应用响应超时，请重试。"),
            Ok(Err(_)) => return refusal(StatusCode::BAD_GATEWAY, "应用服务暂时无法连接。"),
            Err(_) => return refusal(StatusCode::GATEWAY_TIMEOUT, "应用响应超时，请重试。"),
        }
    };
    drop(upstream_sender);
    if (response.status().is_redirection() && response.status() != StatusCode::NOT_MODIFIED)
        || response.status() == StatusCode::SWITCHING_PROTOCOLS
    {
        return refusal(
            StatusCode::BAD_GATEWAY,
            "应用返回了不支持的跳转或连接方式。",
        );
    }
    let status = response.status();
    let headers = business_headers(response.headers(), true);
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let mut stopping = state.stopping.clone();
    let mut upstream = Box::pin(response.into_body().into_data_stream());
    let work=lease.hold(Box::pin(async move {
        loop {
            if *stopping.borrow() {break;}
            let next=tokio::select! {biased; _=stopped(&mut stopping)=>break,_=sender.closed()=>break,next=upstream.next()=>next};
            let Some(next)=next else {break;};
            let item=next.map_err(|_|io::Error::other("应用连接已结束。"));
            tokio::select! {biased; _=stopped(&mut stopping)=>break, sent=sender.send(item)=>if sent.is_err(){break;}}
        }
        drop(upstream);
        drop(cancellation);
        if let Ok(Err(error)) = driver.await {
            let _ = sender.try_send(Err(error));
        }
    }));
    // The accepted request transfers its original lease to the producer. No
    // second admission is attempted after stop may have sealed the endpoint.
    drop(state.owner.work(work));
    let stream = stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    let mut response = Body::from_stream(stream).into_response();
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    protect(response)
}

#[cfg(test)]
fn protected_web_path(project: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(project) else {
        return true;
    };
    let project_data = relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str().eq_ignore_ascii_case("data"));
    project_data
        || relative.components().any(|component| {
            let value = component.as_os_str().to_string_lossy();
            value.starts_with('.')
                || value.eq_ignore_ascii_case("node_modules")
                || value.eq_ignore_ascii_case("panda.json")
        })
}

/// Resolve aliases, then open only the resolved components under the retained
/// ordinary root descriptor/HANDLE. Hard-link checks remain in native open_file.
#[cfg(test)]
fn resolved_web_parts(project: &Path, root: &Path, parts: &[String]) -> io::Result<Vec<String>> {
    let mut candidate = root.to_path_buf();
    for part in parts {
        candidate.push(part);
    }
    if protected_web_path(project, &candidate) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let resolved = candidate.canonicalize()?;
    if !resolved.starts_with(root) || protected_web_path(project, &resolved) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    resolved
        .strip_prefix(root)
        .map_err(|_| io::ErrorKind::PermissionDenied)?
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
        })
        .collect()
}

#[cfg(test)]
async fn static_file(
    state: Arc<ProxyState>,
    head: bool,
    parts: Vec<String>,
    page: bool,
    lease: IoLease,
) -> Response {
    let parts = if parts.is_empty() {
        state.entry.clone()
    } else {
        parts
    };
    let root = state.root.clone();
    let checked = parts.clone();
    let entry = state.entry.clone();
    let fallback = page
        && parts
            .last()
            .is_some_and(|part| Path::new(part).extension().is_none());
    let root_path = state.root_path.clone();
    let project_root = state.project_root.clone();
    let opened = state
        .owner
        .blocking(move || {
            let mut declared = root_path.clone();
            for part in &checked {
                declared.push(part);
            }
            if protected_web_path(&project_root, &declared) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            // A no-follow traversal already resolves a path without aliases.
            // Retain the original open directory even if its pathname moves.
            // Only an alias needs canonical resolution before a second bound open.
            let opened = open_file(&root, &checked).or_else(|_| {
                let resolved = resolved_web_parts(&project_root, &root_path, &checked)?;
                open_file(&root, &resolved)
            });
            let (file, served) = match opened {
                Ok(file) => (file, checked),
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && fallback
                        && plain_missing_path(&root_path, &checked) =>
                {
                    let resolved = resolved_web_parts(&project_root, &root_path, &entry)?;
                    (open_file(&root, &resolved)?, entry)
                }
                Err(error) => return Err(error),
            };
            let size = file.metadata()?.len();
            Ok::<_, io::Error>((file, size, served))
        })
        .await;
    let (file, size, parts) = match opened {
        Ok(Ok(Ok(opened))) => opened,
        _ => return refusal(StatusCode::NOT_FOUND, "没有找到这个网页文件。"),
    };
    let body = if head {
        drop(file);
        drop(lease);
        Body::empty()
    } else {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let producer_owner = state.owner.clone();
        let work = lease.hold(move || {
            let mut file = file;
            loop {
                if producer_owner.lifetime.check_open().is_err() || sender.is_closed() {
                    return;
                }
                let mut buffer = vec![0; 16 * 1024];
                let read = read_proxy_file(&producer_owner, &mut file, &mut buffer);
                let mut item = match read {
                    Ok(0) => return,
                    Ok(count) => {
                        buffer.truncate(count);
                        Ok(Bytes::from(buffer))
                    }
                    Err(_) => Err(io::Error::other("网页读取已结束。")),
                };
                let failed = item.is_err();
                loop {
                    match sender.try_send(item) {
                        Ok(()) => break,
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                        Err(tokio::sync::mpsc::error::TrySendError::Full(value)) => {
                            if producer_owner.lifetime.check_open().is_err() {
                                return;
                            }
                            item = value;
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                }
                if failed {
                    return;
                }
            }
        });
        drop(state.owner.blocking(move || work.run_blocking()));
        let stream = stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|item| (item, receiver))
        });
        Body::from_stream(stream)
    };
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(
            mime_guess::from_path(parts.join("/"))
                .first_or_octet_stream()
                .as_ref(),
        )
        .expect("MIME guess contains a valid header"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).expect("file size is a valid header"),
    );
    protect(response)
}
#[cfg(test)]
fn plain_missing_path(root: &Path, parts: &[String]) -> bool {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    false
}
#[cfg(test)]
fn read_proxy_file(owner: &ProxyShutdown, file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    #[cfg(test)]
    {
        let pause = owner.hooks.read.lock().unwrap().take();
        if let Some((entered, release)) = pause {
            let _ = entered.send(());
            let _ = release.recv();
        }
        if owner
            .hooks
            .panic_read
            .load(std::sync::atomic::Ordering::Acquire)
        {
            panic!("controlled artificial proxy file worker panic");
        }
    }
    let _ = owner;
    std::io::Read::read(file, buffer)
}

/// Decode once, reject both traversal and double-encoded separators. Encoded
/// slashes and percent signs are intentionally unsupported to keep routing exact.
fn safe_path(path: &str) -> Option<Vec<String>> {
    let encoded = path.strip_prefix('/')?.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        let byte = if encoded[index] == b'%' {
            let high = (encoded.get(index + 1).copied()? as char).to_digit(16)?;
            let low = (encoded.get(index + 2).copied()? as char).to_digit(16)?;
            index += 3;
            let byte = (high * 16 + low) as u8;
            if b"/\\%".contains(&byte) {
                return None;
            }
            byte
        } else {
            let byte = encoded[index];
            index += 1;
            byte
        };
        if byte.is_ascii_control() || b"\\:%?".contains(&byte) {
            return None;
        }
        decoded.push(byte);
    }
    if decoded.is_empty() {
        return Some(Vec::new());
    }
    let decoded = String::from_utf8(decoded).ok()?;
    let mut parts = Vec::new();
    for part in decoded.strip_suffix('/').unwrap_or(&decoded).split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        parts.push(part.to_owned());
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::any, Json};
    use serde_json::json;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn fixed_origin_reopens_with_new_auth_and_page_fallback_never_serves_private_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(
            root.join("index.html"),
            "<h1>disposable fixed Node origin</h1>",
        )
        .unwrap();
        let reservation = platform_native::app_listener::bind_loopback(0).unwrap();
        let address = crate::FixedAppAddress {
            port: reservation.local_addr().unwrap().port(),
        };
        drop(reservation);
        let start = || {
            NodeAppProxy::start_project_retaining_failed(
                root.clone(),
                root.clone(),
                "index.html".into(),
                9,
                "disposable-backend-token".into(),
                address,
                crate::panda_manifest::default_backend_api_prefixes(),
            )
        };
        let mut proxy = start().await.map_err(|failure| failure.error).unwrap();
        let origin = proxy.origin.clone();
        assert_eq!(origin, address.origin());
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client.get(proxy.open_url()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let response = client
            .get(format!("{origin}/notes/deep/"))
            .header(header::COOKIE, &cookie)
            .header(header::ACCEPT, "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("disposable fixed Node origin"));
        for path in [
            "/data/missing",
            "/.hidden/missing",
            "/node_modules/missing",
            "/missing.js",
            "/_platform/unknown",
        ] {
            let response = client
                .get(format!("{origin}{path}"))
                .header(header::COOKIE, &cookie)
                .header(header::ACCEPT, "text/html")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        proxy.stop().await.unwrap();
        let occupied = address.bind().await.unwrap();
        let failure = match start().await {
            Ok(_) => panic!("An occupied fixed port must not be replaced"),
            Err(failure) => failure,
        };
        assert_eq!(failure.error.code, "APP_PORT_IN_USE");
        failure.proxy.stop().await.unwrap();
        drop(occupied);
        let reopened = start().await.map_err(|failure| failure.error).unwrap();
        assert_eq!(reopened.origin, origin);
        assert_eq!(
            client
                .get(format!("{origin}/"))
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        reopened.stop().await.unwrap();
    }

    pub(super) struct Fixture {
        _directory: TempDir,
        pub(super) root: PathBuf,
        pub(super) proxy: NodeAppProxy,
        pub(super) client: reqwest::Client,
        pub(super) cookie: String,
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<HeaderMap>>,
        backend_task: JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.backend_task.abort();
        }
    }
    pub(super) async fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("web");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("index.html"),
            b"<!doctype html><title>Radio fixture</title>",
        )
        .unwrap();
        std::fs::write(root.join("app.js"), b"console.log('fixture')").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(HeaderMap::new()));
        let call_count = calls.clone();
        let captured_headers = received.clone();
        let backend = Router::new()
            .route(
                "/api/echo",
                any(move |request: Request| {
                    let calls = call_count.clone();
                    let received = captured_headers.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        *received.lock().unwrap() = request.headers().clone();
                        let method = request.method().to_string();
                        let query = request.uri().query().map(str::to_owned);
                        let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
                            .await
                            .unwrap();
                        Json(json!({ "method": method, "query": query, "length": body.len() }))
                    }
                }),
            )
            .route(
                "/api/redirect",
                any(|| async {
                    (
                        StatusCode::FOUND,
                        [(header::LOCATION, "http://198.51.100.1:9/never")],
                    )
                }),
            )
            .route(
                "/api/events",
                any(|| async {
                    let chunks = stream::once(async {
                        Ok::<_, Infallible>(Bytes::from_static(b"data: fixture\n\n"))
                    })
                    .chain(stream::pending());
                    (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        Body::from_stream(chunks),
                    )
                }),
            )
            .route(
                "/tts/fixture.mp3",
                any(|| async {
                    (
                        StatusCode::PARTIAL_CONTENT,
                        [
                            (header::CONTENT_TYPE, "audio/mpeg"),
                            (header::CONTENT_RANGE, "bytes 0-2/6"),
                            (header::ACCEPT_RANGES, "bytes"),
                            (header::SET_COOKIE, "backend_secret=must-not-forward"),
                        ],
                        "abc",
                    )
                }),
            );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend_task = tokio::spawn(async move {
            let _ = axum::serve(listener, backend).await;
        });
        let proxy = NodeAppProxy::start(
            root.clone(),
            "index.html".to_string(),
            port,
            "backend-private-fixture".to_string(),
        )
        .await
        .unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let mut fixture = Fixture {
            _directory: directory,
            root,
            proxy,
            client,
            cookie: String::new(),
            calls,
            received,
            backend_task,
        };
        let open = fixture.proxy.open_url();
        let response = fixture.client.get(open).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let full_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(full_cookie.contains("HttpOnly; SameSite=Strict"));
        fixture.cookie = full_cookie.split(';').next().unwrap().to_string();
        fixture
    }
    impl Fixture {
        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.proxy.origin)
        }
        pub(super) fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
            self.client
                .request(method, self.url(path))
                .header(header::COOKIE, &self.cookie)
        }
    }

    #[test]
    fn decoded_paths_reject_traversal_links_and_ambiguous_encodings() {
        for path in [
            "/..",
            "/a/../x",
            "/%2e%2E/x",
            "/a//x",
            "//x",
            "/a\\x",
            "/a%2fx",
            "/a%5cx",
            "/%252e%252e/x",
            "/bad%",
            "/bad%xx",
            "/a%00x",
            "/C:/x",
            "/%ff",
        ] {
            assert!(safe_path(path).is_none(), "{path}");
        }
        assert_eq!(safe_path("/"), Some(vec![]));
        assert_eq!(
            safe_path("/assets/%E9%9F%B3%E4%B9%90.js"),
            Some(vec!["assets".to_string(), "音乐.js".to_string()])
        );
    }

    #[tokio::test]
    async fn bootstrap_is_single_use_revocable_and_not_a_backend_bearer() {
        let mut f = fixture().await;
        let first = f.proxy.open_url();
        assert!(!first.contains("backend-private-fixture"));
        let second = f.proxy.open_url();
        assert_eq!(
            f.client.get(first).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let response = f.client.get(&second).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(
            f.client.get(second).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            f.client.get(f.url("/")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn expired_bootstrap_and_duplicate_session_cookies_are_rejected() {
        let mut f = fixture().await;
        let url = f.proxy.open_url();
        f.proxy
            .authorization
            .lock()
            .unwrap()
            .bootstrap
            .as_mut()
            .unwrap()
            .issued = Instant::now() - BOOTSTRAP_LIFETIME - Duration::from_secs(1);
        assert_eq!(
            f.client.get(url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let duplicate = format!("{}; {}", f.cookie, f.cookie);
        assert_eq!(
            f.client
                .get(f.url("/"))
                .header(header::COOKIE, duplicate)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn strict_host_origin_and_fetch_site_protect_backend() {
        let f = fixture().await;
        let response = f
            .request(Method::GET, "/api/echo")
            .header(header::HOST, "localhost:1")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        for origin in [None, Some("http://localhost:1"), Some("null")] {
            let request = f.request(Method::POST, "/api/echo");
            let request = if let Some(origin) = origin {
                request.header(header::ORIGIN, origin)
            } else {
                request
            };
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            f.request(Method::GET, "/api/echo")
                .header("sec-fetch-site", "same-site")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn forward_injects_only_backend_bearer_and_preserves_allowed_headers() {
        let f = fixture().await;
        let response = f
            .request(Method::POST, "/api/echo?q=music%20fixture")
            .header(header::ORIGIN, &f.proxy.origin)
            .header(header::AUTHORIZATION, "Bearer malicious-browser-value")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::RANGE, "bytes=0-2")
            .header("x-platform-admin", "must-not-forward")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["method"], "POST");
        assert_eq!(body["length"], 2);
        assert_eq!(body["query"], "q=music%20fixture");
        let headers = f.received.lock().unwrap().clone();
        assert_eq!(
            headers[header::AUTHORIZATION],
            "Bearer backend-private-fixture"
        );
        assert_eq!(headers[header::RANGE], "bytes=0-2");
        assert!(!headers.contains_key(header::COOKIE));
        assert!(!headers.contains_key(header::ORIGIN));
        assert!(!headers.contains_key("x-platform-admin"));
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn unsupported_methods_and_non_api_routes_do_not_reach_backend() {
        let f = fixture().await;
        assert_eq!(
            f.request(Method::TRACE, "/api/echo")
                .header(header::ORIGIN, &f.proxy.origin)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            f.request(Method::GET, "/apiary")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request(Method::GET, "/_platform/health")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn backend_redirects_and_cookies_are_never_forwarded_to_page() {
        let f = fixture().await;
        let response = f
            .request(Method::GET, "/api/redirect")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(!response.headers().contains_key(header::LOCATION));
        let response = f
            .request(Method::GET, "/tts/fixture.mp3")
            .header(header::RANGE, "bytes=0-2")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/mpeg");
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 0-2/6");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert_eq!(response.text().await.unwrap(), "abc");
        f.proxy.stop().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn static_pages_have_local_script_policy_and_do_not_serve_links() {
        use std::os::unix::fs::symlink;
        let f = fixture().await;
        let response = f.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self' 'unsafe-inline' 'unsafe-eval' https:;"));
        assert!(csp.contains("connect-src 'self';"));
        assert_eq!(
            response.text().await.unwrap(),
            "<!doctype html><title>Radio fixture</title>"
        );
        let outside = f.root.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "private-fixture").unwrap();
        symlink(&outside, f.root.join("linked.txt")).unwrap();
        std::fs::hard_link(&outside, f.root.join("hardlinked.txt")).unwrap();
        symlink(f.root.parent().unwrap(), f.root.join("linked-directory")).unwrap();
        for path in [
            "/linked.txt",
            "/hardlinked.txt",
            "/linked-directory/outside.txt",
        ] {
            let response = f.request(Method::GET, path).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(!response.text().await.unwrap().contains("private-fixture"));
        }
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn static_files_never_expose_project_data_or_control_files() {
        let f = fixture().await;
        for relative in [
            "data/main.sqlite",
            "node_modules/pkg.js",
            ".private/value",
            "panda.json",
        ] {
            let path = f.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "must remain private").unwrap();
            let response = f
                .request(Method::GET, &format!("/{relative}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{relative}");
            assert!(!response
                .text()
                .await
                .unwrap()
                .contains("must remain private"));
        }
        std::fs::create_dir_all(f.root.join("assets/data")).unwrap();
        std::fs::write(f.root.join("assets/data/chart.json"), "public chart data").unwrap();
        let response = f
            .request(Method::GET, "/assets/data/chart.json")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "public chart data");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(f.root.join("data/main.sqlite"), f.root.join("innocent.txt")).unwrap();
            assert_eq!(
                f.request(Method::GET, "/innocent.txt")
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
            symlink(f.root.join("app.js"), f.root.join("ordinary-alias.js")).unwrap();
            let response = f
                .request(Method::GET, "/ordinary-alias.js")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.text().await.unwrap(), "console.log('fixture')");
        }
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn declared_dot_entry_opens_without_relaxing_request_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("index.html"), "declared dot entry").unwrap();
        let mut proxy =
            NodeAppProxy::start(root, "./index.html".into(), 9, "disposable-token".into())
                .await
                .unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = client.get(proxy.open_url()).send().await.unwrap();
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let page = client
            .get(format!("{}/", proxy.origin))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.text().await.unwrap(), "declared dot entry");
        assert!(safe_path("/./index.html").is_none());
        proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn each_application_has_a_distinct_cookie_and_rejects_other_app_sessions() {
        let first = fixture().await;
        let second = fixture().await;
        assert_ne!(
            first.cookie.split('=').next(),
            second.cookie.split('=').next()
        );
        let response = second
            .client
            .get(second.url("/"))
            .header(header::COOKIE, &first.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // Browsers share loopback-host cookies across ports; only the matching
        // per-port name/value grants access, and none are sent to Node.
        let both = format!("{}; {}", first.cookie, second.cookie);
        assert_eq!(
            second
                .client
                .get(second.url("/"))
                .header(header::COOKIE, both)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        first.proxy.stop().await.unwrap();
        second.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn stop_cancels_sse_stream_and_closes_listener() {
        let f = fixture().await;
        let mut response = f.request(Method::GET, "/api/events").send().await.unwrap();
        let first = response.chunk().await.unwrap().unwrap();
        assert_eq!(first.as_ref(), b"data: fixture\n\n");
        let address = f.proxy.origin.trim_start_matches("http://").to_string();
        f.proxy.stop().await.unwrap();
        let next = tokio::time::timeout(Duration::from_secs(1), response.chunk())
            .await
            .expect("stop must cancel open SSE");
        assert!(matches!(next, Ok(None) | Err(_)));
        assert!(tokio::net::TcpStream::connect(&address).await.is_err());
        f.proxy.stop().await.unwrap();
    }

    #[tokio::test]
    async fn unavailable_backend_errors_do_not_expose_tokens_or_internal_details() {
        let mut f = fixture().await;
        f.backend_task.abort();
        let _ = (&mut f.backend_task).await;
        let response = f.request(Method::GET, "/api/echo").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = response.text().await.unwrap();
        assert!(!body.contains("backend-private-fixture"));
        assert!(!body.contains("127.0.0.1"));
        f.proxy.stop().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn anchored_web_root_does_not_follow_replacement_parent_directory() {
        use std::os::unix::fs::symlink;
        let f = fixture().await;
        let saved = f.root.with_file_name("original-web");
        std::fs::rename(&f.root, &saved).unwrap();
        let foreign = f.root.with_file_name("foreign-web");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("index.html"), "foreign-private").unwrap();
        symlink(&foreign, &f.root).unwrap();
        let response = f.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.text().await.unwrap().contains("foreign-private"));
        f.proxy.stop().await.unwrap();
    }
}

#[cfg(test)]
#[path = "node_app_proxy_shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "node_app_proxy_streaming_tests.rs"]
mod streaming_tests;
