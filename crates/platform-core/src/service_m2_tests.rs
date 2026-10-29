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
async fn generation_urls_pin_old_assets_and_nested_html_uses_its_own_directory() {
    use crate::builds::BuildProvenance;
    use crate::environment::ExecutableInfo;
    let (_temp, paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "versioned page").await;
    let first = "task_11111111111111111111111111111111";
    let second = "task_22222222222222222222222222222222";
    let prepare = |id: &str, text: &str| {
        let root = project
            .project_root
            .join(format!(".platform/artifacts/{id}/dist/pages"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("index.HTML"),
            "<html><head></head><body><script src='./main.js'></script></body></html>",
        )
        .unwrap();
        std::fs::write(root.join("main.js"), text).unwrap();
    };
    let publish = |build_id: &str| {
        let executable = ExecutableInfo {
            path: PathBuf::from("/test/node"),
            version: "v24.0.0".into(),
        };
        let record = BuildProvenance {
            schema_version: 1,
            build_id: build_id.into(),
            task_id: build_id.into(),
            completed_at: 0,
            digest_rule_version: 1,
            input_digest: "test".into(),
            artifact_digest: "test".into(),
            node: executable.clone(),
            npm: executable,
            sdk_version: Some("0.1.0".into()),
            entry_relative: "pages/index.HTML".into(),
        };
        std::fs::write(
            paths.workspace_root().join(format!(
                ".platform/project-state/builds/{}.json",
                project.id
            )),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    };
    prepare(first, "old");
    publish(first);
    let (native_launch, window_pin) = service.handle().launch_window(&project.id).await.unwrap();
    let cookie = super::tests::connect(&service, &client, &project.id).await;
    let base = format!("{}/p/{}/", service.info().base_url, project.id);
    let html = client
        .get(&base)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains(&format!(
            "<base href=\"/p/{}/_builds/{first}/pages/\">",
            project.id
        )),
        "{html}"
    );
    assert!(html.contains("platform-document-epoch"));
    {
        let access = service.handle().artifact_access();
        let _publication = access.publication().unwrap();
        prepare(second, "new");
        publish(second);
    }
    // A native ticket must keep the exact generation selected before the new
    // build, rather than silently opening the newer initial document.
    let ticket = native_launch.launch_url.split("#ticket=").nth(1).unwrap();
    let exchange = client
        .post(format!("{}/_platform/session", service.info().base_url))
        .header("origin", &service.info().base_url)
        .json(&json!({"ticket":ticket}))
        .send()
        .await
        .unwrap();
    assert_eq!(exchange.status(), StatusCode::OK);
    let envelope: Value = exchange.json().await.unwrap();
    assert_eq!(
        envelope["data"]["url"],
        format!("/p/{}/_builds/{first}/pages/index.HTML", project.id)
    );
    let html = client
        .get(&base)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains(&format!("/_builds/{second}/pages/")));
    for (id, text) in [(first, "old"), (second, "new")] {
        let response = client
            .get(format!("{base}_builds/{id}/pages/main.js"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), text);
    }
    drop(window_pin);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_native_window_without_provenance_uses_its_owned_snapshot_and_diagnostic_tickets_stay_dynamic(
) {
    let (_temp, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "Disposable unversioned native page").await;
    let (launch, window) = service.handle().launch_window(&project.id).await.unwrap();
    let view = window.as_ref().unwrap().build_id().to_owned();
    let view_root = window
        .as_ref()
        .unwrap()
        .root()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(view.starts_with("view_"));
    std::fs::write(
        project.project_root.join("dist/index.html"),
        "<html><head></head><body>new live page</body></html>",
    )
    .unwrap();
    std::fs::write(project.project_root.join("dist/app.js"), "new live script").unwrap();
    let exchange = client
        .post(format!("{}/_platform/session", service.info().base_url))
        .header("origin", &service.info().base_url)
        .json(&json!({"ticket":launch.launch_url.split("#ticket=").nth(1).unwrap()}))
        .send()
        .await
        .unwrap();
    let cookie = exchange.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let envelope: Value = exchange.json().await.unwrap();
    let path = envelope["data"]["url"].as_str().unwrap();
    assert_eq!(path, format!("/p/{}/_builds/{view}/index.html", project.id));
    let html = client
        .get(format!("{}{path}", service.info().base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains(&format!("/_builds/{view}/")));
    assert!(!html.contains("new live page"));
    let script = client
        .get(format!(
            "{}/p/{}/_builds/{view}/app.js",
            service.info().base_url,
            project.id
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(script, "document.body.dataset.ready='true';");
    let diagnostic = service.launch(&project.id).await.unwrap();
    let exchange = client
        .post(format!("{}/_platform/session", service.info().base_url))
        .header("origin", &service.info().base_url)
        .json(&json!({"ticket":diagnostic.launch_url.split("#ticket=").nth(1).unwrap()}))
        .send()
        .await
        .unwrap();
    let envelope: Value = exchange.json().await.unwrap();
    assert_eq!(envelope["data"]["url"], format!("/p/{}/", project.id));
    drop(window);
    tokio::time::timeout(Duration::from_secs(3), async {
        while view_root.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}
