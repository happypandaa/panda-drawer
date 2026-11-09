use super::*;

fn disposable() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-node-intent-中文 space-")
        .tempdir()
        .unwrap()
}
fn cleanup_names(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(START_INTENT_CLEANUP_PREFIX)
        })
        .collect()
}

#[test]
fn actual_intent_publish_keeps_full_identity_and_creation_time_then_retires() {
    let temporary = disposable();
    let slot = temporary.path().join("selected-slot");
    fs::create_dir(&slot).unwrap();
    let path = slot.join("active.json");
    let mut owner = StartIntentOwnership::create(&path, &json!({"phase":"launching"})).unwrap();
    let before = native::open_no_reparse(&path).unwrap().identity;
    let created = owner.file.metadata().unwrap().created().unwrap();
    owner.require_current(&path).unwrap();
    owner
        .publish(&path, &json!({"phase":"running","disposable":true}))
        .unwrap();
    assert_eq!(native::open_no_reparse(&path).unwrap().identity, before);
    assert_eq!(owner.file.metadata().unwrap().created().unwrap(), created);
    assert_eq!(
        serde_json::from_slice::<Value>(&read_small(&path).unwrap()).unwrap()["phase"],
        "running"
    );
    owner.remove_if_owned(&path).unwrap();
    assert!(!path.exists());
    assert!(cleanup_names(&slot).is_empty());
    owner.remove_if_owned(&path).unwrap();
    assert!(owner.file.metadata().is_err());
    // A completed marker receipt must not keep the retired slot pinned merely
    // because its Rust receipt object remains available to the caller.
    fs::rename(&slot, temporary.path().join("retired-slot")).unwrap();
}

#[test]
fn existing_current_user_records_do_not_require_or_repair_a_private_dacl() {
    let temporary = disposable();
    let path = temporary.path().join("active.json");
    fs::write(&path, b"{\"ordinary\":true}").unwrap();
    let opened = native::open_no_reparse(&path).unwrap();
    assert!(native::verify_private(&opened.file).is_err());
    let original = opened.identity.clone();
    drop(opened);
    let owner = StartIntentOwnership::open_existing(&path).unwrap();
    owner.require_current(&path).unwrap();
    let reopened = native::open_no_reparse(&path).unwrap();
    assert!(native::verify_private(&reopened.file).is_err());
    assert_eq!(reopened.identity, original);
    drop(reopened);
    owner.remove_if_owned(&path).unwrap();
    assert!(!path.exists());
    assert!(cleanup_names(temporary.path()).is_empty());
}

#[test]
fn receipt_budget_bounds_actual_reads_without_tightening_the_source_record_limit() {
    let temporary = disposable();
    let oversized = temporary.path().join("oversized-receipt.json");
    let file = native::create_private_file(&oversized).unwrap();
    file.set_len(RECEIPT_BYTES as u64 + 1).unwrap();
    file.sync_all().unwrap();
    let before = native::identity(&file).unwrap();
    assert!(read_receipt_bytes(&oversized).is_err());
    assert_eq!(native::identity(&file).unwrap(), before);
    assert_eq!(file.metadata().unwrap().len(), RECEIPT_BYTES as u64 + 1);
    drop(file);

    // This real original record remains inside Mac's existing 256 KiB budget;
    // its compact receipt needs space for UTF-8 bytes expanded into JSON u8s.
    let active = temporary.path().join("active.json");
    let owner =
        StartIntentOwnership::create(&active, &json!({"disposable":"测".repeat(85_000)})).unwrap();
    assert!(owner.contents.len() <= SOURCE_RECORD_BYTES);
    owner.remove_if_owned(&active).unwrap();
    assert!(!active.exists());
    assert!(cleanup_names(temporary.path()).is_empty());
}

#[test]
fn actual_shared_reader_keeps_pending_retirement_until_retry_after_release() {
    let temporary = disposable();
    let path = temporary.path().join("active.json");
    let owner =
        StartIntentOwnership::create(&path, &json!({"disposable":"pending reader"})).unwrap();
    let reader = File::open(&path).unwrap();
    let error = owner.remove_if_owned(&path).unwrap_err();
    assert!(error.details["remainingPath"].is_string());
    assert!(!path.exists());
    assert_eq!(cleanup_names(temporary.path()).len(), 2);
    drop(reader);
    owner.remove_if_owned(&path).unwrap();
    assert!(cleanup_names(temporary.path()).is_empty());
}

