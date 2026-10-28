use super::*;
use std::fs;
use std::os::unix::fs::symlink;

struct Fixture {
    temporary: tempfile::TempDir,
    owner: WebProjectOwnership,
    id: String,
    path: PathBuf,
    expected: PersistedIdentity,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let data = base.join("data");
        let workspace = base.join("workspace");
        let external = base.join("external-target-volume");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&external).unwrap();
        let owner = WebProjectOwnership::new(data, workspace);
        let id = format!("prj_{}", Uuid::new_v4().simple());
        let task = format!("task_{}", Uuid::new_v4().simple());
        owner.initialize(&id).unwrap();
        owner.record_task(&id, &task).unwrap();
        let path = external.join(format!(".export-{task}.partial"));
        owner.reserve_export_stage(&id, &task, &path).unwrap();
        fs::write(&path, "platform owned archive").unwrap();
        let expected = PersistedIdentity::capture(&path).unwrap();
        owner
            .capture_export_stage(&id, &task, &path, &expected)
            .unwrap();
        Self {
            temporary,
            owner,
            id,
            path,
            expected,
        }
    }
    fn reopened(&self) -> WebProjectOwnership {
        let base = self.temporary.path().canonicalize().unwrap();
        WebProjectOwnership::new(base.join("data"), base.join("workspace"))
    }
    fn remove(&self) -> Result<()> {
        remove_owned_file(&self.owner, &self.id, &self.path, &self.expected)
    }
    fn assert_clean(&self) {
        assert!(!self.path.exists());
        assert!(self.owner.file_deletion_plans(&self.id).unwrap().is_empty());
        assert!(fs::read_dir(self.path.parent().unwrap())
            .unwrap()
            .next()
            .is_none());
    }
}
fn interruption() -> ApiError {
    ApiError::new("TEST_INTERRUPTED", "temporary fixture crash")
}

