use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::Builder::new()
        .prefix("panda-disposable-export-publication-中文 space-")
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}
#[test]
fn exact_hardlinked_stage_is_delivered_without_touching_its_other_name() {
    let (_temp, root) = fixture();
    let stage = root.join("stage.zip");
    let final_path = root.join("delivered.zip");
    fs::write(&stage, b"disposable archive").unwrap();
    let original = root.join("external-name");
    fs::hard_link(&stage, &original).unwrap();
    let id = PersistedIdentity::capture(&stage).unwrap();
    publish_export_file_with(&stage, &final_path, &id, |_| Ok(())).unwrap();
    assert!(!stage.exists());
    assert!(id.matches_path(&final_path).unwrap());
    assert!(id.matches_path(&original).unwrap());
    assert_eq!(fs::read(original).unwrap(), b"disposable archive");
}
#[test]
fn destination_collision_and_wrong_high_identity_preserve_both_objects() {
    let (_temp, root) = fixture();
    let stage = root.join("stage.zip");
    let dest = root.join("delivered.zip");
    fs::write(&stage, b"stage").unwrap();
    fs::write(&dest, b"retained").unwrap();
    let id = PersistedIdentity::capture(&stage).unwrap();
    let error = publish_export_file_with(&stage, &dest, &id, |_| Ok(())).unwrap_err();
    assert_eq!(error.code, "EXPORT_DESTINATION_EXISTS");
    let mut wrong = id.clone();
    wrong.windows_file_id.as_mut().unwrap()[15] ^= 1;
    assert!(
        publish_export_file_with(&stage, &root.join("not-created.zip"), &wrong, |_| panic!(
            "wrong identity before mutation"
        ))
        .is_err()
    );
    assert_eq!(fs::read(stage).unwrap(), b"stage");
    assert_eq!(fs::read(dest).unwrap(), b"retained");
}
#[test]
fn source_name_replacement_cannot_publish_the_foreign_file() {
    let (_temp, root) = fixture();
    let stage = root.join("stage.zip");
    let dest = root.join("delivered.zip");
    let retained = root.join("retained-stage");
    fs::write(&stage, b"ours").unwrap();
    let id = PersistedIdentity::capture(&stage).unwrap();
    let error = publish_export_file_with(&stage, &dest, &id, |point| {
        if point == ExportPublicationPoint::BeforeRename {
            fs::rename(&stage, &retained)?;
            fs::write(&stage, b"foreign")?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert!(!dest.exists());
    assert!(id.matches_path(&retained).unwrap());
    assert_eq!(fs::read(stage).unwrap(), b"foreign");
}
#[test]
fn postmove_foreign_destination_is_not_restored_or_deleted_as_our_stage() {
    let (_temp, root) = fixture();
    let stage = root.join("stage.zip");
    let dest = root.join("delivered.zip");
    let retained = root.join("retained-delivery");
    fs::write(&stage, b"ours").unwrap();
    let id = PersistedIdentity::capture(&stage).unwrap();
    let error = publish_export_file_with(&stage, &dest, &id, |point| {
        if point == ExportPublicationPoint::Renamed {
            fs::rename(&dest, &retained)?;
            fs::write(&dest, b"foreign")?;
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert!(!stage.exists());
    assert!(id.matches_path(&retained).unwrap());
    assert_eq!(fs::read(dest).unwrap(), b"foreign");
}
#[test]
fn actual_parent_pin_prevents_redirecting_the_export_namespace() {
    let (_temp, root) = fixture();
    let parent = root.join("delivery");
    fs::create_dir(&parent).unwrap();
    let stage = parent.join("stage.zip");
    let dest = parent.join("delivered.zip");
    fs::write(&stage, b"ours").unwrap();
    let id = PersistedIdentity::capture(&stage).unwrap();
    publish_export_file_with(&stage, &dest, &id, |point| {
        if point == ExportPublicationPoint::BeforeRename {
            assert!(fs::rename(&parent, root.join("moved-parent")).is_err());
        }
        Ok(())
    })
    .unwrap();
    assert!(id.matches_path(&dest).unwrap());
}
