use super::*;

#[tokio::test]
async fn local_capture_is_explicit_authenticated_and_preserves_author_files() {
    let (temp, _paths, service, client) = tests::start().await;
    let state = service.handle().current();
    let root = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-capture-http");
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("README.md"), "disposable source\n").unwrap();
    std::fs::write(root.join(".env"), "ARTIFICIAL_SECRET=do-not-copy\n").unwrap();
    std::fs::write(
        root.join("data/business.sqlite"),
        "disposable data marker\n",
    )
    .unwrap();
    let endpoint = format!("{}/api/v1/sources/captures", service.info().base_url);
    let body = json!({"sourcePath":root,"name":"Disposable capture HTTP"});
    assert_eq!(
        client
            .post(&endpoint)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client
        .post(&endpoint)
        .bearer_auth(&state.token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response: Value = response.json().await.unwrap();
    let task_id = response["data"]["taskId"].as_str().unwrap();
    let final_task = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = state.builds.task(task_id).unwrap();
            if task.status.is_final() {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        final_task.status,
        crate::TaskStatus::Succeeded,
        "{final_task:?}"
    );
    let list = service.handle().source_list().await.unwrap();
    assert_eq!(list[0]["provider"], "local");
    assert!(list[0]["commit"].is_null());
    let copy = PathBuf::from(list[0]["sourcePath"].as_str().unwrap());
    assert_eq!(
        std::fs::read(copy.join("README.md")).unwrap(),
        b"disposable source\n"
    );
    assert!(!copy.join(".env").exists());
    assert!(!copy.join("data").exists());
    assert_eq!(
        std::fs::read(root.join(".env")).unwrap(),
        b"ARTIFICIAL_SECRET=do-not-copy\n"
    );
    assert_eq!(
        std::fs::read(root.join("data/business.sqlite")).unwrap(),
        b"disposable data marker\n"
    );
    assert!(service.store().list().await.unwrap().is_empty());
    // Node installation has no HTTP endpoint that could bypass native window ownership.
    assert_eq!(
        client
            .post(format!(
                "{}/api/v1/node-install/apply",
                service.info().base_url
            ))
            .bearer_auth(&state.token)
            .json(&json!({"candidateId":"invalid"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    service.shutdown().await.unwrap();
}

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
