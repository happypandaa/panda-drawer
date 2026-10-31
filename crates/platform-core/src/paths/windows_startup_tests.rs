//! Test only the real initialization checkpoint and recovery delegation. The
//! no service or application is started here; every path belongs to TempDir.
use super::*;
use crate::{file_identity::PersistedIdentity, ApiError};
use serde_json::{json, Value};

fn fixture() -> (tempfile::TempDir, RuntimePaths) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-workspace-checkpoint-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap().join("data");
    ensure_directory_tree(&root).unwrap();
    let paths = RuntimePaths::at(root);
    create_private_json(
        &paths.data_root.join(MARKER),
        &EnvironmentMarker {
            schema_version: 1,
            identifier: APP_IDENTIFIER.into(),
            environment: ENVIRONMENT.into(),
        },
    )
    .unwrap();
    paths.validate_marker().unwrap();
    (temporary, paths)
}

fn settings(paths: &RuntimePaths, migration: Option<&str>) -> Vec<u8> {
    create_private_json(
        &paths.settings_path(),
        &Settings {
            schema_version: 1,
            identifier: APP_IDENTIFIER.into(),
            environment: ENVIRONMENT.into(),
            workspace_root: paths.workspace_root.clone(),
            last_port: None,
            migration_id: migration.map(str::to_owned),
        },
    )
    .unwrap();
    fs::read(paths.settings_path()).unwrap()
}

