//! These tests inject boot IDs to exercise recovery, not to prove a physical
//! restart or hibernation transition. All files live in disposable TempDirs.
use super::super::schema::{BootEvidence, Marker};
use super::*;
use crate::file_identity::PersistedIdentity;
use std::{io::Write, os::windows::fs::OpenOptionsExt};
use uuid::Uuid;

fn fixture() -> (tempfile::TempDir, WriterOwner) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-writer-boot-recovery-中文 space-")
        .tempdir()
        .unwrap();
    let data = temporary.path().canonicalize().unwrap();
    (temporary, WriterOwner::platform(&data).unwrap())
}

fn writers(owner: &WriterOwner) -> PathBuf {
    let root = io::owner_path(owner);
    if !root.exists() {
        native::create_private_directory(&root).unwrap();
    }
    let writers = root.join("writers");
    if !writers.exists() {
        native::create_private_directory(&writers).unwrap();
    }
    writers
}

fn published(owner: &WriterOwner, boot_id: u32) -> PathBuf {
    let token = Uuid::new_v4().to_string();
    let path = writers(owner).join(format!("{token}.json"));
    let mut file = native::create_private_file(&path).unwrap();
    let marker = Marker {
        version: 2,
        platform: "windows".into(),
        kind: super::super::schema::kind(owner).into(),
        owner_id: owner._id.clone(),
        token,
        component: "disposable boot recovery fixture".into(),
        file_identity: PersistedIdentity::from_file(&file).unwrap(),
        boot_evidence: BootEvidence {
            source: boot_session::SOURCE.into(),
            boot_id,
        },
    };
    file.write_all(&serde_json::to_vec(&marker).unwrap())
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    path
}

