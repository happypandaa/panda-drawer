use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    time::{Duration, Instant},
};

struct Fixture {
    temporary: tempfile::TempDir,
    owner: WebProjectOwnership,
    id: String,
    path: PathBuf,
    expected: PersistedIdentity,
}
impl Fixture {
    fn new() -> Self {
        Self::with_depth(false)
    }
    fn with_depth(long: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-w5-web-file-中文 space-")
            .tempdir()
            .unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let data = base.join("data");
        let workspace = base.join("workspace");
        let mut external = base.join("external-target-volume");
        if long {
            for index in 0..12 {
                external = external.join(format!("中文 space &% literal-{index:02}"));
            }
        }
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::create_dir_all(&external).unwrap();
        let owner = WebProjectOwnership::new(data, workspace);
        let id = Uuid::new_v4().to_string();
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
    fn interrupt(&self, stop: Point) {
        let issue = remove_with(
            &self.owner,
            &self.id,
            &self.path,
            &self.expected,
            |point, _| {
                if point == stop {
                    Err(interruption())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(issue.code, "TEST_INTERRUPTED", "{stop:?}: {issue:?}");
    }
    fn plan(&self) -> FileDeletionPlan {
        self.owner
            .file_deletion_plans(&self.id)
            .unwrap()
            .pop()
            .unwrap()
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
    ApiError::new("TEST_INTERRUPTED", "disposable fixture interruption")
}
fn junction(target: &Path, name: &Path) {
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(name)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normal_file_removal_binds_private_same_volume_stage_without_parent_acl_changes() {
    let fixture = Fixture::new();
    let parent = native::open_no_reparse(fixture.path.parent().unwrap()).unwrap();
    let parent_was_private = native::verify_private(&parent.file).is_ok();
    let mut observed = false;
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::StageBound {
                observed = true;
                let stage = native::open_no_reparse(&plan.staging_path).unwrap();
                native::verify_private(&stage.file).unwrap();
                assert_eq!(stage.identity.volume_serial, parent.identity.volume_serial);
                assert_eq!(plan.staging_path.parent(), fixture.path.parent());
                assert!(plan
                    .staging_identity
                    .as_ref()
                    .unwrap()
                    .matches_open_runtime(&stage.file)
                    .unwrap());
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(observed);
    assert_eq!(
        native::verify_private(&parent.file).is_ok(),
        parent_was_private
    );
    assert!(parent.identity.same_object(
        &native::open_no_reparse(fixture.path.parent().unwrap())
            .unwrap()
            .identity
    ));
    fixture.assert_clean();
}

#[test]
fn every_bound_persistent_boundary_reopens_and_finishes_without_retained_history() {
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
        fixture.interrupt(stop);
        remove_owned_file(
            &fixture.reopened(),
            &fixture.id,
            &fixture.path,
            &fixture.expected,
        )
        .unwrap_or_else(|issue| panic!("{stop:?}: {issue:?}"));
        fixture.assert_clean();
    }
}

#[test]
fn unbound_created_stage_is_preserved_and_never_adopted_after_reopen() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::StageCreated);
    let plan = fixture.plan();
    assert!(plan.staging_identity.is_none());
    assert!(plan.staging_path.is_dir());
    let issue = remove_owned_file(
        &fixture.reopened(),
        &fixture.id,
        &fixture.path,
        &fixture.expected,
    )
    .unwrap_err();
    assert_eq!(issue.code, "WEB_PROJECT_MATERIAL_CHANGED");
    assert!(issue
        .message
        .contains(&*plan.staging_path.to_string_lossy()));
    assert_eq!(fs::read(&fixture.path).unwrap(), b"platform owned archive");
    assert!(plan.staging_path.is_dir());
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn replacement_before_handle_rename_is_never_moved_or_unlinked() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("original-kept");
    let issue = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                fs::rename(&fixture.path, &held).unwrap();
                fs::write(&fixture.path, b"foreign public replacement").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(issue.code, "WEB_PROJECT_MATERIAL_CHANGED");
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"foreign public replacement"
    );
    assert_eq!(fs::read(&held).unwrap(), b"platform owned archive");
    assert!(!fixture.plan().staged_payload().exists());
    assert!(owned_scopes(&fixture.plan()).is_err());
}

#[test]
fn junction_public_replacement_preserves_target_and_original() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("original-kept");
    let victim = fixture.temporary.path().join("external-victim");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("sentinel"), b"untouched target").unwrap();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                fs::rename(&fixture.path, &held).unwrap();
                junction(&victim, &fixture.path);
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(platform_native::filesystem::is_reparse(
        &fs::symlink_metadata(&fixture.path).unwrap()
    ));
    assert_eq!(
        fs::read(victim.join("sentinel")).unwrap(),
        b"untouched target"
    );
    assert_eq!(fs::read(&held).unwrap(), b"platform owned archive");
    fs::remove_dir(&fixture.path).unwrap();
}

#[test]
fn new_public_file_after_rename_blocks_unlink_until_moved_away() {
    let fixture = Fixture::new();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::Renamed {
                fs::write(&fixture.path, b"new public file").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture.plan();
    assert_eq!(
        fs::read(plan.staged_payload()).unwrap(),
        b"platform owned archive"
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), b"new public file");
    let held = fixture.temporary.path().join("new-public-kept");
    fs::rename(&fixture.path, &held).unwrap();
    remove_owned_file(
        &fixture.reopened(),
        &fixture.id,
        &fixture.path,
        &fixture.expected,
    )
    .unwrap();
    fixture.assert_clean();
    assert_eq!(fs::read(held).unwrap(), b"new public file");
}

#[test]
fn foreign_staged_payload_is_preserved_without_inheriting_ownership() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("staged-original-kept");
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::BeforeUnlink {
                fs::rename(plan.staged_payload(), &held).unwrap();
                fs::write(plan.staged_payload(), b"foreign staged replacement").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture.plan();
    assert_eq!(plan.state, FileDeletionState::Removing);
    assert_eq!(
        fs::read(plan.staged_payload()).unwrap(),
        b"foreign staged replacement"
    );
    assert_eq!(fs::read(held).unwrap(), b"platform owned archive");
    assert!(fixture.remove().is_err());
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn replaced_persistent_stage_is_not_rebound_or_recursively_deleted() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::StagePersisted);
    let plan = fixture.plan();
    let held = fixture.temporary.path().join("real-stage-kept");
    fs::rename(&plan.staging_path, &held).unwrap();
    native::create_private_directory(&plan.staging_path).unwrap();
    fs::write(plan.staging_path.join("foreign"), b"do not remove").unwrap();
    assert!(fixture.remove().is_err());
    assert_eq!(
        fs::read(plan.staging_path.join("foreign")).unwrap(),
        b"do not remove"
    );
    assert_eq!(
        fs::read(held.join(PAYLOAD)).unwrap(),
        b"platform owned archive"
    );
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn replaced_persistent_parent_is_not_used_as_deletion_scope() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::PlanPersisted);
    let parent = fixture.path.parent().unwrap();
    let held = fixture.temporary.path().join("real-parent-kept");
    fs::rename(parent, &held).unwrap();
    fs::create_dir(parent).unwrap();
    fs::write(&fixture.path, b"foreign parent payload").unwrap();
    assert!(fixture.remove().is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), b"foreign parent payload");
    assert_eq!(
        fs::read(held.join(fixture.path.file_name().unwrap())).unwrap(),
        b"platform owned archive"
    );
}

