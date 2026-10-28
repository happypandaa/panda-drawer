use super::*;
use crate::CreateProjectInput;
async fn fixture() -> (tempfile::TempDir, RuntimePaths, RuntimeService, String) {
    let (temp, paths) = RuntimePaths::for_test();
    let lock = paths.acquire_lock().unwrap();
    let service = RuntimeService::start(paths.clone(), lock).await.unwrap();
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "M4 HTTP disposable".into(),
            description: None,
        })
        .await
        .unwrap();
    std::fs::create_dir_all(project.project_root.join("dist")).unwrap();
    std::fs::write(
        project.project_root.join("dist/index.html"),
        b"<!doctype html><h1>M4</h1>",
    )
    .unwrap();
    let c = rusqlite::Connection::open(project.project_root.join("data/main.sqlite")).unwrap();
    c.execute_batch("CREATE TABLE notes(id TEXT PRIMARY KEY,note TEXT);INSERT INTO notes VALUES('business-key','before');").unwrap();
    drop(c);
    (temp, paths, service, project.id)
}
async fn wait(handle: &RuntimeHandle, task: Value) -> Value {
    let id = task["taskId"].as_str().unwrap();
    for _ in 0..1000 {
        let t = if task["type"] == "delete" {
            handle
                .task_for_project(id, task["projectId"].as_str().unwrap(), "delete")
                .await
                .unwrap()
        } else {
            handle.task(id).await.unwrap()
        };
        if t["final"] == true {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(10)).await
    }
    panic!("task timed out")
}
async fn browser(service: &RuntimeService, id: &str) -> (String, String) {
    let launch = service.handle().launch(id).await.unwrap();
    let ticket = launch.launch_url.split("#ticket=").nth(1).unwrap();
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/_platform/session",
            launch.url.split("/p/").next().unwrap()
        ))
        .header("origin", service.info().base_url)
        .json(&json!({"ticket":ticket}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (cookie, launch.document_epoch)
}
async fn browser_sql(base: &str, id: &str, cookie: &str, epoch: &str, sql: &str) -> Value {
    reqwest::Client::new()
        .post(format!("{base}/p/{id}/api/v1/db/execute"))
        .header("origin", base)
        .header("cookie", cookie)
        .header("x-platform-document-epoch", epoch)
        .json(&json!({"sql":sql}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
#[tokio::test]
async fn permanent_delete_requires_bound_route_and_removes_owned_exported_archives() {
    let (_temp, paths, service, id) = fixture().await;
    let handle = service.handle();
    let archive = paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-before-permanent-delete.zip");
    let exported = wait(
        &handle,
        handle
            .project_export(&id, json!({"destinationPath":archive}))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(exported["status"], "succeeded");
    assert!(archive.is_file());
    let root = service.store().project(&id).unwrap().root;
    let expected_epoch = service.store().registered_epoch(&id).unwrap().unwrap();
    let client = reqwest::Client::new();
    let token = service.shared.snapshot().token.clone();
    let old = client
        .post(format!(
            "{}/api/v1/projects/{id}/trash",
            service.info().base_url
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(!old.status().is_success());
    let old: Value = old.json().await.unwrap();
    assert_eq!(old["error"]["code"], "NOT_FOUND");
    assert!(root.is_dir());
    let missing = client
        .post(format!(
            "{}/api/v1/projects/{id}/deletions",
            service.info().base_url
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(!missing.status().is_success());
    let missing: Value = missing.json().await.unwrap();
    assert_eq!(
        missing["error"]["code"],
        "PROJECT_DELETE_CONFIRMATION_REQUIRED"
    );
    assert!(root.is_dir());
    service.store().begin_workspace_migration().unwrap();
    let blocked = client
        .post(format!(
            "{}/api/v1/projects/{id}/deletions",
            service.info().base_url
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(!blocked.status().is_success());
    let blocked: Value = blocked.json().await.unwrap();
    assert_eq!(blocked["error"]["code"], "WORKSPACE_MIGRATING");
    assert!(root.is_dir());
    service.store().resume_workspace();
    let accepted = client
        .post(format!(
            "{}/api/v1/projects/{id}/deletions",
            service.info().base_url
        ))
        .bearer_auth(&token)
        .json(&json!({"expectedDocumentEpoch":expected_epoch}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted: Value = accepted.json().await.unwrap();
    let deleted = wait(&handle, accepted["data"].clone()).await;
    assert_eq!(deleted["status"], "succeeded", "{deleted}");
    assert_eq!(deleted["result"]["operationOutcome"], "deleted");
    assert_eq!(deleted["result"]["restoreAvailable"], false);
    assert!(!root.exists());
    assert!(!paths.workspace_root().join(".platform/trash").exists());
    assert!(!archive.exists());
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn migration_rotates_runtime_credentials_preserves_data_and_rejects_old_cookie_and_epoch() {
    let (temp, paths, service, id) = fixture().await;
    let handle = service.handle();
    let before = service.info();
    let original_project_root = service.store().project(&id).unwrap().root;
    let old_token = paths.read_control_token().unwrap();
    let (old_cookie, old_epoch) = browser(&service, &id).await;
    let orphan = paths.workspace_root().join("projects/unrecognized orphan");
    std::fs::create_dir(&orphan).unwrap();
    std::fs::write(orphan.join("keep"), b"unknown files preserved").unwrap();
    let destination = temp.path().canonicalize().unwrap().join("migration-target");
    std::fs::create_dir(&destination).unwrap();
    let task = handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let task = wait(&handle, task).await;
    assert_eq!(task["status"], "succeeded", "{task}");
    let after = handle.info();
    assert_ne!(after.instance_id, before.instance_id);
    assert_eq!(after.base_url, before.base_url);
    assert_eq!(after.workspace_root, destination);
    assert!(original_project_root.exists());
    assert!(destination
        .join("projects")
        .join(original_project_root.file_name().unwrap())
        .exists());
    assert_eq!(
        std::fs::read(destination.join("projects/unrecognized orphan/keep")).unwrap(),
        b"unknown files preserved"
    );
    let selected = paths.reinitialize_for_test().unwrap();
    let new_token = selected.read_control_token().unwrap();
    assert_ne!(old_token, new_token);
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{}/api/v1/projects", after.base_url))
            .bearer_auth(old_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let old = browser_sql(
        &after.base_url,
        &id,
        &old_cookie,
        &old_epoch,
        "SELECT * FROM notes",
    )
    .await;
    assert_eq!(old["error"]["code"], "INVALID_SESSION");
    let (new_cookie, new_epoch) = browser(&service, &id).await;
    assert_ne!(new_epoch, old_epoch);
    let rejected = browser_sql(
        &after.base_url,
        &id,
        &new_cookie,
        &old_epoch,
        "DELETE FROM notes",
    )
    .await;
    assert_eq!(rejected["error"]["code"], "PROJECT_RELOAD_REQUIRED");
    let current = browser_sql(
        &after.base_url,
        &id,
        &new_cookie,
        &new_epoch,
        "SELECT * FROM notes",
    )
    .await;
    assert_eq!(current["data"]["rows"].as_array().unwrap().len(), 1);
    let status: Value = client
        .get(format!("{}/api/v1/workspace", after.base_url))
        .bearer_auth(new_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["data"]["workspaceRoot"], json!(destination));
    assert_eq!(status["data"]["retainedRoots"].as_array().unwrap().len(), 1);
    service.shutdown().await.unwrap();
    assert!(!selected.connection_path().exists());
}
#[tokio::test]
async fn target_disappearing_before_commit_resumes_only_source_without_invalidating_old_document() {
    let (temp, paths, service, id) = fixture().await;
    let handle = service.handle();
    let before = handle.info();
    let (cookie, epoch) = browser(&service, &id).await;
    let store = service.store();
    let lease = store.reserve_task(&id, "task_hold_accepted").unwrap();
    let destination = temp.path().canonicalize().unwrap().join("offline-target");
    std::fs::create_dir(&destination).unwrap();
    let task = handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    std::fs::remove_dir(&destination).unwrap();
    drop(lease);
    let final_task = wait(&handle, task).await;
    assert_eq!(final_task["status"], "failed");
    assert_eq!(
        final_task["error"]["details"]["operationOutcome"], "not_committed",
        "{final_task}"
    );
    assert_eq!(handle.info().instance_id, before.instance_id);
    assert_eq!(handle.info().workspace_root, paths.workspace_root());
    let data = browser_sql(
        &before.base_url,
        &id,
        &cookie,
        &epoch,
        "SELECT * FROM notes",
    )
    .await;
    assert_eq!(data["ok"], true, "{data}");
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn deletion_preview_uses_private_identity_without_opening_a_broken_database() {
    let (_temp, paths, service, id) = fixture().await;
    let store = service.store();
    let root = store.project(&id).unwrap().root;
    let epoch = store.registered_epoch(&id).unwrap().unwrap();
    let registration = paths
        .workspace_root()
        .join(".platform/project-state")
        .join(format!("{id}.json"));
    let registration_bytes = std::fs::read(&registration).unwrap();
    let database = root.join("data/main.sqlite");
    std::fs::write(
        &database,
        b"disposable invalid database for deletion preview",
    )
    .unwrap();
    let database_bytes = std::fs::read(&database).unwrap();
    assert!(!store.has_active_project_work(&id));
    let token = service.shared.snapshot().token.clone();
    let url = format!(
        "{}/api/v1/projects/{id}/deletion-preview",
        service.info().base_url
    );
    let client = reqwest::Client::new();
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client.get(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    let preview = &response["data"];
    assert_eq!(preview["projectId"], id);
    assert_eq!(preview["expectedDocumentEpoch"], epoch);
    assert_eq!(preview["projectRoot"], json!(root));
    assert!(preview["deletePaths"]
        .as_array()
        .unwrap()
        .contains(&json!(root)));
    assert_eq!(std::fs::read(&registration).unwrap(), registration_bytes);
    assert_eq!(std::fs::read(&database).unwrap(), database_bytes);
    assert!(
        !store.has_active_project_work(&id),
        "preview must not create a database worker"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn deletion_preview_never_accepts_a_replacement_project_directory() {
    let (_temp, paths, service, id) = fixture().await;
    let root = service.store().project(&id).unwrap().root;
    let displaced = paths.data_root().join("disposable-displaced-web-project");
    std::fs::rename(&root, &displaced).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("foreign-sentinel"), b"keep foreign replacement").unwrap();
    assert!(service.handle().project_delete_preview(&id).await.is_err());
    assert_eq!(
        std::fs::read(root.join("foreign-sentinel")).unwrap(),
        b"keep foreign replacement"
    );
    assert!(displaced.join("data/main.sqlite").exists());
    service.shutdown().await.unwrap();
}
