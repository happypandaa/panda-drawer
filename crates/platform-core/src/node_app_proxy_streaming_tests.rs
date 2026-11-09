//! Disposable loopback HTTP fixtures; no application data or credentials.
use super::*;
use axum::{routing::any, Json};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
};

struct Fixture {
    _directory: tempfile::TempDir,
    proxy: NodeAppProxy,
    client: reqwest::Client,
    cookie: String,
    backend_stop: Option<oneshot::Sender<()>>,
    backend: JoinHandle<()>,
}

impl Fixture {
    async fn start(router: Router) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send, receive) = oneshot::channel();
        let backend = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = receive.await;
                })
                .await
                .unwrap();
        });
        Self::with_backend(port, backend, Some(send)).await
    }

    async fn with_backend(
        port: u16,
        backend: JoinHandle<()>,
        backend_stop: Option<oneshot::Sender<()>>,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("index.html"), "artificial streaming proxy").unwrap();
        let mut proxy = NodeAppProxy::start(
            root,
            "index.html".into(),
            port,
            "artificial-private-backend-token".into(),
        )
        .await
        .unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let bootstrap = client.get(proxy.open_url()).send().await.unwrap();
        assert_eq!(bootstrap.status(), StatusCode::SEE_OTHER);
        let cookie = bootstrap.headers()[header::SET_COOKIE]
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
            backend_stop,
            backend,
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.proxy.origin))
            .header(header::COOKIE, &self.cookie)
            .header(header::ORIGIN, &self.proxy.origin)
    }

    async fn finish(mut self) {
        tokio::time::timeout(Duration::from_secs(3), self.proxy.stop())
            .await
            .unwrap()
            .unwrap();
        if let Some(stop) = self.backend_stop.take() {
            let _ = stop.send(());
        }
        tokio::time::timeout(Duration::from_secs(3), &mut self.backend)
            .await
            .unwrap()
            .unwrap();
        assert!(self.proxy.owner.status().is_complete());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.backend.abort();
    }
}

#[tokio::test]
async fn large_upload_reaches_backend_before_client_finishes_sending() {
    let (entered, first_received) = oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let backend = Router::new().route(
        "/api/upload",
        any(move |request: Request| {
            let entered = entered.clone();
            async move {
                let mut body = request.into_body().into_data_stream();
                let first = body.next().await.unwrap().unwrap();
                let mut length = first.len();
                if let Some(entered) = entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                while let Some(next) = body.next().await {
                    length += next.unwrap().len();
                }
                Json(json!({"length": length}))
            }
        }),
    );
    let fixture = Fixture::start(backend).await;
    let (release, next_chunk) = oneshot::channel();
    let body = stream::once(async { Ok::<_, io::Error>(Bytes::from(vec![b'a'; 64 * 1024])) })
        .chain(stream::once(async move {
            next_chunk.await.unwrap();
            Ok::<_, io::Error>(Bytes::from(vec![b'b'; 128 * 1024]))
        }));
    let request = fixture
        .request(Method::POST, "/api/upload")
        .body(reqwest::Body::wrap_stream(body));
    let sending = tokio::spawn(request.send());
    tokio::time::timeout(Duration::from_secs(3), first_received)
        .await
        .expect("backend did not see the first chunk before the upload ended")
        .unwrap();
    assert!(!sending.is_finished());
    release.send(()).unwrap();
    let response = sending.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["length"],
        192 * 1024
    );
    let response = fixture
        .request(Method::POST, "/api/upload")
        .body(vec![b'c'; 256 * 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["length"],
        256 * 1024
    );
    fixture.finish().await;
}