#[test]
fn replacements_before_or_after_capture_keep_foreign_bytes_and_cleanup_gate() {
    for after_capture in [false, true] {
        let temporary = disposable();
        let path = temporary.path().join("active.json");
        let owner = StartIntentOwnership::create(&path, &json!({"disposable":"original"})).unwrap();
        let saved = temporary.path().join("preserved-original.json");
        let result = owner.remove_if_owned_with(&path, |point| match (point, after_capture) {
            (IntentCleanupPoint::BeforeCapture, false) => {
                fs::rename(&path, &saved).unwrap();
                fs::write(&path, b"foreign before capture").unwrap();
            }
            (IntentCleanupPoint::Captured, true) => {
                fs::write(&path, b"foreign after capture").unwrap();
            }
            _ => (),
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            if after_capture {
                b"foreign after capture".as_slice()
            } else {
                b"foreign before capture".as_slice()
            }
        );
        assert_eq!(cleanup_names(temporary.path()).len(), 2);
        if !after_capture {
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(&saved).unwrap()).unwrap()["disposable"],
                "original"
            );
        }
    }
}

#[test]
fn changed_bytes_and_hard_links_are_not_adopted_or_removed() {
    let temporary = disposable();
    let path = temporary.path().join("active.json");
    let mut owner = StartIntentOwnership::create(&path, &json!({"disposable":"original"})).unwrap();
    fs::write(&path, b"changed in place").unwrap();
    assert!(owner
        .publish(&path, &json!({"must":"not overwrite"}))
        .is_err());
    assert!(owner.remove_if_owned(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"changed in place");
    let other = temporary.path().join("external-original.json");
    fs::hard_link(&path, &other).unwrap();
    assert!(StartIntentOwnership::open_existing(&path).is_err());
    assert_eq!(fs::read(other).unwrap(), b"changed in place");
    assert!(cleanup_names(temporary.path()).is_empty());
}

#[test]
fn real_private_record_creation_and_exclusive_move_preserve_occupied_names() {
    let temporary = disposable();
    let root = temporary.path().join("slot");
    private_dir(&root).unwrap();
    let bound = native::open_no_reparse(&root).unwrap();
    native::verify_private(&bound.file).unwrap();
    drop(bound);
    let source = root.join("registration.pending");
    let target = root.join("registration.json");
    write_new_json(&source, &json!({"disposable":true})).unwrap();
    fs::write(&target, b"foreign occupied").unwrap();
    assert!(move_node_record_exclusive(&source, &target).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"foreign occupied");
    assert!(source.exists());
    let free = root.join("registration-ready.json");
    move_node_record_exclusive(&source, &free).unwrap();
    assert!(!source.exists());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&free).unwrap()).unwrap()["disposable"],
        true
    );
}

#[test]
fn prepared_and_captured_receipts_resume_after_the_original_owner_is_lost() {
    for captured in [false, true] {
        let temporary = disposable();
        let path = temporary.path().join("active.json");
        let owner = StartIntentOwnership::create(
            &path,
            &json!({"disposable":"durable interruption","captured":captured}),
        )
        .unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.remove_if_owned_with(&path, |point| {
                if matches!(
                    (point, captured),
                    (IntentCleanupPoint::BeforeCapture, false)
                        | (IntentCleanupPoint::Captured, true)
                ) {
                    panic!("controlled interruption after the actual durable receipt");
                }
            })
        }));
        assert!(interrupted.is_err());
        assert_eq!(path.exists(), !captured);
        assert_eq!(cleanup_names(temporary.path()).len(), 2);
        let journal = cleanup_names(temporary.path())
            .into_iter()
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap();
        let receipt: CleanupReceipt = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
        assert!(receipt.resources_closed);
        assert!(receipt
            .journal
            .matches(&native::open_no_reparse(&journal).unwrap().identity));
        native::verify_private(&File::open(&journal).unwrap()).unwrap();
        drop(owner);
        // No retained owner or in-memory request flag participates in recovery.
        resume_cleanup_receipts(temporary.path(), std::time::SystemTime::UNIX_EPOCH).unwrap();
        assert!(!path.exists());
        assert!(cleanup_names(temporary.path()).is_empty());
        assert!(
            !run_marker::cleanup_pending(temporary.path(), std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
        );
    }
}

#[test]
fn an_external_gate_reader_keeps_the_receipt_until_the_gate_really_disappears() {
    let temporary = disposable();
    let path = temporary.path().join("active.json");
    let owner = StartIntentOwnership::create(&path, &json!({"disposable":"gate reader"})).unwrap();
    let mut reader = None;
    let error = owner
        .remove_if_owned_with(&path, |point| {
            if matches!(point, IntentCleanupPoint::Captured) {
                let gate = cleanup_names(temporary.path())
                    .into_iter()
                    .find(|path| path.is_dir())
                    .unwrap();
                reader = Some(native::directory_file(&gate).unwrap());
            }
        })
        .unwrap_err();
    assert_eq!(error.code, "FILESYSTEM_ERROR");
    assert!(!path.exists());
    let journal = cleanup_names(temporary.path())
        .into_iter()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    assert!(journal.is_file());
    assert!(Retirement::load(&journal, receipt_gate(&journal).unwrap()).is_ok());
    drop(owner);
    drop(reader);
    resume_cleanup_receipts(temporary.path(), std::time::SystemTime::UNIX_EPOCH).unwrap();
    assert!(cleanup_names(temporary.path()).is_empty());
}

