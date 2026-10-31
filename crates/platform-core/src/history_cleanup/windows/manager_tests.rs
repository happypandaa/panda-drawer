use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

type Fixture = (
    tempfile::TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    Arc<BuildManager>,
    Arc<CleanupManager>,
);
fn fixture() -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let cleanup = CleanupManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        "disposable-w5-history".into(),
    )
    .unwrap();
    (temporary, paths, store, tasks, cleanup)
}
fn task(tasks: &BuildManager, owner: Option<&str>, candidate: &str, path: &Path) -> TaskRecord {
    let task = tasks
        .create_workspace_operation("workspaceCleanupPreview")
        .unwrap();
    tasks
        .finish(
            &task.task_id,
            Ok(json!({"items":[{"id":candidate,"projectId":owner,"path":path}]})),
        )
        .unwrap();
    tasks.task(&task.task_id).unwrap()
}

fn plan(paths: &RuntimePaths, owner: &str, candidate: &str, path: &Path, task: &str) -> Plan {
    let mut item = Candidate::new(
        candidate.into(),
        "restoreOriginal",
        "disposable typed plan",
        path.into(),
    );
    item.project_id = Some(owner.into());
    Plan {
        schema_version: 1,
        plan_id: format!("cleanup_plan_{}", Uuid::new_v4().simple()),
        instance_id: "disposable-w5-history".into(),
        workspace_root: paths.workspace_root().into(),
        workspace_identity: directory_identity(paths.workspace_root()).unwrap(),
        expires_at: now_ms() + PLAN_TTL,
        items: vec![item],
        task_id: Some(task.into()),
        selected_ids: vec![],
        preview_task_id: task.into(),
    }
}

#[tokio::test]
async fn deletion_forget_waits_actual_history_runner_and_invalidates_only_matching_typed_results() {
    let (_temporary, paths, store, tasks, cleanup) = fixture();
    let owner = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let source = format!("src_{}", uuid::Uuid::new_v4().simple());
    let root = paths.workspace_root().join("disposable-owned-source");
    let selected = task(&tasks, Some(&owner), "owned", &root);
    let protected = task(&tasks, Some(&other), &source, &root);
    let source_task = task(
        &tasks,
        None,
        &source,
        &paths.workspace_root().join("other-path"),
    );
    let path_task = task(&tasks, None, "path-only", &root.join("child"));
    let unrelated = task(
        &tasks,
        None,
        "unrelated",
        &paths.workspace_root().join("unrelated"),
    );
    let malformed = tasks
        .create_workspace_operation("workspaceCleanupPreview")
        .unwrap();
    tasks
        .finish(
            &malformed.task_id,
            Ok(json!({"items":[{"nested":{"projectId":owner,"path":root},"path":root}]})),
        )
        .unwrap();
    *cleanup.coordinator.latest.lock().unwrap() = Some(selected.task_id.clone());
    let mut owned_plan = plan(&paths, &owner, "owned", &root, &selected.task_id);
    owned_plan.preview_task_id = source_task.task_id.clone();
    cleanup
        .plans
        .lock()
        .unwrap()
        .insert("owned-plan".into(), owned_plan);
    cleanup.plans.lock().unwrap().insert(
        "other-plan".into(),
        plan(&paths, &other, &source, &root, &protected.task_id),
    );
    let (release, released) = tokio::sync::oneshot::channel();
    let joined = Arc::new(AtomicBool::new(false));
    let observed = joined.clone();
    cleanup
        .coordinator
        .running
        .lock()
        .unwrap()
        .push(tokio::spawn(async move {
            released.await.unwrap();
            observed.store(true, Ordering::Release);
        }));
    let sources = vec![source];
    let roots = vec![root];
    let forgetting = cleanup.forget_deleted_project(&owner, &sources, &roots);
    tokio::pin!(forgetting);
    tokio::select! {
        biased;
        _ = &mut forgetting => panic!("forget must join the real registered history runner"),
        _ = tokio::task::yield_now() => (),
    }
    assert!(cleanup
        .coordinator
        .deleting
        .lock()
        .unwrap()
        .contains(&owner));
    assert!(!joined.load(Ordering::Acquire));
    release.send(()).unwrap();
    let ids = forgetting.await.unwrap();
    assert!(joined.load(Ordering::Acquire));
    for id in [&selected.task_id, &source_task.task_id, &path_task.task_id] {
        assert!(ids.contains(id));
        assert!(tasks.task(id).is_err());
        assert!(!paths
            .workspace_root()
            .join(".platform/tasks")
            .join(id)
            .exists());
    }
    for id in [&protected.task_id, &unrelated.task_id, &malformed.task_id] {
        assert!(!ids.contains(id));
        assert!(tasks.task(id).is_ok());
    }
    assert!(cleanup.latest_task().unwrap().is_none());
    assert!(!cleanup.plans.lock().unwrap().contains_key("owned-plan"));
    assert!(cleanup.plans.lock().unwrap().contains_key("other-plan"));
    cleanup.finish_deleted_project(&owner);
    assert!(cleanup.coordinator.deleting.lock().unwrap().is_empty());
    assert!(cleanup.shutdown().await.is_empty());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn restarted_history_forget_uses_persisted_typed_tasks_and_join_failure_retains_them() {
    let (_temporary, paths, store, tasks, cleanup) = fixture();
    let owner = uuid::Uuid::new_v4().to_string();
    let selected = task(
        &tasks,
        Some(&owner),
        "owned",
        &paths.workspace_root().join("disposable-source"),
    );
    cleanup
        .coordinator
        .running
        .lock()
        .unwrap()
        .push(tokio::spawn(async {
            panic!("controlled disposable history worker failure");
        }));
    let error = cleanup
        .forget_deleted_project(&owner, &[], &[])
        .await
        .unwrap_err();
    assert_eq!(error.code, "WORKSPACE_CLEANUP_BLOCKED");
    assert!(tasks.task(&selected.task_id).is_ok());
    assert!(cleanup
        .coordinator
        .deleting
        .lock()
        .unwrap()
        .contains(&owner));
    cleanup.finish_deleted_project(&owner);
    cleanup.shutdown().await;
    tasks.shutdown().await;
    let next_tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let next = CleanupManager::new(
        paths,
        store.clone(),
        next_tasks.clone(),
        "disposable-history-restarted".into(),
    )
    .unwrap();
    assert!(next.plans.lock().unwrap().is_empty());
    assert_eq!(
        next.forget_deleted_project(&owner, &[], &[]).await.unwrap(),
        vec![selected.task_id.clone()]
    );
    assert!(next_tasks.task(&selected.task_id).is_err());
    next.finish_deleted_project(&owner);
    next.shutdown().await;
    next_tasks.shutdown().await;
    store.shutdown().await;
}