fn preparation(owner: &WriterOwner, bytes: &[u8]) -> PathBuf {
    let path = writers(owner).join(format!(".preparing-{}", Uuid::new_v4()));
    let mut file = native::create_private_file(&path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    path
}

fn recover(owner: &WriterOwner, current_boot: u32) -> Result<()> {
    owner_with_boot(owner, &mut || Ok(current_boot), |_, _| Ok(()))
}

fn assert_unchanged(path: &Path, identity: &FileIdentity, bytes: &[u8]) {
    assert!(identity.same_object(&native::open_no_reparse(path).unwrap().identity));
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn same_boot_is_blocked_and_another_boot_retires_only_the_exact_published_marker() {
    for (old_boot, next_boot) in [(0, 1), (158, 159), (u32::MAX, 0)] {
        let (_temporary, owner) = fixture();
        let path = published(&owner, old_boot);
        let identity = native::open_no_reparse(&path).unwrap().identity;
        let bytes = fs::read(&path).unwrap();
        for _ in 0..2 {
            let error = recover(&owner, old_boot).unwrap_err();
            assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
            assert_eq!(error.details["operationOutcome"], "unknown");
            assert_unchanged(&path, &identity, &bytes);
        }
        recover(&owner, next_boot).unwrap();
        assert!(!path.exists());
        assert!(io::owner_path(&owner).is_dir());
        assert!(writers(&owner).is_dir());
        recover(&owner, next_boot).unwrap();
    }
}

#[test]
fn old_bootless_wrong_source_or_inconsistent_marker_proofs_are_not_migrated() {
    for change in [
        "old-version",
        "bootless",
        "wrong-source",
        "wrong-owner",
        "wrong-kind",
        "wrong-platform",
        "nil-token",
        "wrong-token",
        "empty-component",
        "control-component",
        "long-component",
        "high-id-byte",
        "wrong-volume",
        "missing-full-id",
        "wrong-name",
    ] {
        let (_temporary, owner) = fixture();
        let mut path = published(&owner, 10);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match change {
            "old-version" => {
                value["version"] = 1.into();
                value.as_object_mut().unwrap().remove("bootEvidence");
            }
            "bootless" => {
                value.as_object_mut().unwrap().remove("bootEvidence");
            }
            "wrong-source" => value["bootEvidence"]["source"] = "prefetch-registry-counter".into(),
            "wrong-owner" => value["ownerId"] = "other".into(),
            "wrong-kind" => value["kind"] = "node".into(),
            "wrong-platform" => value["platform"] = "macos".into(),
            "nil-token" => value["token"] = Uuid::nil().to_string().into(),
            "wrong-token" => value["token"] = Uuid::new_v4().to_string().into(),
            "empty-component" => value["component"] = "".into(),
            "control-component" => value["component"] = "control\n".into(),
            "long-component" => value["component"] = "x".repeat(257).into(),
            "high-id-byte" => {
                let previous = value["fileIdentity"]["windowsFileId"][15].as_u64().unwrap();
                value["fileIdentity"]["windowsFileId"][15] = (previous ^ 1).into();
            }
            "wrong-volume" => {
                let previous = value["fileIdentity"]["device"].as_u64().unwrap();
                value["fileIdentity"]["device"] = (previous ^ 1).into();
            }
            "missing-full-id" => {
                value["fileIdentity"]
                    .as_object_mut()
                    .unwrap()
                    .remove("windowsFileId");
            }
            "wrong-name" => {
                let renamed = path.with_file_name("unknown-name.json");
                fs::rename(&path, &renamed).unwrap();
                path = renamed;
            }
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let identity = native::open_no_reparse(&path).unwrap().identity;
        let bytes = fs::read(&path).unwrap();
        assert!(recover(&owner, 11).is_err(), "{change}");
        assert_unchanged(&path, &identity, &bytes);
    }
}

#[test]
fn live_read_write_preparation_is_preserved_then_unlocked_preparation_can_retire_same_boot() {
    let (_temporary, owner) = fixture();
    let path = writers(&owner).join(format!(".preparing-{}", Uuid::new_v4()));
    let mut file = native::create_private_file_for_publication(&path).unwrap();
    file.write_all(b"partial preparation without JSON").unwrap();
    file.sync_all().unwrap();
    let identity = native::identity(&file).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(recover(&owner, 0).is_err());
    assert_unchanged(&path, &identity, &bytes);
    drop(file);
    // No boot query is necessary for an unlocked preparation: the process
    // spawn boundary comes strictly after publication in the live protocol.
    owner_with_boot(
        &owner,
        &mut || Err(std::io::Error::other("boot observation unavailable")),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!path.exists());
}

#[test]
fn any_invalid_member_prevents_all_other_retirement_in_the_owner() {
    let (_temporary, owner) = fixture();
    let published = published(&owner, 10);
    let preparing = preparation(&owner, b"safe unopened preparation");
    let unknown = writers(&owner).join("unknown-material");
    let mut file = native::create_private_file(&unknown).unwrap();
    file.write_all(b"retain unknown material").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let material: Vec<_> = [&published, &preparing, &unknown]
        .into_iter()
        .map(|path| {
            (
                path.clone(),
                native::open_no_reparse(path).unwrap().identity,
                fs::read(path).unwrap(),
            )
        })
        .collect();
    assert!(recover(&owner, 11).is_err());
    for (path, identity, bytes) in material {
        assert_unchanged(&path, &identity, &bytes);
    }
}

#[test]
fn global_inventory_validates_every_owner_before_retiring_an_earlier_owner() {
    let (_temporary, platform) = fixture();
    let path = published(&platform, 10);
    let bytes = fs::read(&path).unwrap();
    let identity = native::open_no_reparse(&path).unwrap().identity;
    native::create_private_directory(&platform._data.join("app-ownership")).unwrap();
    let web = WriterOwner::web(&platform._data, "60b35473-42f2-5735-90cb-f6c89986d8fa").unwrap();
    let unknown = writers(&web).join("unknown");
    fs::write(&unknown, b"retain later owner's evidence").unwrap();
    assert!(all_with_boot(&platform._data, &mut || Ok(11), |_, _| Ok(())).is_err());
    assert_unchanged(&path, &identity, &bytes);
    assert_eq!(fs::read(unknown).unwrap(), b"retain later owner's evidence");
}

#[test]
fn missing_empty_and_preparing_only_scopes_do_not_add_a_boot_provider_admission() {
    let (_temporary, owner) = fixture();
    let mut unavailable = || Err(std::io::Error::other("boot API unavailable"));
    owner_with_boot(&owner, &mut unavailable, |_, _| Ok(())).unwrap();
    assert!(!io::owner_path(&owner).exists());
    writers(&owner);
    owner_with_boot(&owner, &mut unavailable, |_, _| Ok(())).unwrap();
    let preparing = preparation(&owner, b"bounded private preparation");
    owner_with_boot(&owner, &mut unavailable, |_, _| Ok(())).unwrap();
    assert!(!preparing.exists());
}

#[test]
fn failed_or_changing_boot_observation_never_grants_retirement() {
    for unavailable in [true, false] {
        let (_temporary, owner) = fixture();
        let path = published(&owner, 10);
        let identity = native::open_no_reparse(&path).unwrap().identity;
        let bytes = fs::read(&path).unwrap();
        let mut calls = 0;
        let result = owner_with_boot(
            &owner,
            &mut || {
                calls += 1;
                if unavailable {
                    Err(std::io::Error::other("boot API unavailable"))
                } else {
                    Ok(10 + calls)
                }
            },
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert_unchanged(&path, &identity, &bytes);
    }
}

#[test]
fn locked_inventory_rejects_writes_and_full_identity_replacement_is_preserved() {
    let (_temporary, owner) = fixture();
    let path = published(&owner, 10);
    let bytes = fs::read(&path).unwrap();
    let original = native::open_no_reparse(&path).unwrap().identity;
    let mut inventory = Inventory::default();
    inventory
        .owner(&owner, &mut || Ok(11), &mut |_, _| Ok(()))
        .unwrap();
    assert!(fs::write(&path, b"blocked concurrent writer").is_err());
    let retained = owner._data.join("retained-original-marker");
    fs::rename(&path, &retained).unwrap();
    let mut replacement = native::create_private_file(&path).unwrap();
    replacement.write_all(&bytes).unwrap();
    replacement.sync_all().unwrap();
    drop(replacement);
    let replacement_id = native::open_no_reparse(&path).unwrap().identity;
    assert!(!original.same_object(&replacement_id));
    assert!(inventory.markers[0].verify().is_err());
    assert!(inventory.recover(&mut || Ok(11)).is_err());
    assert_unchanged(&path, &replacement_id, &bytes);
    assert_unchanged(&retained, &original, &bytes);
}

#[test]
fn an_added_member_after_validation_preserves_the_original_set() {
    let (_temporary, owner) = fixture();
    let path = published(&owner, 10);
    let bytes = fs::read(&path).unwrap();
    let identity = native::open_no_reparse(&path).unwrap().identity;
    let new_path = writers(&owner).join("appeared-evidence");
    assert!(owner_with_boot(&owner, &mut || Ok(11), |point, _| {
        if point == Observation::Validated {
            fs::write(&new_path, b"retain newly appeared evidence").unwrap();
        }
        Ok(())
    })
    .is_err());
    assert_unchanged(&path, &identity, &bytes);
    assert_eq!(
        fs::read(new_path).unwrap(),
        b"retain newly appeared evidence"
    );
}

#[test]
fn hardlinks_nonprivate_files_and_oversized_preparations_never_grant_retirement() {
    for change in ["hardlink", "nonprivate", "oversized", "directory"] {
        let (_temporary, owner) = fixture();
        let valid = published(&owner, 10);
        let valid_id = native::open_no_reparse(&valid).unwrap().identity;
        let valid_bytes = fs::read(&valid).unwrap();
        let path = writers(&owner).join(format!(".preparing-{}", Uuid::new_v4()));
        match change {
            "hardlink" => {
                fs::hard_link(&valid, &path).unwrap();
            }
            "nonprivate" => {
                let outside = owner._data.join("ordinary-outside-file");
                fs::write(&outside, b"ordinary outside permissions").unwrap();
                fs::rename(outside, &path).unwrap();
                assert!(native::verify_private(&File::open(&path).unwrap()).is_err());
            }
            "oversized" => {
                let mut file = native::create_private_file(&path).unwrap();
                file.write_all(&vec![b'x'; io::MAX_MARKER as usize + 1])
                    .unwrap();
                file.sync_all().unwrap();
            }
            "directory" => {
                native::create_private_directory(&path).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(recover(&owner, 11).is_err(), "{change}");
        assert_unchanged(&valid, &valid_id, &valid_bytes);
        assert!(path.exists());
    }
}

#[test]
fn marker_byte_budget_and_directory_count_budget_apply_to_preparations() {
    let (_temporary, owner) = fixture();
    let path = preparation(&owner, &vec![b'x'; io::MAX_MARKER as usize]);
    recover(&owner, 0).unwrap();
    assert!(!path.exists());
    // Exercise the same bounded collector at a small limit; the production
    // limit remains the Mac contract's 4096 independent marker files.
    for _ in 0..3 {
        preparation(&owner, b"disposable");
    }
    assert_eq!(MAX_WRITERS, 4096);
    assert!(collect_names(&writers(&owner), Some(2)).is_err());
    assert_eq!(collect_names(&writers(&owner), Some(3)).unwrap().len(), 3);
    assert_eq!(fs::read_dir(writers(&owner)).unwrap().count(), 3);
}

#[test]
fn sharing_failure_and_delete_pending_are_retryable_without_false_clear() {
    let (_temporary, owner) = fixture();
    let path = published(&owner, 10);
    let identity = native::open_no_reparse(&path).unwrap().identity;
    let bytes = fs::read(&path).unwrap();
    let exclusive = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert!(recover(&owner, 11).is_err());
    drop(exclusive);
    assert_unchanged(&path, &identity, &bytes);
    let reader = File::open(&path).unwrap();
    let result = recover(&owner, 11);
    if let Err(error) = result {
        assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
        assert_eq!(error.details["operationOutcome"], "unknown");
    } else {
        io::missing(&path).unwrap();
    }
    drop(reader);
    recover(&owner, 11).unwrap();
    assert!(!path.exists());
    assert!(io::owner_path(&owner).is_dir());
}
