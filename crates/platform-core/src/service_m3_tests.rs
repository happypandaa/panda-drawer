//! HTTP-level maintenance safety checks use temporary, isolated workspaces only.
use super::*;

#[tokio::test]
async fn m3_routes_require_management_auth_and_reject_browser_origin() {
    let (_temp, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "M3 auth fixture").await;
    for route in ["maintenance"] {
        let url = format!(
            "{}/api/v1/projects/{}/{route}",
            service.info().base_url,
            project.id
        );
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&url)
                .bearer_auth(&service.shared.snapshot().token)
                .header("origin", &service.info().base_url)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    // Retired backup, restore and maintenance begin/end writes are gone; the
    // management gate still answers before routing, so unknown writes stay denied.
    for route in ["backups", "restore", "maintenance", "maintenance/end"] {
        let url = format!(
            "{}/api/v1/projects/{}/{route}",
            service.info().base_url,
            project.id
        );
        assert_eq!(
            client
                .post(&url)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .post(&url)
                .bearer_auth(&service.shared.snapshot().token)
                .header("origin", &service.info().base_url)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        service
            .handle()
            .maintenance_status(&project.id)
            .await
            .unwrap()["mode"],
        "ready"
    );
    service.shutdown().await.unwrap();
}
