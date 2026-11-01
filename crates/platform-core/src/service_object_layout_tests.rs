//! Public management responses must not infer version protection from a marker.
use super::*;

#[tokio::test]
async fn object_layout_is_management_only_and_preserves_runtime_and_identity() {
    let (_temp, paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable object boundary HTTP").await;
    let id = &project.id;
    let root = &project.project_root;
    let handle = service.handle();
    service.store().schema(id, None).await.unwrap();
    let before = handle.project(id).await.unwrap();
    let manifest_before = std::fs::read(root.join("panda.json")).unwrap();
    let database_before = std::fs::read(root.join("data/main.sqlite")).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    let endpoint = format!("{}/api/v1/projects/{id}", service.info().base_url);
    assert_eq!(
        client.get(&endpoint).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let cookie = tests::connect(&service, &client, id).await;
    assert_eq!(
        client
            .get(&endpoint)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client
        .get(&endpoint)
        .bearer_auth(paths.read_control_token().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let report = response.json::<Value>().await.unwrap()["data"].clone();
    let layout = &report["objectLayout"];
    assert_eq!(layout["kind"], "web");
    assert_eq!(layout["runtime"]["webRoot"], json!(root.join("dist")));
    assert_eq!(
        layout["data"]["database"],
        json!(root.join("data/main.sqlite"))
    );
    assert_eq!(layout["build"]["state"], "unavailable");
    assert_eq!(layout["versioning"]["state"], "not-integrated");
    assert_eq!(layout["versioning"]["database"]["state"], "not-inspected");
    assert!(layout["versioning"]["markers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|marker| marker["path"] == json!(root.join(".git")) && marker["scope"] == "project"));
    assert_eq!(report["state"], before["state"]);
    assert_eq!(report["documentEpoch"], before["documentEpoch"]);
    assert_eq!(report["projectRoot"], before["projectRoot"]);
    assert_eq!(
        std::fs::read(root.join("panda.json")).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("data/main.sqlite")).unwrap(),
        database_before
    );
    assert!(root.join(".git").exists());
    assert!(!root.join("version-data").exists());

    // Even a parsed manifest must not disclose derived paths after validation fails.
    let mut manifest: Value = serde_json::from_slice(&manifest_before).unwrap();
    manifest["web"]["root"] = json!("../outside-project");
    std::fs::write(
        root.join("panda.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let invalid = handle.project(id).await.unwrap();
    assert!(invalid["objectLayout"].is_null());
    assert_eq!(invalid["state"], "error");
    service.shutdown().await.unwrap();
}
