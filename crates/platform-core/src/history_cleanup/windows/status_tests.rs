use super::*;

#[test]
fn retired_status_observes_actual_absence_on_the_original_volume_without_claiming_cleanup() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let source = paths
        .workspace_root()
        .parent()
        .unwrap()
        .join("disposable-retired-history-source");
    fs::create_dir(&source).unwrap();
    let expected = PersistedIdentity::capture(&source).unwrap();
    let id = format!("task_{}", Uuid::new_v4().simple());
    let slot = paths.migrations_dir().join(&id);
    fs::create_dir(&slot).unwrap();
    let journal = slot.join("journal.json");
    crate::builds::atomic_json(
        &journal,
        &json!({"sourcePath":source,"sourceIdentity":expected}),
    )
    .unwrap();
    let before = fs::read(&journal).unwrap();
    let identity = PersistedIdentity::capture(&journal).unwrap();
    assert!(retired_cleanup_status(&paths, &id).unwrap().is_none());
    assert!(identity.matches_path(&journal).unwrap());
    assert_eq!(fs::read(&journal).unwrap(), before);
    fs::remove_dir(&source).unwrap();
    let status = retired_cleanup_status(&paths, &id).unwrap().unwrap();
    assert_eq!(status["status"], "absent");
    assert_eq!(status["completed"], false);
    assert_eq!(status["accountingComplete"], false);
    assert!(identity.matches_path(&journal).unwrap());
    assert_eq!(fs::read(&journal).unwrap(), before);
    let mut unavailable = expected.clone();
    unavailable.device = u64::MAX;
    crate::builds::atomic_json(
        &journal,
        &json!({"sourcePath":source,"sourceIdentity":unavailable}),
    )
    .unwrap();
    let retained = fs::read(&journal).unwrap();
    assert!(retired_cleanup_status(&paths, &id).is_err());
    assert_eq!(fs::read(&journal).unwrap(), retained);
    assert!(!source.exists());
}