#[test]
fn earliest_checkpoint_retains_migration_material_before_initialization_writes() {
    let (_temporary, paths) = fixture();
    let journal = paths.migrations_dir().join("task_disposable/task");
    fs::create_dir_all(&journal).unwrap();
    let retained = [
        (
            journal.join("task.json"),
            b"retained running task".as_slice(),
        ),
        (
            journal.parent().unwrap().join("journal.json"),
            b"retained journal and epochs".as_slice(),
        ),
    ];
    for (path, bytes) in &retained {
        fs::write(path, bytes).unwrap();
    }
    let error = paths.prepare_runtime_directories().unwrap_err();
    let error = error.downcast_ref::<ApiError>().unwrap();
    assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert!(!paths.runtime_dir().exists());
    assert!(!paths.settings_path().exists());
    assert!(!paths.workspace_root().exists());
    for (path, bytes) in &retained {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
}

#[test]
fn actual_missing_inventory_initializes_only_the_existing_directory_checkpoint() {
    let (_temporary, paths) = fixture();
    let application = paths.data_root().join("node-project-ownership");
    fs::create_dir(&application).unwrap();
    fs::write(application.join("retained"), b"other application evidence").unwrap();
    paths.prepare_runtime_directories().unwrap();
    assert!(paths.runtime_dir().is_dir());
    assert!(paths.migrations_dir().is_dir());
    assert!(fs::read_dir(paths.migrations_dir())
        .unwrap()
        .next()
        .is_none());
    assert!(!paths.settings_path().exists());
    assert!(!paths.workspace_root().exists());
    assert_eq!(
        fs::read(application.join("retained")).unwrap(),
        b"other application evidence"
    );
}

#[test]
fn selected_missing_journal_never_creates_directories_or_clears_settings() {
    let (_temporary, paths) = fixture();
    let before = settings(&paths, Some("task_disposable_migration"));
    let error = paths.prepare_runtime_directories().unwrap_err();
    let error = error.downcast_ref::<ApiError>().unwrap();
    assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
    assert_eq!(error.details["inventory"], "inconsistent");
    assert_eq!(error.details["migrationId"], "task_disposable_migration");
    assert!(!paths.runtime_dir().exists());
    assert!(!paths.migrations_dir().exists());
    assert_eq!(fs::read(paths.settings_path()).unwrap(), before);
}

#[test]
fn selected_valid_journal_reconciles_commit_epochs_and_interruption_without_deleting_source() {
    let (temporary, paths) = fixture();
    let id = "task_disposable_migration";
    let source = temporary.path().join("retained-source");
    let target = paths.workspace_root();
    fs::create_dir_all(source.join(".platform")).unwrap();
    fs::create_dir_all(target).unwrap();
    let source_bytes = b"disposable source evidence";
    let retired_bytes = b"retained retired evidence";
    fs::write(source.join("source.txt"), source_bytes).unwrap();
    fs::write(
        source.join(".platform/workspace-retired.json"),
        retired_bytes,
    )
    .unwrap();
    let journal = json!({
        "schemaVersion":1,"identifier":APP_IDENTIFIER,"environment":ENVIRONMENT,
        "taskId":id,"sourcePath":source,"destinationPath":target,
        "sourceIdentity":PersistedIdentity::capture(&source).unwrap(),
        "targetEmptyIdentity":PersistedIdentity::capture(target).unwrap(),
        "destinationIdentity":PersistedIdentity::capture(target).unwrap(),
        "stagePath":temporary.path().join("retained-stage"),
        "phase":"published","published":true,"committed":false,"resolved":false,
        "epochs":{},"epochsApplied":false,"fileCount":1,"totalBytes":source_bytes.len(),
        "startedAt":1,"cleanupTaskId":"task_disposable_cleanup",
        "sourceCleanup":crate::workspace_migration_cleanup::SourceCleanup::awaiting_reopen()
    });
    let parsed: crate::workspace::MigrationJournal =
        serde_json::from_value(journal.clone()).unwrap();
    assert!(!parsed.committed);
    let journal_path = paths.migrations_dir().join(id).join("journal.json");
    fs::create_dir_all(journal_path.parent().unwrap().join("task")).unwrap();
    let before = serde_json::to_vec(&journal).unwrap();
    fs::write(&journal_path, &before).unwrap();
    let settings_before = settings(&paths, Some(id));
    let task_path = journal_path.parent().unwrap().join("task/task.json");
    let task_before = serde_json::to_vec(&json!({
        "taskId":id,"projectId":null,"type":"workspace_migrate", "status":"running",
        "phase":"published","result":null,"error":null
    }))
    .unwrap();
    fs::write(&task_path, &task_before).unwrap();
    paths.prepare_runtime_directories().unwrap();
    crate::workspace::recover_startup(&paths).unwrap();
    assert_ne!(fs::read(&journal_path).unwrap(), before);
    assert_ne!(fs::read(&task_path).unwrap(), task_before);
    assert_eq!(fs::read(paths.settings_path()).unwrap(), settings_before);
    assert_eq!(fs::read(source.join("source.txt")).unwrap(), source_bytes);
    assert_eq!(
        fs::read(source.join(".platform/workspace-retired.json")).unwrap(),
        retired_bytes
    );
    let actual: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(actual["committed"], true);
    assert_eq!(actual["epochsApplied"], true);
    assert_eq!(actual["sourceCleanup"]["phase"], "awaitingReopen");
    assert_eq!(actual["resolved"], false);
    let task: crate::TaskRecord = serde_json::from_slice(&fs::read(&task_path).unwrap()).unwrap();
    assert_eq!(task.status, crate::TaskStatus::Interrupted);
    assert_eq!(task.error.unwrap().details["operationOutcome"], "committed");
    let journal_after = fs::read(&journal_path).unwrap();
    let task_after = fs::read(&task_path).unwrap();
    let log_path = journal_path.parent().unwrap().join("task/log.txt");
    let log_after = fs::read(&log_path).unwrap();
    crate::workspace::recover_startup(&paths).unwrap();
    assert_eq!(fs::read(&journal_path).unwrap(), journal_after);
    assert_eq!(fs::read(&task_path).unwrap(), task_after);
    assert_eq!(fs::read(&log_path).unwrap(), log_after);
    assert_eq!(fs::read(source.join("source.txt")).unwrap(), source_bytes);
}

#[test]
fn shared_recovery_proves_an_empty_tree_and_unselected_settings_without_writes() {
    let (_temporary, paths) = fixture();
    paths.prepare_runtime_directories().unwrap();
    let before = settings(&paths, None);
    crate::workspace::recover_startup(&paths).unwrap();
    assert_eq!(fs::read(paths.settings_path()).unwrap(), before);
    assert!(fs::read_dir(paths.migrations_dir())
        .unwrap()
        .next()
        .is_none());
    assert!(!paths.workspace_root().exists());
}

#[test]
fn unselected_copy_task_becomes_interrupted_without_replaying_copy_or_touching_unknown_material() {
    let (temporary, paths) = fixture();
    paths.prepare_runtime_directories().unwrap();
    let settings_before = settings(&paths, None);
    let id = "task_disposable_unselected";
    let source = temporary.path().join("retained-unselected-source");
    let destination = temporary.path().join("retained-unselected-destination");
    let stage = temporary.path().join("retained-unselected-stage");
    for directory in [&source, &destination, &stage] {
        fs::create_dir(directory).unwrap();
        fs::write(directory.join("sentinel"), b"retained original bytes").unwrap();
    }
    let record = json!({
        "schemaVersion":1,"identifier":APP_IDENTIFIER,"environment":ENVIRONMENT,
        "taskId":id,"sourcePath":source,"destinationPath":destination,
        "sourceIdentity":PersistedIdentity::capture(&source).unwrap(),
        "targetEmptyIdentity":PersistedIdentity::capture(&destination).unwrap(),
        "destinationIdentity":null,"stagePath":stage,"phase":"copying",
        "published":false,"committed":false,"resolved":false,
        "epochs":{},"epochsApplied":false,"fileCount":1,"totalBytes":23
    });
    let journal_path = paths.migrations_dir().join(id).join("journal.json");
    let task_path = journal_path.parent().unwrap().join("task/task.json");
    fs::create_dir_all(task_path.parent().unwrap()).unwrap();
    let journal_before = serde_json::to_vec(&record).unwrap();
    fs::write(&journal_path, &journal_before).unwrap();
    crate::builds::atomic_json(
        &task_path,
        &crate::TaskRecord {
            task_id: id.into(),
            project_id: None,
            task_type: "workspace_migrate".into(),
            status: crate::TaskStatus::Running,
            phase: "copying".into(),
            result: None,
            error: None,
        },
    )
    .unwrap();
    let unknown = paths.migrations_dir().join("UNRECOGNIZED material");
    fs::create_dir(&unknown).unwrap();
    fs::write(unknown.join("journal.json"), b"unknown independent bytes").unwrap();
    crate::workspace::recover_startup(&paths).unwrap();
    let task: crate::TaskRecord = serde_json::from_slice(&fs::read(&task_path).unwrap()).unwrap();
    assert_eq!(task.status, crate::TaskStatus::Interrupted);
    assert_eq!(
        task.result.as_ref().unwrap()["operationOutcome"],
        "not_committed"
    );
    assert_eq!(
        task.error.unwrap().details["operationOutcome"],
        "not_committed"
    );
    assert!(!paths.workspace_root().exists());
    assert_eq!(fs::read(&journal_path).unwrap(), journal_before);
    assert_eq!(fs::read(paths.settings_path()).unwrap(), settings_before);
    for directory in [&source, &destination, &stage] {
        assert_eq!(
            fs::read(directory.join("sentinel")).unwrap(),
            b"retained original bytes"
        );
    }
    assert_eq!(
        fs::read(unknown.join("journal.json")).unwrap(),
        b"unknown independent bytes"
    );
    let after = fs::read(&task_path).unwrap();
    crate::workspace::recover_startup(&paths).unwrap();
    assert_eq!(fs::read(&task_path).unwrap(), after);
}
