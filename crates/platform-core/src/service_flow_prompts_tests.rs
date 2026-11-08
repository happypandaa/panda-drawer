//! New handoff and directory routes use disposable projects and real loopback
//! HTTP. No external AI tool or source program is launched by these tests.
use super::*;

#[tokio::test]
async fn app_ai_open_delivers_its_text_and_limits_repeated_requests() {
    let (_temporary, _paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable AI handoff").await;
    let cookie = tests::connect(&service, &client, &project.id).await;
    let origin = tests::app_base(&service, &project.id);
    let mut requests = service.handle().subscribe_ai_open_requests();
    let request = |text: &str| {
        client
            .post(format!("{origin}/_platform/api/v1/ai/open"))
            .header("cookie", &cookie)
            .header("origin", &origin)
            .header("x-platform-document-epoch", &project.document_epoch)
            .json(&json!({"text":text}))
    };
    let empty: Value = request("  ").send().await.unwrap().json().await.unwrap();
    assert_eq!(empty["error"]["code"], "INVALID_INPUT", "{empty}");
    let opened: Value = request("  请检查这个应用的更新失败原因。\n  ")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opened["data"]["requested"], true, "{opened}");
    let event = tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.project_id, project.id);
    assert_eq!(event.app_name, "Disposable AI handoff");
    assert_eq!(event.text, "请检查这个应用的更新失败原因。");
    let repeated: Value = request("重复点击")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        repeated["error"]["code"], "AI_OPEN_TOO_FREQUENT",
        "{repeated}"
    );
    assert!(matches!(
        requests.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn directory_routes_add_web_and_node_once_and_reject_runtime_changes() {
    let (temporary, paths, service, client) = tests::start().await;
    let root = temporary.path().canonicalize().unwrap();
    let handle = service.handle();
    let state = handle.current();
    let request = |action: &str, source: &std::path::Path| {
        client
            .post(format!(
                "{}/api/v1/directories/{action}",
                service.info().base_url
            ))
            .bearer_auth(&state.token)
            .json(&json!({"path":source}))
    };
    let unavailable: Value = request("check", &root)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        unavailable["error"]["code"], "DIRECTORY_ADD_UNAVAILABLE",
        "{unavailable}"
    );
    let apps = Arc::new(
        crate::node_apps::NodeApps::with_runtime(
            paths.data_root().into(),
            root.join("must-not-run-node"),
            &handle,
        )
        .unwrap(),
    );
    handle.attach_directory_apps(&apps);
    for runtime in ["web", "node"] {
        let source = root.join(format!("disposable-cli-{runtime}"));
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(source.join("public")).unwrap();
        std::fs::create_dir(source.join("server")).unwrap();
        std::fs::write(source.join("public/index.html"), "disposable web entry").unwrap();
        std::fs::write(
            source.join("server/managed.mjs"),
            "throw Error('must not run');",
        )
        .unwrap();
        let mut manifest = json!({
            "manifestVersion":1,"runtime":runtime,
            "app":{"name":format!("Disposable {runtime}")},
            "web":{"root":"public","entry":"index.html"}
        });
        if runtime == "node" {
            manifest["backend"] = json!({
                "protocolVersion":1,"root":"server","entry":"managed.mjs","nodeMajor":24
            });
        }
        let manifest_path = source.join("panda.json");
        let original = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(&manifest_path, &original).unwrap();
        let checked: Value = request("check", &source)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(checked["data"]["state"], "ready", "{checked}");
        assert_eq!(std::fs::read(&manifest_path).unwrap(), original);
        let added: Value = request("add", &source)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(added["data"]["added"], true, "{added}");
        assert_eq!(added["data"]["app"]["runtime"], runtime, "{added}");
        let duplicate: Value = request("add", &source)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(duplicate["data"]["alreadyAdded"], true, "{duplicate}");
        assert_eq!(
            duplicate["data"]["existingApp"]["id"],
            added["data"]["app"]["id"]
        );
        assert_eq!(std::fs::read(&manifest_path).unwrap(), original);
        manifest["runtime"] = json!(if runtime == "web" { "node" } else { "web" });
        let changed = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(&manifest_path, &changed).unwrap();
        let rejected: Value = request("add", &source)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            rejected["error"]["code"], "APP_RUNTIME_CHANGED",
            "{rejected}"
        );
        assert_eq!(std::fs::read(&manifest_path).unwrap(), changed);
        std::fs::write(&manifest_path, &original).unwrap();
        assert!(!source.join("data").exists());
        assert!(!source.join("node_modules").exists());
    }
    assert!(apps.running_ids().await.is_empty());
    apps.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
}
