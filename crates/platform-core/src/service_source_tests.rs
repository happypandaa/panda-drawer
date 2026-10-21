use super::*;

#[tokio::test]
async fn source_routes_require_manager_auth_and_reject_unsupported_urls_without_tasks() {
    let (_temp, paths, service, client) = tests::start().await;
    let state = service.handle().current();
    let base = &service.info().base_url;
    let unauthenticated = client
        .get(format!("{base}/api/v1/sources"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let listed: Value = client
        .get(format!("{base}/api/v1/sources"))
        .bearer_auth(&state.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["data"], json!([]));
    let response: Value = client
        .post(format!("{base}/api/v1/sources/acquisitions"))
        .bearer_auth(&state.token)
        .json(&json!({"url":"git@github.com:private/repository.git"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], "SOURCE_URL_INVALID");
    assert!(!paths.workspace_root().join(".platform/sources").exists());
    assert!(!state.store.has_active_tasks());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_source_inspection_is_read_only_and_bad_source_area_does_not_block_projects() {
    let (temp, paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Existing disposable project").await;
    let directory = temp.path().join("local-source");
    std::fs::create_dir(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    std::fs::write(
        directory.join("package.json"),
        r#"{"scripts":{"install":"touch must-not-run"}}"#,
    )
    .unwrap();
    let base = &service.info().base_url;
    let state = service.handle().current();
    let inspected: Value = client
        .post(format!("{base}/api/v1/sources/inspect"))
        .bearer_auth(&state.token)
        .json(&json!({"sourcePath":directory}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inspected["ok"], true, "{inspected}");
    assert_eq!(
        inspected["data"]["inspection"]["executionState"],
        "not-executed"
    );
    assert_eq!(inspected["data"]["inspection"]["kind"], "needs-adaptation");
    assert!(!directory.join("must-not-run").exists());
    assert_eq!(service.store().list().await.unwrap().len(), 1);
    service.shutdown().await.unwrap();
    std::fs::write(
        paths.workspace_root().join(".platform/sources"),
        "preserved bad root",
    )
    .unwrap();
    let lock = paths.acquire_lock().unwrap();
    let reopened = RuntimeService::start(paths.clone(), lock).await.unwrap();
    assert!(reopened.handle().source_list().await.is_err());
    assert_eq!(reopened.store().list().await.unwrap()[0].id, project.id);
    assert_eq!(
        std::fs::read_to_string(paths.workspace_root().join(".platform/sources")).unwrap(),
        "preserved bad root"
    );
    reopened.shutdown().await.unwrap();
}
