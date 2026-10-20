//! Private per-application loopback web adapter. It exposes no platform management
//! API, never forwards the browser cookie, and keeps the backend bearer in Rust.
use crate::ApiError;
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures_util::{stream, StreamExt};
#[cfg(unix)]
use std::{ffi::CString, path::Component};
use std::{
    fs::File,
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::watch, task::JoinHandle};
use uuid::Uuid;

const BOOTSTRAP_PATH: &str = "/_platform/bootstrap";
const BODY_LIMIT: usize = 64 * 1024;
const BOOTSTRAP_LIFETIME: Duration = Duration::from_secs(60);
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data: https:; media-src 'self' https:; font-src 'self'; manifest-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'";

struct Bootstrap {
    token: String,
    issued: Instant,
}
struct Authorization {
    bootstrap: Option<Bootstrap>,
    session: String,
}
struct ProxyState {
    root: Arc<File>,
    entry: Vec<String>,
    authority: String,
    cookie_name: String,
    origin: String,
    backend: String,
    bearer: HeaderValue,
    client: reqwest::Client,
    authorization: Arc<Mutex<Authorization>>,
    stopping: watch::Receiver<bool>,
}

/// A browser endpoint for one already-started, authenticated Node application.
/// Dropping it revokes the page session and cancels active response streams.
pub struct NodeAppProxy {
    origin: String,
    authorization: Arc<Mutex<Authorization>>,
    stopping: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl NodeAppProxy {
    pub async fn start(
        web_root: PathBuf,
        entry: String,
        backend_port: u16,
        backend_token: String,
    ) -> Result<Self, ApiError> {
        if backend_port == 0 || backend_token.is_empty() || backend_token.len() > 4096 {
            return Err(proxy_error("NODE_APP_PROXY_INVALID", "应用连接信息无效。"));
        }
        let mut bearer = HeaderValue::from_str(&format!("Bearer {backend_token}"))
            .map_err(|_| proxy_error("NODE_APP_PROXY_INVALID", "应用连接信息无效。"))?;
        bearer.set_sensitive(true);
        let entry = safe_path(&format!("/{entry}"))
            .filter(|parts| !parts.is_empty())
            .ok_or_else(|| proxy_error("NODE_APP_WEB_INVALID", "应用网页入口无效。"))?;
        let checked_entry = entry.clone();
        let root = tokio::task::spawn_blocking(move || {
            let root = open_root(&web_root)?;
            let _entry = open_file(&root, &checked_entry)?;
            Ok::<_, io::Error>(root)
        })
        .await
        .map_err(|_| proxy_error("NODE_APP_WEB_INVALID", "无法检查应用网页。"))?
        .map_err(|_| proxy_error("NODE_APP_WEB_INVALID", "应用网页缺失，或包含不支持的链接。"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .http1_only()
            .build()
            .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "无法准备应用连接。"))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "无法打开本机应用端口。"))?;
        let port = listener
            .local_addr()
            .map_err(|_| proxy_error("NODE_APP_PROXY_UNAVAILABLE", "无法读取本机应用端口。"))?
            .port();
        let authority = format!("127.0.0.1:{port}");
        let origin = format!("http://{authority}");
        let authorization = Arc::new(Mutex::new(Authorization {
            bootstrap: None,
            session: Uuid::new_v4().simple().to_string(),
        }));
        let (stopping, stop_rx) = watch::channel(false);
        let state = Arc::new(ProxyState {
            root: Arc::new(root),
            entry,
            authority,
            cookie_name: format!("node_app_session_{port}"),
            origin: origin.clone(),
            backend: format!("http://127.0.0.1:{backend_port}"),
            bearer,
            client,
            authorization: authorization.clone(),
            stopping: stop_rx.clone(),
        });
        let router = Router::new().fallback(handle).with_state(state);
        let task = tokio::spawn(async move {
            let mut stop_rx = stop_rx;
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    if !*stop_rx.borrow() {
                        let _ = stop_rx.changed().await;
                    }
                })
                .await;
        });
        Ok(Self {
            origin,
            authorization,
            stopping,
            task: Some(task),
        })
    }

    /// A fresh one-use URL. It expires after 60 seconds; issuing another URL
    /// revokes an earlier unused URL. It never contains the backend bearer.
    pub fn open_url(&mut self) -> String {
        let token = Uuid::new_v4().simple().to_string();
        let mut authorization = self
            .authorization
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        authorization.bootstrap = Some(Bootstrap {
            token: token.clone(),
            issued: Instant::now(),
        });
        format!("{}{BOOTSTRAP_PATH}?token={token}", self.origin)
    }

    pub async fn stop(&mut self) {
        self.revoke();
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }

    fn revoke(&self) {
        let _ = self.stopping.send(true);
        let mut authorization = self
            .authorization
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        authorization.bootstrap = None;
        authorization.session.clear();
    }
}
impl Drop for NodeAppProxy {
    fn drop(&mut self) {
        self.revoke();
        if let Some(task) = self.task.take() {
            task.abort();
        }
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
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
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
fn exact_header(headers: &HeaderMap, name: header::HeaderName, value: &str) -> bool {
    let mut values = headers.get_all(name).iter();
    values
        .next()
        .is_some_and(|entry| entry.as_bytes() == value.as_bytes())
        && values.next().is_none()
}
fn constant_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |different, (a, b)| different | (a ^ b))
        == 0
}
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