fn values(headers: &HeaderMap, name: &str) -> Vec<String> {
    headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn business_headers_roundtrip_with_duplicates_without_transport_or_session_headers() {
    let backend = Router::new().route(
        "/api/headers",
        any(|request: Request| async move {
            let headers = request.headers();
            let sensitive = [
                "cookie",
                "origin",
                "referer",
                "x-platform-admin",
                "x-forwarded-host",
                "x-hop-one",
                "x-hop-two",
                "sec-fetch-site",
            ]
            .map(|name| values(headers, name));
            let captured = json!({
                "authorization": values(headers, "authorization"),
                "idempotency": values(headers, "idempotency-key"),
                "conditional": values(headers, "if-none-match"),
                "eventId": values(headers, "last-event-id"),
                "privacy": values(headers, "sec-gpc"),
                "business": values(headers, "x-business"),
                "sensitive": sensitive,
            });
            let mut response = Json(captured).into_response();
            let headers = response.headers_mut();
            headers.append("x-business-reply", HeaderValue::from_static("first"));
            headers.append("x-business-reply", HeaderValue::from_static("second"));
            headers.append(header::CACHE_CONTROL, HeaderValue::from_static("private"));
            headers.append(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=60"),
            );
            headers.insert(
                header::CONTENT_ENCODING,
                HeaderValue::from_static("identity"),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=fixture.json"),
            );
            headers.insert(
                header::ETAG,
                HeaderValue::from_static("\"artificial-etag\""),
            );
            headers.append(
                header::CONNECTION,
                HeaderValue::from_static("x-hop-reply-one"),
            );
            headers.append(
                header::CONNECTION,
                HeaderValue::from_static("x-hop-reply-two"),
            );
            for name in [
                "x-hop-reply-one",
                "x-hop-reply-two",
                "set-cookie",
                "location",
                "refresh",
                "clear-site-data",
                "access-control-allow-origin",
                "x-platform-private",
            ] {
                headers.insert(
                    name,
                    HeaderValue::from_static("artificial-must-not-forward"),
                );
            }
            headers.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src *"),
            );
            response
        }),
    );
    let fixture = Fixture::start(backend).await;
    let response = fixture
        .request(Method::GET, "/api/headers")
        .header(header::AUTHORIZATION, "Bearer artificial-browser-token")
        .header(header::REFERER, "http://example.invalid/private")
        .header("sec-fetch-site", "same-origin")
        .header("idempotency-key", "artificial-idempotency")
        .header(header::IF_NONE_MATCH, "\"conditional-fixture\"")
        .header("last-event-id", "artificial-event-42")
        .header("sec-gpc", "1")
        .header("x-business", "one")
        .header("x-business", "two")
        .header("x-platform-admin", "must-not-forward")
        .header("x-forwarded-host", "example.invalid")
        .header(header::CONNECTION, "keep-alive, x-hop-one")
        .header(header::CONNECTION, "x-hop-two")
        .header("x-hop-one", "private-one")
        .header("x-hop-two", "private-two")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        values(response.headers(), "x-business-reply"),
        ["first", "second"]
    );
    assert_eq!(
        values(response.headers(), "cache-control"),
        ["private", "max-age=60"]
    );
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "identity");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=fixture.json"
    );
    assert_eq!(response.headers()[header::ETAG], "\"artificial-etag\"");
    assert_ne!(
        response.headers()[header::CONTENT_SECURITY_POLICY],
        "default-src *"
    );
    for name in [
        "x-hop-reply-one",
        "x-hop-reply-two",
        "set-cookie",
        "location",
        "refresh",
        "clear-site-data",
        "access-control-allow-origin",
        "x-platform-private",
    ] {
        assert!(!response.headers().contains_key(name), "{name}");
    }
    let captured = response.json::<Value>().await.unwrap();
    assert_eq!(
        captured["authorization"],
        json!(["Bearer artificial-private-backend-token"])
    );
    assert_eq!(captured["idempotency"], json!(["artificial-idempotency"]));
    assert_eq!(captured["conditional"], json!(["\"conditional-fixture\""]));
    assert_eq!(captured["eventId"], json!(["artificial-event-42"]));
    assert_eq!(captured["privacy"], json!(["1"]));
    assert_eq!(captured["business"], json!(["one", "two"]));
    assert_eq!(
        captured["sensitive"],
        json!([[], [], [], [], [], [], [], []])
    );
    fixture.finish().await;
}

