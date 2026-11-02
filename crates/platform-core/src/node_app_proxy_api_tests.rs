use super::*;
use axum::Json;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    _directory: tempfile::TempDir,
    proxy: NodeAppProxy,
    client: reqwest::Client,
    cookie: String,
    calls: Arc<AtomicUsize>,
    backend: JoinHandle<()>,
}
impl Fixture {
    async fn new(prefixes: Option<Vec<String>>) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("panda-disposable-api-prefixes-")
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        for (path, contents) in [
            ("index.html", "<h1>disposable static page</h1>"),
            ("v1", "static v1 file"),
            ("api", "static api file"),
        ] {
            std::fs::write(root.join(path), contents).unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = calls.clone();
        let backend_app = Router::new().fallback(move |request: Request| {
            let calls = captured.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "method": request.method().as_str(),
                    "path": request.uri().path(),
                    "query": request.uri().query(),
                    "authenticated": request.headers().get(header::AUTHORIZATION)
                        .is_some_and(|value| value == "Bearer disposable-prefix-token")
                }))
            }
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = tokio::spawn(async move {
            axum::serve(listener, backend_app).await.unwrap();
        });
        let mut proxy = NodeAppProxy::finish_public_start(
            NodeAppProxy::start_with_project_owner(
                root.clone(),
                "index.html".into(),
                port,
                "disposable-prefix-token".into(),
                ProxyShutdown::new(),
                ProxySource {
                    project_root: Some(root),
                    fixed_address: None,
                    api_prefixes: prefixes,
                },
            )
            .await,
        )
        .await
        .unwrap();
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
        Self {
            _directory: directory,
            proxy,
            client,
            cookie,
            calls,
            backend,
        }
    }
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.proxy.origin))
            .header(header::COOKIE, &self.cookie)
            .header(header::ORIGIN, &self.proxy.origin)
    }
    async fn shutdown(mut self) {
        self.proxy.stop().await.unwrap();
        self.backend.abort();
        let _ = (&mut self.backend).await;
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.backend.abort();
    }
}

#[tokio::test]
async fn declared_prefixes_forward_get_post_and_nested_routes_before_static_files() {
    let fixture = Fixture::new(Some(vec!["/v1".into(), "/auth/tokens".into()])).await;
    for (method, path) in [
        (Method::GET, "/v1"),
        (Method::GET, "/v1/"),
        (Method::POST, "/v1/chat?stream=true"),
        (Method::GET, "/auth/tokens/refresh"),
    ] {
        let response = fixture.request(method.clone(), path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response: Value = response.json().await.unwrap();
        assert_eq!(response["method"], method.as_str());
        assert_eq!(response["path"], path.split('?').next().unwrap());
        assert_eq!(response["authenticated"], true);
        if path.contains('?') {
            assert_eq!(response["query"], "stream=true");
        }
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 4);
    fixture.shutdown().await;
}

#[tokio::test]
async fn prefixes_match_whole_segments_and_leave_platform_and_static_routes_separate() {
    let fixture = Fixture::new(Some(vec!["/v1".into(), "/auth/tokens".into()])).await;
    for path in [
        "/v10",
        "/auth/token",
        "/auth/tokens-extra",
        "/api/legacy",
        "/_platform/health",
    ] {
        let response = fixture.request(Method::GET, path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(
        fixture
            .request(Method::POST, "/v10")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    let page = fixture
        .request(Method::GET, "/notes/deep")
        .header(header::ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page
        .text()
        .await
        .unwrap()
        .contains("disposable static page"));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn empty_prefixes_disable_api_proxy_while_omission_preserves_existing_prefixes() {
    let fixture = Fixture::new(Some(vec![])).await;
    for (path, contents) in [("/v1", "static v1 file"), ("/api", "static api file")] {
        let response = fixture.request(Method::GET, path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), contents);
    }
    assert_eq!(
        fixture
            .request(Method::POST, "/api")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    fixture.shutdown().await;

    let fixture = Fixture::new(None).await;
    for path in ["/api", "/tts/audio"] {
        let response = fixture.request(Method::GET, path).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap()["path"], path);
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .request(Method::GET, "/v1")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "static v1 file"
    );
    fixture.shutdown().await;
}
