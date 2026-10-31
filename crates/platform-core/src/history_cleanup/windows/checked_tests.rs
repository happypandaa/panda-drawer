//! Content-verified removal against real, named disposable NTFS objects.
use super::*;
use std::os::windows::fs::OpenOptionsExt;

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-w5-checked-history-中文 space-")
        .tempdir()
        .unwrap()
}

#[test]
fn real_checked_snapshot_has_full_ids_and_readonly_and_preserves_an_external_hardlink() {
    let temporary = fixture();
    let base = temporary.path().canonicalize().unwrap();
    let selected = base.join("selected");
    let relative = PathBuf::from("中".repeat(86)).join("只读文件.txt");
    fs::create_dir_all(selected.join(relative.parent().unwrap())).unwrap();
    let payload = selected.join(&relative);
    fs::write(&payload, b"real private payload").unwrap();
    native::set_read_only(&payload, true).unwrap();
    let original = base.join("external-original");
    fs::write(&original, b"external must remain").unwrap();
    native::set_read_only(&original, true).unwrap();
    fs::hard_link(&original, selected.join("owned-link")).unwrap();
    let original_identity = PersistedIdentity::capture(&original).unwrap();
    let id = native::open_no_reparse(&payload).unwrap().identity;
    let expected = PersistedIdentity::capture(&selected).unwrap();
    let mut called = false;
    let removed = remove_checked(&selected, &expected, |snapshot| {
        called = true;
        let file = snapshot
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == json!(relative))
            .unwrap();
        assert_eq!(file["identity"]["device"], id.volume_serial);
        assert_eq!(file["identity"]["windowsFileId"], json!(id.file_id));
        assert_eq!(file["readOnly"], true);
        assert!(file.get("mode").is_none());
        assert_eq!(file["digest"].as_str().unwrap().len(), 64);
        assert_eq!(file["bytes"], 20);
        Ok(())
    })
    .unwrap();
    assert!(called);
    assert_eq!(removed["completed"], true);
    assert_eq!(removed["removedFileCount"], 2);
    assert!(!selected.exists());
    assert!(original_identity.matches_path(&original).unwrap());
    assert_eq!(fs::read(&original).unwrap(), b"external must remain");
    assert!(fs::metadata(&original).unwrap().permissions().readonly());
    native::set_read_only(&original, false).unwrap();
}

#[test]
fn rejected_snapshot_never_starts_removal_or_fabricates_success() {
    let temporary = fixture();
    let selected = temporary.path().canonicalize().unwrap().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("payload"), b"must remain").unwrap();
    let expected = PersistedIdentity::capture(&selected).unwrap();
    let error = remove_checked(&selected, &expected, |_| {
        Err(ApiError::new(
            "DISPOSABLE_PROOF_REJECTED",
            "controlled proof mismatch",
        ))
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_REJECTED");
    assert!(expected.matches_path(&selected).unwrap());
    assert_eq!(fs::read(selected.join("payload")).unwrap(), b"must remain");
}

#[test]
fn callback_content_changes_and_new_names_are_rechecked_before_any_removal() {
    for addition in [false, true] {
        let temporary = fixture();
        let selected = temporary.path().canonicalize().unwrap().join("selected");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("a-payload"), b"original").unwrap();
        fs::write(selected.join("z-other"), b"also remains").unwrap();
        let expected = PersistedIdentity::capture(&selected).unwrap();
        assert!(remove_checked(&selected, &expected, |_| {
            if addition {
                fs::write(selected.join("new-unplanned"), b"new user material")?;
            } else {
                fs::write(selected.join("a-payload"), b"modified")?;
            }
            Ok(())
        })
        .is_err());
        assert!(expected.matches_path(&selected).unwrap());
        assert_eq!(fs::read(selected.join("z-other")).unwrap(), b"also remains");
        if addition {
            assert_eq!(fs::read(selected.join("a-payload")).unwrap(), b"original");
            assert_eq!(
                fs::read(selected.join("new-unplanned")).unwrap(),
                b"new user material"
            );
        } else {
            assert_eq!(fs::read(selected.join("a-payload")).unwrap(), b"modified");
        }
    }
}

#[test]
fn replacing_the_root_never_invokes_a_verifier_on_the_foreign_directory() {
    let temporary = fixture();
    let base = temporary.path().canonicalize().unwrap();
    let selected = base.join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("original"), b"original owned material").unwrap();
    let expected = PersistedIdentity::capture(&selected).unwrap();
    fs::rename(&selected, base.join("preserved-original")).unwrap();
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("foreign"), b"foreign material").unwrap();
    assert!(remove_checked(&selected, &expected, |_| panic!(
        "no proof callback on a replacement root"
    ))
    .is_err());
    assert_eq!(
        fs::read(selected.join("foreign")).unwrap(),
        b"foreign material"
    );
    assert_eq!(
        fs::read(base.join("preserved-original/original")).unwrap(),
        b"original owned material"
    );
}

#[test]
fn checked_removal_reports_real_occupancy_and_retry_after_release_is_actually_absent() {
    let temporary = fixture();
    let selected = temporary.path().canonicalize().unwrap().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("occupied"), b"retained while open").unwrap();
    let expected = PersistedIdentity::capture(&selected).unwrap();
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(selected.join("occupied"))
        .unwrap();
    let mut called = false;
    let error = remove_checked(&selected, &expected, |_| {
        called = true;
        Ok(())
    })
    .unwrap_err();
    assert!(called);
    assert_eq!(error.details["reason"], "file_in_use");
    assert_eq!(error.details["cleanupResult"]["completed"], false);
    assert_eq!(
        fs::read(selected.join("occupied")).unwrap(),
        b"retained while open"
    );
    drop(reader);
    assert_eq!(
        remove_checked(&selected, &expected, |_| Ok(())).unwrap()["completed"],
        true
    );
    assert!(!selected.exists());
}
