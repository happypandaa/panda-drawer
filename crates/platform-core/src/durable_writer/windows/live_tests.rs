use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, WriterOwner) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-live-writer-中文 space-")
        .tempdir()
        .unwrap();
    let data = temporary.path().canonicalize().unwrap();
    (temporary, WriterOwner::platform(&data).unwrap())
}

#[test]
fn a_real_private_marker_is_flushed_published_and_retired_only_by_explicit_finish() {
    let (_temporary, owner) = fixture();
    let mut guard = begin(&owner, "disposable live Job phase").unwrap();
    let path = guard.retained_path();
    let bytes = fs::read(&path).unwrap();
    let marker: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(marker["platform"], "windows");
    assert_eq!(marker["ownerId"], "platform");
    assert!(marker.get("bootSessionUuid").is_none());
    assert_eq!(
        marker["fileIdentity"]["windowsFileId"]
            .as_array()
            .unwrap()
            .len(),
        16
    );
    let actual = native::open_no_reparse(&path).unwrap();
    assert!(guard.identity.same_object(&actual.identity));
    assert_eq!(actual.identity.links, 1);
    native::verify_private(&actual.file).unwrap();
    drop(actual);
    assert_eq!(
        super::super::require_owner_clear(&owner).unwrap_err().code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    assert!(fs::rename(path.parent().unwrap(), owner._data.join("held-writers")).is_err());
    guard.finish().unwrap();
    guard.finish().unwrap();
    assert!(!path.exists());
    super::super::require_owner_clear(&owner).unwrap();
    super::super::require_all_clear(&owner._data).unwrap();
}

#[test]
fn dropping_a_guard_never_removes_or_confirms_its_marker() {
    let (_temporary, owner) = fixture();
    let guard = begin(&owner, "disposable interrupted owner").unwrap();
    let path = guard.retained_path();
    let bytes = fs::read(&path).unwrap();
    let identity = native::open_no_reparse(&path).unwrap().identity;
    drop(guard);
    for _ in 0..2 {
        let error = super::super::require_owner_clear(&owner).unwrap_err();
        assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
        assert_eq!(error.details["operationOutcome"], "unknown");
    }
    assert!(identity.same_object(&native::open_no_reparse(&path).unwrap().identity));
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn occupied_publication_preserves_both_our_preparation_and_the_foreign_destination() {
    let (_temporary, owner) = fixture();
    let mut paths = None;
    let error = begin_with(
        &owner,
        "disposable occupied publication",
        |preparing, path| {
            fs::write(path, b"foreign occupied name").unwrap();
            paths = Some((preparing.to_owned(), path.to_owned()));
            Ok(())
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
    let (preparing, path) = paths.unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"foreign occupied name");
    let marker: serde_json::Value = serde_json::from_slice(&fs::read(preparing).unwrap()).unwrap();
    assert_eq!(marker["platform"], "windows");
    assert!(super::super::require_owner_clear(&owner).is_err());
}

#[test]
fn source_name_replacement_cannot_redirect_bound_handle_publication() {
    let (_temporary, owner) = fixture();
    let mut foreign = None;
    let mut guard = begin_with(&owner, "disposable source-name race", |preparing, _| {
        let moved = preparing.with_extension("held-original");
        fs::rename(preparing, &moved).unwrap();
        fs::write(preparing, b"foreign replacement preparation").unwrap();
        foreign = Some((preparing.to_owned(), moved));
        Ok(())
    })
    .unwrap();
    let (foreign, original_alias) = foreign.unwrap();
    assert!(!original_alias.exists());
    assert!(guard
        .identity
        .same_object(&native::open_no_reparse(&guard.path).unwrap().identity));
    guard.finish().unwrap();
    assert_eq!(
        fs::read(foreign).unwrap(),
        b"foreign replacement preparation"
    );
    assert!(super::super::require_owner_clear(&owner).is_err());
}

#[test]
fn changed_bytes_full_identity_and_added_hardlinks_are_preserved() {
    for change in ["bytes", "high-id-byte", "hardlink", "replacement"] {
        let (_temporary, owner) = fixture();
        let mut guard = begin(&owner, "disposable retirement mismatch").unwrap();
        let path = guard.retained_path();
        let original = fs::read(&path).unwrap();
        let extra = path.with_extension("held-original");
        match change {
            "bytes" => fs::write(&path, b"changed private marker").unwrap(),
            "high-id-byte" => guard.identity.file_id[15] ^= 1,
            "hardlink" => fs::hard_link(&path, &extra).unwrap(),
            "replacement" => {
                fs::rename(&path, &extra).unwrap();
                let mut file = native::create_private_file(&path).unwrap();
                file.write_all(&original).unwrap();
                file.sync_all().unwrap();
            }
            _ => unreachable!(),
        }
        let before = fs::read(&path).unwrap();
        assert_eq!(
            guard.finish().unwrap_err().code,
            "PROJECT_WRITER_UNCONFIRMED",
            "{change}"
        );
        assert_eq!(fs::read(&path).unwrap(), before, "{change}");
        if matches!(change, "hardlink" | "replacement") {
            assert_eq!(fs::read(extra).unwrap(), original);
        }
    }
}

#[test]
fn delete_pending_external_reader_keeps_the_live_guard_pending_until_name_absence() {
    let (_temporary, owner) = fixture();
    let mut guard = begin(&owner, "disposable held reader").unwrap();
    let path = guard.retained_path();
    let reader = File::open(&path).unwrap();
    let first = guard.finish();
    if let Err(error) = first {
        assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
    }
    drop(reader);
    guard.finish().unwrap();
    assert!(!path.exists());
    super::super::require_owner_clear(&owner).unwrap();
}

#[test]
fn node_and_web_require_the_existing_private_nonempty_owner_without_recreating_it() {
    for (kind, id, index) in [
        (Kind::Web, "prj_disposable_writer", "web-project-ownership"),
        (
            Kind::Node,
            "50bfcf0c-5102-4a27-8446-b3c7312e2bc1",
            "node-project-ownership",
        ),
    ] {
        let (_temporary, platform) = fixture();
        let owner = WriterOwner::new(&platform._data, kind, id).unwrap();
        assert!(begin(&owner, "disposable missing owner").is_err());
        assert!(!io::owner_path(&owner).exists());
        native::create_private_directory(&platform._data.join(index)).unwrap();
        native::create_private_directory(&io::owner_path(&owner)).unwrap();
        let record = io::owner_path(&owner).join("ownership.json");
        let mut file = native::create_private_file(&record).unwrap();
        assert!(begin(&owner, "disposable empty owner").is_err());
        file.write_all(b"{}").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut guard = begin(&owner, "disposable registered owner").unwrap();
        guard.finish().unwrap();
        assert_eq!(fs::read(record).unwrap(), b"{}");
    }
}

#[test]
fn invalid_components_fail_before_any_marker_or_owner_creation() {
    let (_temporary, owner) = fixture();
    for value in ["".to_owned(), "bad\ncomponent".to_owned(), "x".repeat(257)] {
        assert!(begin(&owner, &value).is_err());
        assert!(!io::owner_path(&owner).exists());
    }
}
