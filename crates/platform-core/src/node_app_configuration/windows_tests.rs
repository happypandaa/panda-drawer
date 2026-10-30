// Actual disposable plaintext slots; no OS credential services or user keys.
use super::*;
use platform_native::windows::files as native;
use std::io::Write;

fn local_store() -> (
    tempfile::TempDir,
    Arc<LocalFileVault>,
    NodeConfigurationStore,
) {
    let temporary = tempfile::Builder::new()
        .prefix("disposable-config-中文 space-")
        .tempdir()
        .unwrap();
    let directory = temporary.path().canonicalize().unwrap();
    let instance = Uuid::new_v4();
    let vault = Arc::new(LocalFileVault::new(directory.join("credentials"), instance).unwrap());
    let store = NodeConfigurationStore::new(directory, instance, vault.clone()).unwrap();
    (temporary, vault, store)
}
fn private_bytes(path: &Path, bytes: &[u8]) {
    let mut file = native::create_private_file(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn actual_private_slot_save_reload_launch_and_overwrite_preserve_plaintext_contract() {
    let (_temporary, vault, store) = local_store();
    let credential = "disposable Windows plaintext credential";
    let view = store
        .save(&manifest(), request(0, Some(credential)))
        .unwrap();
    assert_eq!(view.secrets["API_KEY"], SecretStatus::Set);
    assert!(!serde_json::to_string(&view).unwrap().contains(credential));
    let record = store.read().unwrap();
    let entry = entry_uuid(&record.secrets["API_KEY"]).unwrap();
    assert_eq!(
        fs::read_to_string(vault.directory.join(entry.to_string())).unwrap(),
        credential
    );
    for directory in [&store.directory, &vault.directory] {
        native::verify_private(&native::pin_directory(directory).unwrap().file).unwrap();
    }
    for path in [
        store.directory.join("configuration.json"),
        store.directory.join("configuration.lock"),
        vault.directory.join(entry.to_string()),
    ] {
        let file = native::open_read_file(&path).unwrap();
        native::verify_private(&file).unwrap();
        assert_eq!(native::identity(&file).unwrap().links, 1);
    }
    let json = fs::read_to_string(store.directory.join("configuration.json")).unwrap();
    assert!(!json.contains(credential));
    let reopened_vault =
        Arc::new(LocalFileVault::new(vault.directory.clone(), store.instance).unwrap());
    let reopened =
        NodeConfigurationStore::new(store.directory.clone(), store.instance, reopened_vault)
            .unwrap();
    let snapshot = reopened.launch_snapshot(&manifest()).unwrap();
    assert_eq!(snapshot.environment["API_KEY"], credential);
    assert_eq!(snapshot.environment["COUNT"], "0");
    assert_eq!(snapshot.environment["ENABLED"], "false");
    assert_eq!(snapshot.redaction_values, [credential]);
    reopened
        .save(
            &manifest(),
            request(1, Some("disposable replacement credential")),
        )
        .unwrap();
    assert_eq!(reopened.launch_snapshot(&manifest()).unwrap().revision, 2);
    assert_eq!(vault.get(store.instance, entry).unwrap(), credential);
}

#[test]
fn inherited_credential_acl_links_and_invalid_sizes_are_refused_without_repair() {
    let (temporary, vault, store) = local_store();
    let outside = temporary.path().join("caller-owned-artificial-secret");
    fs::write(&outside, b"retain caller bytes").unwrap();
    let hard = Uuid::new_v4();
    fs::hard_link(&outside, vault.directory.join(hard.to_string())).unwrap();
    assert!(vault.contains(store.instance, hard).is_err());
    assert!(vault.get(store.instance, hard).is_err());
    let inherited = Uuid::new_v4();
    let inherited_path = vault.directory.join(inherited.to_string());
    fs::write(&inherited_path, b"unprotected artificial value").unwrap();
    assert!(
        native::verify_private(&native::open_no_reparse(&inherited_path).unwrap().file).is_err()
    );
    assert!(vault.get(store.instance, inherited).is_err());
    assert!(
        native::verify_private(&native::open_no_reparse(&inherited_path).unwrap().file).is_err()
    );
    for bytes in [Vec::new(), vec![b'x'; MAX_SECRET_BYTES + 1], vec![0xff]] {
        let entry = Uuid::new_v4();
        private_bytes(&vault.directory.join(entry.to_string()), &bytes);
        assert!(vault.get(store.instance, entry).is_err());
    }
    let alias = vault.directory.join(Uuid::new_v4().to_string());
    let target = temporary.path().join("external-disposable-directory");
    fs::create_dir(&target).unwrap();
    assert!(std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&target)
        .output()
        .unwrap()
        .status
        .success());
    let alias_entry = Uuid::parse_str(alias.file_name().unwrap().to_str().unwrap()).unwrap();
    assert!(vault.get(store.instance, alias_entry).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"retain caller bytes");
    assert!(std::process::Command::new("icacls.exe")
        .arg(&vault.directory)
        .arg("/inheritance:e")
        .output()
        .unwrap()
        .status
        .success());
    assert!(LocalFileVault::new(vault.directory.clone(), store.instance).is_err());
    assert!(
        native::verify_private(&native::pin_directory(&vault.directory).unwrap().file).is_err()
    );
}

#[test]
fn rollback_refuses_replaced_entry_and_credential_directory_and_preserves_unknown_files() {
    let (temporary, vault, store) = local_store();
    let entry = Uuid::new_v4();
    vault
        .insert(store.instance, entry, "owned disposable secret")
        .unwrap();
    let path = vault.directory.join(entry.to_string());
    let retained = temporary.path().join("retained-original-secret");
    fs::rename(&path, &retained).unwrap();
    private_bytes(&path, b"foreign replacement private bytes");
    assert!(vault.discard(store.instance, entry).is_err());
    assert_eq!(
        fs::read(&path).unwrap(),
        b"foreign replacement private bytes"
    );
    assert_eq!(fs::read(&retained).unwrap(), b"owned disposable secret");
    let unknown = Uuid::new_v4();
    let unknown_path = vault.directory.join(unknown.to_string());
    private_bytes(&unknown_path, b"unregistered private entry");
    assert!(vault.discard(store.instance, unknown).is_err());
    assert_eq!(
        fs::read(unknown_path).unwrap(),
        b"unregistered private entry"
    );
    let held = temporary.path().join("retained-original-credentials");
    fs::rename(&vault.directory, &held).unwrap();
    native::create_private_directory(&vault.directory).unwrap();
    private_bytes(
        &vault.directory.join(entry.to_string()),
        b"replacement directory bytes",
    );
    assert!(vault.get(store.instance, entry).is_err());
    assert!(vault.discard(store.instance, entry).is_err());
    assert_eq!(
        fs::read(vault.directory.join(entry.to_string())).unwrap(),
        b"replacement directory bytes"
    );
    assert_eq!(
        fs::read(held.join(entry.to_string())).unwrap(),
        b"foreign replacement private bytes"
    );
}

#[test]
fn failed_configuration_publication_discards_only_new_local_credential() {
    struct FailPublication {
        local: Arc<LocalFileVault>,
        directory: PathBuf,
        fail: AtomicBool,
    }
    impl SecretVault for FailPublication {
        fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
            self.local.insert(instance, entry, value)?;
            if self.fail.load(Ordering::SeqCst) {
                fs::rename(
                    self.directory.join("configuration.json"),
                    self.directory.join("retained-original-configuration"),
                )
                .unwrap();
                fs::create_dir(self.directory.join("configuration.json")).unwrap();
            }
            Ok(())
        }
        fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
            self.local.get(instance, entry)
        }
        fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
            self.local.contains(instance, entry)
        }
        fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
            self.local.discard(instance, entry)
        }
    }
    let (_temporary, local, original) = local_store();
    let provider = Arc::new(FailPublication {
        local: local.clone(),
        directory: original.directory.clone(),
        fail: AtomicBool::new(false),
    });
    let store = NodeConfigurationStore::new(
        original.directory.clone(),
        original.instance,
        provider.clone(),
    )
    .unwrap();
    store
        .save(&manifest(), request(0, Some("original disposable secret")))
        .unwrap();
    let before = fs::read(store.directory.join("configuration.json")).unwrap();
    provider.fail.store(true, Ordering::SeqCst);
    assert!(store
        .save(
            &manifest(),
            request(1, Some("new uncommitted disposable secret"))
        )
        .is_err());
    assert_eq!(fs::read_dir(&local.directory).unwrap().count(), 1);
    assert_eq!(
        fs::read(store.directory.join("retained-original-configuration")).unwrap(),
        before
    );
    assert!(store.directory.join("configuration.json").is_dir());
    fs::remove_dir(store.directory.join("configuration.json")).unwrap();
    fs::rename(
        store.directory.join("retained-original-configuration"),
        store.directory.join("configuration.json"),
    )
    .unwrap();
    assert_eq!(
        store.launch_snapshot(&manifest()).unwrap().environment["API_KEY"],
        "original disposable secret"
    );
    assert!(fs::read_dir(&store.directory).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".configuration-")));
}
