//! Coordinator integration evidence uses only disposable RuntimeService roots.
use crate::{
    app_windows::{AppWindowConfig, LogicalWindowSize, WindowIdentity},
    node_apps::{DeleteNodeAppInput, NodeApps},
    project_cleanup::{
        DeletedProjectNotice, NativeCleanupStage, NativeProjectCleanup, ProjectKind,
    },
    ApiError, CreateProjectInput, RuntimeHandle, RuntimePaths, RuntimeService,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

#[derive(Default)]
struct NativeFixture {
    events: Mutex<Vec<(NativeCleanupStage, DeletedProjectNotice)>>,
    purge_entered: Notify,
    committed: Notify,
    block_purge: Mutex<Option<Arc<Semaphore>>>,
    fail_purge_once: AtomicBool,
}
impl NativeFixture {
    fn hook(self: &Arc<Self>) -> NativeProjectCleanup {
        let fixture = self.clone();
        Arc::new(move |stage, notice| {
            let fixture = fixture.clone();
            Box::pin(async move {
                fixture.events.lock().unwrap().push((stage, notice));
                if stage == NativeCleanupStage::Purge {
                    fixture.purge_entered.notify_one();
                    if fixture.fail_purge_once.swap(false, Ordering::AcqRel) {
                        return Err(ApiError::new(
                            "DISPOSABLE_NATIVE_PURGE_FAILURE",
                            "Artificial native purge failure",
                        ));
                    }
                    let gate = fixture.block_purge.lock().unwrap().clone();
                    if let Some(gate) = gate {
                        gate.acquire().await.unwrap().forget();
                    }
                }
                if stage == NativeCleanupStage::Committed {
                    fixture.committed.notify_one();
                }
                Ok(())
            })
        })
    }
    fn stages(&self) -> Vec<NativeCleanupStage> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(stage, _)| *stage)
            .collect()
    }
}
async fn start() -> (tempfile::TempDir, RuntimePaths, RuntimeService) {
    let (temp, paths) = RuntimePaths::for_test();
    let runtime = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    (temp, paths, runtime)
}
async fn create(runtime: &RuntimeService, name: &str) -> crate::ProjectInfo {
    runtime
        .store()
        .create(CreateProjectInput {
            name: name.into(),
            description: None,
        })
        .await
        .unwrap()
}
async fn notified(notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(15), notify.notified())
        .await
        .unwrap();
}
async fn final_web(handle: &RuntimeHandle, task: &Value) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let value = handle
                .task_for_project(
                    task["taskId"].as_str().unwrap(),
                    task["projectId"].as_str().unwrap(),
                    "delete",
                )
                .await
                .unwrap();
            if value["final"] == true {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn web_deletion_waits_for_native_purge_then_removes_identity_tasks_and_late_window_writes() {
    use NativeCleanupStage::{Committed, Prepare, Purge};
    let (_temp, paths, runtime) = start().await;
    let a = create(&runtime, "Disposable coordinated web A").await;
    let b = create(&runtime, "Disposable preserved web B").await;
    let handle = runtime.handle();
    let sizes = handle.window_size_store();
    let a_window = WindowIdentity::project(paths.workspace_root(), &a.id);
    let b_window = WindowIdentity::project(paths.workspace_root(), &b.id);
    let a_permit = sizes.permit(&a_window).unwrap();
    let b_permit = sizes.permit(&b_window).unwrap();
    sizes.remember(
        &a_permit,
        AppWindowConfig::default(),
        LogicalWindowSize {
            width: 900.0,
            height: 650.0,
        },
    );
    let b_size = LogicalWindowSize {
        width: 1110.0,
        height: 760.0,
    };
    sizes.remember(&b_permit, AppWindowConfig::default(), b_size);
    sizes.flush().unwrap();
    let owner_a = paths.data_root().join("web-project-ownership").join(&a.id);
    let owner_b = paths.data_root().join("web-project-ownership").join(&b.id);
    let b_preferences = fs::read(owner_b.join("window-size.json")).unwrap();
    let native = Arc::new(NativeFixture::default());
    let release = Arc::new(Semaphore::new(0));
    *native.block_purge.lock().unwrap() = Some(release.clone());
    handle.set_project_cleanup_hook(native.hook());
    let accepted = handle
        .project_delete(&a.id, json!({"expectedDocumentEpoch":a.document_epoch}))
        .await
        .unwrap();
    notified(&native.purge_entered).await;
    let pending = handle
        .task_for_project(accepted["taskId"].as_str().unwrap(), &a.id, "delete")
        .await
        .unwrap();
    assert_eq!(pending["final"], false, "{pending}");
    assert_eq!(native.stages(), [Prepare, Purge]);
    assert!(
        owner_a.join("ownership.json").is_file(),
        "owner must survive native purge"
    );
    assert!(paths
        .workspace_root()
        .join(".platform/tasks")
        .join(accepted["taskId"].as_str().unwrap())
        .is_dir());
    assert!(a_permit.is_revoked());
    assert!(!b_permit.is_revoked());
    assert_eq!(
        fs::read(owner_b.join("window-size.json")).unwrap(),
        b_preferences
    );
    release.add_permits(1);
    notified(&native.committed).await;
    let done = final_web(&handle, &accepted).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(native.stages(), [Prepare, Purge, Committed]);
    assert!(!owner_a.exists());
    assert!(!a.project_root.exists());
    assert!(handle
        .node_task_manager()
        .task(accepted["taskId"].as_str().unwrap())
        .is_err());
    assert!(runtime
        .store()
        .deleted_project_identity_absent(&a.id)
        .unwrap());
    sizes.remember(
        &a_permit,
        AppWindowConfig::default(),
        LogicalWindowSize {
            width: 1500.0,
            height: 900.0,
        },
    );
    sizes.flush().unwrap();
    assert!(
        !owner_a.exists(),
        "a cloned old permit must not recreate a deleted owner"
    );
    assert!(sizes.permit(&a_window).is_err());
    assert_eq!(
        fs::read(owner_b.join("window-size.json")).unwrap(),
        b_preferences
    );
    assert_eq!(
        sizes.initial_size(
            &b_window,
            AppWindowConfig::default(),
            Some(LogicalWindowSize {
                width: 2000.0,
                height: 1300.0
            })
        ),
        b_size
    );
    assert!(runtime.store().project(&b.id).is_ok());
    {
        let events = native.events.lock().unwrap();
        let notice = &events.last().unwrap().1;
        assert_eq!(notice.kind, ProjectKind::Web);
        assert_eq!(notice.id, a.id);
        assert!(notice
            .task_ids
            .iter()
            .any(|id| accepted["taskId"] == id.as_str()));
        assert!(notice.source_roots.contains(&a.project_root));
    }
    runtime.shutdown().await.unwrap();
}

fn node_source(path: &Path) {
    fs::create_dir_all(path.join("public")).unwrap();
    fs::create_dir(path.join("server")).unwrap();
    fs::write(path.join("public/index.html"), "disposable").unwrap();
    fs::write(
        path.join("server/main.mjs"),
        "throw Error('never executed')",
    )
    .unwrap();
    crate::builds::atomic_json(&path.join("local-app.json"), &json!({
        "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
        "app":{"id":"disposable.coordinator.node","name":"Disposable coordinated Node","version":"1.0.0"},
        "web":{"root":"public","entry":"index.html","routing":"files"},
        "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
    })).unwrap();
}
async fn node_phase(apps: &NodeApps, task: &Value, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let value = apps
                .deletion_task_status(
                    task["taskId"].as_str().unwrap(),
                    task["appId"].as_str().unwrap(),
                )
                .unwrap();
            if value["phase"] == expected && value["active"] == false {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn node_native_purge_failure_retries_with_original_null_project_source_task_notice() {
    use NativeCleanupStage::{Committed, Prepare, Purge};
    let (temp, paths, runtime) = start().await;
    let b = create(&runtime, "Disposable Node retry preserved web B").await;
    let handle = runtime.handle();
    let source = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-external-node-source");
    node_source(&source);
    let apps = NodeApps::with_runtime(
        paths.data_root().into(),
        "/disposable/nonexistent-node".into(),
        &handle,
    )
    .unwrap();
    let id = apps.register(source.clone()).await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    let source_id = format!("src_{}", uuid::Uuid::new_v4().simple());
    ownership.record_source(&id, &source_id).unwrap();
    let material = paths
        .workspace_root()
        .join(".platform/sources")
        .join(&source_id);
    fs::create_dir_all(&material).unwrap();
    fs::write(material.join("disposable-material"), "owned source copy").unwrap();
    ownership.record_source(&id, &source_id).unwrap();
    let tasks = handle.node_task_manager();
    let source_task = tasks.create_node_operation(&id, "sourceAcquire").unwrap();
    assert!(source_task.project_id.is_none());
    tasks
        .finish(
            &source_task.task_id,
            Ok(json!({"ownerAppId":id,"sourceId":source_id})),
        )
        .unwrap();
    let native = Arc::new(NativeFixture::default());
    native.fail_purge_once.store(true, Ordering::Release);
    handle.set_project_cleanup_hook(native.hook());
    let preview = apps.delete_preview(&id).await.unwrap();
    let input: DeleteNodeAppInput =
        serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]})).unwrap();
    let accepted = apps.submit_deletion(&id, input).await.unwrap();
    let failed = node_phase(&apps, &accepted, "failed").await;
    assert_eq!(native.stages(), [Prepare, Purge]);
    assert_eq!(failed["canRetry"], true, "{failed}");
    assert!(apps
        .deletion_tasks()
        .unwrap()
        .iter()
        .any(|task| task["taskId"] == accepted["taskId"]));
    assert!(source.join("server/main.mjs").is_file());
    let retried = apps
        .retry_deletion(
            accepted["taskId"].as_str().unwrap(),
            &id,
            failed["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retried["taskId"], accepted["taskId"]);
    notified(&native.committed).await;
    let done = node_phase(&apps, &accepted, "deleted").await;
    assert_eq!(done["result"]["operationOutcome"], "deleted");
    assert_eq!(native.stages(), [Prepare, Purge, Prepare, Purge, Committed]);
    assert!(apps.deletion_tasks().unwrap().is_empty());
    assert!(!paths.data_root().join("node-apps").join(&id).exists());
    assert!(!ownership.directory(&id).unwrap().exists());
    assert!(!material.exists());
    assert!(tasks.task(&source_task.task_id).is_err());
    {
        let events = native.events.lock().unwrap();
        let notice = &events.last().unwrap().1;
        assert_eq!(notice.kind, ProjectKind::Node);
        assert_eq!(notice.id, id);
        assert!(notice.task_ids.contains(&source_task.task_id));
        assert!(notice.source_ids.contains(&source_id));
        assert!(notice.source_roots.contains(&source));
        assert_eq!(
            events[1].1.task_ids, notice.task_ids,
            "retry must retain task identity after its files disappeared"
        );
    }
    assert!(runtime.store().project(&b.id).is_ok());
    apps.stop_all().await.unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_workspace_migration_keeps_the_coordinator_hook_and_window_store() {
    use NativeCleanupStage::{Committed, Prepare, Purge};
    let (temp, paths, runtime) = start().await;
    let project = create(&runtime, "Disposable migrated cleanup coordination").await;
    let handle = runtime.handle();
    let coordinator = handle.project_cleanup();
    let windows = handle.window_size_store();
    let native = Arc::new(NativeFixture::default());
    handle.set_project_cleanup_hook(native.hook());
    let destination = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-public-migration");
    fs::create_dir(&destination).unwrap();
    let task = handle
        .workspace_migrate_backend_fixture(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let migrated = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let current = handle.task(task["taskId"].as_str().unwrap()).await.unwrap();
            if current["final"] == true {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(migrated["status"], "succeeded", "{migrated}");
    assert_eq!(handle.info().workspace_root, destination);
    assert!(Arc::ptr_eq(&coordinator, &handle.project_cleanup()));
    assert!(Arc::ptr_eq(&windows, &handle.window_size_store()));
    let epoch = handle
        .store()
        .registered_epoch(&project.id)
        .unwrap()
        .unwrap();
    let deletion = handle
        .project_delete(&project.id, json!({"expectedDocumentEpoch":epoch}))
        .await
        .unwrap();
    notified(&native.committed).await;
    assert_eq!(final_web(&handle, &deletion).await["status"], "succeeded");
    assert_eq!(native.stages(), [Prepare, Purge, Committed]);
    assert!(
        !project.project_root.exists(),
        "the same owner removes its retained old workspace copy"
    );
    assert!(!destination
        .join("projects")
        .join(project.project_root.file_name().unwrap())
        .exists());
    assert!(!paths
        .data_root()
        .join("web-project-ownership")
        .join(&project.id)
        .exists());
    runtime.shutdown().await.unwrap();
}