#[test]
fn normal_file_removal_uses_private_same_volume_stage_and_leaves_no_plan_or_stage() {
    let fixture = Fixture::new();
    let mut observed = false;
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::StageBound {
                observed = true;
                let metadata = fs::metadata(&plan.staging_path).unwrap();
                assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
                assert_eq!(metadata.dev(), fs::metadata(&fixture.path).unwrap().dev());
                assert_eq!(plan.staging_path.parent(), fixture.path.parent());
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(observed);
    fixture.assert_clean();
}

#[test]
fn every_bound_file_deletion_boundary_resumes_after_reopening_without_retained_history() {
    for stop in [
        Point::PlanPersisted,
        Point::StageBound,
        Point::RenameIntentPersisted,
        Point::Renamed,
        Point::StagePersisted,
        Point::RemoveIntentPersisted,
        Point::Unlinked,
        Point::RemovedPersisted,
        Point::StageRemoved,
        Point::PlanForgotten,
    ] {
        let fixture = Fixture::new();
        let error = remove_with(
            &fixture.owner,
            &fixture.id,
            &fixture.path,
            &fixture.expected,
            |point, _| {
                if point == stop {
                    Err(interruption())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "TEST_INTERRUPTED", "{stop:?}");
        remove_owned_file(
            &fixture.reopened(),
            &fixture.id,
            &fixture.path,
            &fixture.expected,
        )
        .unwrap_or_else(|error| panic!("{stop:?}: {error:?}"));
        fixture.assert_clean();
    }
}

#[test]
fn crash_after_stage_mkdir_before_binding_reports_exact_unbound_path_and_preserves_everything() {
    let fixture = Fixture::new();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::StageCreated {
                Err(interruption())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert!(plan.staging_identity.is_none());
    assert!(plan.staging_path.is_dir());
    let error = remove_owned_file(
        &fixture.reopened(),
        &fixture.id,
        &fixture.path,
        &fixture.expected,
    )
    .unwrap_err();
    assert_eq!(error.code, "WEB_PROJECT_MATERIAL_CHANGED");
    assert!(error
        .message
        .contains(&plan.staging_path.to_string_lossy().to_string()));
    assert_eq!(
        fs::read_to_string(&fixture.path).unwrap(),
        "platform owned archive"
    );
    assert!(plan.staging_path.is_dir());
}

#[test]
fn replacement_immediately_before_public_rename_is_restored_and_never_unlinked() {
    let fixture = Fixture::new();
    let held = fixture.path.with_file_name("temporary-original-kept");
    let error = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                fs::rename(&fixture.path, &held).unwrap();
                fs::write(&fixture.path, "foreign replacement").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "WEB_PROJECT_MATERIAL_CHANGED");
    assert_eq!(
        fs::read_to_string(&fixture.path).unwrap(),
        "foreign replacement"
    );
    assert_eq!(fs::read_to_string(&held).unwrap(), "platform owned archive");
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert!(!plan.staged_payload().exists());
}

#[test]
fn symlink_replacement_is_restored_without_following_or_deleting_its_target() {
    let fixture = Fixture::new();
    let held = fixture.path.with_file_name("temporary-original-kept");
    let victim = fixture.path.with_file_name("temporary-foreign-victim");
    fs::write(&victim, "untouched target").unwrap();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                fs::rename(&fixture.path, &held).unwrap();
                symlink(&victim, &fixture.path).unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(fs::symlink_metadata(&fixture.path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched target");
    assert_eq!(fs::read_to_string(&held).unwrap(), "platform owned archive");
}

#[test]
fn a_new_public_file_after_rename_is_preserved_and_blocks_unlink_until_moved_away() {
    let fixture = Fixture::new();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::Renamed {
                fs::write(&fixture.path, "new public file").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        fs::read_to_string(plan.staged_payload()).unwrap(),
        "platform owned archive"
    );
    assert_eq!(
        fs::read_to_string(&fixture.path).unwrap(),
        "new public file"
    );
    let held = fixture.temporary.path().join("temporary-new-public-kept");
    fs::rename(&fixture.path, &held).unwrap();
    remove_owned_file(
        &fixture.reopened(),
        &fixture.id,
        &fixture.path,
        &fixture.expected,
    )
    .unwrap();
    fixture.assert_clean();
    assert_eq!(fs::read_to_string(held).unwrap(), "new public file");
}

#[test]
fn payload_replacement_in_private_stage_is_preserved_and_does_not_inherit_ownership() {
    let fixture = Fixture::new();
    let held = fixture
        .temporary
        .path()
        .join("temporary-staged-original-kept");
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::BeforeUnlink {
                fs::rename(plan.staged_payload(), &held).unwrap();
                fs::write(plan.staged_payload(), "foreign staged replacement").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(plan.state, FileDeletionState::Removing);
    assert_eq!(
        fs::read_to_string(plan.staged_payload()).unwrap(),
        "foreign staged replacement"
    );
    assert_eq!(fs::read_to_string(&held).unwrap(), "platform owned archive");
    assert!(fixture.remove().is_err());
    assert_eq!(
        fs::read_to_string(plan.staged_payload()).unwrap(),
        "foreign staged replacement"
    );
}

#[test]
fn replacement_staging_directory_is_never_removed_or_rebound() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("temporary-real-stage-kept");
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::BeforeUnlink {
                fs::rename(&plan.staging_path, &held).unwrap();
                fs::create_dir(&plan.staging_path).unwrap();
                fs::set_permissions(&plan.staging_path, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(plan.staging_path.join("foreign"), "do not remove").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        fs::read_to_string(plan.staging_path.join("foreign")).unwrap(),
        "do not remove"
    );
    assert_eq!(
        fs::read_to_string(held.join(PAYLOAD)).unwrap(),
        "platform owned archive"
    );
    assert!(fixture.remove().is_err());
}

#[test]
fn a_missing_external_file_on_a_different_expected_volume_is_not_success() {
    let fixture = Fixture::new();
    let held = fixture
        .temporary
        .path()
        .join("temporary-offline-material-kept");
    fs::rename(&fixture.path, &held).unwrap();
    let mut unavailable = fixture.expected.clone();
    #[cfg(target_os = "macos")]
    {
        unavailable.volume_uuid = Some(Uuid::new_v4().to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        unavailable.device = unavailable.device.wrapping_add(1);
    }
    let error =
        remove_owned_file(&fixture.owner, &fixture.id, &fixture.path, &unavailable).unwrap_err();
    assert_eq!(error.code, "WEB_PROJECT_MATERIAL_UNAVAILABLE");
    assert_eq!(fs::read_to_string(held).unwrap(), "platform owned archive");
    assert!(fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .is_empty());
}

#[test]
fn uninstall_scope_includes_only_identity_bound_payload_and_rejects_unknown_stage_entries() {
    let fixture = Fixture::new();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::StagePersisted {
                Err(interruption())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        owned_scopes(&plan).unwrap(),
        vec![plan.staging_path.clone()]
    );
    fs::write(plan.staging_path.join("foreign"), "unowned content").unwrap();
    assert!(owned_scopes(&plan).is_err());
    assert_eq!(
        fs::read_to_string(plan.staging_path.join("foreign")).unwrap(),
        "unowned content"
    );
}

#[test]
fn uninstall_scope_never_claims_a_foreign_file_captured_by_public_name_race() {
    let fixture = Fixture::new();
    let held = fixture
        .temporary
        .path()
        .join("temporary-original-for-scope-kept");
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                fs::rename(&fixture.path, &held).unwrap();
                fs::write(&fixture.path, "foreign race payload").unwrap();
            }
            if point == Point::Renamed {
                fs::write(&fixture.path, "foreign race public name").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        fs::read_to_string(plan.staged_payload()).unwrap(),
        "foreign race payload"
    );
    assert!(owned_scopes(&plan).is_err());
    assert_eq!(
        fs::read_to_string(&fixture.path).unwrap(),
        "foreign race public name"
    );
    assert_eq!(fs::read_to_string(held).unwrap(), "platform owned archive");
}
