use super::*;
use platform_native::windows::files as native;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-writer-inventory-中文 space-")
        .tempdir()
        .unwrap();
    let data = temporary.path().canonicalize().unwrap();
    (temporary, data)
}

#[test]
fn genuine_missing_and_empty_scopes_do_not_create_or_adopt_material() {
    let (_temporary, data) = fixture();
    fs::write(data.join("unrelated"), b"retain unrelated bytes").unwrap();
    require_all_clear(&data).unwrap();
    let owner = WriterOwner::platform(&data).unwrap();
    require_owner_clear(&owner).unwrap();
    assert!(!io::owner_path(&owner).exists());
    require_all_clear(&data.join("missing-data")).unwrap();
    assert!(!data.join("missing-data").exists());
    // The inventory contract checks only a present writers leaf's privacy.
    // Its ordinary parent is not an extra private-DACL admission condition.
    fs::create_dir(io::owner_path(&owner)).unwrap();
    native::create_private_directory(&io::owner_path(&owner).join("writers")).unwrap();
    require_owner_clear(&owner).unwrap();
    require_all_clear(&data).unwrap();
    assert_eq!(
        fs::read(data.join("unrelated")).unwrap(),
        b"retain unrelated bytes"
    );
}

#[test]
fn every_prepared_published_or_unknown_child_remains_unconfirmed_and_unchanged() {
    for name in [
        ".preparing-50bfcf0c-5102-4a27-8446-b3c7312e2bc1",
        "50bfcf0c-5102-4a27-8446-b3c7312e2bc1.json",
        "unknown",
    ] {
        let (_temporary, data) = fixture();
        let owner = WriterOwner::platform(&data).unwrap();
        let root = io::owner_path(&owner);
        native::create_private_directory(&root).unwrap();
        let writers = root.join("writers");
        native::create_private_directory(&writers).unwrap();
        let path = writers.join(name);
        fs::write(&path, b"retained unverified closure evidence").unwrap();
        let before = native::open_no_reparse(&path).unwrap().identity;
        for error in [
            require_owner_clear(&owner).unwrap_err(),
            require_all_clear(&data).unwrap_err(),
        ] {
            assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
            assert_eq!(error.details["operationOutcome"], "unknown");
        }
        assert!(before.same_object(&native::open_no_reparse(&path).unwrap().identity));
        assert_eq!(
            fs::read(path).unwrap(),
            b"retained unverified closure evidence"
        );
    }
}

#[test]
fn appearing_roots_or_markers_never_turn_into_a_clear_receipt() {
    for empty in [false, true] {
        let (_temporary, data) = fixture();
        let owner = WriterOwner::platform(&data).unwrap();
        if empty {
            native::create_private_directory(&io::owner_path(&owner)).unwrap();
            native::create_private_directory(&io::owner_path(&owner).join("writers")).unwrap();
        }
        let mut appeared = None;
        let error = owner_with(&owner, |point, path| {
            assert_eq!(
                point,
                if empty {
                    Observation::Empty
                } else {
                    Observation::Missing
                }
            );
            if !empty {
                fs::create_dir(path).unwrap();
            }
            let file = path.join("new-evidence");
            fs::write(&file, b"appeared retained marker").unwrap();
            appeared = Some(file);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
        assert_eq!(
            fs::read(appeared.unwrap()).unwrap(),
            b"appeared retained marker"
        );
    }
}

#[test]
fn an_earlier_empty_owner_cannot_gain_a_marker_during_the_global_scan() {
    let (_temporary, data) = fixture();
    let index = data.join("web-project-ownership");
    native::create_private_directory(&index).unwrap();
    for id in ["prj_disposable_a", "prj_disposable_b"] {
        native::create_private_directory(&index.join(id)).unwrap();
        native::create_private_directory(&index.join(id).join("writers")).unwrap();
    }
    let appeared = index.join("prj_disposable_a/writers/new.json");
    let error = all_with(&data, |point, path| {
        if point == Observation::Empty && path == index.join("prj_disposable_b/writers") {
            fs::write(&appeared, b"retained new writer").unwrap();
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
    assert_eq!(fs::read(appeared).unwrap(), b"retained new writer");
}

#[test]
fn junctions_and_nonprivate_existing_writer_leaves_are_never_clear() {
    let (_temporary, data) = fixture();
    let owner = WriterOwner::platform(&data).unwrap();
    let root = io::owner_path(&owner);
    native::create_private_directory(&root).unwrap();
    let writers = root.join("writers");
    fs::create_dir(&writers).unwrap();
    assert!(require_owner_clear(&owner).is_err());
    fs::remove_dir(&writers).unwrap();
    let target = data.join("retained-foreign");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"foreign writer evidence").unwrap();
    let status = std::process::Command::new(std::env::var_os("ComSpec").unwrap())
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&writers)
        .arg(&target)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(require_owner_clear(&owner).is_err());
    assert_eq!(
        fs::read(target.join("sentinel")).unwrap(),
        b"foreign writer evidence"
    );
    fs::remove_dir(writers).unwrap();
}

#[test]
fn mixed_separators_and_pending_deletion_sibling_keep_the_original_scope() {
    let (_temporary, data) = fixture();
    let nested = data.join("nested/data");
    fs::create_dir_all(&nested).unwrap();
    let mut raw = data.as_os_str().to_os_string();
    raw.push(r"\nested/data");
    let owner = WriterOwner::platform(&PathBuf::from(raw)).unwrap();
    let guard = super::super::live::begin(&owner, "disposable mixed path").unwrap();
    let marker = guard.retained_path();
    drop(guard);
    assert_eq!(
        require_owner_clear(&owner).unwrap_err().code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    assert!(marker.exists());
    let index = nested.join("node-project-ownership");
    native::create_private_directory(&index).unwrap();
    let pending = index.join(".pending-deletions");
    fs::create_dir(&pending).unwrap();
    fs::write(pending.join("retained"), b"separate deletion material").unwrap();
    // Remove only the marker produced by this test through its live guard in
    // the normal case elsewhere; this intentionally dropped one stays pending.
    assert!(require_all_clear(&nested).is_err());
    assert_eq!(
        fs::read(pending.join("retained")).unwrap(),
        b"separate deletion material"
    );
}
