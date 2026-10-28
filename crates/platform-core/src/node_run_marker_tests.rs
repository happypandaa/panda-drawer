use super::*;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, NodeApps, String, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let manager = NodeApps::new(root.clone(), PathBuf::from("/disposable/no-executable")).unwrap();
    let id = Uuid::new_v4().to_string();
    let slot = root.join("node-apps").join(&id);
    fs::create_dir_all(slot.join("runs")).unwrap();
    fs::create_dir(slot.join("data")).unwrap();
    fs::write(slot.join("data/sentinel"), b"preserve disposable data").unwrap();
    (temporary, manager, id, slot)
}

#[tokio::test]
async fn previous_boot_markers_are_retired_without_touching_app_files() {
    let (_temporary, manager, id, slot) = fixture();
    let marker = slot.join("active.json");
    {
        let value = serde_json::to_value(RunMarker::launching(&id, slot.join("runs/new"))).unwrap();
        let _owner = StartIntentOwnership::create(&marker, &value).unwrap();
        let future_boot = marker.metadata().unwrap().created().unwrap() + Duration::from_secs(1);
        let report = manager.reconcile_run_markers_at(future_boot).await.unwrap();
        assert_eq!(report["retired"], 1);
        assert!(!marker.exists());
        assert_eq!(
            fs::read(slot.join("data/sentinel")).unwrap(),
            b"preserve disposable data"
        );
        assert!(matches!(
            inspect(&slot, &id, future_boot).unwrap().state,
            RunState::Absent
        ));
    }
}

#[tokio::test]
async fn same_boot_incomplete_v2_and_unsupported_records_remain_unknown() {
    let (_temporary, manager, id, slot) = fixture();
    for value in [
        json!({"version":2}),
        json!({"version":3,"hostPid":2147483646u32}),
        json!({"version":"1","hostPid":2147483646u32}),
        serde_json::to_value(RunMarker::launching(&id, slot.join("runs/new"))).unwrap(),
    ] {
        let marker = slot.join("active.json");
        let owner = StartIntentOwnership::create(&marker, &value).unwrap();
        let original = fs::read(&marker).unwrap();
        let report = manager
            .reconcile_run_markers_at(node_process_identity::boot_time().unwrap())
            .await
            .unwrap();
        assert_eq!(report["unconfirmed"], 1);
        assert!(manager.stop_runtime(&id).await.is_err());
        assert_eq!(fs::read(&marker).unwrap(), original);
        if value.get("appId").is_none() {
            let future_boot =
                marker.metadata().unwrap().created().unwrap() + Duration::from_secs(1);
            assert!(matches!(
                inspect(&slot, &id, future_boot).unwrap().state,
                RunState::Unconfirmed
            ));
        }
        owner.remove_if_owned(&marker).unwrap();
    }
}

#[test]
fn second_phase_keeps_original_inode_and_birth_time_and_does_not_overwrite_replacement() {
    let (_temporary, _manager, id, slot) = fixture();
    let path = slot.join("active.json");
    let mut record = RunMarker::launching(&id, slot.join("runs/new"));
    let mut owner = StartIntentOwnership::create(&path, &record).unwrap();
    let before = path.metadata().unwrap();
    record.bind(RuntimeProcessIdentity {
        pid: 999999,
        pgid: 999999,
        started_at_sec: 1,
        started_at_usec: 0,
        executable: PathBuf::from("/disposable/node"),
        nonce: "disposable-identity-nonce".into(),
    });
    owner.publish(&path, &record).unwrap();
    let after = path.metadata().unwrap();
    assert_eq!(
        (before.dev(), before.ino(), before.created().unwrap()),
        (after.dev(), after.ino(), after.created().unwrap())
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["phase"],
        "running"
    );
    fs::rename(&path, slot.join("displaced-owned-marker")).unwrap();
    fs::write(&path, b"new foreign marker").unwrap();
    assert!(owner.publish(&path, &record).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"new foreign marker");
}

#[test]
fn same_boot_cleanup_gate_is_unknown_but_previous_boot_evidence_is_preserved_without_gate() {
    let (_temporary, _manager, id, slot) = fixture();
    let cleanup = slot.join(format!("{START_INTENT_CLEANUP_PREFIX}disposable"));
    fs::create_dir(&cleanup).unwrap();
    fs::write(cleanup.join("evidence"), b"preserve cleanup evidence").unwrap();
    let boot = node_process_identity::boot_time().unwrap();
    assert!(matches!(
        inspect(&slot, &id, boot).unwrap().state,
        RunState::Unconfirmed
    ));
    let future_boot = cleanup.metadata().unwrap().created().unwrap() + Duration::from_secs(1);
    assert!(!cleanup_pending(&slot, future_boot).unwrap());
    assert_eq!(
        fs::read(cleanup.join("evidence")).unwrap(),
        b"preserve cleanup evidence"
    );
}

#[test]
fn unsafe_marker_types_and_partial_publications_do_not_authorize_cleanup_or_signals() {
    use std::os::unix::fs::symlink;
    let (_temporary, _manager, id, slot) = fixture();
    let path = slot.join("active.json");
    symlink("/disposable/missing", &path).unwrap();
    assert!(inspect(&slot, &id, node_process_identity::boot_time().unwrap()).is_err());
    fs::remove_file(&path).unwrap();
    let owner = StartIntentOwnership::create(&path, &json!({"version":2})).unwrap();
    assert!(matches!(
        inspect(&slot, &id, node_process_identity::boot_time().unwrap())
            .unwrap()
            .state,
        RunState::Unconfirmed
    ));
    assert!(path.exists());
    owner.remove_if_owned(&path).unwrap();
}