#[test]
fn replaced_receipts_gate_or_source_and_unknown_material_are_preserved() {
    for replacement in ["receipt", "gate", "source", "bytes", "closed"] {
        let temporary = disposable();
        let path = temporary.path().join("active.json");
        let owner = StartIntentOwnership::create(&path, &json!({"disposable":"original"})).unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner.remove_if_owned_with(&path, |point| {
                if matches!(point, IntentCleanupPoint::BeforeCapture) {
                    panic!("controlled prepared interruption");
                }
            })
        }));
        assert!(interrupted.is_err());
        drop(owner);
        let journal = cleanup_names(temporary.path())
            .into_iter()
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap();
        let gate = receipt_gate(&journal).unwrap();
        let original = fs::read(&path).unwrap();
        match replacement {
            "receipt" => {
                let bytes = fs::read(&journal).unwrap();
                fs::rename(&journal, temporary.path().join("preserved-receipt.json")).unwrap();
                let mut foreign = native::create_private_file(&journal).unwrap();
                foreign.write_all(&bytes).unwrap();
                foreign.sync_all().unwrap();
            }
            "gate" => {
                fs::rename(&gate, temporary.path().join("preserved-gate")).unwrap();
                native::create_private_directory(&gate).unwrap();
                fs::write(gate.join("foreign-child"), b"preserve foreign gate").unwrap();
            }
            "source" => {
                fs::rename(&path, temporary.path().join("preserved-original.json")).unwrap();
                fs::write(&path, &original).unwrap();
            }
            "bytes" => fs::write(&path, b"changed original bytes").unwrap(),
            "closed" => {
                let mut receipt: CleanupReceipt =
                    serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
                receipt.resources_closed = false;
                fs::write(&journal, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            resume_cleanup_receipts(temporary.path(), std::time::SystemTime::UNIX_EPOCH).is_err()
        );
        assert!(journal.exists() && gate.exists() && path.exists());
        assert_eq!(
            fs::read(&path).unwrap(),
            if replacement == "bytes" {
                b"changed original bytes".to_vec()
            } else {
                original
            }
        );
        if replacement == "gate" {
            assert_eq!(
                fs::read(gate.join("foreign-child")).unwrap(),
                b"preserve foreign gate"
            );
        }
    }
    let temporary = disposable();
    let old_gate = temporary
        .path()
        .join(format!("{START_INTENT_CLEANUP_PREFIX}old-unknown"));
    fs::create_dir(&old_gate).unwrap();
    fs::write(old_gate.join("foreign-evidence"), b"preserve unknown scope").unwrap();
    assert!(!run_marker::cleanup_pending(
        temporary.path(),
        std::time::SystemTime::now() + Duration::from_secs(60)
    )
    .unwrap());
    assert_eq!(
        fs::read(old_gate.join("foreign-evidence")).unwrap(),
        b"preserve unknown scope"
    );
}

#[test]
fn moving_original_receipt_resources_to_a_replacement_parent_never_adopts_that_parent() {
    let temporary = disposable();
    let slot = temporary.path().join("slot");
    fs::create_dir(&slot).unwrap();
    let path = slot.join("active.json");
    let owner =
        StartIntentOwnership::create(&path, &json!({"disposable":"parent identity"})).unwrap();
    let retired = {
        let file = owner.file.0.lock().unwrap();
        Retirement::prepare(&path, file.as_ref().unwrap(), &owner.contents).unwrap()
    };
    let original_parent = retired.receipt.parent.clone();
    drop(retired);
    drop(owner);
    let preserved = temporary.path().join("preserved-parent");
    fs::rename(&slot, &preserved).unwrap();
    fs::create_dir(&slot).unwrap();
    for entry in fs::read_dir(&preserved).unwrap() {
        let entry = entry.unwrap();
        fs::rename(entry.path(), slot.join(entry.file_name())).unwrap();
    }
    assert!(!original_parent.matches(&native::open_no_reparse(&slot).unwrap().identity));
    assert!(resume_cleanup_receipts(&slot, std::time::SystemTime::UNIX_EPOCH).is_err());
    assert_eq!(cleanup_names(&slot).len(), 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["disposable"],
        "parent identity"
    );
}

