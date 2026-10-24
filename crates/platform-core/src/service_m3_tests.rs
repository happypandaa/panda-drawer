//! HTTP-level maintenance safety checks use temporary, isolated workspaces only.
use super::*;
use reqwest::Client;

async fn management(
    service: &RuntimeService,
    client: &Client,
    project_id: &str,
    route: &str,
    body: Value,
) -> Value {
    client
        .post(format!(
            "{}/api/v1/projects/{project_id}/{route}",
            service.info().base_url
        ))
        .bearer_auth(&service.shared.snapshot().token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn wait_task(service: &RuntimeService, client: &Client, accepted: &Value) -> Value {
    assert_eq!(accepted["ok"], true, "{accepted}");
    let task_id = accepted["data"]["taskId"]
        .as_str()
        .expect("accepted operation has a task ID");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let value: Value = client
            .get(format!(
                "{}/api/v1/tasks/{task_id}",
                service.info().base_url
            ))
            .bearer_auth(&service.shared.snapshot().token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(value["ok"], true, "{value}");
        if value["data"]["final"] == true {
            return value["data"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "operation did not finish: {value}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn context(
    service: &RuntimeService,
    client: &Client,
    id: &str,
    cookie: &str,
    epoch: &str,
) -> Value {
    client
        .get(format!("{}/p/{id}/api/v1/context", service.info().base_url))
        .header("cookie", cookie)
        .header("x-platform-document-epoch", epoch)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn browser_sql(
    service: &RuntimeService,
    client: &Client,
    id: &str,
    cookie: &str,
    epoch: &str,
    sql: &str,
) -> Value {
    client
        .post(format!(
            "{}/p/{id}/api/v1/db/execute",
            service.info().base_url
        ))
        .header("cookie", cookie)
        .header("origin", &service.info().base_url)
        .header("x-platform-document-epoch", epoch)
        .json(&json!({"sql":sql,"params":[]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn m3_routes_require_management_auth_and_reject_browser_origin() {
    let (_temp, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "M3 auth fixture").await;
    for route in ["backups", "backups/backup_test", "maintenance"] {
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

#[tokio::test]
async fn maintenance_end_rejects_old_document_even_with_a_new_shared_cookie() {
    let (_temp, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "M3 old document fixture").await;
    let cookie = super::tests::connect(&service, &client, &project.id).await;
    let initial = context(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
    )
    .await;
    assert_eq!(
        initial["data"]["state"]["mode"], "normal",
        "SDK 0.1.0 compatibility"
    );
    let table = management(
        &service,
        &client,
        &project.id,
        "db/execute",
        json!({"sql":"CREATE TABLE notes(value TEXT)","params":[]}),
    )
    .await;
    assert_eq!(table["ok"], true);
    // Materialize an explicit pre-retirement maintenance session. The HTTP
    // checks below exercise its real gates and end route, never new admission.
    let prepared = crate::maintenance::legacy_test_fixtures::historical_operation(
        &service.shared.snapshot().maintenance,
        &project.id,
        "maintenanceBegin",
        crate::maintenance::OperationInput {
            reason: Some("Historical HTTP gating fixture".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(prepared.status, TaskStatus::Succeeded, "{prepared:?}");
    let maintenance = service
        .handle()
        .maintenance_status(&project.id)
        .await
        .unwrap();
    let maintenance_id = maintenance["maintenanceId"].as_str().unwrap();
    assert_eq!(maintenance["readyForChanges"], true);
    let blocked = browser_sql(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
        "INSERT INTO notes VALUES ('browser blocked')",
    )
    .await;
    assert_eq!(blocked["error"]["code"], "PROJECT_MAINTENANCE");
    for maintenance in [Value::Null, json!("maintenance_wrong")] {
        let mut input =
            json!({"sql":"INSERT INTO notes VALUES ('management blocked')","params":[]});
        if !maintenance.is_null() {
            input["maintenanceId"] = maintenance;
        }
        let blocked = management(&service, &client, &project.id, "db/execute", input).await;
        assert_eq!(blocked["ok"], false, "{blocked}");
    }
    let allowed = management(&service, &client, &project.id, "db/execute", json!({"sql":"INSERT INTO notes VALUES ('managed')","params":[],"maintenanceId":maintenance_id})).await;
    assert_eq!(allowed["ok"], true, "{allowed}");
    let during = context(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
    )
    .await;
    assert_eq!(during["data"]["state"]["mode"], "maintenance");
    assert!(
        !during.to_string().contains(maintenance_id),
        "browser context leaks management identifier"
    );
    assert!(during["data"]["state"].get("baselineBackupId").is_none());

    let ended = wait_task(
        &service,
        &client,
        &management(
            &service,
            &client,
            &project.id,
            "maintenance/end",
            json!({"maintenanceId":maintenance_id}),
        )
        .await,
    )
    .await;
    assert_eq!(ended["status"], "succeeded", "{ended}");
    let new_epoch = ended["result"]["documentEpoch"].as_str().unwrap();
    assert_ne!(new_epoch, project.document_epoch);
    let fresh_cookie = super::tests::connect(&service, &client, &project.id).await;
    let stale = browser_sql(
        &service,
        &client,
        &project.id,
        &fresh_cookie,
        &project.document_epoch,
        "INSERT INTO notes VALUES ('stale browser')",
    )
    .await;
    assert_eq!(stale["error"]["code"], "PROJECT_RELOAD_REQUIRED");
    let old_context = context(
        &service,
        &client,
        &project.id,
        &fresh_cookie,
        &project.document_epoch,
    )
    .await;
    assert_eq!(old_context["ok"], true);
    assert_eq!(old_context["data"]["state"]["mode"], "normal");
    assert_eq!(old_context["data"]["state"]["reloadRequired"], true);
    let attachment: Value = client
        .put(format!(
            "{}/p/{}/api/v1/file?path=stale.txt",
            service.info().base_url,
            project.id
        ))
        .header("cookie", &fresh_cookie)
        .header("origin", &service.info().base_url)
        .header("x-platform-document-epoch", &project.document_epoch)
        .body("must not publish")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(attachment["error"]["code"], "PROJECT_RELOAD_REQUIRED");
    assert!(!project
        .project_root
        .join("data/attachments/stale.txt")
        .exists());
    let fresh = browser_sql(
        &service,
        &client,
        &project.id,
        &fresh_cookie,
        new_epoch,
        "INSERT INTO notes VALUES ('fresh browser')",
    )
    .await;
    assert_eq!(fresh["ok"], true, "{fresh}");
    let count = management(
        &service,
        &client,
        &project.id,
        "db/execute",
        json!({"sql":"SELECT count(*) AS count FROM notes","params":[]}),
    )
    .await;
    assert_eq!(count["data"]["rows"][0]["count"], 2, "{count}");
    let page = client
        .get(format!("{}/p/{}/", service.info().base_url, project.id))
        .header("cookie", &fresh_cookie)
        .send()
        .await
        .unwrap();
    assert!(page.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("'nonce-"));
    assert!(page
        .text()
        .await
        .unwrap()
        .contains("local-projects-platform-status"));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn retired_backup_admission_keeps_context_readable_without_freeze_or_document_change() {
    let (_temp, _paths, service, client) = super::tests::start().await;
    let project = super::tests::create(&service, "M3 freeze fixture").await;
    let cookie = super::tests::connect(&service, &client, &project.id).await;
    // Rejected HTTP admission must not wait on the write gate, acquire a freeze,
    // create a task or make an otherwise usable browser session stale.
    let gate = service.store().gate(&project.id);
    let held = gate.read().await;
    for (route, body) in [
        ("backups", json!({"note":"retired HTTP backup fixture"})),
        (
            "maintenance",
            json!({"reason":"retired HTTP baseline fixture"}),
        ),
    ] {
        let rejected = management(&service, &client, &project.id, route, body).await;
        assert_eq!(rejected["ok"], false, "{rejected}");
        assert_eq!(
            rejected["error"]["code"], "BACKUP_WORKFLOW_RETIRED",
            "{rejected}"
        );
        assert_eq!(
            rejected["error"]["details"]["operationOutcome"],
            "not_started"
        );
        assert!(!service.store().has_active_tasks());
    }
    let status = context(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
    )
    .await;
    assert_eq!(status["data"]["state"]["busy"], false);
    assert_eq!(status["data"]["state"]["mode"], "normal");
    let allowed = browser_sql(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
        "SELECT 1",
    )
    .await;
    assert_eq!(allowed["ok"], true, "{allowed}");
    drop(held);
    let status = context(
        &service,
        &client,
        &project.id,
        &cookie,
        &project.document_epoch,
    )
    .await;
    assert_eq!(status["data"]["state"]["busy"], false);
    assert_eq!(status["data"]["state"]["reloadRequired"], false);
    assert_eq!(
        service.store().project(&project.id).unwrap().document_epoch,
        project.document_epoch
    );
    assert_eq!(
        browser_sql(
            &service,
            &client,
            &project.id,
            &cookie,
            &project.document_epoch,
            "SELECT 1"
        )
        .await["ok"],
        true
    );
    service.shutdown().await.unwrap();
}
