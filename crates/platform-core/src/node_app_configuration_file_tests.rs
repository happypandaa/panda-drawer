// Real, disposable filesystem verification. No OS credential services or user keys.
use super::*;

fn local_store() -> (
    tempfile::TempDir,
    Arc<LocalFileVault>,
    NodeConfigurationStore,
) {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().canonicalize().unwrap();
    let instance = Uuid::new_v4();
    let vault = Arc::new(LocalFileVault::new(directory.join("credentials"), instance).unwrap());
    let store = NodeConfigurationStore::new(directory, instance, vault.clone()).unwrap();
    (temporary, vault, store)
}

#[cfg(unix)]
#[test]
fn local_save_reload_launch_and_private_permissions_without_secret_echo() {
    use std::os::unix::fs::PermissionsExt;
    let (temporary, vault, store) = local_store();
    let credential = "artificial-local-key-not-a-user-key";
    let view = store
        .save(&manifest(), request(0, Some(credential)))
        .unwrap();
    assert_eq!(view.secrets["API_KEY"], SecretStatus::Set);
    assert!(!serde_json::to_string(&view).unwrap().contains(credential));
    let record = store.read().unwrap();
    let entry = entry_uuid(&record.secrets["API_KEY"]).unwrap();
    let path = vault.directory.join(entry.to_string());
    assert_eq!(fs::read_to_string(&path).unwrap(), credential);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert_eq!(
        fs::metadata(&vault.directory).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    let disk = fs::read_to_string(temporary.path().join("configuration.json")).unwrap();
    assert!(!disk.contains(credential));
    let reopened_vault =
        Arc::new(LocalFileVault::new(vault.directory.clone(), store.instance).unwrap());
    let reopened =
        NodeConfigurationStore::new(store.directory.clone(), store.instance, reopened_vault)
            .unwrap();
    let snapshot = reopened.launch_snapshot(&manifest()).unwrap();
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.environment["API_KEY"], credential);
    assert_eq!(snapshot.environment["COUNT"], "0");
    assert_eq!(snapshot.environment["ENABLED"], "false");
}

#[test]
fn old_provider_binding_without_local_file_requires_reentering_key() {
    let (temporary, _old_provider, old_store) = store();
    old_store
        .save(
            &manifest(),
            request(0, Some("artificial-old-provider-value")),
        )
        .unwrap();
    let directory = temporary.path().canonicalize().unwrap();
    let vault =
        Arc::new(LocalFileVault::new(directory.join("credentials"), old_store.instance).unwrap());
    let local = NodeConfigurationStore::new(directory, old_store.instance, vault).unwrap();
    let view = local.view(&manifest()).unwrap();
    assert_eq!(view.secrets["API_KEY"], SecretStatus::Missing);
    assert!(!view.can_start);
    assert!(local
        .launch_snapshot(&manifest())
        .err()
        .unwrap()
        .to_string()
        .contains("重新填写"));
    let mut rebind = request(1, None);
    rebind
        .secrets
        .insert("API_KEY".into(), SecretAction::Rebind);
    assert!(local.save(&manifest(), rebind).is_err());
    assert_eq!(local.view(&manifest()).unwrap().revision, 1);
    local
        .save(&manifest(), request(1, Some("artificial-new-local-value")))
        .unwrap();
    assert_eq!(
        local.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
        "artificial-new-local-value"
    );
}

#[test]
fn immutable_entries_and_rollback_cannot_remove_preexisting_files() {
    let (_temporary, vault, store) = local_store();
    let entry = Uuid::new_v4();
    vault
        .insert(store.instance, entry, "artificial-original")
        .unwrap();
    assert!(vault
        .insert(store.instance, entry, "artificial-replacement")
        .is_err());
    assert_eq!(
        vault.get(store.instance, entry).unwrap(),
        "artificial-original"
    );
    let other = LocalFileVault::new(vault.directory.clone(), store.instance).unwrap();
    assert!(other.discard(store.instance, entry).is_err());
    assert_eq!(
        vault.get(store.instance, entry).unwrap(),
        "artificial-original"
    );
    assert!(vault.get(Uuid::new_v4(), entry).is_err());
    vault.discard(store.instance, entry).unwrap();
    assert!(!vault.contains(store.instance, entry).unwrap());
}

#[cfg(unix)]
#[test]
fn directory_permissions_links_special_files_and_size_are_rejected() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (temporary, vault, store) = local_store();
    let outside = temporary.path().join("artificial-external");
    fs::write(&outside, b"artificial-secret").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    let linked = Uuid::new_v4();
    symlink(&outside, vault.directory.join(linked.to_string())).unwrap();
    assert!(vault.contains(store.instance, linked).is_err());
    assert!(vault.get(store.instance, linked).is_err());
    let hardlink = Uuid::new_v4();
    fs::hard_link(&outside, vault.directory.join(hardlink.to_string())).unwrap();
    assert!(vault.get(store.instance, hardlink).is_err());
    let special = Uuid::new_v4();
    let special_path = vault.directory.join(special.to_string());
    let raw = std::ffi::CString::new(special_path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
    assert!(vault.get(store.instance, special).is_err());
    for (bytes, mode) in [
        (vec![b'x'; MAX_SECRET_BYTES + 1], 0o600),
        (vec![0xff], 0o600),
        (b"artificial".to_vec(), 0o644),
    ] {
        let entry = Uuid::new_v4();
        let path = vault.directory.join(entry.to_string());
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(vault.get(store.instance, entry).is_err());
    }
    fs::set_permissions(&vault.directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(LocalFileVault::new(vault.directory.clone(), store.instance).is_err());
    assert_eq!(
        fs::metadata(&vault.directory).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert_eq!(fs::read(&outside).unwrap(), b"artificial-secret");
}

#[cfg(unix)]
#[test]
fn failed_configuration_commit_rolls_back_only_new_local_credential() {
    use std::os::unix::fs::PermissionsExt;
    struct ReadOnlyAfterInsert {
        local: Arc<LocalFileVault>,
        configuration: PathBuf,
        fail: AtomicBool,
    }
    impl SecretVault for ReadOnlyAfterInsert {
        fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
            self.local.insert(instance, entry, value)?;
            if self.fail.load(Ordering::SeqCst) {
                fs::set_permissions(&self.configuration, fs::Permissions::from_mode(0o500))
                    .unwrap();
            }
            Ok(())
        }
        fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
            self.local.get(instance, entry)
        }
        fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
            self.local.discard(instance, entry)
        }
        fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
            self.local.contains(instance, entry)
        }
    }
    let (temporary, local, original) = local_store();
    let provider = Arc::new(ReadOnlyAfterInsert {
        local: local.clone(),
        configuration: original.directory.clone(),
        fail: AtomicBool::new(false),
    });
    let store = NodeConfigurationStore::new(
        original.directory.clone(),
        original.instance,
        provider.clone(),
    )
    .unwrap();
    store
        .save(&manifest(), request(0, Some("artificial-original")))
        .unwrap();
    let before = fs::read(temporary.path().join("configuration.json")).unwrap();
    provider.fail.store(true, Ordering::SeqCst);
    let attempted = store.save(
        &manifest(),
        request(1, Some("artificial-new-not-committed")),
    );
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(attempted.is_err());
    assert_eq!(
        before,
        fs::read(temporary.path().join("configuration.json")).unwrap()
    );
    assert_eq!(fs::read_dir(&local.directory).unwrap().count(), 1);
    assert_eq!(
        store.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
        "artificial-original"
    );
}
