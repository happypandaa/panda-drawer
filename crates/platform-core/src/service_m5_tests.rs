use super::*;

#[tokio::test]
async fn author_metadata_has_no_write_api_and_live_sessions_keep_their_manifest_snapshot() {
    let (_temp, _paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable manifest snapshot").await;
    let id = &project.id;
    let cookie = tests::connect(&service, &client, id).await;
    let base = service.info().base_url;
    let path = project.project_root.join("panda.json");
    let original = std::fs::read(&path).unwrap();
    let response = client
        .patch(format!("{base}/api/v1/projects/{id}/metadata"))
        .bearer_auth(&service.shared.snapshot().token)
        .json(&json!({"name":"must not write"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!project.project_root.join("data").exists());
    let mut declaration: Value = serde_json::from_slice(&original).unwrap();
    declaration["app"]["name"] = json!("Edited author name");
    declaration.as_object_mut().unwrap().remove("platform");
    let changed = serde_json::to_vec(&declaration).unwrap();
    std::fs::write(&path, &changed).unwrap();
    let context = client
        .get(format!("{base}/p/{id}/api/v1/context"))
        .header("cookie", &cookie)
        .header("x-platform-document-epoch", &project.document_epoch)
        .send()
        .await
        .unwrap();
    assert_eq!(context.status(), StatusCode::OK);
    let context: Value = context.json().await.unwrap();
    assert_eq!(context["data"]["name"], "Disposable manifest snapshot");
    let next_cookie = tests::connect(&service, &client, id).await;
    let response = client
        .get(format!("{base}/p/{id}/api/v1/context"))
        .header("cookie", next_cookie)
        .header("x-platform-document-epoch", &project.document_epoch)
        .send()
        .await
        .unwrap();
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["error"]["code"], "SDK_NOT_ENABLED");
    assert_eq!(std::fs::read(path).unwrap(), changed);
    assert!(!project.project_root.join("data").exists());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn storage_routes_use_management_identity_and_scan_returns_persistent_accepted_task() {
    let (_temp, paths, service, client) = tests::start().await;
    let project = tests::create(&service, "M5 storage HTTP").await;
    let endpoint = format!(
        "{}/api/v1/projects/{}/storage/scans",
        service.info().base_url,
        project.id
    );
    assert_eq!(
        client
            .post(&endpoint)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client
        .post(&endpoint)
        .bearer_auth(paths.read_control_token().unwrap())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let wire = response.json::<Value>().await.unwrap();
    let id = wire["data"]["taskId"].as_str().unwrap();
    let mut completed = false;
    for _ in 0..300 {
        let task = service.handle().task(id).await.unwrap();
        if task["final"] == true {
            assert_eq!(task["status"], "succeeded", "{task}");
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(completed);
    let info = service.handle().storage_info(&project.id).await.unwrap();
    assert!(info["currentProject"].is_object(), "{info}");
    let response = client
        .get(format!("{}/healthz", service.info().base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!response.contains("installationPath"));
    assert!(!response.contains("workspaceRoot"));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn runtime_declarations_control_resource_policy_and_native_device_gate() {
    let (_temp, _paths, service, client) = tests::start().await;
    let project = tests::create(&service, "M5 runtime policy").await;
    let manifest_path = project.project_root.join("panda.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["runtimeRequirements"] = json!({"network":{"connect":[{"origin":"https://api.example.com","purpose":"public test","dataSent":"query"}],"images":[{"origin":"https://images.example.com","purpose":"art","dataSent":"address"}]},"devices":{"camera":{"purpose":"local photo"}}});
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(service
        .handle()
        .device_permission_declared(&project.id, "camera"));
    assert!(!service
        .handle()
        .device_permission_declared(&project.id, "microphone"));
    let launch = service.handle().launch(&project.id).await.unwrap();
    let ticket = launch.launch_url.split("#ticket=").nth(1).unwrap();
    let exchange = client
        .post(format!("{}/_platform/session", service.info().base_url))
        .header("origin", service.info().base_url)
        .json(&json!({"ticket":ticket}))
        .send()
        .await
        .unwrap();
    let cookie = exchange.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let response = client
        .get(launch.url)
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("connect-src 'self' https://api.example.com"));
    assert!(!csp.contains("connect-src *"));
    let permissions = response.headers()["permissions-policy"].to_str().unwrap();
    assert!(permissions.contains("camera=(self)"));
    assert!(permissions.contains("microphone=()"));
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("platform-runtime-policy"));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_project_directory_remains_diagnostic_without_hiding_other_manager_cards() {
    let (_temp, _paths, service, _client) = tests::start().await;
    let missing = tests::create(&service, "M5 missing diagnostic").await;
    let healthy = tests::create(&service, "M5 healthy card").await;
    std::fs::remove_dir_all(&missing.project_root).unwrap();
    let projects = service.handle().projects().await.unwrap();
    let cards = projects.as_array().unwrap();
    assert!(cards.iter().any(|card| card["id"] == healthy.id));
    let diagnostic = cards.iter().find(|card| card["id"] == missing.id).unwrap();
    assert_eq!(diagnostic["state"], "missing");
    assert!(diagnostic["storage"]["currentProject"].is_null());
    assert!(diagnostic["metadataRevision"].is_null());
    service.shutdown().await.unwrap();
}