#[test]
fn missing_file_on_wrong_expected_volume_is_not_success() {
    let fixture = Fixture::new();
    let held = fixture.temporary.path().join("offline-original-kept");
    fs::rename(&fixture.path, &held).unwrap();
    let mut unavailable = fixture.expected.clone();
    unavailable.device ^= 1;
    let issue =
        remove_owned_file(&fixture.owner, &fixture.id, &fixture.path, &unavailable).unwrap_err();
    assert_eq!(issue.code, "WEB_PROJECT_MATERIAL_UNAVAILABLE");
    assert_eq!(fs::read(held).unwrap(), b"platform owned archive");
    assert!(fixture
        .owner
        .file_deletion_plans(&fixture.id)
        .unwrap()
        .is_empty());
}

#[test]
fn read_only_hardlink_removes_only_owned_name_and_preserves_external_attributes() {
    let fixture = Fixture::new();
    let external = fixture.temporary.path().join("external-original-preserved");
    fs::hard_link(&fixture.path, &external).unwrap();
    native::set_read_only(&external, true).unwrap();
    let original = native::open_no_reparse(&external).unwrap().identity;
    // Only the selected link is removed. No shared readonly attribute is
    // cleared; no external name's ownership is taken by this plan.
    fixture.remove().unwrap();
    fixture.assert_clean();
    assert_eq!(fs::read(&external).unwrap(), b"platform owned archive");
    assert!(fs::metadata(&external).unwrap().permissions().readonly());
    assert!(original.same_object(&native::open_no_reparse(&external).unwrap().identity));
    native::set_read_only(&external, false).unwrap();
}