#[tokio::test]
async fn conditional_not_modified_is_not_treated_as_a_redirect() {
    let backend = Router::new().route(
        "/api/cached",
        any(|request: Request| async move {
            assert_eq!(
                request.headers()[header::IF_NONE_MATCH],
                "\"cached-fixture\""
            );
            (
                StatusCode::NOT_MODIFIED,
                [
                    (header::ETAG, "\"cached-fixture\""),
                    (header::CACHE_CONTROL, "private, max-age=60"),
                    (header::CONTENT_LENGTH, "123"),
                ],
            )
        }),
    );
    let fixture = Fixture::start(backend).await;
    let response = fixture
        .request(Method::GET, "/api/cached")
        .header(header::IF_NONE_MATCH, "\"cached-fixture\"")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::ETAG], "\"cached-fixture\"");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, max-age=60"
    );
    assert!(response.bytes().await.unwrap().is_empty());
    fixture.finish().await;
}

async fn incomplete_upload_fixture() -> (Fixture, oneshot::Receiver<()>, oneshot::Receiver<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (received, first_chunk) = oneshot::channel();
    let (closed, socket_closed) = oneshot::channel();
    let backend = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(6), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                request.push(byte[0]);
            }
            let mut data = [0; 1024];
            assert!(socket.read(&mut data).await.unwrap() > 0);
            let _ = received.send(());
            loop {
                match socket.read(&mut data).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => (),
                }
            }
            let _ = closed.send(());
            let _ = socket.shutdown().await;
        })
        .await
        .expect("incomplete artificial upload did not release its backend socket");
    });
    (
        Fixture::with_backend(port, backend, None).await,
        first_chunk,
        socket_closed,
    )
}

fn unfinished_body() -> reqwest::Body {
    reqwest::Body::wrap_stream(
        stream::once(async { Ok::<_, io::Error>(Bytes::from_static(b"first-upload-chunk")) })
            .chain(stream::pending()),
    )
}

#[tokio::test]
async fn stopping_an_unfinished_upload_waits_for_the_actual_driver_join() {
    let (fixture, first_chunk, socket_closed) = incomplete_upload_fixture().await;
    let (entered, driver_closed) = oneshot::channel();
    let (release, blocked) = oneshot::channel();
    *fixture.proxy.owner.hooks.driver_closed.lock().unwrap() = Some((entered, blocked));
    let request = tokio::spawn(
        fixture
            .request(Method::POST, "/api/upload")
            .body(unfinished_body())
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(3), first_chunk)
        .await
        .unwrap()
        .unwrap();
    let owner = fixture.proxy.owner.clone();
    let stopping = tokio::spawn(async move { owner.wait().await });
    tokio::time::timeout(Duration::from_secs(3), driver_closed)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !stopping.is_finished(),
        "socket closure is not driver-join proof"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .unwrap()
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn dropping_the_client_during_upload_releases_the_owned_backend_connection() {
    let (fixture, first_chunk, socket_closed) = incomplete_upload_fixture().await;
    let request = tokio::spawn(
        fixture
            .request(Method::POST, "/api/upload")
            .body(unfinished_body())
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(3), first_chunk)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .expect("client cancellation did not release the upstream upload")
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn client_disconnect_after_completed_upload_cancels_waiting_for_response_headers() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (uploaded, ready) = oneshot::channel();
    let (closed, socket_closed) = oneshot::channel();
    let backend = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(6), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
            assert!(head.contains("content-length: 15\r\n"));
            let mut body = [0; 15];
            socket.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"upload-complete");
            let _ = uploaded.send(());
            let mut bytes = Vec::new();
            let _ = socket.read_to_end(&mut bytes).await;
            let _ = closed.send(());
        })
        .await
        .expect("completed upload kept its backend connection after client cancellation");
    });
    let fixture = Fixture::with_backend(port, backend, None).await;
    let sending = tokio::spawn(
        fixture
            .request(Method::POST, "/api/upload")
            .body("upload-complete")
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    sending.abort();
    assert!(sending.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .expect("pre-response cancellation waited for the 120-second request budget")
        .unwrap();
    fixture.finish().await;
}

async fn early_response_fixture(pause_reading: Duration) -> (Fixture, oneshot::Receiver<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (closed, socket_closed) = oneshot::channel();
    let backend = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(6), async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                head.push(byte[0]);
            }
            let mut bytes = [0; 64 * 1024];
            assert!(socket.read(&mut bytes).await.unwrap() > 0);
            let event = b"data: early-response\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                event.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(event).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            // A peer may reply early, then stop reading the ongoing upload.
            // Its TCP backpressure must not disable our absolute request budget.
            tokio::time::sleep(pause_reading).await;
            loop {
                match socket.read(&mut bytes).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => (),
                }
            }
            let _ = closed.send(());
        })
        .await
        .expect("early-response fixture did not release its backend connection");
    });
    (
        Fixture::with_backend(port, backend, None).await,
        socket_closed,
    )
}

