//! Test only the real initialization checkpoint and recovery delegation. The
//! total Windows runtime gate remains closed; no service is started here.
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
    assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
    assert_eq!(error.details["operationOutcome"], "not_started");
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
fn shared_recovery_retains_a_selected_valid_journal_without_committing_or_retiring() {
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
        "startedAt":1,"cleanupTaskId":"task_disposable_cleanup"
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
        "taskId":id,"projectId":null,"taskType":"workspace_migrate", "status":"running",
        "phase":"published","result":null,"error":null
    }))
    .unwrap();
    fs::write(&task_path, &task_before).unwrap();
    let error = crate::workspace::recover_startup(&paths).unwrap_err();
    assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(fs::read(&journal_path).unwrap(), before);
    assert_eq!(fs::read(&task_path).unwrap(), task_before);
    assert_eq!(fs::read(paths.settings_path()).unwrap(), settings_before);
    assert_eq!(fs::read(source.join("source.txt")).unwrap(), source_bytes);
    assert_eq!(
        fs::read(source.join(".platform/workspace-retired.json")).unwrap(),
        retired_bytes
    );
    let actual: Value = serde_json::from_slice(&fs::read(journal_path).unwrap()).unwrap();
    assert_eq!(actual["committed"], false);
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
