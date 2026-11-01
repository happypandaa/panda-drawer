//! Core receipts test actual HTTP/SQLite closure; native window behavior is a
//! separate manual acceptance. No user workspace or physical devices are used.
use super::*;
use crate::service::tests::{app_base, app_state, connect, create, start};

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
