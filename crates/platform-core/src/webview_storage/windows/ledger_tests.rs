//! Actual disposable UDFs; these tests never initialize a user data root or engine.
use super::*;

fn fixture() -> (tempfile::TempDir, StoreLedger) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-windows-store-retirement-中文 space-")
        .tempdir()
        .unwrap();
    let ledger = StoreLedger::new(temporary.path().canonicalize().unwrap());
    (temporary, ledger)
}
fn id(kind: ProjectKind, name: &str) -> String {
    match kind {
        ProjectKind::Web => format!("prj_disposable_{name}"),
        ProjectKind::Node => uuid::Uuid::new_v4().to_string(),
    }
}
fn payload(ledger: &StoreLedger, kind: ProjectKind, id: &str) -> std::path::PathBuf {
    let root = ledger.project_directory(kind, id).unwrap();
    fs::create_dir(root.join("Default")).unwrap();
    let path = root.join("Default/engine-data");
    fs::write(&path, b"disposable engine profile bytes").unwrap();
    path
}
fn receipt(ledger: &StoreLedger, kind: ProjectKind, id: &str) -> std::path::PathBuf {
    ledger.confirmation_path(kind, id).unwrap()
}

#[test]
fn both_kinds_remove_only_the_original_profile_and_leave_no_success_receipt() {
    for kind in [ProjectKind::Web, ProjectKind::Node] {
        let (_temporary, ledger) = fixture();
        let a = id(kind, "retire_a");
        let b = id(kind, "keep_b");
        ledger.record(kind, &a).unwrap();
        ledger.record(kind, &b).unwrap();
        payload(&ledger, kind, &a);
        let other = payload(&ledger, kind, &b);
        let other_record = fs::read(ledger.path(kind, &b).unwrap()).unwrap();
        assert!(!ledger.confirmed_cleanup_pending(kind, &a).unwrap());
        ledger.forget_confirmed(kind, &a).unwrap();
        ledger.forget_confirmed(kind, &a).unwrap();
        assert!(!ledger.project_directory(kind, &a).unwrap().exists());
        assert!(!receipt(&ledger, kind, &a).exists());
        assert!(!ledger.confirmed_cleanup_pending(kind, &a).unwrap());
        assert!(!ledger.contains(kind, &a).unwrap());
        assert_eq!(fs::read(other).unwrap(), b"disposable engine profile bytes");
        assert_eq!(
            fs::read(ledger.path(kind, &b).unwrap()).unwrap(),
            other_record
        );
        assert_eq!(ledger.records().unwrap().len(), 1);
    }
}

#[test]
fn both_kinds_resume_all_three_confirmed_tails_with_a_new_ledger() {
    for kind in [ProjectKind::Web, ProjectKind::Node] {
        for tail in ["receipt", "store-unlinked", "directory-unlinked"] {
            let (_temporary, ledger) = fixture();
            let selected = id(kind, tail.replace('-', "_").as_str());
            ledger.record(kind, &selected).unwrap();
            let original = ledger.read(kind, &selected).unwrap().unwrap();
            ledger.persist_confirmation(&original).unwrap();
            if tail != "receipt" {
                fs::remove_file(ledger.path(kind, &selected).unwrap()).unwrap();
            }
            if tail == "directory-unlinked" {
                fs::remove_dir(ledger.project_directory(kind, &selected).unwrap()).unwrap();
            }
            let reopened = StoreLedger::new(ledger.data.clone());
            assert!(reopened.confirmed_cleanup_pending(kind, &selected).unwrap());
            assert!(reopened.record(kind, &selected).is_err());
            reopened.forget_confirmed(kind, &selected).unwrap();
            assert!(!reopened.contains(kind, &selected).unwrap());
            assert!(!receipt(&reopened, kind, &selected).exists());
        }
    }
}

