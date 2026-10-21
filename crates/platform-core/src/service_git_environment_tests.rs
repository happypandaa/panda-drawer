//! The management API diagnoses tools without entering a business repository.
use super::*;

#[tokio::test]
async fn git_environment_requires_management_auth_and_an_explicit_setting_value() {
    let (_temp, paths, service, client) = tests::start().await;
    let project = tests::create(&service, "Disposable Git environment HTTP").await;
    let cookie = tests::connect(&service, &client, &project.id).await;
    let endpoint = format!("{}/api/v1/environment/git", service.info().base_url);
    let token = paths.read_control_token().unwrap();
    let settings = paths.data_root().join("git-toolchain.json");
    let node_settings = paths.data_root().join("toolchain.json");
    let node_bytes = b"{\"schemaVersion\":1,\"nodePath\":null}\n";
    std::fs::write(&node_settings, node_bytes).unwrap();

    for response in [
        client.get(&endpoint).send().await.unwrap(),
        client
            .get(&endpoint)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
        client
            .put(&endpoint)
            .json(&json!({"gitPath":null}))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        client
            .put(&endpoint)
            .bearer_auth(&token)
            .header("origin", &service.info().base_url)
            .json(&json!({"gitPath":null}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let report: Value = client
        .get(&endpoint)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["data"]["ready"], false);
    assert_eq!(report["data"]["selection"], "managed");
    assert_eq!(report["data"]["authentication"]["state"], "not-checked");
    assert!(!settings.exists(), "a diagnostic must not create settings");
    let original = b"{\"schemaVersion\":1,\"gitPath\":\"/disposable-missing-git\"}\n";
    std::fs::write(&settings, original).unwrap();
    for input in [
        json!({}),
        json!({"gitPath": 3}),
        json!({"gitPath": []}),
        json!({"gitPath":null,"unknown":true}),
        json!({"gitPath":"relative-git"}),
    ] {
        let response: Value = client
            .put(&endpoint)
            .bearer_auth(&token)
            .json(&input)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["ok"], false, "invalid setting accepted: {input}");
        assert_eq!(std::fs::read(&settings).unwrap(), original);
    }
    let clear: Value = client
        .put(&endpoint)
        .bearer_auth(&token)
        .json(&json!({"gitPath":null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(clear["ok"], true);
    assert_eq!(clear["data"]["selection"], "managed");
    assert!(clear["data"]["configuredGitPath"].is_null());
    assert_eq!(std::fs::read(&node_settings).unwrap(), node_bytes);
    assert!(!project.project_root.join(".git").exists());
    assert!(!project.project_root.join("app/.git").exists());
    let current = service.handle().project(&project.id).await.unwrap();
    assert_eq!(current["state"], "ready");
    service.shutdown().await.unwrap();
}
