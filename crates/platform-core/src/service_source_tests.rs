use super::*;

#[tokio::test]
async fn retired_source_library_routes_do_not_create_material_or_bypass_authentication() {
    let (_temp, paths, service, client) = tests::start().await;
    let state = service.handle().current();
    for (method, route) in [
        (Method::GET, "sources"),
        (Method::POST, "sources/captures"),
        (Method::POST, "sources/acquisitions"),
        (Method::POST, "sources/inspect"),
        (Method::POST, "node-install/apply"),
    ] {
        let endpoint = format!("{}/api/v1/{route}", service.info().base_url);
        assert_eq!(
            client
                .request(method.clone(), &endpoint)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .request(method, &endpoint)
                .bearer_auth(&state.token)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert!(!paths.workspace_root().join(".platform/sources").exists());
    assert!(!state.store.has_active_tasks());
    service.shutdown().await.unwrap();
}