async fn handle(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
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
    if matches!(parts.first().map(String::as_str), Some("api" | "tts")) {
        forward(state, request, parts).await
    } else if safe_method {
        static_file(state, request.method() == Method::HEAD, parts).await
    } else {
        refusal(StatusCode::METHOD_NOT_ALLOWED, "此网页不接受该请求方式。")
    }
}

async fn forward(state: Arc<ProxyState>, request: Request, parts: Vec<String>) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return refusal(StatusCode::METHOD_NOT_ALLOWED, "不支持此请求方式。");
    }
    let (request, body) = request.into_parts();
    let mut stop = state.stopping.clone();
    let bytes = tokio::select! {
        biased;
        _ = stop.changed() => return refusal(StatusCode::SERVICE_UNAVAILABLE, "应用已停止。"),
        result = tokio::time::timeout(Duration::from_secs(15), to_bytes(body, BODY_LIMIT)) => match result {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(_)) => return refusal(StatusCode::PAYLOAD_TOO_LARGE, "请求内容不能超过 64 KiB。"),
            Err(_) => return refusal(StatusCode::REQUEST_TIMEOUT, "读取请求超时，请重试。"),
        }
    };
    let mut url = reqwest::Url::parse(&state.backend).expect("fixed loopback URL is valid");
    url.set_path(&format!("/{}", parts.join("/")));
    url.set_query(request.uri.query());
    let mut upstream = state
        .client
        .request(request.method, url)
        .header(header::AUTHORIZATION, state.bearer.clone())
        .body(bytes);
    for name in [
        header::ACCEPT,
        header::CONTENT_TYPE,
        header::RANGE,
        header::IF_RANGE,
    ] {
        if let Some(value) = request.headers.get(&name) {
            upstream = upstream.header(name, value);
        }
    }
    let response = tokio::select! {
        biased;
        _ = stop.changed() => return refusal(StatusCode::SERVICE_UNAVAILABLE, "应用已停止。"),
        result = tokio::time::timeout(Duration::from_secs(120), upstream.send()) => match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return refusal(StatusCode::BAD_GATEWAY, "应用服务暂时无法连接。"),
            Err(_) => return refusal(StatusCode::GATEWAY_TIMEOUT, "应用响应超时，请重试。"),
        }
    };
    if response.status().is_redirection() || response.status() == StatusCode::SWITCHING_PROTOCOLS {
        return refusal(
            StatusCode::BAD_GATEWAY,
            "应用返回了不支持的跳转或连接方式。",
        );
    }
    let status = response.status();
    let mut headers = HeaderMap::new();
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        if let Some(value) = response.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    let stream = stream::unfold(
        (Box::pin(response.bytes_stream()), state.stopping.clone()),
        |(mut upstream, mut stopping)| async move {
            if *stopping.borrow() {
                return None;
            }
            tokio::select! {
                biased;
                _ = stopping.changed() => None,
                next = upstream.next() => next.map(|result| (
                    result.map_err(|_| io::Error::other("应用连接已结束。")),
                    (upstream, stopping),
                )),
            }
        },
    );
    let mut response = Body::from_stream(stream).into_response();
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    protect(response)
}

async fn static_file(state: Arc<ProxyState>, head: bool, parts: Vec<String>) -> Response {
    let parts = if parts.is_empty() {
        state.entry.clone()
    } else {
        parts
    };
    let root = state.root.clone();
    let checked = parts.clone();
    let file = match tokio::task::spawn_blocking(move || open_file(&root, &checked)).await {
        Ok(Ok(file)) => file,
        _ => return refusal(StatusCode::NOT_FOUND, "没有找到这个网页文件。"),
    };
    let size = match file.metadata() {
        Ok(info) => info.len(),
        Err(_) => return refusal(StatusCode::NOT_FOUND, "无法读取这个网页文件。"),
    };
    let body = if head {
        Body::empty()
    } else {
        let file = tokio::fs::File::from_std(file);
        let chunks = stream::unfold(
            (file, state.stopping.clone()),
            |(mut file, mut stopping)| async move {
                if *stopping.borrow() {
                    return None;
                }
                let mut bytes = vec![0; 16 * 1024];
                tokio::select! {
                    biased;
                    _ = stopping.changed() => None,
                    result = file.read(&mut bytes) => match result {
                        Ok(0) => None,
                        Ok(count) => {
                            bytes.truncate(count);
                            Some((Ok::<_, io::Error>(Bytes::from(bytes)), (file, stopping)))
                        }
                        Err(_) => Some((Err(io::Error::other("网页读取已结束。")), (file, stopping))),
                    }
                }
            },
        );
        Body::from_stream(chunks)
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
    for part in decoded.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        parts.push(part.to_owned());
    }
    Some(parts)
}