#[test]
fn long_chinese_space_literal_path_and_readonly_file_are_removed() {
    let fixture = Fixture::with_depth(true);
    assert!(fixture.path.as_os_str().len() > 320);
    native::set_read_only(&fixture.path, true).unwrap();
    fixture.remove().unwrap();
    fixture.assert_clean();
}

#[test]
fn sharing_failure_is_bounded_retains_removing_plan_and_retry_succeeds_after_release() {
    let fixture = Fixture::new();
    let mut occupied = None;
    let started = Instant::now();
    let issue = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::BeforeUnlink {
                occupied = Some(
                    OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(plan.staged_payload())
                        .unwrap(),
                );
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(issue.details["reason"], "file_in_use");
    let plan = fixture.plan();
    assert_eq!(plan.state, FileDeletionState::Removing);
    assert!(fixture
        .expected
        .matches_open_runtime(occupied.as_ref().unwrap())
        .unwrap());
    assert_eq!(
        fs::read(plan.staged_payload()).unwrap(),
        b"platform owned archive"
    );
    drop(occupied);
    fixture.remove().unwrap();
    fixture.assert_clean();
}

#[test]
fn rename_sharing_failure_retains_intent_and_original_until_explicit_retry() {
    let fixture = Fixture::new();
    let mut occupied = None;
    let started = Instant::now();
    let issue = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, _| {
            if point == Point::BeforeRename {
                occupied = Some(
                    OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(&fixture.path)
                        .unwrap(),
                );
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(issue.details["reason"], "file_in_use");
    let plan = fixture.plan();
    assert_eq!(plan.state, FileDeletionState::RenameIntent);
    assert_eq!(fs::read(&fixture.path).unwrap(), b"platform owned archive");
    assert!(!plan.staged_payload().exists());
    drop(occupied);
    fixture.remove().unwrap();
    fixture.assert_clean();
}

#[test]
fn actual_delete_pending_file_keeps_plan_until_last_external_handle_closes() {
    let fixture = Fixture::new();
    let mut retained: Option<File> = None;
    let issue = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::BeforeUnlink {
                let mut opened = OpenOptions::new()
                    .read(true)
                    .share_mode(7)
                    .open(plan.staged_payload())
                    .unwrap();
                let mut bytes = Vec::new();
                opened.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"platform owned archive");
                retained = Some(opened);
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(issue.details["operationOutcome"], "unknown", "{issue:?}");
    assert_eq!(issue.details["reason"], "file_in_use", "{issue:?}");
    assert_eq!(fixture.plan().state, FileDeletionState::Removing);
    assert!(fixture
        .expected
        .matches_open_runtime(retained.as_ref().unwrap())
        .unwrap());
    drop(retained);
    remove_owned_file(
        &fixture.reopened(),
        &fixture.id,
        &fixture.path,
        &fixture.expected,
    )
    .unwrap();
    fixture.assert_clean();
}

#[test]
fn actual_delete_pending_stage_does_not_forget_plan_before_absence_receipt() {
    let fixture = Fixture::new();
    let mut retained = None;
    let issue = remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::RemovedPersisted {
                retained = Some(native::open_no_reparse(&plan.staging_path).unwrap());
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(issue.details["operationOutcome"], "unknown", "{issue:?}");
    assert_eq!(issue.details["reason"], "file_in_use", "{issue:?}");
    let plan = fixture.plan();
    assert_eq!(plan.state, FileDeletionState::Removed);
    assert!(plan
        .staging_identity
        .as_ref()
        .unwrap()
        .matches_open_runtime(&retained.as_ref().unwrap().file)
        .unwrap());
    drop(retained);
    fixture.remove().unwrap();
    fixture.assert_clean();
}

#[test]
fn uninstall_scope_checks_exact_payload_and_unknown_stage_content_is_never_removed() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::StagePersisted);
    let plan = fixture.plan();
    assert_eq!(
        owned_scopes(&plan).unwrap(),
        vec![plan.staging_path.clone()]
    );
    fs::write(plan.staging_path.join("foreign"), b"unowned content").unwrap();
    assert!(owned_scopes(&plan).is_err());
    assert!(fixture.remove().is_err());
    assert_eq!(
        fs::read(plan.staging_path.join("foreign")).unwrap(),
        b"unowned content"
    );
    assert!(fixture
        .plan()
        .staging_identity
        .unwrap()
        .matches_path(&plan.staging_path)
        .unwrap());
}

#[test]
fn foreign_payload_on_rename_recovery_is_exclusively_restored_without_adoption() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::RenameIntentPersisted);
    let plan = fixture.plan();
    let held = fixture.temporary.path().join("real-original-kept");
    fs::rename(&fixture.path, &held).unwrap();
    fs::write(plan.staged_payload(), b"foreign capture").unwrap();
    assert!(fixture.remove().is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), b"foreign capture");
    assert_eq!(fs::read(held).unwrap(), b"platform owned archive");
    assert!(!plan.staged_payload().exists());
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn occupied_public_name_prevents_foreign_restoration_and_preserves_both_names() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::RenameIntentPersisted);
    let plan = fixture.plan();
    fs::write(plan.staged_payload(), b"foreign payload").unwrap();
    assert!(fixture.remove().is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), b"platform owned archive");
    assert_eq!(fs::read(plan.staged_payload()).unwrap(), b"foreign payload");
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn payload_appearing_after_unlink_is_preserved_with_removed_plan() {
    let fixture = Fixture::new();
    remove_with(
        &fixture.owner,
        &fixture.id,
        &fixture.path,
        &fixture.expected,
        |point, plan| {
            if point == Point::Unlinked {
                fs::write(plan.staged_payload(), b"late foreign file").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    let plan = fixture.plan();
    assert_eq!(plan.state, FileDeletionState::Removed);
    assert_eq!(
        fs::read(plan.staged_payload()).unwrap(),
        b"late foreign file"
    );
    assert!(fixture.remove().is_err());
    assert!(owned_scopes(&plan).is_err());
}

#[test]
fn all_plan_identities_require_full_windows_id_without_macos_volume_uuid() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::StageBound);
    let plan = fixture.plan();
    for index in 0..3 {
        for mac_uuid in [false, true] {
            let mut damaged = plan.clone();
            let identity = match index {
                0 => &mut damaged.expected,
                1 => &mut damaged.parent_identity,
                _ => damaged.staging_identity.as_mut().unwrap(),
            };
            if mac_uuid {
                identity.volume_uuid = Some(Uuid::new_v4().to_string());
            } else {
                identity.windows_file_id = None;
            }
            assert!(damaged.validate().is_err());
            assert!(owned_scopes(&damaged).is_err());
        }
    }
    let mut upper_half = plan.clone();
    upper_half.expected.windows_file_id.as_mut().unwrap()[15] ^= 1;
    assert!(owned_scopes(&upper_half).is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), b"platform owned archive");
}
