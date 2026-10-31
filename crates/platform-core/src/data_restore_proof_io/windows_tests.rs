//! Real disposable proofs only. The transaction/swap gate remains separate.
use super::*;
use std::{fs, time::Duration};

fn invalid() -> ApiError {
    ApiError::new("DISPOSABLE_PROOF_INVALID", "Invalid disposable proof")
}
fn changed() -> ApiError {
    ApiError::new("DISPOSABLE_PROOF_CHANGED", "Changed disposable proof")
}
fn fixture(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-windows-data-proof-中文 space-")
        .tempdir()
        .unwrap();
    let path = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("evidence 中文.json");
    fs::write(&path, bytes).unwrap();
    (temporary, path)
}

#[test]
fn real_streamed_proof_keeps_exact_sha_bytes_full_file_id_and_optional_content() {
    let bytes = (0..197_123)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let (_temporary, path) = fixture(&bytes);
    let before = FileIdentity::capture(&path).unwrap();
    let expected_hash = format!("{:x}", Sha256::digest(&bytes));
    for keep in [false, true] {
        let (proof, kept) = read_proof(&path, bytes.len() as u64, keep, invalid, changed).unwrap();
        assert_eq!(proof.sha256, expected_hash);
        assert_eq!(proof.bytes, bytes.len() as u64);
        assert!(before.matches_captured(&proof.identity));
        assert_eq!(proof.identity.windows_file_id, before.windows_file_id);
        assert_eq!(kept, if keep { bytes.clone() } else { Vec::new() });
        assert!(!proof.read_only);
        assert!(proof.verify(&path, bytes.len() as u64).is_ok());
    }
    assert!(before.matches_path(&path).unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn readonly_is_a_real_schema_field_and_existing_nonprivate_files_are_not_repaired() {
    let (_temporary, path) = fixture(b"retained ordinary file");
    let directory = path.parent().unwrap();
    let file = native::open_no_reparse(&path).unwrap();
    let parent = native::open_no_reparse(directory).unwrap();
    assert!(native::verify_private(&file.file).is_err());
    assert!(native::verify_private(&parent.file).is_err());
    let (writable, _) = read_proof(&path, 1024, true, invalid, changed).unwrap();
    native::set_read_only(&path, true).unwrap();
    let (readonly, bytes) = read_proof(&path, 1024, true, invalid, changed).unwrap();
    assert!(readonly.read_only);
    assert!(!writable.matches(&readonly));
    assert!(writable.verify(&path, 1024).is_err());
    assert_eq!(bytes, b"retained ordinary file");
    let value = serde_json::to_value(&readonly).unwrap();
    assert_eq!(value["readOnly"], true);
    assert!(value.get("mode").is_none());
    let restored: FileProof = serde_json::from_value(value).unwrap();
    assert!(readonly.matches(&restored));
    assert!(native::verify_private(&file.file).is_err());
    assert!(native::verify_private(&parent.file).is_err());
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    native::set_read_only(&path, false).unwrap();
}

#[test]
fn schema_retains_strict_identity_without_accepting_mode_or_ignoring_high_file_id_bits() {
    let (_temporary, path) = fixture(b"strict native identity");
    let (proof, _) = read_proof(&path, 1024, false, invalid, changed).unwrap();
    let original = serde_json::to_value(&proof).unwrap();
    let mut extra_mode = original.clone();
    extra_mode["mode"] = serde_json::json!(0o100600);
    assert!(serde_json::from_value::<FileProof>(extra_mode).is_err());
    let mut missing_readonly = original.clone();
    missing_readonly.as_object_mut().unwrap().remove("readOnly");
    assert!(serde_json::from_value::<FileProof>(missing_readonly).is_err());
    for identity in [
        serde_json::json!([1, 2]),
        serde_json::json!({"device":1,"inode":2,"unknownIdentityField":true}),
    ] {
        let mut damaged = original.clone();
        damaged["identity"] = identity;
        assert!(serde_json::from_value::<FileProof>(damaged).is_err());
    }
    let mut high = proof.clone();
    high.identity.windows_file_id.as_mut().unwrap()[15] ^= 1;
    assert!(!proof.matches(&high));
    let mut volume = proof.clone();
    volume.identity.device ^= 1;
    assert!(!proof.matches(&volume));
    let mut missing_id = original;
    missing_id["identity"]
        .as_object_mut()
        .unwrap()
        .remove("windowsFileId");
    // strict_identity's original optional-field parsing remains unchanged.
    // A missing Windows identity never matches a real captured proof.
    let missing_id: FileProof = serde_json::from_value(missing_id).unwrap();
    assert!(missing_id.identity.is_legacy());
    assert!(!proof.matches(&missing_id));
}

#[test]
fn empty_exact_and_growing_files_obey_the_original_budget_without_creating_inputs() {
    let (_temporary, path) = fixture(b"");
    let (empty, kept) = read_proof(&path, 0, true, invalid, changed).unwrap();
    assert_eq!(empty.bytes, 0);
    assert_eq!(empty.sha256, format!("{:x}", Sha256::digest(b"")));
    assert!(kept.is_empty());
    fs::write(&path, b"1234").unwrap();
    assert_eq!(
        read_proof(&path, 3, false, invalid, changed)
            .unwrap_err()
            .code,
        "DISPOSABLE_PROOF_INVALID"
    );
    assert_eq!(
        read_proof(&path, 4, true, invalid, changed).unwrap().1,
        b"1234"
    );
    let error = read_proof_with(&path, 4, true, invalid, changed, |stage, _| {
        if stage == Checkpoint::AfterOpen {
            fs::write(&path, b"12345")?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_INVALID");
    assert_eq!(fs::read(&path).unwrap(), b"12345");
    let missing = path.with_file_name("missing-proof");
    assert!(read_proof(&missing, 1024, false, invalid, changed).is_err());
    assert!(!missing.exists());
}

#[test]
fn actual_leaf_replacement_with_identical_bytes_is_preserved_and_never_recaptured() {
    let (_temporary, path) = fixture(b"identical but foreign proof bytes");
    let held = path.with_file_name("retained-original");
    let original = FileIdentity::capture(&path).unwrap();
    let error = read_proof_with(&path, 1024, true, invalid, changed, |stage, _| {
        if stage == Checkpoint::AfterRead {
            fs::rename(&path, &held)?;
            fs::write(&path, b"identical but foreign proof bytes")?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_CHANGED");
    assert!(original.matches_path(&held).unwrap());
    assert!(!original.matches_path(&path).unwrap());
    assert_eq!(
        fs::read(&held).unwrap(),
        b"identical but foreign proof bytes"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        b"identical but foreign proof bytes"
    );
}

#[test]
fn real_parent_and_ancestor_pins_last_through_the_complete_proof_read() {
    let (temporary, original_path) = fixture(b"retained parent namespace");
    let root = original_path.parent().unwrap();
    let directory = root.join("nested");
    fs::create_dir(&directory).unwrap();
    let path = directory.join("proof");
    fs::rename(&original_path, &path).unwrap();
    let root_held = temporary.path().with_extension("retained-ancestor");
    let directory_held = root.join("retained-parent");
    let (proof, bytes) = read_proof_with(&path, 1024, true, invalid, changed, |_, _| {
        assert!(fs::rename(&directory, &directory_held).is_err());
        assert!(fs::rename(root, &root_held).is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(bytes, b"retained parent namespace");
    assert!(proof.identity.matches_path(&path).unwrap());
    fs::rename(&directory, &directory_held).unwrap();
    assert!(proof
        .identity
        .matches_path(&directory_held.join("proof"))
        .unwrap());
}

#[test]
fn original_single_link_file_read_policy_rejects_added_links_before_or_during_capture() {
    let (_temporary, path) = fixture(b"hardlink proof bytes");
    let outside = path.with_file_name("retained-other-name");
    fs::hard_link(&path, &outside).unwrap();
    assert!(read_proof(&path, 1024, true, invalid, changed).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"hardlink proof bytes");
    fs::remove_file(&outside).unwrap();
    let error = read_proof_with(&path, 1024, true, invalid, changed, |stage, _| {
        if stage == Checkpoint::AfterRead {
            fs::hard_link(&path, &outside)?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_CHANGED");
    assert_eq!(fs::read(&outside).unwrap(), b"hardlink proof bytes");
    assert_eq!(
        native::identity(&File::open(&path).unwrap()).unwrap().links,
        2
    );
}

#[test]
fn real_readonly_and_change_time_mutations_cannot_hide_behind_restored_mtime() {
    let (_temporary, path) = fixture(b"before proof");
    let original_modified = fs::metadata(&path).unwrap().modified().unwrap();
    let error = read_proof_with(&path, 1024, false, invalid, changed, |stage, file| {
        if stage == Checkpoint::AfterRead {
            let before = Facts::from_file(file)?;
            std::thread::sleep(Duration::from_millis(10));
            fs::write(&path, b"after! proof")?;
            let writer = fs::OpenOptions::new().write(true).open(&path)?;
            writer.set_times(fs::FileTimes::new().set_modified(original_modified))?;
            drop(writer);
            let after = Facts::from_file(file)?;
            assert!(before.same_modified_file(&after));
            assert!(!before.same_modified_and_changed(&after));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_CHANGED");
    assert_eq!(fs::read(&path).unwrap(), b"after! proof");
    let error = read_proof_with(&path, 1024, true, invalid, changed, |stage, _| {
        if stage == Checkpoint::AfterRead {
            native::set_read_only(&path, true)?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_PROOF_CHANGED");
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    native::set_read_only(&path, false).unwrap();
}

#[test]
fn directories_and_parent_junctions_never_authorize_reading_or_repairing_external_material() {
    let (_temporary, path) = fixture(b"external original remains");
    let root = path.parent().unwrap();
    assert!(read_proof(root, 1024, true, invalid, changed).is_err());
    let external = root.join("external-original");
    fs::create_dir(&external).unwrap();
    fs::rename(&path, external.join("proof")).unwrap();
    let link = root.join("linked-parent");
    let command = platform_native::windows::system_paths::SystemPaths::current()
        .unwrap()
        .system
        .join("cmd.exe");
    assert!(std::process::Command::new(command)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&external)
        .output()
        .unwrap()
        .status
        .success());
    assert!(read_proof(&link.join("proof"), 1024, true, invalid, changed).is_err());
    assert_eq!(
        fs::read(external.join("proof")).unwrap(),
        b"external original remains"
    );
    fs::remove_dir(&link).unwrap();
}
