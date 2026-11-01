use super::*;
use crate::{CreateProjectInput, SqlCaller, SqlRequest};
use sha2::{Digest, Sha256};
use std::os::unix::fs::symlink;
use std::sync::Arc;

#[cfg(target_os = "macos")]
#[tokio::test]
async fn stable_selected_workspace_survives_device_renumbering_but_rejects_wrong_volume_or_inode() {
    let (_temp, paths, store, _id, dest) = fixture().await;
    let (_selected, mut journal) = commit(&paths, &store, &dest).await;
    let original = journal.destination_identity.clone().unwrap();
    assert!(!original.is_legacy());
    journal.destination_identity.as_mut().unwrap().device ^= 0x1000;
    save_journal(&paths, &journal).unwrap();
    assert!(paths.reinitialize_for_test().is_ok());
    for change_volume in [true, false] {
        let mut changed = original.clone();
        if change_volume {
            changed.volume_uuid = Some(Uuid::new_v4().to_string());
        } else {
            changed.inode += 1;
        }
        journal.destination_identity = Some(changed);
        save_journal(&paths, &journal).unwrap();
        assert!(paths.reinitialize_for_test().is_err());
        assert!(dest.join("projects").is_dir());
    }
    store.shutdown().await;
}
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
    store.pause_workspace().await.unwrap();
    copy_and_commit(paths, store, &task.task_id).unwrap()
}

