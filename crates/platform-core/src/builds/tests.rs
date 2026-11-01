use super::*;

#[tokio::test]
async fn startup_interrupts_waiting_confirmation_without_retrying_or_writing_project_files() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let id = format!("task_{}", Uuid::new_v4().simple());
    let directory = paths.workspace_root().join(".platform/tasks").join(&id);
    ensure_dir(&directory).unwrap();
    let record = TaskRecord {
        task_id: id.clone(),
        project_id: None,
        task_type: "gitImport".into(),
        status: TaskStatus::WaitingConfirmation,
        phase: "awaiting-build-confirmation".into(),
        result: None,
        error: None,
    };
    atomic_json(&directory.join("task.json"), &record).unwrap();
    let manager = BuildManager::new(paths.clone(), store.clone()).unwrap();
    let recovered = manager.task(&id).unwrap();
    assert_eq!(recovered.status, TaskStatus::Interrupted);
    assert_eq!(recovered.error.unwrap().code, "TASK_INTERRUPTED");
    assert!(fs::read_dir(paths.workspace_root().join("projects"))
        .unwrap()
        .next()
        .is_none());
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn latest_result_is_the_explicit_submission_even_after_unrelated_tasks() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = store
        .create(crate::CreateProjectInput {
            name: "Disposable task selection".into(),
            description: None,
        })
        .await
        .unwrap();
    let manager = BuildManager::new(paths.clone(), store.clone()).unwrap();
    let first = manager
        .create_readonly_operation(&project.id, "build")
        .unwrap();
    build_result::remember(&paths, &first).unwrap();
    manager
        .finish(&first.task_id, Ok(json!({"completedAt":1})))
        .unwrap();
    let latest = manager
        .create_readonly_operation(&project.id, "build")
        .unwrap();
    build_result::remember(&paths, &latest).unwrap();
    manager
        .finish(
            &latest.task_id,
            Err(ApiError::new("BUILD_FAILED", "disposable failure")),
        )
        .unwrap();
    let unrelated = manager
        .create_readonly_operation(&project.id, "storageScan")
        .unwrap();
    manager.finish(&unrelated.task_id, Ok(json!({}))).unwrap();
    assert_eq!(
        manager.latest_build(&project.id).task.unwrap().task_id,
        latest.task_id
    );
    assert!(!project.project_root.join(".platform").exists());
    assert!(paths
        .data_root()
        .join("web-apps")
        .join(&project.id)
        .join("latest-build.json")
        .is_file());
    manager.shutdown().await;
    store.shutdown().await;
}

#[test]
fn waiting_confirmation_is_nonfinal_and_has_an_explicit_wire_state() {
    assert!(!TaskStatus::WaitingConfirmation.is_final());
    assert_eq!(
        json!(TaskStatus::WaitingConfirmation),
        "waitingConfirmation"
    );
}
