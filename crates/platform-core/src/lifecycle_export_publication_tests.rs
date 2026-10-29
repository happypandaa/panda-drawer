//! Public filename races use only a disposable temporary delivery directory.
use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    temporary: tempfile::TempDir,
    parent: PathBuf,
    stage: PathBuf,
    destination: PathBuf,
    expected: PersistedIdentity,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("user-deliveries");
        fs::create_dir(&parent).unwrap();
        let stage = parent.join(format!(".export-task_{}.partial", Uuid::new_v4().simple()));
        fs::write(&stage, b"verified platform archive").unwrap();
        let expected = PersistedIdentity::capture(&stage).unwrap();
        let destination = parent.join("user-delivery.zip");
        Self {
            temporary,
            parent,
            stage,
            destination,
            expected,
        }
    }
    fn held(&self) -> PathBuf {
        self.parent.parent().unwrap().join("held-original-archive")
    }
}

#[test]
fn normal_publication_delivers_only_the_bound_inode_without_extra_material() {
    let fixture = Fixture::new();
    publish_export_file(&fixture.stage, &fixture.destination, &fixture.expected).unwrap();
    assert!(!fixture.stage.exists());
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"verified platform archive"
    );
    assert!(fixture.expected.matches_path(&fixture.destination).unwrap());
    assert_eq!(fs::read_dir(&fixture.parent).unwrap().count(), 1);
}

#[test]
fn stage_changed_before_entry_is_rejected_without_moving_either_file() {
    let fixture = Fixture::new();
    fs::rename(&fixture.stage, fixture.held()).unwrap();
    fs::write(&fixture.stage, b"foreign staged file").unwrap();
    let error =
        publish_export_file(&fixture.stage, &fixture.destination, &fixture.expected).unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert!(!fixture.destination.exists());
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"foreign staged file");
    assert_eq!(
        fs::read(fixture.held()).unwrap(),
        b"verified platform archive"
    );
}

#[test]
fn stage_replaced_after_capture_is_exclusively_returned_and_keeps_its_original_owner_binding() {
    let fixture = Fixture::new();
    let base = fixture.temporary.path().canonicalize().unwrap();
    let data = base.join("platform-data");
    let workspace = base.join("workspace");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&workspace).unwrap();
    let ownership = crate::web_project_ownership::WebProjectOwnership::new(data, workspace);
    let id = format!("prj_{}", Uuid::new_v4().simple());
    let task = fixture
        .stage
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches(".export-")
        .trim_end_matches(".partial")
        .to_owned();
    ownership.initialize(&id).unwrap();
    ownership.record_task(&id, &task).unwrap();
    // Reserve before creating the public partial, as the real writer does.
    fs::rename(&fixture.stage, fixture.held()).unwrap();
    ownership
        .reserve_export_stage(&id, &task, &fixture.stage)
        .unwrap();
    fs::rename(fixture.held(), &fixture.stage).unwrap();
    ownership
        .capture_export_stage(&id, &task, &fixture.stage, &fixture.expected)
        .unwrap();
    let ownership_before = fs::read(ownership.path(&id).unwrap()).unwrap();
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::BeforeRename {
                fs::rename(&fixture.stage, fixture.held()).unwrap();
                fs::write(&fixture.stage, b"foreign captured file").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert!(!fixture.destination.exists());
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"foreign captured file");
    assert_eq!(
        fs::read(fixture.held()).unwrap(),
        b"verified platform archive"
    );
    assert_eq!(
        fs::read(ownership.path(&id).unwrap()).unwrap(),
        ownership_before
    );
    assert!(ownership.material_identity(&id, &fixture.stage).is_err());
    assert!(fixture.expected.matches_path(&fixture.held()).unwrap());
    assert!(ownership.material_paths(&id).is_err());
}

#[test]
fn final_name_appearing_at_publication_is_never_overwritten() {
    let fixture = Fixture::new();
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::BeforeRename {
                fs::write(&fixture.destination, b"new user final").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_DESTINATION_EXISTS");
    assert_eq!(
        fs::read(&fixture.stage).unwrap(),
        b"verified platform archive"
    );
    assert_eq!(fs::read(&fixture.destination).unwrap(), b"new user final");
}

#[test]
fn substituted_symlink_is_returned_without_following_its_target() {
    let fixture = Fixture::new();
    let target = fixture.temporary.path().join("foreign-target");
    fs::write(&target, b"foreign target remains").unwrap();
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::BeforeRename {
                fs::rename(&fixture.stage, fixture.held()).unwrap();
                symlink(&target, &fixture.stage).unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert!(fs::symlink_metadata(&fixture.stage)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!fixture.destination.exists());
    assert_eq!(fs::read(target).unwrap(), b"foreign target remains");
    assert_eq!(
        fs::read(fixture.held()).unwrap(),
        b"verified platform archive"
    );
}

#[test]
fn failed_exclusive_restoration_keeps_both_foreign_objects_without_claiming_the_final() {
    let fixture = Fixture::new();
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::BeforeRename {
                fs::rename(&fixture.stage, fixture.held()).unwrap();
                fs::write(&fixture.stage, b"captured foreign").unwrap();
            } else {
                fs::write(&fixture.stage, b"new stage occupant").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"new stage occupant");
    assert_eq!(fs::read(&fixture.destination).unwrap(), b"captured foreign");
    assert_eq!(
        fs::read(fixture.held()).unwrap(),
        b"verified platform archive"
    );
}

#[test]
fn changed_parent_before_mutation_does_not_publish_into_a_replacement_directory() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("held-original-directory");
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::BeforeRename {
                fs::rename(&fixture.parent, &held).unwrap();
                fs::create_dir(&fixture.parent).unwrap();
                fs::write(&fixture.stage, b"foreign directory stage").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert_eq!(
        fs::read(held.join(fixture.stage.file_name().unwrap())).unwrap(),
        b"verified platform archive"
    );
    assert_eq!(
        fs::read(&fixture.stage).unwrap(),
        b"foreign directory stage"
    );
    assert!(!fixture.destination.exists());
}

#[test]
fn changed_parent_after_rename_preserves_the_new_directory_and_returns_the_owned_partial_by_fd() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("held-original-directory");
    let error = publish_export_file_with(
        &fixture.stage,
        &fixture.destination,
        &fixture.expected,
        |point| {
            if point == ExportPublicationPoint::Renamed {
                fs::rename(&fixture.parent, &held).unwrap();
                fs::create_dir(&fixture.parent).unwrap();
                fs::write(&fixture.destination, b"foreign final in new directory").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "EXPORT_STAGE_CHANGED");
    assert_eq!(
        fs::read(held.join(fixture.stage.file_name().unwrap())).unwrap(),
        b"verified platform archive"
    );
    assert!(!held.join(fixture.destination.file_name().unwrap()).exists());
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"foreign final in new directory"
    );
}
