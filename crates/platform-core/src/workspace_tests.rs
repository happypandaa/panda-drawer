use super::*;
use crate::{CreateProjectInput, SqlCaller, SqlRequest};
use std::sync::Arc;
async fn fixture() -> (
    tempfile::TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    String,
    PathBuf,
) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = store
        .create(CreateProjectInput {
            name: "M4 migration disposable".into(),
            description: Some("Keep metadata".into()),
        })
        .await
        .unwrap();
    let db = rusqlite::Connection::open(project.project_root.join("data/main.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE records(id TEXT PRIMARY KEY,value TEXT);INSERT INTO records VALUES('stable-business-id','persisted');").unwrap();
    drop(db);
    fs::write(
        project.project_root.join("data/attachments/.hidden"),
        b"attachment",
    )
    .unwrap();
    let destination = temp.path().canonicalize().unwrap().join("destination");
    fs::create_dir(&destination).unwrap();
    (temp, paths, store, project.id, destination)
}
async fn commit(
    paths: &RuntimePaths,
    store: &ProjectStore,
    dest: &Path,
) -> (RuntimePaths, MigrationJournal) {
    store.begin_workspace_migration().unwrap();
    let task = create(paths, dest).unwrap();
    store.pause_workspace().await;
    copy_and_commit(paths, store, &task.task_id).unwrap()
}
#[tokio::test]
async fn migration_preserves_protected_trees_links_database_and_state_excludes_only_active_dependencies(
) {
    let (_temp, paths, store, id, dest) = fixture().await;
    let root = store.project(&id).unwrap().root;
    fs::create_dir_all(root.join("app/node_modules/pkg")).unwrap();
    fs::write(root.join("app/node_modules/pkg/cache"), b"rebuildable").unwrap();
    let original = paths
        .workspace_root()
        .join(".platform/operations/old_restore/original/node_modules/.bin");
    fs::create_dir_all(&original).unwrap();
    symlink("../pkg/cli", original.join("command")).unwrap();
    fs::write(original.parent().unwrap().join("keep"), b"protected").unwrap();
    let trash = paths
        .workspace_root()
        .join(".platform/trash/trash_disposable/project/.cache");
    fs::create_dir_all(&trash).unwrap();
    fs::write(trash.join("keep"), b"trash must be complete").unwrap();
    let control = store.control(&id).unwrap();
    let mut state = control.get();
    state.mode = "maintenance".into();
    state.maintenance_id = Some("maintenance_persisted".into());
    state.ready_for_changes = false;
    control.set(state.clone()).unwrap();
    let old_epoch = store.project(&id).unwrap().document_epoch;
    let (selected, mut journal) = commit(&paths, &store, &dest).await;
    assert_eq!(selected.workspace_root(), dest);
    assert!(!dest
        .join("projects")
        .join(&id)
        .join("app/node_modules")
        .exists());
    assert_eq!(
        fs::read(dest.join(".platform/operations/old_restore/original/node_modules/keep")).unwrap(),
        b"protected"
    );
    assert_eq!(
        fs::read_link(
            dest.join(".platform/operations/old_restore/original/node_modules/.bin/command")
        )
        .unwrap(),
        PathBuf::from("../pkg/cli")
    );
    assert_eq!(
        fs::read(dest.join(".platform/trash/trash_disposable/project/.cache/keep")).unwrap(),
        b"trash must be complete"
    );
    let new = ProjectStore::new(dest.clone()).unwrap();
    initialize_selected(&selected, &new, &mut journal).unwrap();
    let project = new.project(&id).unwrap();
    assert_ne!(project.document_epoch, old_epoch);
    assert_eq!(project.document_epoch, journal.epochs[&id]);
    assert_eq!(
        new.control(&id).unwrap().get().maintenance_id,
        state.maintenance_id
    );
    assert!(!new.control(&id).unwrap().get().ready_for_changes);
    let c = rusqlite::Connection::open(project.root.join("data/main.sqlite")).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT value FROM records WHERE id='stable-business-id'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "persisted"
    );
    drop(c);
    mark_old_retired(&selected, &journal).unwrap();
    assert!(paths.workspace_root().join("projects").join(&id).exists());
    assert!(store.project(&id).unwrap().check().is_err());
    new.shutdown().await;
    store.shutdown().await;
}
#[tokio::test]
async fn committed_settings_reopen_only_destination_and_missing_media_never_recreates_default() {
    let (_temp, paths, store, id, dest) = fixture().await;
    let (selected, j) = commit(&paths, &store, &dest).await;
    let reopened = paths.reinitialize_for_test().unwrap();
    assert_eq!(reopened.workspace_root(), dest);
    // Simulate process death between settings commit and epoch initialization.
    recover_startup(&reopened).unwrap();
    let new = ProjectStore::new(dest.clone()).unwrap();
    assert_eq!(new.project(&id).unwrap().document_epoch, j.epochs[&id]);
    let epoch = new.project(&id).unwrap().document_epoch;
    recover_startup(&reopened).unwrap();
    assert_eq!(new.project(&id).unwrap().document_epoch, epoch);
    let t = task(&selected, &j.task_id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Interrupted);
    assert_eq!(t.result.unwrap()["operationOutcome"], "committed");
    let unavailable = dest.with_file_name("offline-target");
    fs::rename(&dest, &unavailable).unwrap();
    let error = paths.reinitialize_for_test().unwrap_err().to_string();
    assert!(
        error.contains("Committed workspace is unavailable"),
        "{error}"
    );
    assert!(paths.workspace_root().is_dir());
    assert_eq!(
        read_json::<Value>(&paths.data_root().join("settings.json")).unwrap()["workspaceRoot"],
        json!(dest)
    );
    fs::rename(unavailable, &dest).unwrap();
    assert!(paths.reinitialize_for_test().is_ok());
    new.shutdown().await;
    store.shutdown().await;
}
#[tokio::test]
async fn precommit_restart_preserves_source_and_inactive_partial_without_publishing() {
    let (_temp, paths, store, id, dest) = fixture().await;
    let t = create(&paths, &dest).unwrap();
    let mut j = read_journal(&paths, &t.task_id).unwrap();
    fs::create_dir(&j.stage_path).unwrap();
    fs::write(j.stage_path.join("partial"), b"incomplete").unwrap();
    j.phase = "copying".into();
    save_journal(&paths, &j).unwrap();
    phase(&paths, &t.task_id, "copying").unwrap();
    recover_startup(&paths).unwrap();
    assert_eq!(
        paths.reinitialize_for_test().unwrap().workspace_root(),
        paths.workspace_root()
    );
    assert!(store.project(&id).is_ok());
    assert!(j.stage_path.join("partial").exists());
    assert!(fs::read_dir(dest).unwrap().next().is_none());
    let t = task(&paths, &t.task_id).unwrap().unwrap();
    assert_eq!(t.status, TaskStatus::Interrupted);
    assert_eq!(t.result.unwrap()["operationOutcome"], "not_committed");
    store.shutdown().await;
}
#[tokio::test]
async fn preflight_rejects_aliases_nonempty_configuration_and_source_overlap() {
    let (temp, paths, store, _id, dest) = fixture().await;
    for target in [
        paths.workspace_root().to_path_buf(),
        paths.data_root().to_path_buf(),
        paths.runtime_dir(),
        paths.migrations_dir(),
        paths.data_root().join("cache"),
    ] {
        if !target.exists() {
            fs::create_dir_all(&target).unwrap();
        }
        assert!(preflight(&paths, &target).is_err(), "{}", target.display());
    }
    let child = paths.workspace_root().join("inside");
    fs::create_dir(&child).unwrap();
    assert!(preflight(&paths, &child).is_err());
    fs::write(dest.join("must-keep"), b"data").unwrap();
    assert!(preflight(&paths, &dest).is_err());
    assert_eq!(fs::read(dest.join("must-keep")).unwrap(), b"data");
    let alias = temp.path().canonicalize().unwrap().join("alias");
    symlink(&dest, &alias).unwrap();
    assert!(preflight(&paths, &alias).is_err());
    store.shutdown().await;
}
#[tokio::test]
async fn migration_gate_prevents_new_creates_tasks_and_database_worker_revival() {
    let (_temp, _paths, store, id, _dest) = fixture().await;
    let held = store.reserve_task(&id, "task_accepted").unwrap();
    store.begin_workspace_migration().unwrap();
    assert!(store.has_active_tasks());
    assert_eq!(
        store.reserve_task(&id, "task_late").err().unwrap().code,
        "WORKSPACE_MIGRATING"
    );
    assert_eq!(
        store
            .create(CreateProjectInput {
                name: "must not create".into(),
                description: None
            })
            .await
            .unwrap_err()
            .code,
        "WORKSPACE_MIGRATING"
    );
    drop(held);
    assert!(!store.has_active_tasks());
    store.pause_workspace().await;
    let error = store
        .execute(
            &id,
            SqlRequest {
                sql: "INSERT INTO records VALUES('late','bad')".into(),
                params: vec![],
                options: Default::default(),
            },
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "WORKSPACE_MIGRATING");
    store.resume_workspace();
    let rows = store
        .execute(
            &id,
            SqlRequest {
                sql: "SELECT * FROM records".into(),
                params: vec![],
                options: Default::default(),
            },
            None,
            SqlCaller::Management,
        )
        .await
        .unwrap();
    assert_eq!(rows.rows.len(), 1);
    store.shutdown().await;
}
#[tokio::test]
async fn missing_old_source_does_not_block_a_committed_new_workspace() {
    let (_temp, paths, store, id, dest) = fixture().await;
    let (selected, mut j) = commit(&paths, &store, &dest).await;
    let new = ProjectStore::new(dest).unwrap();
    initialize_selected(&selected, &new, &mut j).unwrap();
    let offline = paths.data_root().join("retained-offline");
    fs::rename(paths.workspace_root(), &offline).unwrap();
    mark_old_retired(&selected, &j).unwrap();
    recover_startup(&selected).unwrap();
    assert!(new.project(&id).is_ok());
    assert!(read_journal(&selected, &j.task_id)
        .unwrap()
        .retained_source_warning
        .is_some());
    assert_eq!(
        selected.reinitialize_for_test().unwrap().workspace_root(),
        selected.workspace_root()
    );
    new.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn settings_commit_before_journal_checkpoint_recovers_and_retains_evidence_when_old_source_is_offline(
) {
    let (_temp, paths, store, _id, dest) = fixture().await;
    let (selected, mut j) = commit(&paths, &store, &dest).await;
    // The settings rename succeeded, but the subsequent journal save did not.
    j.committed = false;
    save_journal(&selected, &j).unwrap();
    let offline = paths.data_root().join("old-source-offline");
    fs::rename(paths.workspace_root(), &offline).unwrap();
    recover_startup(&selected).unwrap();
    let saved = read_journal(&selected, &j.task_id).unwrap();
    assert!(saved.committed && saved.epochs_applied && saved.resolved);
    assert!(saved.retained_source_warning.is_some());
    let new = ProjectStore::new(dest).unwrap();
    assert_eq!(
        info(&selected, &new).unwrap()["retainedRoots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::rename(&offline, paths.workspace_root()).unwrap();
    fs::remove_file(paths.data_root().join("settings.json")).unwrap();
    let error = paths.reinitialize_for_test().unwrap_err().to_string();
    assert!(error.contains("selection settings are missing"), "{error}");
    assert!(!paths.data_root().join("settings.json").exists());
    new.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn copying_a_blocked_database_does_not_implicitly_repair_its_registration() {
    let (_temp, paths, store, id, dest) = fixture().await;
    let root = store.project(&id).unwrap().root;
    // An externally replaced registered database is blocked before migration.
    fs::rename(
        root.join("data/main.sqlite"),
        root.join("data/preserved-old.sqlite"),
    )
    .unwrap();
    let c = rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap();
    c.execute_batch("CREATE TABLE external_change(value TEXT);")
        .unwrap();
    drop(c);
    assert_eq!(
        store.project(&id).err().unwrap().code,
        "PROJECT_IDENTITY_CHANGED"
    );
    let (selected, mut j) = commit(&paths, &store, &dest).await;
    assert!(j.blocked_projects.contains_key(&id));
    let new = ProjectStore::new(dest).unwrap();
    initialize_selected(&selected, &new, &mut j).unwrap();
    assert_eq!(
        new.project(&id).err().unwrap().code,
        "PROJECT_IDENTITY_CHANGED"
    );
    assert_eq!(
        new.control(&id).unwrap().get().last_error.unwrap().code,
        "MIGRATED_PROJECT_DIAGNOSTIC"
    );
    assert!(root.join("data/preserved-old.sqlite").is_file());
    assert!(new
        .workspace_root()
        .join("projects")
        .join(id)
        .join("data/preserved-old.sqlite")
        .is_file());
    new.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn unresolved_switch_journals_outside_active_projects_prevent_migration() {
    let (_temp, paths, store, _id, dest) = fixture().await;
    let task = create(&paths, &dest).unwrap();
    let journal = paths
        .workspace_root()
        .join(".platform/operations/task_hidden/lifecycle.json");
    builds::atomic_json(
        &journal,
        &json!({"taskId":"task_hidden","projectId":"prj_retired","resolved":false}),
    )
    .unwrap();
    store.begin_workspace_migration().unwrap();
    store.pause_workspace().await;
    let error = copy_and_commit(&paths, &store, &task.task_id)
        .err()
        .unwrap();
    assert_eq!(error.code, "RECOVERY_REQUIRED");
    assert!(!is_committed(&paths, &task.task_id).unwrap());
    assert!(fs::read_dir(dest).unwrap().next().is_none());
    store.shutdown().await;
}
