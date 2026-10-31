use super::*;
use transaction::Point;

fn fixture(selected: bool) -> (tempfile::TempDir, WebProjectOwnership, String, Plan) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let data = root.join("data");
    let workspace = root.join("workspace");
    let outside = root.join("用户 外部");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&outside).unwrap();
    let owner = WebProjectOwnership::new(data, workspace);
    let id = "prj_disposable_delivery".to_string();
    owner.initialize(&id).unwrap();
    let task = "task_disposable_delivery".to_string();
    owner.record_task(&id, &task).unwrap();
    let stage = outside.join(format!(".export-{task}.partial"));
    let destination = outside.join("用户 保存.txt");
    owner.reserve_export_stage(&id, &task, &stage).unwrap();
    use std::io::Write;
    let mut output = native::create_private_file_for_publication(&stage).unwrap();
    let payload = PersistedIdentity::from_file(&output).unwrap();
    owner
        .capture_export_stage(&id, &task, &stage, &payload)
        .unwrap();
    output.write_all(b"new payload").unwrap();
    output.sync_all().unwrap();
    drop(output);
    let old = if selected {
        fs::write(&destination, b"old original").unwrap();
        Some(PersistedIdentity::capture(&destination).unwrap())
    } else {
        None
    };
    if let Some(old) = &old {
        owner
            .prepare_export_stage_overwrite(&id, &task, &stage, &payload, &destination, old)
            .unwrap();
    }
    let plan = Plan {
        task,
        stage,
        destination,
        parent_identity: PersistedIdentity::capture(&outside).unwrap(),
        payload,
        selected: old,
    };
    (temporary, owner, id, plan)
}
fn interrupt(owner: &WebProjectOwnership, id: &str, plan: Plan, point: Point) {
    let result = transaction::publish(owner, id, plan, |observed| {
        if observed == point {
            Err(failure("disposable crash checkpoint"))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
}
#[test]
fn fresh_and_overwrite_publish_release_final_ownership_and_keep_other_hardlink() {
    for selected in [false, true] {
        let (_temporary, owner, id, plan) = fixture(selected);
        let other = plan.destination.with_file_name("other original.txt");
        if selected {
            fs::hard_link(&plan.destination, &other).unwrap();
        }
        transaction::publish(&owner, &id, plan.clone(), |_| Ok(())).unwrap();
        assert_eq!(fs::read(&plan.destination).unwrap(), b"new payload");
        if selected {
            assert_eq!(fs::read(other).unwrap(), b"old original");
        }
        assert!(!plan.stage.exists());
        assert!(!plan.held().exists());
        let record = owner.read(&id).unwrap();
        assert!(record.native_deliveries.is_empty());
        assert!(record.export_stages.is_empty());
        assert!(record.tasks.is_empty());
        assert!(!owner
            .material_paths(&id)
            .unwrap()
            .contains(&plan.destination));
    }
}
#[test]
fn every_published_checkpoint_recovers_without_redelivering_after_commit() {
    for point in [
        Point::Bound,
        Point::IntentPublished,
        Point::OldHeld,
        Point::PayloadPublished,
        Point::OldAtStage,
        Point::DecisionPublished,
        Point::StageForgotten,
        Point::IntentRetired,
        Point::ControlsRetired,
    ] {
        let (_temporary, owner, id, plan) = fixture(true);
        interrupt(&owner, &id, plan.clone(), point);
        let after_commit = matches!(
            point,
            Point::DecisionPublished
                | Point::StageForgotten
                | Point::IntentRetired
                | Point::ControlsRetired
        );
        let moved = plan
            .destination
            .with_file_name("completed moved by user.txt");
        if after_commit {
            fs::rename(&plan.destination, &moved).unwrap();
            fs::write(&plan.destination, b"later external file").unwrap();
        }
        recover_project(&owner, &id).unwrap();
        recover_project(&owner, &id).unwrap();
        assert!(
            owner.read(&id).unwrap().native_deliveries.is_empty(),
            "{point:?}"
        );
        if after_commit {
            assert_eq!(fs::read(moved).unwrap(), b"new payload");
            assert_eq!(fs::read(&plan.destination).unwrap(), b"later external file");
        } else if matches!(
            point,
            Point::Bound | Point::IntentPublished | Point::OldHeld
        ) {
            assert_eq!(fs::read(&plan.destination).unwrap(), b"old original");
            assert_eq!(fs::read(&plan.stage).unwrap(), b"new payload");
        } else {
            assert_eq!(fs::read(&plan.destination).unwrap(), b"new payload");
            assert!(!plan.stage.exists());
        }
    }
}
#[test]
fn occupied_held_or_replaced_destination_preserves_all_evidence() {
    for replacement in [false, true] {
        let (_temporary, owner, id, plan) = fixture(true);
        interrupt(&owner, &id, plan.clone(), Point::IntentPublished);
        let external = if replacement {
            plan.destination.clone()
        } else {
            plan.held()
        };
        let saved = plan.destination.with_file_name("preserved original.txt");
        if replacement {
            fs::rename(&plan.destination, &saved).unwrap();
        }
        fs::write(&external, b"unowned outsider").unwrap();
        assert!(recover_project(&owner, &id).is_err());
        assert_eq!(fs::read(external).unwrap(), b"unowned outsider");
        assert_eq!(fs::read(&plan.stage).unwrap(), b"new payload");
        if replacement {
            assert_eq!(fs::read(saved).unwrap(), b"old original");
        }
        assert!(!owner.read(&id).unwrap().native_deliveries.is_empty());
    }
}
#[test]
fn missing_or_corrupt_published_intent_never_adopts_moved_user_file() {
    for corrupt in [false, true] {
        let (_temporary, owner, id, plan) = fixture(true);
        interrupt(&owner, &id, plan.clone(), Point::OldHeld);
        let path = directory(&owner, &id, &plan.task).unwrap().join(INTENT);
        if corrupt {
            fs::write(&path, b"changed private journal").unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        assert!(recover_project(&owner, &id).is_err());
        assert_eq!(fs::read(plan.held()).unwrap(), b"old original");
        assert_eq!(fs::read(plan.stage).unwrap(), b"new payload");
    }
}