#[cfg(unix)]
fn open_at(directory: &File, name: &str, directory_only: bool) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory_only { libc::O_DIRECTORY } else { 0 };
    // Both descriptors and the component name remain alive throughout openat;
    // O_NOFOLLOW applies to each individual component, never just the final path.
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // A successful openat returns a fresh descriptor with sole ownership.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    if (directory_only && !metadata.is_dir()) || (!directory_only && !metadata.is_file()) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if !directory_only {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(file)
}
#[cfg(unix)]
fn open_root(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut directory = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                let name = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
                directory = open_at(&directory, name, true)?;
            }
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(directory)
}
#[cfg(unix)]
fn open_file(root: &File, parts: &[String]) -> io::Result<File> {
    let (last, parents) = parts.split_last().ok_or(io::ErrorKind::InvalidInput)?;
    let mut directory = root.try_clone()?;
    for component in parents {
        directory = open_at(&directory, component, true)?;
    }
    open_at(&directory, last, false)
}
#[cfg(not(unix))]
fn open_root(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Node browser adapter is not yet verified on this platform",
    ))
}
#[cfg(not(unix))]
fn open_file(_root: &File, _parts: &[String]) -> io::Result<File> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::{routing::any, Json};
    use serde_json::json;
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tempfile::TempDir;

    struct Fixture {
        _directory: TempDir,
        root: PathBuf,
        proxy: NodeAppProxy,
        client: reqwest::Client,
        cookie: String,
        calls: Arc<AtomicUsize>,
        received: Arc<Mutex<HeaderMap>>,
        backend_task: JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.backend_task.abort();
        }
    }
    async fn fixture() -> Fixture {
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
                        let body = to_bytes(request.into_body(), BODY_LIMIT).await.unwrap();
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
        fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
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
        f.proxy.stop().await;
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn strict_host_origin_and_fetch_site_protect_backend() {
        let mut f = fixture().await;
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn forward_injects_only_backend_bearer_and_preserves_allowed_headers() {
        let mut f = fixture().await;
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn request_limit_unsupported_methods_and_non_api_routes_do_not_reach_backend() {
        let mut f = fixture().await;
        assert_eq!(
            f.request(Method::POST, "/api/echo")
                .header(header::ORIGIN, &f.proxy.origin)
                .body(vec![b'x'; BODY_LIMIT + 1])
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn backend_redirects_and_cookies_are_never_forwarded_to_page() {
        let mut f = fixture().await;
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn static_pages_have_local_script_policy_and_do_not_serve_links() {
        use std::os::unix::fs::symlink;
        let mut f = fixture().await;
        let response = f.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("script-src 'self';"));
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn each_application_has_a_distinct_cookie_and_rejects_other_app_sessions() {
        let mut first = fixture().await;
        let mut second = fixture().await;
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
        first.proxy.stop().await;
        second.proxy.stop().await;
    }

    #[tokio::test]
    async fn stop_cancels_sse_stream_and_closes_listener() {
        let mut f = fixture().await;
        let mut response = f.request(Method::GET, "/api/events").send().await.unwrap();
        let first = response.chunk().await.unwrap().unwrap();
        assert_eq!(first.as_ref(), b"data: fixture\n\n");
        let address = f.proxy.origin.trim_start_matches("http://").to_string();
        f.proxy.stop().await;
        let next = tokio::time::timeout(Duration::from_secs(1), response.chunk())
            .await
            .expect("stop must cancel open SSE");
        assert!(matches!(next, Ok(None) | Err(_)));
        assert!(tokio::net::TcpStream::connect(&address).await.is_err());
        f.proxy.stop().await;
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
        f.proxy.stop().await;
    }

    #[tokio::test]
    async fn anchored_web_root_does_not_follow_replacement_parent_directory() {
        use std::os::unix::fs::symlink;
        let mut f = fixture().await;
        let saved = f.root.with_file_name("original-web");
        std::fs::rename(&f.root, &saved).unwrap();
        let foreign = f.root.with_file_name("foreign-web");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("index.html"), "foreign-private").unwrap();
        symlink(&foreign, &f.root).unwrap();
        let response = f.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.text().await.unwrap().contains("foreign-private"));
        f.proxy.stop().await;
    }
}
