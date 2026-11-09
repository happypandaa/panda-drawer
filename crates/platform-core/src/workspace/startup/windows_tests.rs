//! Fixed-tree observation only; actual shared recovery is exercised by Paths tests.
use super::*;
use std::io::Write;
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-workspace-startup-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}
fn settings(root: &Path, migration: Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,"identifier":"disposable-inventory-fixture",
        "environment":"development","workspaceRoot":root.join("retained-workspace"),
        "lastPort":null,"migrationId":migration
    }))
    .unwrap();
    let mut file = native::create_private_file(&root.join("settings.json")).unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    bytes
}
#[test]
fn missing_or_inherited_empty_migration_roots_preserve_other_application_material() {
    let (_temporary, root) = fixture();
    fs::create_dir(root.join("source-import-ownership")).unwrap();
    fs::write(
        root.join("node-project-ownership/retained"),
        b"other application evidence",
    )
    .unwrap();
    assert_eq!(
        inspect_with(&root, false, |_, _| Ok(())).unwrap().0,
        Observation::Missing
    );
    assert!(!root.join("migrations").exists());
    assert!(!root.join("settings.json").exists());
    let bytes = settings(&root, Value::Null);
    require_recovery_inventory(&root).unwrap();
    fs::create_dir(root.join("migrations")).unwrap();
    assert_eq!(
        inspect_with(&root, true, |_, _| Ok(())).unwrap().0,
        Observation::Empty
    );
    assert_eq!(fs::read(root.join("settings.json")).unwrap(), bytes);
    assert_eq!(
        fs::read(root.join("node-project-ownership/retained")).unwrap(),
        b"other application evidence"
    );
    assert_eq!(fs::read_dir(root.join("migrations")).unwrap().count(), 0);
}
#[test]
fn private_hard_linked_settings_keep_the_existing_paths_admission_contract() {
    let (_temporary, root) = fixture();
    let bytes = settings(&root, Value::Null);
    let original = root.join("settings.json");
    let retained_link = root.join("retained-settings-link.json");
    fs::hard_link(&original, &retained_link).unwrap();
    let before = Facts::capture(&original).unwrap();
    assert_eq!(before.links(), 2);
    let opened = native::open_read_file_allowing_hard_links(&original).unwrap();
    native::verify_private(&opened).unwrap();
    for empty_root in [false, true] {
        if empty_root {
            fs::create_dir(root.join("migrations")).unwrap();
        }
        require_before_initialization(&root).unwrap();
        require_recovery_inventory(&root).unwrap();
        assert_eq!(fs::read(&original).unwrap(), bytes);
        assert_eq!(fs::read(&retained_link).unwrap(), bytes);
        assert!(before.unchanged(&Facts::capture(&original).unwrap()));
        assert!(before.same_object(&Facts::capture(&retained_link).unwrap()));
    }
}
#[test]
fn selected_missing_journals_never_imply_old_source_cleanup_has_finished() {
    for empty_root in [false, true] {
        let (_temporary, root) = fixture();
        let bytes = settings(&root, Value::String("task_disposable_migration".into()));
        if empty_root {
            fs::create_dir(root.join("migrations")).unwrap();
        }
        for result in [
            require_before_initialization(&root),
            require_recovery_inventory(&root),
        ] {
            let error = result.err().unwrap();
            assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
            assert_eq!(error.details["inventory"], "inconsistent");
            assert_eq!(error.details["operationOutcome"], "unknown");
            assert_eq!(error.details["migrationId"], "task_disposable_migration");
        }
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), bytes);
    }
}
#[test]
fn newly_appearing_migration_material_cannot_be_reported_absent_or_empty() {
    for observation in [Observation::Missing, Observation::Empty] {
        let (_temporary, root) = fixture();
        if observation == Observation::Empty {
            fs::create_dir(root.join("migrations")).unwrap();
        }
        let error = inspect_with(&root, false, |actual, path| {
            assert_eq!(actual, observation);
            if observation == Observation::Missing {
                fs::create_dir(path).unwrap();
            }
            fs::write(path.join("appeared"), b"retained concurrent evidence").unwrap();
            Ok(())
        })
        .err()
        .unwrap();
        assert!(matches!(
            error.code.as_str(),
            "WORKSPACE_STARTUP_INVENTORY_UNCONFIRMED" | "PLATFORM_FEATURE_UNAVAILABLE"
        ));
        assert_eq!(
            fs::read(root.join("migrations/appeared")).unwrap(),
            b"retained concurrent evidence"
        );
    }
}
#[test]
fn migration_junctions_and_ordinary_files_are_never_treated_as_missing() {
    let (_temporary, root) = fixture();
    let target = root.join("disposable-external-target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("retained"), b"retained junction bytes").unwrap();
    let link = root.join("migrations");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(
        require_before_initialization(&root).err().unwrap().code,
        "WORKSPACE_STARTUP_INVENTORY_UNCONFIRMED"
    );
    assert_eq!(
        fs::read(target.join("retained")).unwrap(),
        b"retained junction bytes"
    );
    fs::remove_dir(&link).unwrap();
    fs::write(&link, b"retained unexpected migration file").unwrap();
    assert!(require_before_initialization(&root).is_err());
    assert_eq!(
        fs::read(link).unwrap(),
        b"retained unexpected migration file"
    );
}
#[test]
fn required_settings_and_non_notfound_read_failures_do_not_become_empty_success() {
    let (_temporary, root) = fixture();
    assert!(require_recovery_inventory(&root).is_err());
    assert!(!root.join("settings.json").exists());
    let mut file = native::create_private_file(&root.join("settings.json")).unwrap();
    file.write_all(b"not-json retained settings").unwrap();
    drop(file);
    assert!(require_before_initialization(&root).is_err());
    assert_eq!(
        fs::read(root.join("settings.json")).unwrap(),
        b"not-json retained settings"
    );
}
#[test]
fn raw_verbatim_mixed_roots_observe_the_actual_settings_reference() {
    let (_temporary, root) = fixture();
    let actual = root.join("nested/data 中文");
    fs::create_dir_all(&actual).unwrap();
    let bytes = settings(
        &actual,
        Value::String("task_retained_source_cleanup".into()),
    );
    let mut raw = root.as_os_str().to_os_string();
    raw.push(r"\nested/data 中文");
    let error = require_before_initialization(&PathBuf::from(raw))
        .err()
        .unwrap();
    assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
    assert_eq!(fs::read(actual.join("settings.json")).unwrap(), bytes);
}
fn require_recovery_inventory(root: &Path) -> Result<()> {
    pin_recovery(root).map(|_| ())
}

fn journal(root: &Path, id: &str) -> Value {
    let identity = crate::file_identity::PersistedIdentity::capture(root).unwrap();
    serde_json::json!({
        "schemaVersion":1,"identifier":crate::APP_IDENTIFIER,"environment":crate::ENVIRONMENT,
        "taskId":id,"sourcePath":root.join("retained-source"),
        "destinationPath":root.join("retained-workspace"),"sourceIdentity":identity,
        "targetEmptyIdentity":identity,"destinationIdentity":identity,
        "stagePath":root.join("retained-stage"),"phase":"published",
        "published":true,"committed":false,"resolved":false,"epochs":{},
        "epochsApplied":false,"fileCount":0,"totalBytes":0
    })
}

#[test]
fn valid_fixed_selected_journal_is_admitted_without_creating_or_inspecting_a_workspace() {
    let (_temporary, root) = fixture();
    let id = "task_selected_disposable";
    let settings_before = settings(&root, Value::String(id.into()));
    let directory = root.join("migrations").join(id);
    fs::create_dir_all(&directory).unwrap();
    let bytes = serde_json::to_vec(&journal(&root, id)).unwrap();
    fs::write(directory.join("journal.json"), &bytes).unwrap();
    require_before_initialization(&root).unwrap();
    require_recovery_inventory(&root).unwrap();
    assert!(!root.join("retained-workspace").exists());
    assert!(!root.join("retained-source").exists());
    assert!(!directory.join("task").exists());
    assert_eq!(fs::read(directory.join("journal.json")).unwrap(), bytes);
    assert_eq!(
        fs::read(root.join("settings.json")).unwrap(),
        settings_before
    );
}

#[test]
fn valid_unselected_journal_and_unrecognized_material_are_retained_without_a_nonempty_gate() {
    let (_temporary, root) = fixture();
    settings(&root, Value::Null);
    let id = "task_unselected_disposable";
    let directory = root.join("migrations").join(id);
    fs::create_dir_all(&directory).unwrap();
    let bytes = serde_json::to_vec(&journal(&root, id)).unwrap();
    fs::write(directory.join("journal.json"), &bytes).unwrap();
    let unrelated = root.join("migrations/UNRECOGNIZED material");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("journal.json"), b"unknown retained bytes").unwrap();
    fs::create_dir(root.join("migrations/task_empty")).unwrap();
    require_before_initialization(&root).unwrap();
    require_recovery_inventory(&root).unwrap();
    assert_eq!(fs::read(directory.join("journal.json")).unwrap(), bytes);
    assert_eq!(
        fs::read(unrelated.join("journal.json")).unwrap(),
        b"unknown retained bytes"
    );
    assert!(!directory.join("task").exists());
}

#[test]
fn recognized_invalid_journal_is_unknown_and_preserved_before_any_initialization_write() {
    let (_temporary, root) = fixture();
    let directory = root.join("migrations/task_bad");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("journal.json"), b"invalid retained record").unwrap();
    let error = require_before_initialization(&root).unwrap_err();
    assert_eq!(error.code, "WORKSPACE_MIGRATION_FAILED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert!(!root.join("settings.json").exists());
    assert!(!root.join("runtime").exists());
    assert_eq!(
        fs::read(directory.join("journal.json")).unwrap(),
        b"invalid retained record"
    );
}
