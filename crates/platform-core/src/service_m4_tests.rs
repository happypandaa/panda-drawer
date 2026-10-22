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
        let t = handle.task(id).await.unwrap();
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
async fn trash_restore_requires_new_session_and_old_document_remains_rejected_with_new_cookie() {
    let (_temp, _paths, service, id) = fixture().await;
    let handle = service.handle();
    let base = service.info().base_url;
    let (old_cookie, old_epoch) = browser(&service, &id).await;
    let deleted = wait(
        &handle,
        serde_json::to_value(
            handle
                .current()
                .lifecycle
                .submit_legacy_delete(&id, json!({}))
                .await
                .unwrap(),
        )
        .unwrap(),
    )
    .await;
    assert_eq!(deleted["status"], "succeeded", "{deleted}");
    let trash = deleted["result"]["trashId"].as_str().unwrap();
    let wire = browser_sql(&base, &id, &old_cookie, &old_epoch, "SELECT * FROM notes").await;
    assert_eq!(wire["error"]["code"], "PROJECT_TRASHED");
    let restored = wait(&handle, handle.trash_restore(trash).await.unwrap()).await;
    assert_eq!(restored["status"], "succeeded", "{restored}");
    // Reusing the old Cookie to fetch a new document is also forbidden.
    let response = reqwest::Client::new()
        .get(format!("{base}/p/{id}/"))
        .header("cookie", &old_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (new_cookie, new_epoch) = browser(&service, &id).await;
    assert_ne!(new_epoch, old_epoch);
    let rejected = browser_sql(
        &base,
        &id,
        &new_cookie,
        &old_epoch,
        "INSERT INTO notes VALUES('late','bad')",
    )
    .await;
    assert_eq!(rejected["error"]["code"], "PROJECT_RELOAD_REQUIRED");
    let current = browser_sql(&base, &id, &new_cookie, &new_epoch, "SELECT * FROM notes").await;
    assert_eq!(current["ok"], true);
    assert_eq!(current["data"]["rows"].as_array().unwrap().len(), 1);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn migration_rotates_runtime_credentials_preserves_data_and_rejects_old_cookie_and_epoch() {
    let (temp, paths, service, id) = fixture().await;
    let handle = service.handle();
    let before = service.info();
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
    assert!(paths.workspace_root().join("projects").join(&id).exists());
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
async fn system_trash_delete_retires_old_browser_session_and_preserves_other_project() {
    let (_temporary, paths, service, id) = fixture().await;
    let handle = service.handle();
    let keep = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable other project retained".into(),
            description: None,
        })
        .await
        .unwrap();
    let (cookie, epoch) = browser(&service, &id).await;
    let destination = paths.data_root().join("disposable-system-trash");
    let retained = destination.clone();
    handle
        .current()
        .lifecycle
        .set_system_trash_sink_for_test(move |source| {
            std::fs::rename(source, &destination)?;
            Ok(destination.clone())
        });
    let deleted = wait(
        &handle,
        handle.project_delete(&id, json!({})).await.unwrap(),
    )
    .await;
    assert_eq!(deleted["status"], "succeeded", "{deleted}");
    assert_eq!(deleted["result"]["operationOutcome"], "system_trashed");
    assert_eq!(deleted["result"]["platformRestoreAvailable"], false);
    assert!(retained.join("data/main.sqlite").is_file());
    assert!(service.store().project(&keep.id).is_ok());
    let response = browser_sql(
        &service.info().base_url,
        &id,
        &cookie,
        &epoch,
        "INSERT INTO notes VALUES('late','must fail')",
    )
    .await;
    assert_eq!(response["error"]["code"], "PROJECT_TRASHED");
    assert_eq!(handle.trash_list().await.unwrap(), json!([]));
    service.shutdown().await.unwrap();
}
