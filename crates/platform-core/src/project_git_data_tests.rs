//! Retired Git-write records remain readable only to protect unfinished work.
use super::*;

#[test]
fn unfinished_legacy_save_blocks_new_operations_without_changing_the_record() {
    let (_temp, paths) = RuntimePaths::for_test();
    let root = paths
        .workspace_root()
        .join(".platform/project-state/git/disposable-record");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("task_disposable.json");
    let record = Journal {
        schema_version: 1,
        project_id: "disposable-record".into(),
        task_id: "task_disposable".into(),
        operation: "save-data".into(),
        phase: "prepared".into(),
        old_head: None,
        new_head: Some("a".repeat(40)),
        branch: Some("refs/heads/main".into()),
        previous_branch: None,
        snapshot_digest: "b".repeat(64),
        paths: vec![],
        data_publication: None,
    };
    atomic_json(&path, &record).unwrap();
    let original = fs::read(&path).unwrap();
    assert_eq!(
        check_data_operations_available(paths.workspace_root(), "disposable-record")
            .unwrap_err()
            .code,
        "PROJECT_GIT_DATA_RECOVERY_REQUIRED"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let mut completed = record;
    completed.phase = "completed".into();
    atomic_json(&path, &completed).unwrap();
    check_data_operations_available(paths.workspace_root(), "disposable-record").unwrap();
}
