//! TempDir files, actual loopback HTTP and deterministically paused real-file
//! readers. These do not prove WebKit or Node subprocess-tree shutdown.
use super::*;
use crate::shutdown::ShutdownStatus;
use tokio::sync::oneshot;

#[tokio::test]
async fn real_file_response_outlives_old_two_second_abort_and_drains() {
    let f = tests::fixture().await;
    let before = std::fs::read(f.root.join("index.html")).unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    f.proxy.fixture_read_pause(entered, blocked);
    let request = tokio::spawn(f.request(Method::GET, "/").send());
    ready.await.unwrap();
    let owner = f.proxy.owner.clone();
    let drain = tokio::spawn(async move { owner.wait().await });
    tokio::time::sleep(Duration::from_millis(2150)).await;
    assert!(
        !drain.is_finished(),
        "timeout/abort cannot prove a blocked file has closed"
    );
    assert_eq!(f.proxy.owner.status().status, ShutdownStatus::Pending);
    release.send(()).unwrap();
    let response = request.await.unwrap().unwrap();
    let _ = response.bytes().await;
    tokio::time::timeout(Duration::from_secs(3), drain)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(f.proxy.owner.status().is_complete());
    assert!(
        f.proxy
            .owner
            .hooks
            .state
            .lock()
            .unwrap()
            .upgrade()
            .is_none(),
        "serve/connection state still owns the anchored root descriptor"
    );
    assert!(std::fs::read(f.root.join("index.html")).unwrap() == before);
}

#[tokio::test]
async fn unconsumed_and_dropped_body_does_not_hide_a_blocked_real_file() {
    let f = tests::fixture().await;
    let state = f.proxy.owner.hooks.state.lock().unwrap().upgrade().unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    f.proxy.fixture_read_pause(entered, blocked);
    let receive = f
        .proxy
        .owner
        .request(move |lease| static_file(state, false, vec![], false, lease))
        .unwrap();
    let body = receive.await.unwrap();
    ready.await.unwrap();
    f.proxy.owner.begin();
    drop(body);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.proxy.owner.status().status, ShutdownStatus::Pending);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(f.proxy.owner.status().is_complete());
}

#[tokio::test]
async fn dropping_a_stop_waiter_keeps_the_owned_file_drain_alive() {
    let f = tests::fixture().await;
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    f.proxy.fixture_read_pause(entered, blocked);
    let request = tokio::spawn(f.request(Method::GET, "/").send());
    ready.await.unwrap();
    let owner = f.proxy.owner.clone();
    let stop = tokio::spawn(async move { owner.wait().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    stop.abort();
    assert!(stop.await.unwrap_err().is_cancelled());
    assert_eq!(f.proxy.owner.status().status, ShutdownStatus::Pending);
    release.send(()).unwrap();
    let response = request.await.unwrap().unwrap();
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(f.proxy.owner.status().is_complete());
    assert_eq!(f.proxy.owner.status().attempt, 1);
}

#[tokio::test]
async fn accepted_request_transfers_its_lease_to_io_derived_after_seal() {
    let f = tests::fixture().await;
    let state = f.proxy.owner.hooks.state.lock().unwrap().upgrade().unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = oneshot::channel();
    let response = f
        .proxy
        .owner
        .request(move |lease| async move {
            let _ = entered.send(());
            blocked.await.unwrap();
            static_file(state, false, vec![], false, lease).await
        })
        .unwrap();
    ready.await.unwrap();
    f.proxy.owner.begin();
    assert!(f.proxy.owner.lifetime.check_open().is_err());
    release.send(()).unwrap();
    let response = response.await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "derived worker must not attempt a new admission"
    );
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(f.proxy.owner.status().is_complete());
    assert!(f.proxy.owner.request(|_lease| async {}).is_err());
}

#[tokio::test]
async fn file_worker_panic_is_sticky_after_request_failure() {
    let f = tests::fixture().await;
    f.proxy
        .owner
        .hooks
        .panic_read
        .store(true, std::sync::atomic::Ordering::Release);
    if let Ok(response) = f.request(Method::GET, "/").send().await {
        let _ = response.bytes().await;
    }
    let error = tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "NODE_APP_PROXY_DRAIN_BLOCKED");
    let report = f.proxy.owner.status();
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(!report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == "NODE_APP_PROXY_WORKER_JOIN_FAILED"));
    assert!(f.proxy.stop().await.is_err());
    assert_eq!(f.proxy.owner.status().attempt, report.attempt);
}

#[tokio::test]
async fn real_serve_join_panic_is_preserved_after_proxy_drop() {
    let f = tests::fixture().await;
    f.proxy.fixture_serve_panic();
    let weak = Arc::downgrade(&f.proxy.owner);
    assert!(f.proxy.stop().await.is_err());
    let report = f.proxy.owner.status();
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == "NODE_APP_PROXY_SERVE_JOIN_FAILED"));
    drop(f);
    let owner = weak
        .upgrade()
        .expect("failed proxy owner must remain retained");
    assert!(owner.wait().await.is_err());
    assert_eq!(owner.status().status, ShutdownStatus::Blocked);
    assert!(!owner.status().resource_closed);
}

#[tokio::test]
async fn cancelled_start_waiter_keeps_the_real_preflight_worker_owned() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("index.html"), b"artificial startup file").unwrap();
    let owner = ProxyShutdown::new();
    let (entered, ready) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *owner.hooks.open.lock().unwrap() = Some((entered, blocked));
    let startup = owner.clone();
    let request = tokio::spawn(async move {
        NodeAppProxy::start_with_owner(
            root,
            "index.html".into(),
            9,
            "artificial-startup-token".into(),
            startup,
        )
        .await
    });
    ready.await.unwrap();
    request.abort();
    assert!(request.await.is_err_and(|error| error.is_cancelled()));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(owner.status().status, ShutdownStatus::Pending);
    assert!(owner.lifetime.check_open().is_err());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), owner.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(owner.status().is_complete());
    assert!(owner.server_for_test().await);
}

