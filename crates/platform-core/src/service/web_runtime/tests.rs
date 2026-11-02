//! Core receipts test actual HTTP/SQLite closure; native window behavior is a
//! separate manual acceptance. No user workspace or physical devices are used.
use super::*;
use crate::service::tests::{app_base, app_state, connect, create, start};

#[tokio::test]
async fn native_thread_last_window_release_closes_only_its_web_runtime() {
    let (_temporary, _paths, service, client) = start().await;
    let project = create(&service, "Disposable native-thread web close").await;
    let other = create(&service, "Disposable continuing web application").await;
    let handle = service.handle();
    let (first, window) = handle.launch_window(&project.id).await.unwrap();
    let (_, other_window) = handle.launch_window(&other.id).await.unwrap();
    let cookie = connect(&service, &client, &project.id).await;
    let other_cookie = connect(&service, &client, &other.id).await;
    let origin = app_base(&service, &project.id);
    let other_origin = app_base(&service, &other.id);
    let store = service.store();
    for (application, base, session) in [
        (&project, &origin, &cookie),
        (&other, &other_origin, &other_cookie),
    ] {
        assert_eq!(
            client
                .post(format!("{base}/_platform/api/v1/db/execute"))
                .header("cookie", session)
                .header("origin", base)
                .header("x-platform-document-epoch", &application.document_epoch)
                .json(&json!({"sql":"CREATE TABLE known(value TEXT)"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(store.fixture_has_database_worker(&application.id));
    }

    // Cocoa's Destroyed callback releases the lease outside Tokio. A normal
    // thread reproduces that boundary without opening a real native window.
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        drop(window);
    })
    .join()
    .expect("releasing the last native window must not panic outside Tokio");
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.web_status(&project.id)["state"] != "closed" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(handle.web_status(&project.id)["resourceClosed"], true);
    assert!(!store.fixture_has_database_worker(&project.id));
    assert!(client.get(&first.url).send().await.is_err());

    let health: Value = client
        .get(format!("{}/healthz", service.info().base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["instanceId"], service.info().instance_id);
    assert_eq!(handle.web_status(&other.id)["state"], "running");
    assert!(store.fixture_has_database_worker(&other.id));
    let continuing: Value = client
        .post(format!("{other_origin}/_platform/api/v1/db/execute"))
        .header("cookie", &other_cookie)
        .header("origin", &other_origin)
        .header("x-platform-document-epoch", &other.document_epoch)
        .json(&json!({"sql":"SELECT name FROM sqlite_master WHERE name='known'"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(continuing["data"]["rows"][0]["name"], "known");

    let (reopened, reopened_window) = handle.launch_window(&project.id).await.unwrap();
    assert_eq!(reopened.url, first.url);
    let reopened_cookie = connect(&service, &client, &project.id).await;
    let persisted: Value = client
        .post(format!("{origin}/_platform/api/v1/db/execute"))
        .header("cookie", &reopened_cookie)
        .header("origin", &origin)
        .header("x-platform-document-epoch", &project.document_epoch)
        .json(&json!({"sql":"SELECT name FROM sqlite_master WHERE name='known'"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(persisted["data"]["rows"][0]["name"], "known");
    drop(reopened_window);
    drop(other_window);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn last_window_drains_sqlite_and_partial_json_without_changing_the_next_origin() {
    let (_temporary, _paths, service, client) = start().await;
    let project = create(&service, "Disposable owned web close").await;
    let declaration = project.project_root.join("panda.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&declaration).unwrap()).unwrap();
    manifest["web"] = json!({"root":".","entry":"dist/index.html"});
    std::fs::write(declaration, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let handle = service.handle();
    let (first, window_a) = handle.launch_window(&project.id).await.unwrap();
    let (_, window_b) = handle.launch_window(&project.id).await.unwrap();
    let cookie = connect(&service, &client, &project.id).await;
    let origin = app_base(&service, &project.id);
    let initial = client
        .get(format!("{origin}/_platform/api/v1/context"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let initial: Value = initial.json().await.unwrap();
    assert_eq!(initial["data"]["projectId"], project.id);
    assert_eq!(initial["data"]["documentEpoch"], project.document_epoch);
    assert!(!project.project_root.join("data").exists());
    let execute = client
        .post(format!("{origin}/_platform/api/v1/db/execute"))
        .header("cookie", &cookie)
        .header("origin", &origin)
        .header("x-platform-document-epoch", &project.document_epoch)
        .json(&json!({"sql":"CREATE TABLE known(value TEXT)"}))
        .send()
        .await
        .unwrap();
    assert_eq!(execute.status(), StatusCode::OK);
    let store = service.store();
    assert!(store.fixture_has_database_worker(&project.id));
    for (path, expected) in [
        ("dashboard/nested", StatusCode::OK),
        ("missing.js", StatusCode::NOT_FOUND),
        ("data/private", StatusCode::NOT_FOUND),
        ("data/main.sqlite", StatusCode::NOT_FOUND),
        ("_platform/missing", StatusCode::NOT_FOUND),
    ] {
        let response = client
            .get(format!("{origin}/{path}"))
            .header("cookie", &cookie)
            .header("accept", "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
    drop(window_a);
    assert_eq!(handle.web_status(&project.id)["state"], "running");
    let admitted = store.gate(&project.id).read_owned().await;
    drop(window_b);
    let closer = handle.clone();
    let id = project.id.clone();
    let closing = tokio::spawn(async move { closer.close_web(&id).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.web_status(&project.id)["state"] != "closing" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!closing.is_finished());
    assert_eq!(
        store
            .reserve_web_build(&project.id, "task_disposable_close_barrier")
            .err()
            .unwrap()
            .code,
        "WEB_APP_OPEN"
    );
    drop(admitted);
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(handle.web_status(&project.id)["state"], "closed");
    assert!(!store.fixture_has_database_worker(&project.id));
    assert!(client
        .get(&first.url)
        .header("cookie", &cookie)
        .send()
        .await
        .is_err());
    let (reopened, window) = handle.launch_window(&project.id).await.unwrap();
    assert_eq!(reopened.url, first.url);
    assert_eq!(
        client
            .get(format!("{origin}/_platform/api/v1/context"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = connect(&service, &client, &project.id).await;
    let child = app_state(&service, &project.id).await;
    let authority = origin.strip_prefix("http://").unwrap();
    let mut socket = tokio::net::TcpStream::connect(authority).await.unwrap();
    let request = format!("POST /_platform/api/v1/db/execute HTTP/1.1\r\nHost: {authority}\r\nOrigin: {origin}\r\nCookie: {cookie}\r\nX-Platform-Document-Epoch: {}\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\n\r\n{{\"sql\":\"CREATE TABLE must_not_execute(x)", project.document_epoch);
    socket.write_all(request.as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while child.active.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(window);
    tokio::time::timeout(Duration::from_secs(5), handle.close_web(&project.id))
        .await
        .unwrap()
        .unwrap();
    assert!(!store.fixture_has_database_worker(&project.id));
    let result = store
        .execute(
            &project.id,
            SqlRequest {
                sql: "SELECT name FROM sqlite_master WHERE name='must_not_execute'".into(),
                params: vec![],
                options: Default::default(),
            },
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    assert!(result.rows.is_empty());
    assert!(!store.fixture_has_database_worker(&project.id));
    drop(socket);
    service.shutdown().await.unwrap();
}
