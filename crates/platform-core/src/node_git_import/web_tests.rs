//! Shared, offline transaction regressions run unchanged on macOS and Windows.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    apps: Arc<NodeApps>,
    manager: Arc<NodeGitImportManager>,
    journal: Journal,
}
async fn fixture() -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(
        ProjectStore::with_paths(
            paths.workspace_root().into(),
            paths.data_root().into(),
            None,
            Default::default(),
        )
        .unwrap(),
    );
    let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().into(),
            temporary.path().join("must-not-run-node"),
        )
        .unwrap(),
    );
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let manager = NodeGitImportManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        Err(invalid("No network or tools in transaction fixture")),
    )
    .unwrap();
    let id = Uuid::new_v4().to_string();
    manager.ownership().initialize(&id).unwrap();
    let task = tasks.create_node_operation(&id, "nodeGitImport").unwrap();
    let root = paths
        .workspace_root()
        .join("project-sources/disposable-web-transaction");
    private_dir(&root).unwrap();
    fs::write(root.join("index.html"), "disposable transaction entry").unwrap();
    atomic_json(&root.join("panda.json"), &json!({"manifestVersion":1,"runtime":"web","app":{"name":"可丢弃事务回归"},"web":{"root":".","entry":"index.html"}})).unwrap();
    let identity = capture(&root).unwrap();
    manager
        .ownership()
        .reserve_published_source_identity(&id, "disposable-web-transaction", &identity)
        .unwrap();
    let journal = Journal {
        version: 1,
        input: StartNodeGitImportInput {
            catalog_id: None,
            import_id: Uuid::new_v4().to_string(),
            url: "https://github.com/example/disposable-web.git".into(),
            branch: None,
            name: None,
        },
        app_id: id.clone(),
        task_id: task.task_id,
        source_id: format!("src_{}", Uuid::new_v4().simple()),
        workspace: paths.workspace_root().into(),
        workspace_identity: capture(paths.workspace_root()).unwrap(),
        owner_identity: Some(capture(&manager.ownership().directory(&id).unwrap()).unwrap()),
        runtime: Some(PandaRuntime::Web),
        web_project_id: Some(format!("prj_{}", Uuid::parse_str(&id).unwrap().simple())),
        build_confirmation: None,
        retiring_control: false,
        phase: "web-reserving".into(),
        registration_attempted: false,
        commit_state: CommitState::NotCommitted,
        source_path: Some(root.clone()),
        source_identity: Some(identity),
        manifest_digest: Some(format!(
            "{:x}",
            Sha256::digest(read_manifest(&root).unwrap())
        )),
        temporary: BTreeMap::new(),
        cleanup_pending: false,
        remaining_paths: vec![],
        app: None,
        final_task: None,
    };
    manager.save(&journal).unwrap();
    Fixture {
        _temporary: temporary,
        paths,
        store,
        tasks,
        apps,
        manager,
        journal,
    }
}
fn reserve(f: &Fixture) {
    let j = &f.journal;
    f.store
        .reserve_managed_source(
            j.web_project_id.as_ref().unwrap(),
            &j.task_id,
            j.source_path.as_ref().unwrap(),
            j.source_identity.as_ref().unwrap(),
        )
        .unwrap();
    f.tasks
        .bind_web_operation(&j.task_id, j.web_project_id.as_ref().unwrap())
        .unwrap();
}
#[tokio::test]
async fn durable_reservation_recovery_retires_import_rights_and_removes_unregistered_web() {
    let f = fixture().await;
    reserve(&f);
    // Crash before release_imported_source; recovery must never let two owners
    // independently delete or retain this source.
    let result = f.manager.recover(f.apps.clone()).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].commit_state, CommitState::NotCommitted);
    assert!(!result[0].cleanup_pending, "{:?}", result[0].task);
    assert!(result[0].task.project_id.is_none());
    assert!(!f.journal.source_path.as_ref().unwrap().exists());
    assert!(!f
        .paths
        .data_root()
        .join("web-apps")
        .join(f.journal.web_project_id.as_ref().unwrap())
        .exists());
    assert!(!f
        .manager
        .ownership()
        .directory(&f.journal.app_id)
        .unwrap()
        .exists());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn registration_is_the_commit_point_even_if_the_import_reply_is_lost() {
    let f = fixture().await;
    reserve(&f);
    let j = &f.journal;
    let root = j.source_path.as_ref().unwrap();
    let id = j.web_project_id.as_ref().unwrap();
    f.store
        .register_managed(id, &j.task_id, root)
        .await
        .unwrap();
    // Original import rights were not yet retired and its journal still says
    // registration was not attempted. The actual registration wins.
    let result = f.manager.recover(f.apps.clone()).await.unwrap();
    assert_eq!(
        result[0].commit_state,
        CommitState::Committed,
        "{:?}",
        result[0].task
    );
    assert!(!result[0].cleanup_pending);
    assert!(root.join("index.html").is_file());
    assert!(f.store.is_registered(id).unwrap());
    assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn unreadable_web_registration_preserves_source_until_actual_commit_is_known() {
    let f = fixture().await;
    reserve(&f);
    let j = &f.journal;
    let id = j.web_project_id.as_ref().unwrap();
    let root = j.source_path.as_ref().unwrap();
    f.store
        .register_managed(id, &j.task_id, root)
        .await
        .unwrap();
    let path = f
        .paths
        .data_root()
        .join("web-apps")
        .join(id)
        .join("registration.json");
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, b"broken registration").unwrap();
    let result = f.manager.recover(f.apps.clone()).await.unwrap();
    assert_eq!(result[0].commit_state, CommitState::Unknown);
    assert!(result[0].cleanup_pending);
    assert!(root.is_dir());
    fs::write(&path, bytes).unwrap();
    let retried = f
        .manager
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert_eq!(
        retried.commit_state,
        CommitState::Committed,
        "{:?}",
        retried.task
    );
    assert!(!retried.cleanup_pending);
    assert!(root.is_dir());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn rollback_retries_after_source_removal_without_recreating_or_adopting_private_directory() {
    let f = fixture().await;
    reserve(&f);
    let j = &f.journal;
    let id = j.web_project_id.as_ref().unwrap();
    let private = f.store.private_dir(id).unwrap();
    let retained = private.with_file_name("disposable-held-original-private");
    fs::rename(&private, &retained).unwrap();
    fs::create_dir(&private).unwrap();
    fs::write(private.join("replacement-user-file"), "must be preserved").unwrap();
    let first = f.manager.recover(f.apps.clone()).await.unwrap();
    assert_eq!(first[0].commit_state, CommitState::NotCommitted);
    assert!(first[0].cleanup_pending);
    assert!(!j.source_path.as_ref().unwrap().exists());
    assert_eq!(
        fs::read_to_string(private.join("replacement-user-file")).unwrap(),
        "must be preserved"
    );
    fs::remove_file(private.join("replacement-user-file")).unwrap();
    fs::remove_dir(&private).unwrap();
    fs::rename(&retained, &private).unwrap();
    let retried = f
        .manager
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert_eq!(
        retried.commit_state,
        CommitState::NotCommitted,
        "{:?}",
        retried.task
    );
    assert!(!retried.cleanup_pending, "{:?}", retried.task);
    assert!(!private.exists());
    assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn damaged_unrelated_web_registration_does_not_block_completed_import_deduplication() {
    let f = fixture().await;
    reserve(&f);
    let j = &f.journal;
    f.store
        .register_managed(
            j.web_project_id.as_ref().unwrap(),
            &j.task_id,
            j.source_path.as_ref().unwrap(),
        )
        .await
        .unwrap();
    f.manager.recover(f.apps.clone()).await.unwrap();
    let damaged = f
        .paths
        .data_root()
        .join("web-apps/prj_disposable_unrelated");
    fs::create_dir(&damaged).unwrap();
    fs::write(
        damaged.join("registration.json"),
        "damaged unrelated metadata",
    )
    .unwrap();
    let reopened = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Err(invalid("No tools")),
    )
    .unwrap();
    let snapshot = reopened.status(&j.input.import_id).unwrap();
    assert_eq!(snapshot.commit_state, CommitState::Committed);
    assert_eq!(snapshot.app_id, *j.web_project_id.as_ref().unwrap());
    assert!(j.source_path.as_ref().unwrap().join("index.html").is_file());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn snapshot_exposes_web_instance_identity_only_after_durable_task_binding() {
    let f = fixture().await;
    let before = f.manager.status(&f.journal.input.import_id).unwrap();
    assert_eq!(before.app_id, f.journal.app_id);
    assert_eq!(before.web_project_id, f.journal.web_project_id);
    assert!(before.task.project_id.is_none());
    reserve(&f);
    let after = f.manager.status(&f.journal.input.import_id).unwrap();
    assert_eq!(Some(after.app_id.clone()), f.journal.web_project_id);
    assert_eq!(
        after.task.project_id.as_deref(),
        Some(after.app_id.as_str())
    );
    f.manager.recover(f.apps.clone()).await.unwrap();
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn snapshot_does_not_publish_waiting_without_a_confirmation_during_phase_handoff() {
    let f = fixture().await;
    reserve(&f);
    f.tasks
        .set_waiting_confirmation(&f.journal.task_id)
        .unwrap();
    let mut journal = f.journal.clone();
    journal.phase = "building".into();
    f.manager.save(&journal).unwrap();
    let snapshot = f.manager.status(&journal.input.import_id).unwrap();
    assert_eq!(snapshot.task.status, crate::TaskStatus::Running);
    assert_eq!(snapshot.task.phase, "building");
    assert!(snapshot.build_confirmation.is_none());
    f.manager.recover(f.apps.clone()).await.unwrap();
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn interrupted_before_reservation_removes_only_exact_empty_web_metadata_then_node_owned_source(
) {
    let f = fixture().await;
    let j = &f.journal;
    let id = j.web_project_id.as_ref().unwrap();
    let owner = crate::web_project_ownership::WebProjectOwnership::new(
        f.paths.data_root().into(),
        f.paths.workspace_root().into(),
    );
    owner.initialize(id).unwrap();
    private_dir(&f.store.private_dir(id).unwrap()).unwrap();
    owner.record_instance_root(id).unwrap();
    assert_eq!(
        f.store
            .web_import_state(
                id,
                &j.task_id,
                j.source_path.as_ref().unwrap(),
                j.source_identity.as_ref().unwrap()
            )
            .unwrap(),
        WebImportState::Absent
    );
    let recovered = f.manager.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered[0].commit_state, CommitState::NotCommitted);
    assert!(!recovered[0].cleanup_pending, "{:?}", recovered[0].task);
    assert!(!j.source_path.as_ref().unwrap().exists());
    assert!(!owner.directory(id).unwrap().exists());
    assert!(!f.store.private_dir(id).unwrap().exists());
    f.tasks.shutdown().await;
}

async fn begin_retirement(f: &Fixture) -> Journal {
    reserve(f);
    let mut j = f.journal.clone();
    let info = f
        .store
        .register_managed(
            j.web_project_id.as_ref().unwrap(),
            &j.task_id,
            j.source_path.as_ref().unwrap(),
        )
        .await
        .unwrap();
    j.app = Some(serde_json::to_value(info).unwrap());
    j.commit_state = CommitState::Committed;
    f.manager
        .ownership()
        .release_imported_source(
            &j.app_id,
            j.source_path.as_ref().unwrap(),
            j.source_identity.as_ref().unwrap(),
        )
        .unwrap();
    f.manager.cleanup(&mut j).unwrap();
    f.manager.begin_control_retirement(&mut j).unwrap();
    j
}
fn reopen(f: &Fixture) -> Arc<NodeGitImportManager> {
    NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Err(invalid("No tools for retirement")),
    )
    .unwrap()
}
#[tokio::test]
async fn partial_control_retirement_recovers_without_reading_the_removed_owner_ledger() {
    let f = fixture().await;
    let j = begin_retirement(&f).await;
    let owner = f.manager.ownership().directory(&j.app_id).unwrap();
    fs::remove_file(owner.join("ownership.json")).unwrap();
    fs::remove_file(owner.join("import.json")).unwrap();
    let reopened = reopen(&f);
    let result = reopened.recover(f.apps.clone()).await.unwrap();
    assert_eq!(result[0].commit_state, CommitState::Committed);
    assert!(!result[0].cleanup_pending, "{:?}", result[0].task);
    assert!(!owner.exists());
    assert!(j.source_path.as_ref().unwrap().join("index.html").is_file());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&j.task_id)
        .join("import-retirement.json")
        .exists());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn fully_removed_control_recovers_the_terminal_reply_from_temporary_retirement_intent() {
    let f = fixture().await;
    let j = begin_retirement(&f).await;
    let owner = f.manager.ownership().directory(&j.app_id).unwrap();
    crate::history_cleanup::remove_owned_directory(&owner, j.owner_identity.as_ref().unwrap())
        .unwrap();
    assert!(!f.tasks.task(&j.task_id).unwrap().status.is_final());
    let reopened = reopen(&f);
    let result = reopened.recover(f.apps.clone()).await.unwrap();
    assert_eq!(result[0].commit_state, CommitState::Committed);
    assert!(!result[0].cleanup_pending);
    assert!(result[0].task.status.is_final());
    assert!(j.source_path.as_ref().unwrap().join("index.html").is_file());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&j.task_id)
        .join("import-retirement.json")
        .exists());
    assert!(reopen(&f).recover(f.apps.clone()).await.unwrap().is_empty());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn control_retirement_rejects_same_path_replacement_and_retries_only_the_original_inode() {
    let f = fixture().await;
    let j = begin_retirement(&f).await;
    let owner = f.manager.ownership().directory(&j.app_id).unwrap();
    let original = owner.with_file_name("disposable-held-original-import-control");
    fs::rename(&owner, &original).unwrap();
    fs::create_dir(&owner).unwrap();
    fs::write(owner.join("unrelated-replacement"), "preserve replacement").unwrap();
    let reopened = reopen(&f);
    assert!(reopened.recover(f.apps.clone()).await.is_err());
    assert_eq!(
        fs::read_to_string(owner.join("unrelated-replacement")).unwrap(),
        "preserve replacement"
    );
    assert!(j.source_path.as_ref().unwrap().join("index.html").is_file());
    fs::remove_file(owner.join("unrelated-replacement")).unwrap();
    fs::remove_dir(&owner).unwrap();
    fs::rename(original, &owner).unwrap();
    let completed = reopened
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert_eq!(completed.commit_state, CommitState::Committed);
    assert!(!completed.cleanup_pending, "{:?}", completed.task);
    assert!(!owner.exists());
    assert!(j.source_path.as_ref().unwrap().join("index.html").is_file());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn large_display_icon_caches_do_not_overflow_journal_intent_or_restarted_task_registry() {
    let f = fixture().await;
    let mut j = begin_retirement(&f).await;
    j.app.as_mut().unwrap()["iconDataUrl"] = json!(format!(
        "data:image/png;base64,{}",
        "x".repeat(2 * 1024 * 1024)
    ));
    f.manager.begin_control_retirement(&mut j).unwrap();
    let task_directory = f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&j.task_id);
    for file in [
        f.manager.journal_path(&j).unwrap(),
        task_directory.join("import-retirement.json"),
    ] {
        let persisted = fs::read(file).unwrap();
        assert!(persisted.len() < 1024 * 1024);
        let loaded: Journal = serde_json::from_slice(&persisted).unwrap();
        assert!(loaded.app.as_ref().unwrap().get("iconDataUrl").is_none());
    }
    assert!(
        f.manager.status(&j.input.import_id).unwrap().app.unwrap()["iconDataUrl"]
            .as_str()
            .unwrap()
            .len()
            > 1024 * 1024
    );
    f.manager.finish_control_retirement(&mut j, None).unwrap();
    assert!(
        fs::metadata(task_directory.join("task.json"))
            .unwrap()
            .len()
            < 1024 * 1024
    );
    assert!(f.tasks.task(&j.task_id).unwrap().result.unwrap()["app"]
        .get("iconDataUrl")
        .is_none());
    f.tasks.shutdown().await;
    let restarted_tasks = BuildManager::new(f.paths.clone(), f.store.clone()).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        restarted_tasks.clone(),
        Err(invalid("No tools on restart")),
    )
    .unwrap();
    let snapshot = restarted.status(&j.input.import_id).unwrap();
    assert_eq!(snapshot.commit_state, CommitState::Committed);
    assert_eq!(snapshot.task.task_id, j.task_id);
    let (_stop, stop) = watch::channel(false);
    assert_eq!(
        restarted
            .start(f.apps.clone(), j.input.clone(), stop)
            .await
            .unwrap()
            .task_id,
        j.task_id
    );
    restarted_tasks.shutdown().await;
}