#[test]
fn high_file_id_bits_replaced_roots_and_unavailable_volumes_never_gain_cleanup_authority() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "full_id");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let data = payload(&ledger, ProjectKind::Web, &selected);
    let original = ledger.read(ProjectKind::Web, &selected).unwrap().unwrap();
    ledger.persist_confirmation(&original).unwrap();
    let path = receipt(&ledger, ProjectKind::Web, &selected);
    let original_bytes = fs::read(&path).unwrap();
    let mut damaged: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    let high = damaged["directoryIdentity"]["windowsFileId"][15]
        .as_u64()
        .unwrap();
    damaged["directoryIdentity"]["windowsFileId"][15] = serde_json::json!(high ^ 1);
    fs::write(&path, serde_json::to_vec(&damaged).unwrap()).unwrap();
    assert!(ledger
        .confirmed_cleanup_pending(ProjectKind::Web, &selected)
        .is_err());
    assert!(ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .is_err());
    assert_eq!(fs::read(&data).unwrap(), b"disposable engine profile bytes");
    fs::write(&path, &original_bytes).unwrap();
    let root = ledger
        .project_directory(ProjectKind::Web, &selected)
        .unwrap();
    fs::rename(&root, ledger.data.join("retained-original-profile")).unwrap();
    native::create_private_directory(&root).unwrap();
    fs::write(root.join("foreign"), b"retain replacement").unwrap();
    assert!(ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .is_err());
    assert_eq!(
        fs::read(root.join("foreign")).unwrap(),
        b"retain replacement"
    );
    fs::remove_file(root.join("foreign")).unwrap();
    fs::remove_dir(&root).unwrap();
    damaged = serde_json::from_slice(&original_bytes).unwrap();
    damaged["directoryIdentity"]["device"] = serde_json::json!(u64::MAX);
    fs::write(&path, serde_json::to_vec(&damaged).unwrap()).unwrap();
    assert!(ledger
        .confirmed_cleanup_pending(ProjectKind::Web, &selected)
        .is_err());
    assert!(path.exists());
}

#[test]
fn retained_root_pin_fails_before_any_payload_is_removed_then_a_new_ledger_retries() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "root_pin");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let data = payload(&ledger, ProjectKind::Web, &selected);
    let original_record = fs::read(ledger.path(ProjectKind::Web, &selected).unwrap()).unwrap();
    let profile = ledger.bind_profile(ProjectKind::Web, &selected).unwrap();
    assert!(ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .is_err());
    assert!(ledger
        .confirmed_cleanup_pending(ProjectKind::Web, &selected)
        .unwrap());
    assert_eq!(fs::read(&data).unwrap(), b"disposable engine profile bytes");
    assert_eq!(
        fs::read(ledger.path(ProjectKind::Web, &selected).unwrap()).unwrap(),
        original_record
    );
    drop(profile);
    StoreLedger::new(ledger.data.clone())
        .forget_confirmed(ProjectKind::Web, &selected)
        .unwrap();
    assert!(!receipt(&ledger, ProjectKind::Web, &selected).exists());
}

#[test]
fn actual_shared_payload_reader_retains_the_receipt_until_handle_release() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Node, "reader");
    ledger.record(ProjectKind::Node, &selected).unwrap();
    let data = payload(&ledger, ProjectKind::Node, &selected);
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0003)
        .open(&data)
        .unwrap();
    assert!(ledger
        .forget_confirmed(ProjectKind::Node, &selected)
        .is_err());
    assert!(ledger
        .confirmed_cleanup_pending(ProjectKind::Node, &selected)
        .unwrap());
    assert_eq!(fs::read(&data).unwrap(), b"disposable engine profile bytes");
    drop(reader);
    StoreLedger::new(ledger.data.clone())
        .forget_confirmed(ProjectKind::Node, &selected)
        .unwrap();
    assert!(!ledger
        .project_directory(ProjectKind::Node, &selected)
        .unwrap()
        .exists());
}

#[test]
fn a_delete_pending_receipt_is_not_reported_as_completed_and_retry_does_not_recreate_it() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "receipt_reader");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let record = ledger.read(ProjectKind::Web, &selected).unwrap().unwrap();
    ledger.persist_confirmation(&record).unwrap();
    let path = receipt(&ledger, ProjectKind::Web, &selected);
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0007)
        .open(&path)
        .unwrap();
    assert!(ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .is_err());
    assert!(!ledger
        .project_directory(ProjectKind::Web, &selected)
        .unwrap()
        .exists());
    assert!(ledger.sync_absence(ProjectKind::Web, &selected).is_err());
    drop(reader);
    StoreLedger::new(ledger.data.clone())
        .forget_confirmed(ProjectKind::Web, &selected)
        .unwrap();
    assert!(!path.exists());
}

