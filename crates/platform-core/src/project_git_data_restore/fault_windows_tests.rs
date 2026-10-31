//! Each fault follows a real filesystem boundary, over a newly named test root.
use super::*;

async fn exercise(reserved: bool) {
    let mut reached_success = false;
    for point in 1..=40 {
        let mut fixture = Prepared::new().await;
        if reserved {
            // Match the original Mac full-reserve fixture: slot crashes are
            // exhausted by the first matrix. Enter committed metadata recovery
            // before testing the independent finite reserve state machine.
            let prepared = fixture
                .execute(DataRestoreDirection::Continue, u32::MAX - 1)
                .await
                .unwrap_err();
            assert_eq!(prepared.code, "PROJECT_DATA_RESTORE_TEST_FAULT");
            assert_eq!(prepared.message, "after-metadata-candidate-create");
            let status = transaction::status(
                fixture.paths.workspace_root(),
                &fixture.operation,
                &fixture.plan.project_id,
                &fixture.plan.task_id,
                &fixture.stop,
            );
            assert!(status.issues.is_empty(), "{status:?}");
            assert!(status.can_continue && !status.can_undo);
            assert_eq!(status.data_state, "applied");
            let root = files::root(&fixture.operation.join("metadata-attempts")).unwrap();
            let existing = fs::read_dir(fixture.operation.join("metadata-attempts"))
                .unwrap()
                .count();
            assert_eq!(existing, 1);
            for _ in existing..128 {
                let file = files::create(
                    &root,
                    &format!("completion-{}.json", uuid::Uuid::new_v4().simple()),
                )
                .unwrap();
                file.sync_all().unwrap();
            }
            drop(root);
            fixture.reopen().await;
        }
        let result = fixture.execute(DataRestoreDirection::Continue, point).await;
        if result.is_ok() {
            reached_success = true;
            assert!(
                point > 15,
                "all real slot/metadata/seal boundaries must execute"
            );
            fixture.finish().await;
            break;
        }
        let error = result.unwrap_err();
        assert_eq!(
            error.code, "PROJECT_DATA_RESTORE_TEST_FAULT",
            "point {point}, reserve {reserved}: {error:?}"
        );
        if reserved && error.message == "after-metadata-partial-write" {
            let partial = ["completion", "terminal"]
                .into_iter()
                .filter_map(|name| {
                    fs::metadata(fixture.operation.join("metadata-reserve").join(name))
                        .ok()
                        .filter(|metadata| metadata.len() > 0)
                        .map(|metadata| metadata.len())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                partial.len(),
                1,
                "the original partial reserve prefix remains"
            );
        }
        let state = guard::inspect(fixture.paths.workspace_root(), &fixture.plan.project_id);
        assert!(
            state.issues.is_empty(),
            "point {point}, reserve {reserved}: {state:?}"
        );
        if !state.pending.is_empty() {
            fixture.reopen().await;
            fixture
                .execute(DataRestoreDirection::Continue, 0)
                .await
                .unwrap_or_else(|error| panic!("point {point}, reserve {reserved}: {error:?}"));
        } else {
            // The original contract releases a visible sealed terminal even if
            // its final sync acknowledgement was interrupted. It never repeats
            // or reverses the already committed epoch on restart.
            assert!(fs::metadata(fixture.operation.join("terminal.json"))
                .unwrap()
                .permissions()
                .readonly());
            fixture.reopen().await;
        }
        assert_eq!(fixture.live_value(), "after");
        let state = guard::inspect(fixture.paths.workspace_root(), &fixture.plan.project_id);
        assert!(
            state.issues.is_empty() && state.pending.is_empty(),
            "{state:?}"
        );
        if reserved {
            assert_eq!(
                fs::read_dir(fixture.operation.join("metadata-attempts"))
                    .unwrap()
                    .count(),
                128
            );
        }
        fixture.finish().await;
    }
    assert!(
        reached_success,
        "every real transaction fault boundary must be covered"
    );
}

#[tokio::test]
async fn every_real_slot_metadata_and_seal_boundary_reopens_and_only_moves_toward_the_same_commit()
{
    exercise(false).await;
}

#[tokio::test]
async fn every_real_full_reserve_partial_append_and_seal_boundary_reopens_without_discarding_prefixes(
) {
    exercise(true).await;
}