#[tokio::test]
async fn migration_keeps_each_node_projects_old_and_new_material_exclusively_owned() {
    use crate::node_project_ownership::NodeProjectOwnership;
    let (_temporary, paths, store, _web_id, destination) = fixture().await;
    let owner = Uuid::new_v4().to_string();
    let ownership =
        NodeProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into());
    ownership.initialize(&owner).unwrap();
    let task = format!("task_{}", Uuid::new_v4().simple());
    let source = format!("src_{}", Uuid::new_v4().simple());
    let relative = [
        PathBuf::from(".platform/tasks").join(&task),
        PathBuf::from(".platform/sources").join(&source),
        PathBuf::from(".platform/source-publications").join(&owner),
        PathBuf::from("project-sources/disposable-owned-node"),
    ];
    for relative in &relative {
        let path = paths.workspace_root().join(relative);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("project-payload"),
            "disposable per-project migration material",
        )
        .unwrap();
    }
    ownership.record_task(&owner, &task).unwrap();
    ownership.record_source(&owner, &source).unwrap();
    ownership.record_publication(&owner).unwrap();
    ownership
        .reserve_published_source_identity(
            &owner,
            "disposable-owned-node",
            &crate::file_identity::PersistedIdentity::capture(
                &paths
                    .workspace_root()
                    .join("project-sources/disposable-owned-node"),
            )
            .unwrap(),
        )
        .unwrap();
    let apps = crate::node_apps::NodeApps::new(
        paths.data_root().into(),
        "/disposable/nonexistent-node".into(),
    )
    .unwrap();
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let prepare =
        |root: &Path| {
            fs::create_dir_all(root.join("web")).unwrap();
            fs::create_dir_all(root.join("backend")).unwrap();
            fs::write(root.join("web/index.html"), "<p>Disposable migration</p>").unwrap();
            fs::write(
                root.join("backend/main.mjs"),
                "throw Error('never execute')",
            )
            .unwrap();
            fs::write(root.join("panda.json"), serde_json::to_vec(&json!({
            "manifestVersion":1,"runtime":"node","app":{"name":"Disposable migration"},
            "web":{"root":"web","entry":"index.html"},
            "backend":{"root":"backend","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        })).unwrap()).unwrap();
        };
    let downloaded = paths
        .workspace_root()
        .join("project-sources/disposable-owned-node");
    prepare(&downloaded);
    apps.register_published_source(
        &owner,
        crate::node_git_import::VerifiedPublishedSource {
            source_root: downloaded.clone(),
            source_identity: crate::file_identity::PersistedIdentity::capture(&downloaded).unwrap(),
            manifest_digest: format!(
                "{:x}",
                Sha256::digest(fs::read(downloaded.join("panda.json")).unwrap())
            ),
            source_url: "https://github.com/disposable/migration".into(),
            source_branch: None,
        },
    )
    .await
    .unwrap();
    let original = _temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-original-author");
    prepare(&original);
    fs::create_dir_all(original.join(".git")).unwrap();
    fs::write(original.join(".git/HEAD"), "ordinary author Git remains").unwrap();
    fs::create_dir(original.join("data")).unwrap();
    fs::write(
        original.join("data/retained.txt"),
        "ordinary business data remains",
    )
    .unwrap();
    let external = apps.register(original.clone()).await.unwrap();
    let external_id = external["id"].as_str().unwrap();
    let (selected, mut journal) = commit(&paths, &store, &destination).await;
    let reopened = ProjectStore::new(destination.clone()).unwrap();
    initialize_selected(&selected, &reopened, &mut journal).unwrap();
    initialize_selected(&selected, &reopened, &mut journal).unwrap();
    let migrated = NodeProjectOwnership::new(paths.data_root().into(), destination.clone());
    let targets = migrated.owned_paths(&owner).unwrap();
    assert_eq!(
        migrated.external_owned_paths(&owner).unwrap().len(),
        relative.len() + 1
    );
    for relative in &relative {
        assert!(targets.contains(&paths.workspace_root().join(relative)));
        assert!(targets.contains(&destination.join(relative)));
    }
    assert!(!targets.contains(&original));
    assert!(!destination.join("disposable-original-author").exists());
    assert!(migrated.user_source_roots().unwrap().contains(&original));
    assert!(!migrated
        .owned_paths(external_id)
        .unwrap()
        .contains(&original));
    // Only the project's persisted scope is removed. The public migration's
    // global journal and both workspace container roots remain.
    for target in targets {
        fs::remove_dir_all(target).unwrap();
    }
    for relative in &relative {
        assert!(!paths.workspace_root().join(relative).exists());
        assert!(!destination.join(relative).exists());
    }
    assert!(journal_path(paths.data_root(), &journal.task_id).exists());
    assert!(paths.workspace_root().exists());
    assert!(destination.exists());
    assert_eq!(
        fs::read_to_string(original.join(".git/HEAD")).unwrap(),
        "ordinary author Git remains"
    );
    assert_eq!(
        fs::read_to_string(original.join("data/retained.txt")).unwrap(),
        "ordinary business data remains"
    );
    reopened.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn deletion_marker_created_after_migration_admission_blocks_copy_before_any_files_move() {
    let (_temp, paths, store, id, dest) = fixture().await;
    let (root, identity) = store.deletion_root(&id).unwrap();
    let before = fs::read(root.join("data/attachments/.hidden")).unwrap();
    // A project task accepted before migration may fail while migration drains.
    store.begin_workspace_migration().unwrap();
    let task = create(&paths, &dest).unwrap();
    store
        .begin_deletion(
            &id,
            "task_disposable_partial_delete",
            &Uuid::new_v4().to_string(),
            &identity,
        )
        .unwrap();
    let marker = paths
        .workspace_root()
        .join(".platform/project-state/deleting")
        .join(format!("{id}.json"));
    let marker_bytes = fs::read(&marker).unwrap();
    assert_eq!(
        validate_idle(&paths).unwrap_err().code,
        "PROJECT_DELETE_INCOMPLETE"
    );
    assert!(copy_and_commit(&paths, &store, &task.task_id).is_err());
    assert!(fs::read_dir(&dest).unwrap().next().is_none());
    assert_eq!(fs::read(&marker).unwrap(), marker_bytes);
    assert_eq!(
        fs::read(root.join("data/attachments/.hidden")).unwrap(),
        before
    );
    assert_eq!(
        paths.reinitialize_for_test().unwrap().workspace_root(),
        paths.workspace_root()
    );
    store.resume_workspace();
    store.shutdown().await;
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
        .join(root.file_name().unwrap())
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
    assert!(store.project_root(&id).unwrap().exists());
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
    store.pause_workspace().await.unwrap();
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
    assert!(saved.committed && saved.epochs_applied && !saved.resolved);
    assert!(saved.source_cleanup.as_ref().unwrap().needs_reopen());
    assert!(saved.retained_source_warning.is_none());
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
        .project_root(&id)
        .unwrap()
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
        .join(".platform/operations/task_hidden/journal.json");
    builds::atomic_json(
        &journal,
        &json!({"taskId":"task_hidden","projectId":"prj_retired","resolved":false}),
    )
    .unwrap();
    store.begin_workspace_migration().unwrap();
    store.pause_workspace().await.unwrap();
    let error = copy_and_commit(&paths, &store, &task.task_id)
        .err()
        .unwrap();
    assert_eq!(error.code, "RECOVERY_REQUIRED");
    assert!(!is_committed(&paths, &task.task_id).unwrap());
    assert!(fs::read_dir(dest).unwrap().next().is_none());
    store.shutdown().await;
}