#[test]
fn readonly_hardlinked_records_payloads_and_receipts_preserve_their_other_names() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "hardlinks");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let data = payload(&ledger, ProjectKind::Web, &selected);
    let record = ledger.read(ProjectKind::Web, &selected).unwrap().unwrap();
    ledger.persist_confirmation(&record).unwrap();
    let originals = [
        ledger.path(ProjectKind::Web, &selected).unwrap(),
        data,
        receipt(&ledger, ProjectKind::Web, &selected),
    ];
    let mut retained = Vec::new();
    for (index, path) in originals.iter().enumerate() {
        let outside = ledger.data.join(format!("external-original-{index}"));
        fs::hard_link(path, &outside).unwrap();
        native::set_read_only(path, true).unwrap();
        retained.push((
            outside,
            fs::read(path).unwrap(),
            native::identity(&native::open_read_file_allowing_hard_links(path).unwrap()).unwrap(),
        ));
    }
    ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .unwrap();
    for (path, bytes, identity) in retained {
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        assert!(identity.same_object(&native::open_no_reparse(&path).unwrap().identity));
        native::set_read_only(&path, false).unwrap();
    }
}

#[test]
fn junction_roots_are_rejected_and_internal_junctions_never_delete_external_targets() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "junction");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let root = ledger
        .project_directory(ProjectKind::Web, &selected)
        .unwrap();
    let external = ledger.data.join("disposable-external-original");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("sentinel"), b"preserve external bytes").unwrap();
    let junction = |link: &Path, target: &Path| {
        let command = platform_native::windows::system_paths::SystemPaths::current()
            .unwrap()
            .system
            .join("cmd.exe");
        assert!(std::process::Command::new(command)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status
            .success());
    };
    junction(&root.join("external-link"), &external);
    ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .unwrap();
    assert_eq!(
        fs::read(external.join("sentinel")).unwrap(),
        b"preserve external bytes"
    );
    ledger.record(ProjectKind::Web, &selected).unwrap();
    fs::rename(&root, ledger.data.join("retained-original-root")).unwrap();
    junction(&root, &external);
    assert!(ledger
        .forget_confirmed(ProjectKind::Web, &selected)
        .is_err());
    assert_eq!(
        fs::read(external.join("sentinel")).unwrap(),
        b"preserve external bytes"
    );
    fs::remove_dir(&root).unwrap();
}

#[test]
fn unknown_or_oversized_receipts_and_unbound_roots_are_read_only_failures() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "unknown");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    let data = payload(&ledger, ProjectKind::Web, &selected);
    let path = receipt(&ledger, ProjectKind::Web, &selected);
    for bytes in [b"unknown receipt".to_vec(), vec![b'x'; 4097]] {
        fs::write(&path, &bytes).unwrap();
        assert!(ledger
            .confirmed_cleanup_pending(ProjectKind::Web, &selected)
            .is_err());
        assert!(ledger
            .forget_confirmed(ProjectKind::Web, &selected)
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(&data).unwrap(), b"disposable engine profile bytes");
    }
    fs::remove_file(path).unwrap();
    let unbound = id(ProjectKind::Web, "unbound");
    let root = ledger
        .project_directory(ProjectKind::Web, &unbound)
        .unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("foreign"), b"unbound root bytes").unwrap();
    assert!(ledger.forget_confirmed(ProjectKind::Web, &unbound).is_err());
    assert_eq!(
        fs::read(root.join("foreign")).unwrap(),
        b"unbound root bytes"
    );
}

#[test]
fn a_receipt_name_replacement_after_udf_removal_is_never_retired_as_the_original() {
    let (_temporary, ledger) = fixture();
    let selected = id(ProjectKind::Web, "receipt_replacement");
    ledger.record(ProjectKind::Web, &selected).unwrap();
    payload(&ledger, ProjectKind::Web, &selected);
    let mut replacement = None;
    let error = forget_confirmed_with(&ledger, ProjectKind::Web, &selected, |path| {
        let original = ledger.data.join("retained-original-clear-receipt");
        let bytes = fs::read(path).unwrap();
        fs::rename(path, original).unwrap();
        let mut file = native::create_private_file(path).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        replacement = Some((path.to_owned(), native::identity(&file).unwrap(), bytes));
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "WEBVIEW_STORE_CLEANUP_UNCONFIRMED");
    let (path, identity, bytes) = replacement.unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(identity.same_object(&native::open_no_reparse(&path).unwrap().identity));
    assert!(!ledger
        .project_directory(ProjectKind::Web, &selected)
        .unwrap()
        .exists());
}
