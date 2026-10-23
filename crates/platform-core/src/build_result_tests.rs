use super::*;

fn record(project: &str, kind: &str) -> TaskRecord {
    TaskRecord {
        task_id: format!("task_{}", Uuid::new_v4().simple()),
        project_id: Some(project.into()),
        task_type: kind.into(),
        status: TaskStatus::Succeeded,
        phase: "completed".into(),
        result: Some(json!({"disposable": true})),
        error: None,
    }
}

fn pointer_path(workspace: &Path, project: &str) -> PathBuf {
    workspace
        .join(".platform/project-state/latest-build")
        .join(format!("{project}.json"))
}

fn save_record(paths: &RuntimePaths, record: &TaskRecord) -> PathBuf {
    let path = paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&record.task_id)
        .join("task.json");
    atomic_json(&path, record).unwrap();
    path
}

#[test]
fn recorded_submission_keeps_failed_build_instead_of_newer_completion_or_other_tasks() {
    let (_temp, paths) = RuntimePaths::for_test();
    let workspace = paths.workspace_root();
    let earlier = record("prj_disposable", "build");
    let mut latest = record("prj_disposable", "build");
    latest.status = TaskStatus::Failed;
    latest.result = None;
    latest.error = Some(ApiError::new("BUILD_FAILED", "Disposable failed build"));
    let scan = record("prj_disposable", "storageScan");
    let maintenance = record("prj_disposable", "maintenanceBegin");
    let other_project = record("prj_other_disposable", "build");
    let records = [
        earlier.clone(),
        latest.clone(),
        scan,
        maintenance,
        other_project,
    ]
    .into_iter()
    .map(|record| (record.task_id.clone(), record))
    .collect();
    let times = HashMap::from([
        (
            earlier.task_id.clone(),
            Some(UNIX_EPOCH + Duration::from_secs(2)),
        ),
        (
            latest.task_id.clone(),
            Some(UNIX_EPOCH + Duration::from_secs(1)),
        ),
    ]);
    build_result::remember(workspace, &earlier).unwrap();
    build_result::remember(workspace, &latest).unwrap();
    let pointer = pointer_path(workspace, "prj_disposable");
    let before = fs::read(&pointer).unwrap();
    let modified = fs::metadata(&pointer).unwrap().modified().unwrap();

    let recovered = build_result::recover(workspace, "prj_disposable", &records, &times);
    assert_eq!(recovered.selection, "recorded-submission");
    assert!(recovered.issues.is_empty());
    let selected = recovered.task.unwrap();
    assert_eq!(selected.task_id, latest.task_id);
    assert_eq!(selected.status, TaskStatus::Failed);
    assert_eq!(selected.error.unwrap().code, "BUILD_FAILED");
    assert_eq!(fs::read(&pointer).unwrap(), before);
    assert_eq!(
        fs::metadata(&pointer).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn malformed_pointer_blocks_fallback_and_is_preserved_on_new_submission() {
    let (_temp, paths) = RuntimePaths::for_test();
    let workspace = paths.workspace_root();
    let build = record("prj_disposable", "build");
    let records = HashMap::from([(build.task_id.clone(), build.clone())]);
    let times = HashMap::from([(build.task_id.clone(), Some(UNIX_EPOCH))]);
    let pointer = pointer_path(workspace, "prj_disposable");
    ensure_dir(pointer.parent().unwrap()).unwrap();
    for bytes in [
        b"unfinished JSON".to_vec(),
        serde_json::to_vec(&json!({"schemaVersion":2,"projectId":"prj_disposable","taskId":build.task_id})).unwrap(),
        serde_json::to_vec(&json!({"schemaVersion":1,"projectId":"prj_other","taskId":build.task_id})).unwrap(),
        serde_json::to_vec(&json!({"schemaVersion":1,"projectId":"prj_disposable","taskId":"../outside"})).unwrap(),
        serde_json::to_vec(&json!({"schemaVersion":1,"projectId":"prj_disposable","taskId":build.task_id,"unknown":true})).unwrap(),
        vec![b' '; 4097],
    ] {
        fs::write(&pointer, &bytes).unwrap();
        let recovered = build_result::recover(workspace, "prj_disposable", &records, &times);
        assert_eq!(recovered.selection, "unavailable");
        assert!(recovered.task.is_none());
        assert_eq!(recovered.issues[0].code, "BUILD_RESULT_UNAVAILABLE");
        assert_eq!(build_result::remember(workspace, &build).unwrap_err().code, "BUILD_RESULT_UNAVAILABLE");
        assert_eq!(fs::read(&pointer).unwrap(), bytes);
    }
}

#[test]
fn recorded_pointer_requires_a_matching_build_task() {
    let (_temp, paths) = RuntimePaths::for_test();
    let workspace = paths.workspace_root();
    let build = record("prj_disposable", "build");
    build_result::remember(workspace, &build).unwrap();
    for records in [
        HashMap::new(),
        HashMap::from([(
            build.task_id.clone(),
            TaskRecord {
                task_type: "storageScan".into(),
                ..build.clone()
            },
        )]),
        HashMap::from([(
            build.task_id.clone(),
            TaskRecord {
                project_id: Some("prj_other".into()),
                ..build.clone()
            },
        )]),
    ] {
        let recovered =
            build_result::recover(workspace, "prj_disposable", &records, &HashMap::new());
        assert_eq!(recovered.selection, "unavailable");
        assert!(recovered.task.is_none());
        assert_eq!(recovered.issues[0].code, "BUILD_RESULT_UNAVAILABLE");
    }
}

#[test]
fn linked_and_special_pointer_paths_are_rejected_without_changing_evidence() {
    use std::os::unix::fs::symlink;

    let (_temp, paths) = RuntimePaths::for_test();
    let workspace = paths.workspace_root();
    let build = record("prj_disposable", "build");
    let pointer = pointer_path(workspace, "prj_disposable");
    let outside = paths.data_root().join("disposable-pointer-evidence.json");
    let evidence = b"retained external evidence";
    fs::write(&outside, evidence).unwrap();
    ensure_dir(pointer.parent().unwrap()).unwrap();
    symlink(&outside, &pointer).unwrap();
    assert_eq!(
        build_result::remember(workspace, &build).unwrap_err().code,
        "BUILD_RESULT_UNAVAILABLE"
    );
    assert!(fs::symlink_metadata(&pointer)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&pointer).unwrap();
    fs::hard_link(&outside, &pointer).unwrap();
    assert_eq!(
        build_result::remember(workspace, &build).unwrap_err().code,
        "BUILD_RESULT_UNAVAILABLE"
    );
    fs::remove_file(&pointer).unwrap();
    fs::create_dir(&pointer).unwrap();
    assert_eq!(
        build_result::remember(workspace, &build).unwrap_err().code,
        "BUILD_RESULT_UNAVAILABLE"
    );
    fs::remove_dir(&pointer).unwrap();
    fs::remove_dir(pointer.parent().unwrap()).unwrap();
    let external_dir = paths.data_root().join("disposable-pointer-directory");
    fs::create_dir(&external_dir).unwrap();
    symlink(&external_dir, pointer.parent().unwrap()).unwrap();
    assert_eq!(
        build_result::remember(workspace, &build).unwrap_err().code,
        "BUILD_RESULT_UNAVAILABLE"
    );
    assert!(fs::read_dir(&external_dir).unwrap().next().is_none());
    assert_eq!(fs::read(&outside).unwrap(), evidence);
}

#[test]
fn legacy_selection_labels_uncertainty_and_refuses_missing_or_tied_times() {
    let (_temp, paths) = RuntimePaths::for_test();
    let workspace = paths.workspace_root();
    let earlier = record("prj_disposable", "build");
    let latest = record("prj_disposable", "build");
    let scan = record("prj_disposable", "storageScan");
    let mut records = HashMap::from([(scan.task_id.clone(), scan)]);
    let empty = build_result::recover(workspace, "prj_disposable", &records, &HashMap::new());
    assert_eq!(empty.selection, "none");
    assert!(empty.task.is_none());
    assert!(empty.issues.is_empty());
    records.extend(
        [earlier.clone(), latest.clone()]
            .into_iter()
            .map(|record| (record.task_id.clone(), record)),
    );
    let mut times = HashMap::from([
        (earlier.task_id.clone(), Some(UNIX_EPOCH)),
        (
            latest.task_id.clone(),
            Some(UNIX_EPOCH + Duration::from_secs(1)),
        ),
    ]);
    let recovered = build_result::recover(workspace, "prj_disposable", &records, &times);
    assert_eq!(recovered.selection, "legacy-modified-time");
    assert_eq!(recovered.task.unwrap().task_id, latest.task_id);
    assert_eq!(recovered.issues[0].code, "BUILD_RESULT_LEGACY_ORDER");
    times.insert(earlier.task_id.clone(), times[&latest.task_id]);
    let tied = build_result::recover(workspace, "prj_disposable", &records, &times);
    assert_eq!(tied.selection, "unavailable");
    assert!(tied.task.is_none());
    assert_eq!(tied.issues[0].code, "BUILD_RESULT_ORDER_UNKNOWN");
    times.insert(earlier.task_id.clone(), None);
    let unknown = build_result::recover(workspace, "prj_disposable", &records, &times);
    assert!(unknown.task.is_none());
    assert_eq!(unknown.issues[0].code, "BUILD_RESULT_UNAVAILABLE");
    assert!(!pointer_path(workspace, "prj_disposable").exists());
}

#[tokio::test]
async fn startup_captures_legacy_times_before_interrupting_an_old_build() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let mut earlier = record("prj_disposable", "build");
    earlier.status = TaskStatus::Running;
    let latest = record("prj_disposable", "build");
    for (record, seconds) in [(&earlier, 1), (&latest, 2)] {
        let path = save_record(&paths, record);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    assert_eq!(
        manager.task(&earlier.task_id).unwrap().status,
        TaskStatus::Interrupted
    );
    let recovered = manager.latest_build("prj_disposable");
    assert_eq!(recovered.selection, "legacy-modified-time");
    assert_eq!(recovered.task.unwrap().task_id, latest.task_id);
    assert_eq!(recovered.issues[0].code, "BUILD_RESULT_LEGACY_ORDER");
    manager.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn recorded_build_survives_restart_and_unrelated_maintenance_tasks() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable build result recovery").await;
    let build = record(&project.manifest.id, "build");
    let scan = record(&project.manifest.id, "storageScan");
    save_record(&paths, &build);
    save_record(&paths, &scan);
    build_result::remember(paths.workspace_root(), &build).unwrap();
    for _ in 0..2 {
        let manager =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance = crate::maintenance::MaintenanceManager::new(
            paths.clone(),
            store.clone(),
            manager.clone(),
        )
        .unwrap();
        let status = maintenance.status(&project.manifest.id).unwrap();
        assert_eq!(status["latestBuild"]["selection"], "recorded-submission");
        assert_eq!(status["latestBuild"]["task"]["taskId"], build.task_id);
        assert_eq!(status["latestBuild"]["task"]["status"], "succeeded");
        assert_eq!(status["activeTask"], Value::Null);
        maintenance.shutdown().await;
        manager.shutdown().await;
    }
    store.shutdown().await;
}

#[tokio::test]
async fn rejected_result_pointer_retains_terminal_task_and_releases_project_lease() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store, "Disposable rejected build admission").await;
    // Probe stubs isolate submission persistence; they do not test Node builds.
    let probe_dir = paths.data_root().join("disposable-probe/bin");
    ensure_dir(&probe_dir).unwrap();
    let node = probe_dir.join("node");
    fs::write(&node, "#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'v24.22.0\\n'; elif [ \"$2\" = '--version' ]; then printf '11.19.1\\n'; else exit 87; fi\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(probe_dir.join("npm"), "synthetic probe entry").unwrap();
    atomic_json(
        &paths.data_root().join("toolchain.json"),
        &json!({"schemaVersion":1,"nodePath":node}),
    )
    .unwrap();
    let pointer = pointer_path(paths.workspace_root(), &project.manifest.id);
    ensure_dir(pointer.parent().unwrap()).unwrap();
    let evidence = b"incomplete result pointer must be retained";
    fs::write(&pointer, evidence).unwrap();
    let before_source = input_digest(&project).unwrap();
    let before_data = tree_digest(&project.root.join("data"), false).unwrap();
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let error = manager.submit(&project.manifest.id).await.unwrap_err();
    assert_eq!(error.code, "BUILD_RESULT_UNAVAILABLE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    let task_id = error.details["taskId"].as_str().unwrap();
    let task = manager.task(task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "BUILD_RESULT_UNAVAILABLE");
    assert!(store.active_task(&project.manifest.id).is_none());
    assert!(manager.registry.lock().unwrap().active.is_empty());
    assert!(manager.registry.lock().unwrap().leases.is_empty());
    assert_eq!(fs::read(&pointer).unwrap(), evidence);
    assert_eq!(input_digest(&project).unwrap(), before_source);
    assert_eq!(
        tree_digest(&project.root.join("data"), false).unwrap(),
        before_data
    );
    manager.shutdown().await;
    let restarted = BuildManager::new(paths, store.clone(), "http://127.0.0.1:9".into()).unwrap();
    assert_eq!(restarted.task(task_id).unwrap().status, TaskStatus::Failed);
    assert!(store.active_task(&project.manifest.id).is_none());
    restarted.shutdown().await;
    store.shutdown().await;
}
