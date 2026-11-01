//! Included by service.rs as a test-only child module. No real workspace is used.
use super::*;

#[tokio::test]
async fn task_management_routes_require_admin_identity_and_never_expose_retired_logs() {
    let (_temp, paths) = RuntimePaths::for_test();
    let task_id = "task_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let task_dir = paths.workspace_root().join(".platform/tasks").join(task_id);
    std::fs::create_dir_all(&task_dir).unwrap();
    let record = crate::tasks::TaskRecord {
        task_id: task_id.into(),
        project_id: Some("prj_test".into()),
        task_type: "build".into(),
        status: crate::tasks::TaskStatus::Failed,
        phase: "build".into(),
        result: None,
        error: Some(ApiError::new("BUILD_FAILED", "Controlled test failure")),
    };
    std::fs::write(
        task_dir.join("task.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let log = "build output: test-only-sentinel\n";
    std::fs::write(task_dir.join("head.log"), log).unwrap();
    std::fs::write(
        task_dir.join("log.json"),
        json!({"total":log.len()}).to_string(),
    )
    .unwrap();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let base = &service.info().base_url;

    for suffix in [
        format!("tasks/{task_id}"),
        format!("tasks/{task_id}/logs"),
        "environment".into(),
        "projects/prj_test/handoff".into(),
    ] {
        let response = client
            .get(format!("{base}/api/v1/{suffix}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{suffix}");
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("test-only-sentinel"));
        let response = client
            .get(format!("{base}/api/v1/{suffix}"))
            .bearer_auth(&service.shared.snapshot().token)
            .header("origin", base)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "browser Origin: {suffix}"
        );
    }
    let response = client
        .post(format!("{base}/api/v1/projects/prj_test/builds"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = client
        .put(format!("{base}/api/v1/environment/node"))
        .json(&json!({"nodePath":null}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = client
        .get(format!("{base}/api/v1/tasks/{task_id}"))
        .bearer_auth(&service.shared.snapshot().token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let envelope: Value = response.json().await.unwrap();
    assert_eq!(
        envelope["ok"], true,
        "querying a failed task is a successful read"
    );
    assert_eq!(envelope["data"]["taskId"], task_id);
    assert_eq!(envelope["data"]["status"], "failed");
    assert_eq!(envelope["data"]["final"], true);
    assert_eq!(envelope["data"]["error"]["code"], "BUILD_FAILED");

    for offset in ["6", "-1", "0"] {
        let response = client
            .get(format!(
                "{base}/api/v1/tasks/{task_id}/logs?offset={offset}"
            ))
            .bearer_auth(&service.shared.snapshot().token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("test-only-sentinel"));
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn direct_project_files_and_nested_html_change_after_ai_build_and_reopen() {
    let (_temporary, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "Disposable direct web files").await;
    let pages = project.project_root.join("dist/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        pages.join("index.HTML"),
        "<html><head></head><body><script src='./main.js'></script>initial page</body></html>",
    )
    .unwrap();
    std::fs::write(pages.join("main.js"), "initial script").unwrap();
    let (_, window) = service.handle().launch_window(&project.id).await.unwrap();
    let cookie = super::tests::connect(&service, &client, &project.id).await;
    let base = format!("{}/", super::tests::app_base(&service, &project.id));
    let html = client
        .get(format!("{base}pages/index.HTML"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("initial page"));
    assert!(html.contains("platform-document-epoch"));
    assert!(!html.contains("<base href="));
    let script = client
        .get(format!("{base}pages/main.js"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(script, "initial script");
    drop(window);
    service.handle().close_web(&project.id).await.unwrap();
    // An AI's direct build replaces the same folder. No platform build or input
    // fingerprint is needed before the next application open.
    std::fs::write(pages.join("main.js"), "AI rebuilt script").unwrap();
    std::fs::write(
        project.project_root.join("dist/index.html"),
        "<html><head></head><body>AI rebuilt entry</body></html>",
    )
    .unwrap();
    let (launch, window) = service.handle().launch_window(&project.id).await.unwrap();
    let exchange = client.get(&launch.launch_url).send().await.unwrap();
    assert_eq!(exchange.status(), StatusCode::SEE_OTHER);
    assert_eq!(exchange.headers()[header::LOCATION], "/");
    let cookie = exchange.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = client
        .get(&base)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("AI rebuilt entry"));
    let script = client
        .get(format!("{base}pages/main.js"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(script, "AI rebuilt script");
    assert_eq!(
        client
            .get(format!(
                "{base}_builds/task_11111111111111111111111111111111/index.html"
            ))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(!project.project_root.join(".platform").exists());
    drop(window);
    service.handle().close_web(&project.id).await.unwrap();
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn opening_and_building_use_the_same_admission_until_the_window_really_closes() {
    let (_temporary, _paths, service, _client) = super::tests::start().await;
    let project = super::tests::create(&service, "Disposable build window exclusion").await;
    let declaration = project.project_root.join("panda.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&declaration).unwrap()).unwrap();
    manifest["development"] = json!({"build":{"kind":"npm","script":"bundle:web"}});
    std::fs::write(&declaration, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let (_, window) = service.handle().launch_window(&project.id).await.unwrap();
    assert_eq!(
        service
            .handle()
            .build_project(&project.id)
            .await
            .unwrap_err()
            .code,
        "WEB_APP_OPEN"
    );
    drop(window);
    service.handle().close_web(&project.id).await.unwrap();
    let store = service.store();
    let build = store
        .reserve_web_build(&project.id, &format!("task_{}", Uuid::new_v4().simple()))
        .unwrap();
    assert!(service.handle().launch_window(&project.id).await.is_err());
    drop(build);
    let (_, window) = service.handle().launch_window(&project.id).await.unwrap();
    drop(window);
    service.handle().close_web(&project.id).await.unwrap();
    service.shutdown().await.unwrap();
}