#[test]
fn unknown_and_invalid_preboot_material_follow_the_original_gate_without_any_mutation() {
    let temporary = disposable();
    let root = temporary.path();
    let old_gate = root.join(format!("{START_INTENT_CLEANUP_PREFIX}unknown-old-gate"));
    fs::create_dir(&old_gate).unwrap();
    let unknown_bytes = old_gate.join("preserved-evidence");
    fs::write(&unknown_bytes, b"unknown gate bytes").unwrap();
    let malformed = root.join(format!(
        "{START_INTENT_CLEANUP_PREFIX}{}.json",
        Uuid::new_v4()
    ));
    let mut file = native::create_private_file(&malformed).unwrap();
    file.write_all(b"{not current receipt JSON").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let gate_id = native::open_no_reparse(&old_gate).unwrap().identity;
    let journal_id = native::open_no_reparse(&malformed).unwrap().identity;
    let bytes = fs::read(&malformed).unwrap();
    assert!(run_marker::cleanup_pending(root, std::time::SystemTime::UNIX_EPOCH).is_err());
    let later_boot = std::time::SystemTime::now() + Duration::from_secs(60);
    assert!(!run_marker::cleanup_pending(root, later_boot).unwrap());
    assert_eq!(
        native::open_no_reparse(&old_gate).unwrap().identity,
        gate_id
    );
    assert_eq!(
        native::open_no_reparse(&malformed).unwrap().identity,
        journal_id
    );
    assert_eq!(fs::read(&malformed).unwrap(), bytes);
    assert_eq!(fs::read(&unknown_bytes).unwrap(), b"unknown gate bytes");
    assert_eq!(cleanup_names(root).len(), 2);
}

#[test]
#[ignore = "Finite Job-owned identity fixture, only started with an explicit disposable directory"]
fn intent_identity_child() {
    let root = PathBuf::from(std::env::var_os("PANDA_DISPOSABLE_INTENT_ROOT").unwrap());
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-node-intent-"));
    std::thread::sleep(Duration::from_secs(60));
}

#[tokio::test]
async fn inspection_stop_can_retry_a_durable_retirement_after_consuming_its_original_owner() {
    use platform_native::windows::process::{OwnedProcess, Spawn};
    let temporary = disposable();
    let slot = temporary.path().join("selected-slot");
    fs::create_dir(&slot).unwrap();
    fs::create_dir(slot.join("runs")).unwrap();
    let child = OwnedProcess::spawn(Spawn {
        program: std::env::current_exe().unwrap(),
        arguments: [
            "--ignored",
            "--exact",
            "backend_run_marker::file_io::tests::intent_identity_child",
            "--nocapture",
            "--test-threads=1",
        ]
        .into_iter()
        .map(Into::into)
        .collect(),
        current_directory: temporary.path().canonicalize().unwrap(),
        environment: vec![(
            "PANDA_DISPOSABLE_INTENT_ROOT".into(),
            temporary.path().as_os_str().to_owned(),
        )],
    })
    .unwrap();
    let identity =
        crate::node_process_identity::capture_owned(child.pid, "disposable-intent-identity-nonce")
            .unwrap();
    let id = Uuid::new_v4().to_string();
    let mut marker =
        run_marker::RunMarker::launching(&id, slot.join("runs").join("disposable-run"));
    marker.bind(identity);
    let path = slot.join("active.json");
    let owner = StartIntentOwnership::create(&path, &marker).unwrap();
    drop(owner);
    let reader = File::open(&path).unwrap();
    child.terminate(0).unwrap();
    assert_eq!(child.wait(Duration::from_secs(5)).unwrap(), Some(0));
    assert!(child.wait_empty(Duration::from_secs(5)).unwrap());
    drop(child);
    let inspection = run_marker::inspect(&slot, &id, std::time::SystemTime::UNIX_EPOCH).unwrap();
    assert!(matches!(inspection.state, run_marker::RunState::Stopped));
    assert!(inspection.stop_and_retire(&slot).await.is_err());
    // Inspection::stop_and_retire consumes and drops its old owner on Err.
    assert!(!path.exists());
    assert_eq!(cleanup_names(&slot).len(), 2);
    drop(reader);
    let retry = run_marker::inspect(&slot, &id, std::time::SystemTime::UNIX_EPOCH).unwrap();
    assert!(matches!(retry.state, run_marker::RunState::Absent));
    retry.stop_and_retire(&slot).await.unwrap();
    assert!(cleanup_names(&slot).is_empty());
    fs::rename(&slot, temporary.path().join("retired-slot")).unwrap();
}
