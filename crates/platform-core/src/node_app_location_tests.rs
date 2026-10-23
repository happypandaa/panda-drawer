use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temporary: tempfile::TempDir,
    apps: NodeApps,
    id: String,
    slot: PathBuf,
}
impl Fixture {
    fn new(managed: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let data = root.join("disposable-local-directory-platform");
        fs::create_dir(&data).unwrap();
        let apps = NodeApps::new(data.clone(), "/disposable-never-executed-node".into()).unwrap();
        let id = Uuid::new_v4().to_string();
        let slot = data.join("node-apps").join(&id);
        fs::create_dir_all(&slot).unwrap();
        let record = if managed {
            json!({"version":2,"id":id,"kind":"managed","generation":format!("gen_{}", "a".repeat(32)),
                "manifestDigest":"b".repeat(64),"programDigest":"c".repeat(64),
                "provenance":{"sourceId":format!("src_{}", "d".repeat(32)),"sourceDigest":"e".repeat(64),"sourceCommit":null,
                "node":{"path":"/disposable-never-used-node","version":"test"},"npm":{"path":"/disposable-never-used-npm","version":"test"},"preparedAt":0}})
        } else {
            // The author directory is deliberately unavailable. The action
            // opens the instance's data/settings slot without executing code.
            json!({"version":1,"id":id,"sourcePath":root.join("disposable-missing-author-source"),"manifestDigest":"a".repeat(64)})
        };
        fs::write(
            slot.join("registration.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            apps,
            id,
            slot,
        }
    }
}

#[tokio::test]
async fn node_local_directory_resolves_fixed_instance_for_external_and_managed_apps() {
    for managed in [false, true] {
        let fixture = Fixture::new(managed);
        let lease = fixture
            .apps
            .local_directory_lease(&fixture.id)
            .await
            .unwrap();
        assert_eq!(lease.location(), fixture.slot);
        lease.validate().unwrap();
        assert!(
            fixture.apps.inner.try_lock().is_err(),
            "delete/update waits for native dispatch"
        );
        drop(lease);
        assert!(fixture.apps.inner.try_lock().is_ok());
    }
}

#[tokio::test]
async fn node_local_directory_rejects_unknown_ids_symlinks_and_changed_slots() {
    let fixture = Fixture::new(false);
    for id in ["../../outside".to_owned(), Uuid::new_v4().to_string()] {
        assert!(fixture.apps.local_directory_lease(&id).await.is_err());
    }
    let lease = fixture
        .apps
        .local_directory_lease(&fixture.id)
        .await
        .unwrap();
    let retained = fixture.slot.with_extension("retained-disposable-original");
    fs::rename(&fixture.slot, &retained).unwrap();
    fs::create_dir(&fixture.slot).unwrap();
    fs::copy(
        retained.join("registration.json"),
        fixture.slot.join("registration.json"),
    )
    .unwrap();
    assert_eq!(
        lease.validate().unwrap_err().code,
        "NODE_APP_LOCATION_CHANGED"
    );
    drop(lease);
    fs::remove_file(fixture.slot.join("registration.json")).unwrap();
    fs::remove_dir(&fixture.slot).unwrap();
    symlink(&retained, &fixture.slot).unwrap();
    assert!(fixture
        .apps
        .local_directory_lease(&fixture.id)
        .await
        .is_err());
    assert!(retained.join("registration.json").is_file());
}

#[tokio::test]
async fn node_local_directory_rejects_changed_registration_before_dispatch() {
    let fixture = Fixture::new(false);
    let lease = fixture
        .apps
        .local_directory_lease(&fixture.id)
        .await
        .unwrap();
    let registration = fixture.slot.join("registration.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    record["sourcePath"] = json!(fixture.slot.with_extension("disposable-other-source"));
    fs::write(&registration, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        lease.validate().unwrap_err().code,
        "NODE_APP_LOCATION_CHANGED"
    );
}

#[tokio::test]
async fn node_local_directory_rejects_pending_delete_and_shutdown() {
    let fixture = Fixture::new(false);
    let receipts = fixture.apps.root.join(".deletions");
    fs::create_dir(&receipts).unwrap();
    fs::write(receipts.join("delete_disposable.json"), serde_json::to_vec(&json!({
        "version":1,"deletionId":"delete_disposable","appId":fixture.id,
        "originalPath":fixture.slot,"slotIdentity":{"device":1,"inode":1},"registrationDigest":"a".repeat(64),
        "phase":"intent","systemTrashPath":null,"error":null
    })).unwrap()).unwrap();
    assert_eq!(
        fixture
            .apps
            .local_directory_lease(&fixture.id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_DELETE_RECOVERY_REQUIRED"
    );
    fs::remove_file(receipts.join("delete_disposable.json")).unwrap();
    fixture.apps.inner.lock().await.shutting_down = true;
    assert_eq!(
        fixture
            .apps
            .local_directory_lease(&fixture.id)
            .await
            .err()
            .unwrap()
            .code,
        "SERVICE_STOPPING"
    );
}