async fn forward_early_response(fixture: &Fixture, body: Body) -> Response {
    let state = fixture
        .proxy
        .owner
        .hooks
        .state
        .lock()
        .unwrap()
        .upgrade()
        .unwrap();
    let mut request = Request::builder().method(Method::POST).uri("/api/upload");
    if let Some(length) = body.size_hint().exact() {
        request = request.header(header::CONTENT_LENGTH, length);
    }
    let request = request.body(body).unwrap();
    let response = fixture
        .proxy
        .owner
        .request(move |lease| {
            forward_with_budget(
                state,
                request,
                vec!["api".into(), "upload".into()],
                lease,
                Duration::from_millis(500),
            )
        })
        .unwrap()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
}

#[tokio::test]
async fn early_response_does_not_remove_an_unfinished_uploads_absolute_budget() {
    let (fixture, socket_closed) = early_response_fixture(Duration::from_millis(700)).await;
    let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
    let body = Body::from_stream(stream::repeat_with(move || {
        Ok::<_, io::Error>(chunk.clone())
    }));
    let mut response = forward_early_response(&fixture, body).await;
    let first = response.body_mut().frame().await.unwrap().unwrap();
    assert_eq!(
        first.into_data().unwrap().as_ref(),
        b"data: early-response\n\n"
    );
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .expect("early response headers removed the unfinished upload deadline")
        .unwrap();
    drop(response);
    fixture.finish().await;
}

#[tokio::test]
async fn completed_upload_allows_sse_to_outlive_the_request_budget() {
    let (fixture, mut socket_closed) = early_response_fixture(Duration::ZERO).await;
    let mut response = forward_early_response(&fixture, Body::from("complete")).await;
    let first = response.body_mut().frame().await.unwrap().unwrap();
    assert_eq!(
        first.into_data().unwrap().as_ref(),
        b"data: early-response\n\n"
    );
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(matches!(
        socket_closed.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .unwrap()
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn known_length_last_frame_keeps_its_deadline_until_socket_writes_finish() {
    let (fixture, socket_closed) = early_response_fixture(Duration::from_millis(700)).await;
    // Windows may accept the whole 32 MiB in one socket write, even with a small
    // SO_SNDBUF. Pause after real header/payload writes to make the remaining
    // final-frame write pending independently of the host's kernel queues.
    *fixture
        .proxy
        .owner
        .hooks
        .socket_write_budget
        .lock()
        .unwrap() = Some(8192);
    let (closed, driver_closed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    *fixture.proxy.owner.hooks.driver_closed.lock().unwrap() = Some((closed, released));
    release.send(()).unwrap();
    // One bounded frame immediately exhausts the source, while the real
    // connection still has an unfinished write and an early response flowing.
    let body = Body::from(Bytes::from(vec![b'k'; 32 * 1024 * 1024]));
    let mut response = forward_early_response(&fixture, body).await;
    let first = response.body_mut().frame().await.unwrap().unwrap();
    assert_eq!(
        first.into_data().unwrap().as_ref(),
        b"data: early-response\n\n"
    );
    tokio::time::timeout(Duration::from_secs(3), driver_closed)
        .await
        .expect("last-frame upload deadline did not release the actual driver")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), socket_closed)
        .await
        .expect("source exhaustion removed the stalled last-frame write budget")
        .unwrap();
    drop(response);
    fixture.finish().await;
}