#[tokio::test]
async fn public_start_failure_preserves_original_error_and_cleanup_issues() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("index.html"), b"artificial startup evidence").unwrap();
    let owner = ProxyShutdown::new();
    owner
        .hooks
        .panic_open
        .store(true, std::sync::atomic::Ordering::Release);
    let weak = Arc::downgrade(&owner);
    let result = NodeAppProxy::start_with_owner(
        root.clone(),
        "index.html".into(),
        9,
        "artificial-start-failure-token".into(),
        owner.clone(),
    )
    .await;
    let error = match NodeAppProxy::finish_public_start(result).await {
        Ok(_) => panic!("faulted startup must fail"),
        Err(error) => error,
    };
    assert_eq!(error.code, "NODE_APP_PROXY_WORKER_FAILED");
    assert!(error.details["shutdownIssues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "NODE_APP_PROXY_WORKER_JOIN_FAILED"));
    assert!(std::fs::read(root.join("index.html")).unwrap() == b"artificial startup evidence");
    drop(owner);
    assert_eq!(
        weak.upgrade().unwrap().status().status,
        ShutdownStatus::Blocked
    );
}

#[tokio::test]
async fn actual_upstream_socket_closes_but_stop_still_waits_for_its_driver_join() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("index.html"), b"artificial upstream fixture").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (socket_closed, closed) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0; 1];
            socket.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
            assert!(header.len() < 8192, "artificial HTTP request is bounded");
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n").await.unwrap();
        let mut byte = [0; 1];
        assert_eq!(
            socket.read(&mut byte).await.unwrap(),
            0,
            "owned driver must really close its backend socket"
        );
        let _ = socket_closed.send(());
    });
    let mut proxy = NodeAppProxy::start(
        root,
        "index.html".into(),
        port,
        "artificial-driver-token".into(),
    )
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let bootstrap = client.get(proxy.open_url()).send().await.unwrap();
    let cookie = bootstrap.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (entered, ready) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    *proxy.owner.hooks.driver_closed.lock().unwrap() = Some((entered, wait));
    let mut response = client
        .get(format!("{}/api/events", proxy.origin))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap(), "hello");
    proxy.request_stop();
    let owner = proxy.owner.clone();
    let stopping = tokio::spawn(async move { owner.wait().await });
    tokio::time::timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), closed)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !stopping.is_finished(),
        "closed socket notification cannot substitute for the real driver join"
    );
    assert_eq!(proxy.owner.status().status, ShutdownStatus::Pending);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    backend.await.unwrap();
    assert!(proxy.owner.status().is_complete());
    assert!(
        tokio::net::TcpStream::connect(proxy.origin.trim_start_matches("http://"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn actual_upstream_driver_panic_cannot_be_replaced_with_a_clean_shutdown() {
    let f = tests::fixture().await;
    f.proxy
        .owner
        .hooks
        .panic_driver
        .store(true, std::sync::atomic::Ordering::Release);
    if let Ok(response) = f.request(Method::GET, "/api/echo").send().await {
        let _ = response.bytes().await;
    }
    let error = tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "NODE_APP_PROXY_DRAIN_BLOCKED");
    let report = f.proxy.owner.status();
    assert_eq!(report.status, ShutdownStatus::Blocked);
    assert!(!report.resource_closed);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == "NODE_APP_PROXY_WORKER_JOIN_FAILED"));
    assert!(f.proxy.stop().await.is_err());
}

#[tokio::test]
async fn lost_response_receiver_releases_the_real_upstream_without_waiting_for_global_stop() {
    let f = tests::fixture().await;
    let state = f.proxy.owner.hooks.state.lock().unwrap().upgrade().unwrap();
    let (entered, ready) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    *f.proxy.owner.hooks.driver_closed.lock().unwrap() = Some((entered, wait));
    let response = f
        .proxy
        .owner
        .request(move |lease| {
            forward(
                state,
                Request::builder()
                    .uri("/api/events")
                    .body(Body::empty())
                    .unwrap(),
                vec!["api".into(), "events".into()],
                lease,
            )
        })
        .unwrap();
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.proxy.owner.lifetime.check_open().is_ok(),
        "connection cancellation must not need a global stop"
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), f.proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(f.proxy.owner.status().is_complete());
}

#[tokio::test]
async fn backend_transport_failure_is_a_body_error_after_known_socket_close() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("index.html"), b"artificial transport fixture").unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (release, wait) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0; 1];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() < 8192);
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhi",
            )
            .await
            .unwrap();
        // Publish a genuine partial response, then really close the backend
        // socket before the declared Content-Length, producing a hyper error.
        wait.await.unwrap();
        drop(socket);
    });
    let mut proxy = NodeAppProxy::start(
        root,
        "index.html".into(),
        port,
        "artificial-transport-token".into(),
    )
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let bootstrap = client.get(proxy.open_url()).send().await.unwrap();
    let cookie = bootstrap.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response = client
        .get(format!("{}/api/partial", proxy.origin))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    release.send(()).unwrap();
    assert!(
        response.bytes().await.is_err(),
        "partial backend response must remain a response failure"
    );
    backend.await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), proxy.stop())
        .await
        .unwrap()
        .unwrap();
    assert!(
        proxy.owner.status().is_complete(),
        "a normally joined driver with a closed socket is not unknown shutdown ownership"
    );
}
