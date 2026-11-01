//! Every delivery and destructive recovery here uses an isolated TempDir.
use super::*;
use crate::{
    builds::BuildManager, maintenance::MaintenanceManager,
    web_project_deletion::WebProjectDeletionManager, CreateProjectInput, ProjectStore,
    RuntimePaths, TaskStatus,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    lifecycle: Arc<WebProjectDeletionManager>,
    project: ProjectHandle,
    source: PathBuf,
    destination: PathBuf,
    other: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        store.set_node_source_data_root(paths.data_root().into());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable native delivery A".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let lifecycle = WebProjectDeletionManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance,
        )
        .unwrap();
        let external = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("external-deliveries");
        fs::create_dir(&external).unwrap();
        let source = external.join("independent-original.bin");
        fs::write(&source, b"disposable download bytes").unwrap();
        let destination = external.join("delivered.bin");
        let other = external.join("independent-B.bin");
        fs::write(&other, b"independent B original").unwrap();
        Self {
            _temp: temp,
            paths,
            store,
            tasks,
            lifecycle,
            project,
            source,
            destination,
            other,
        }
    }
    fn ownership(&self) -> WebProjectOwnership {
        WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        )
    }
    fn copy(&self, hook: impl FnMut(Point, &Path) -> Result<()>) -> Result<()> {
        publish(
            self.paths.data_root(),
            self.paths.workspace_root(),
            &self.project,
            File::open(&self.source).unwrap(),
            &self.destination,
            |_| Ok(()),
            hook,
        )
    }
    fn stages(&self) -> Vec<PathBuf> {
        self.ownership()
            .material_paths(&self.project.id)
            .unwrap()
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(".export-task_") && name.ends_with(".partial")
                    })
            })
            .collect()
    }
    async fn delete(&self) {
        let id = &self.project.id;
        let task = self
            .lifecycle
            .submit_project(
                id,
                "delete",
                json!({"expectedDocumentEpoch":self.project.document_epoch}),
            )
            .await
            .unwrap();
        let final_task = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let task = self
                    .lifecycle
                    .task_for_project(&task.task_id, id, "delete")
                    .unwrap();
                if task.status.is_final() {
                    break task;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(final_task.status, TaskStatus::Succeeded, "{final_task:?}");
        assert_eq!(fs::read(&self.other).unwrap(), b"independent B original");
        assert_eq!(
            fs::read(&self.source).unwrap(),
            b"disposable download bytes"
        );
    }
    async fn stop(self) {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
#[tokio::test]
async fn successful_new_delivery_and_authorized_overwrite_forget_only_their_empty_copy_reservations(
) {
    let fixture = Fixture::new().await;
    publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &fixture.project,
        File::open(&fixture.source).unwrap(),
        &fixture.destination,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"disposable download bytes"
    );
    assert!(fixture.stages().is_empty());
    assert!(fixture
        .ownership()
        .task_ids(&fixture.project.id)
        .unwrap()
        .is_empty());
    let existing = fixture
        .tasks
        .create_readonly_operation(&fixture.project.id, "storageScan")
        .unwrap();
    fixture
        .tasks
        .finish(&existing.task_id, Ok(json!({"disposable":true})))
        .unwrap();
    fs::write(
        &fixture.destination,
        b"explicitly selected previous delivery",
    )
    .unwrap();
    fixture.copy(|_, _| Ok(())).unwrap();
    assert_eq!(
        fixture.ownership().task_ids(&fixture.project.id).unwrap(),
        vec![existing.task_id.clone()]
    );
    assert!(fixture.tasks.task(&existing.task_id).is_ok());
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"disposable download bytes"
    );
    assert!(!fixture
        .ownership()
        .material_paths(&fixture.project.id)
        .unwrap()
        .contains(&fixture.destination));
    fixture.delete().await;
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"disposable download bytes"
    );
    fixture.stop().await;
}
#[tokio::test]
async fn every_bound_copy_checkpoint_reopens_and_deletes_only_its_stage_without_adopting_final_delivery(
) {
    for at in [
        Point::Bound,
        Point::Copied,
        Point::BeforePublish,
        Point::BeforeRename,
        Point::Renamed,
        Point::Published,
    ] {
        let fixture = Fixture::new().await;
        let mut seen = None;
        assert!(fixture
            .copy(|point, path| {
                if std::mem::discriminant(&point) == std::mem::discriminant(&at) {
                    seen = Some(path.to_path_buf());
                    return Err(invalid("disposable crash checkpoint"));
                }
                Ok(())
            })
            .is_err());
        let stage = seen.unwrap();
        let reopened = WebProjectOwnership::new(
            fixture.paths.data_root().into(),
            fixture.paths.workspace_root().into(),
        );
        assert!(reopened
            .material_paths(&fixture.project.id)
            .unwrap()
            .contains(&stage));
        assert!(!reopened
            .material_paths(&fixture.project.id)
            .unwrap()
            .contains(&fixture.destination));
        if !matches!(at, Point::Renamed | Point::Published) {
            assert!(stage.exists());
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&stage).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fixture.delete().await;
        assert!(!stage.exists());
        assert_eq!(
            fixture.destination.exists(),
            matches!(at, Point::Renamed | Point::Published)
        );
        fixture.stop().await;
    }
}
#[tokio::test]
async fn overwrite_exchange_checkpoints_require_verified_delivery_before_cleaning_the_old_inode() {
    for at in [Point::BeforeRename, Point::Renamed, Point::Published] {
        let fixture = Fixture::new().await;
        fs::write(&fixture.destination, b"user approved old delivery").unwrap();
        let mut captured = None;
        assert!(fixture
            .copy(|point, stage| {
                if std::mem::discriminant(&point) == std::mem::discriminant(&at) {
                    captured = Some(stage.to_path_buf());
                    return Err(invalid("disposable interrupted exchange"));
                }
                Ok(())
            })
            .is_err());
        let stage = captured.unwrap();
        let exchanged = !matches!(at, Point::BeforeRename);
        assert_eq!(
            fs::read(&stage).unwrap(),
            if exchanged {
                b"user approved old delivery".as_slice()
            } else {
                b"disposable download bytes".as_slice()
            }
        );
        let reopened = WebProjectOwnership::new(
            fixture.paths.data_root().into(),
            fixture.paths.workspace_root().into(),
        );
        if matches!(at, Point::Renamed) {
            // The exchange exists, but no durable two-ended verification has
            // been recorded. Project cleanup cannot discard the old original.
            assert!(reopened.material_paths(&fixture.project.id).is_err());
            let parent = crate::files::root(stage.parent().unwrap()).unwrap();
            publish_at(&parent, &stage, &fixture.destination, Rename::Swap).unwrap();
        } else {
            let paths = reopened.material_paths(&fixture.project.id).unwrap();
            assert!(paths.contains(&stage));
            assert!(!paths.contains(&fixture.destination));
        }
        fixture.delete().await;
        assert!(!stage.exists());
        assert_eq!(
            fs::read(&fixture.destination).unwrap(),
            if matches!(at, Point::Published) {
                b"disposable download bytes".as_slice()
            } else {
                b"user approved old delivery".as_slice()
            }
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn an_unconfirmed_exchange_never_authorizes_deleting_the_user_selected_old_file() {
    let fixture = Fixture::new().await;
    fs::write(
        &fixture.destination,
        b"user original must survive unknown delivery",
    )
    .unwrap();
    let moved_payload = fixture
        .destination
        .with_extension("externally-moved-new-delivery");
    let mut stage = None;
    let error = fixture
        .copy(|point, path| {
            if matches!(point, Point::Renamed) {
                stage = Some(path.to_path_buf());
                fs::rename(&fixture.destination, &moved_payload).unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    let stage = stage.unwrap();
    assert_eq!(error.details["restorationConfirmed"], false);
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"user original must survive unknown delivery"
    );
    assert!(fixture
        .ownership()
        .material_paths(&fixture.project.id)
        .is_err());
    assert!(fixture
        .ownership()
        .material_identity(&fixture.project.id, &stage)
        .is_err());
    assert!(crate::web_file_deletion::remove_owned_file(
        &fixture.ownership(),
        &fixture.project.id,
        &stage,
        &PersistedIdentity::capture(&stage).unwrap(),
    )
    .is_err());
    assert_eq!(
        fs::read(&stage).unwrap(),
        b"user original must survive unknown delivery"
    );
    // Explicit rechecking restores the original public file and registered
    // payload. No final-path ownership or automatic adoption is introduced.
    fs::rename(&stage, &fixture.destination).unwrap();
    fs::rename(&moved_payload, &stage).unwrap();
    fixture.delete().await;
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"user original must survive unknown delivery"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn replacing_the_selected_target_in_the_final_rename_gap_restores_the_foreign_file() {
    let fixture = Fixture::new().await;
    fs::write(&fixture.destination, b"user approved original").unwrap();
    let original = fixture
        .destination
        .with_extension("selected-original-preserved");
    let error = fixture
        .copy(|point, _| {
            if matches!(point, Point::BeforeRename) {
                fs::rename(&fixture.destination, &original).unwrap();
                fs::write(&fixture.destination, b"foreign final-gap replacement").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["restorationConfirmed"], true);
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"foreign final-gap replacement"
    );
    assert_eq!(fs::read(&original).unwrap(), b"user approved original");
    let stages = fixture.stages();
    assert_eq!(stages.len(), 1);
    assert_eq!(fs::read(&stages[0]).unwrap(), b"disposable download bytes");
    fixture.delete().await;
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"foreign final-gap replacement"
    );
    assert_eq!(fs::read(&original).unwrap(), b"user approved original");
    fixture.stop().await;
}

#[tokio::test]
async fn replacing_the_payload_in_the_final_rename_gap_never_delivers_or_deletes_the_foreign_file()
{
    for overwrite in [false, true] {
        let fixture = Fixture::new().await;
        if overwrite {
            fs::write(&fixture.destination, b"user approved original").unwrap();
        }
        let payload = fixture
            .destination
            .with_extension("owned-payload-preserved");
        let mut stage = None;
        let error = fixture
            .copy(|point, path| {
                if matches!(point, Point::BeforeRename) {
                    stage = Some(path.to_path_buf());
                    fs::rename(path, &payload).unwrap();
                    fs::write(path, b"foreign stage final-gap replacement").unwrap();
                }
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.details["restorationConfirmed"], true);
        let stage = stage.unwrap();
        assert_eq!(
            fs::read(&stage).unwrap(),
            b"foreign stage final-gap replacement"
        );
        assert_eq!(fixture.destination.exists(), overwrite);
        if overwrite {
            assert_eq!(
                fs::read(&fixture.destination).unwrap(),
                b"user approved original"
            );
        }
        assert!(fixture
            .ownership()
            .material_paths(&fixture.project.id)
            .is_err());
        // Simulate explicit external rechecking: restore the registered stage
        // and keep the foreign file outside the project cleanup scope.
        let foreign = fixture
            .destination
            .with_extension("foreign-stage-preserved");
        fs::rename(&stage, &foreign).unwrap();
        fs::rename(&payload, &stage).unwrap();
        fixture.delete().await;
        assert_eq!(
            fs::read(&foreign).unwrap(),
            b"foreign stage final-gap replacement"
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn an_occupied_restore_name_keeps_both_foreign_objects_and_blocks_project_cleanup() {
    let fixture = Fixture::new().await;
    let payload = fixture
        .destination
        .with_extension("owned-payload-preserved");
    let mut stage = None;
    let error = fixture
        .copy(|point, path| {
            if matches!(point, Point::BeforeRename) {
                stage = Some(path.to_path_buf());
                fs::rename(path, &payload).unwrap();
                fs::write(path, b"foreign published object").unwrap();
            } else if matches!(point, Point::Renamed) {
                fs::write(path, b"foreign occupied restore name").unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["restorationConfirmed"], false);
    let stage = stage.unwrap();
    assert_eq!(
        fs::read(&fixture.destination).unwrap(),
        b"foreign published object"
    );
    assert_eq!(fs::read(&stage).unwrap(), b"foreign occupied restore name");
    assert!(fixture
        .ownership()
        .material_paths(&fixture.project.id)
        .is_err());
    let foreign_final = fixture
        .destination
        .with_extension("foreign-final-preserved");
    let foreign_stage = fixture
        .destination
        .with_extension("foreign-stage-preserved");
    fs::rename(&fixture.destination, &foreign_final).unwrap();
    fs::rename(&stage, &foreign_stage).unwrap();
    fs::rename(&payload, &stage).unwrap();
    fixture.delete().await;
    assert_eq!(
        fs::read(&foreign_final).unwrap(),
        b"foreign published object"
    );
    assert_eq!(
        fs::read(&foreign_stage).unwrap(),
        b"foreign occupied restore name"
    );
    fixture.stop().await;
}
#[tokio::test]
async fn a_panicked_copy_keeps_its_registered_inode_for_later_project_deletion() {
    let fixture = Fixture::new().await;
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fixture.copy(|point, _| {
            if matches!(point, Point::Copied) {
                panic!("disposable worker panic");
            }
            Ok(())
        })
    }));
    assert!(stopped.is_err());
    let stages = fixture.stages();
    assert_eq!(stages.len(), 1);
    fixture.delete().await;
    assert!(!stages[0].exists());
    assert!(!fixture.destination.exists());
    fixture.stop().await;
}
#[tokio::test]
async fn target_replacement_and_a_new_filename_collision_preserve_foreign_files_and_owned_stages() {
    for overwrite in [true, false] {
        let fixture = Fixture::new().await;
        if overwrite {
            fs::write(&fixture.destination, b"original selected target").unwrap();
        }
        let kept = fixture.destination.with_extension("kept-original");
        assert!(fixture
            .copy(|point, _| {
                if matches!(point, Point::Copied) {
                    if overwrite {
                        fs::rename(&fixture.destination, &kept).unwrap();
                    }
                    fs::write(&fixture.destination, b"foreign replacement B").unwrap();
                }
                Ok(())
            })
            .is_err());
        assert_eq!(
            fs::read(&fixture.destination).unwrap(),
            b"foreign replacement B"
        );
        let stages = fixture.stages();
        assert_eq!(stages.len(), 1);
        fixture.delete().await;
        assert!(!stages[0].exists());
        assert_eq!(
            fs::read(&fixture.destination).unwrap(),
            b"foreign replacement B"
        );
        if overwrite {
            assert_eq!(fs::read(&kept).unwrap(), b"original selected target");
        }
        fixture.stop().await;
    }
}
#[tokio::test]
async fn replacing_the_public_stage_never_adopts_or_removes_the_foreign_object() {
    let fixture = Fixture::new().await;
    let kept = fixture.destination.with_extension("owned-stage-kept");
    let mut stage = None;
    assert!(fixture
        .copy(|point, path| {
            if matches!(point, Point::Copied) {
                stage = Some(path.to_path_buf());
                fs::rename(path, &kept).unwrap();
                fs::write(path, b"foreign temporary file B").unwrap();
            }
            Ok(())
        })
        .is_err());
    let stage = stage.unwrap();
    assert!(fixture
        .ownership()
        .material_paths(&fixture.project.id)
        .is_err());
    assert_eq!(fs::read(&stage).unwrap(), b"foreign temporary file B");
    let foreign = fixture.destination.with_extension("foreign-B-preserved");
    fs::rename(&stage, &foreign).unwrap();
    fs::rename(&kept, &stage).unwrap();
    fixture.delete().await;
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign temporary file B");
    fixture.stop().await;
}
#[tokio::test]
async fn oversized_or_changed_sources_and_stale_epochs_never_publish() {
    let fixture = Fixture::new().await;
    let big = fixture.destination.with_extension("oversized-input");
    let file = File::create(&big).unwrap();
    file.set_len(LIMIT + 1).unwrap();
    drop(file);
    assert!(publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &fixture.project,
        File::open(&big).unwrap(),
        &fixture.destination,
        |_| Ok(())
    )
    .is_err());
    assert!(fixture.stages().is_empty());
    assert!(fixture
        .copy(|point, _| {
            if matches!(point, Point::Bound) {
                fs::write(&fixture.source, b"changed length").unwrap();
            }
            Ok(())
        })
        .is_err());
    assert!(!fixture.destination.exists());
    fs::write(&fixture.source, b"disposable download bytes").unwrap();
    let mut stale = fixture.project.clone();
    stale.document_epoch = uuid::Uuid::new_v4().to_string();
    let before = fixture.ownership().task_ids(&fixture.project.id).unwrap();
    assert!(publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &stale,
        File::open(&fixture.source).unwrap(),
        &fixture.destination,
        |_| Ok(())
    )
    .is_err());
    assert_eq!(
        before,
        fixture.ownership().task_ids(&fixture.project.id).unwrap()
    );
    fixture.delete().await;
    fixture.stop().await;
}
#[tokio::test]
async fn source_enrollment_and_native_copy_stage_admission_are_serialized_without_locking_the_copy()
{
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Condvar, Mutex,
    };
    fn release(gate: &(Mutex<bool>, Condvar)) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }
    fn wait(gate: &(Mutex<bool>, Condvar)) {
        let mut ready = gate.0.lock().unwrap();
        while !*ready {
            ready = gate.1.wait(ready).unwrap();
        }
    }
    let fixture = Fixture::new().await;
    let node = uuid::Uuid::new_v4().to_string();
    let node_owner = crate::node_project_ownership::NodeProjectOwnership::new(
        fixture.paths.data_root().into(),
        fixture.paths.workspace_root().into(),
    );
    node_owner.initialize(&node).unwrap();
    let validated = Arc::new((Mutex::new(false), Condvar::new()));
    let copying = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (copy_tx, copy_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker_validated = validated.clone();
        let worker_copying = copying.clone();
        let copied = &fixture;
        let publisher = scope.spawn(move || {
            let first = AtomicBool::new(true);
            publish(
                copied.paths.data_root(),
                copied.paths.workspace_root(),
                &copied.project,
                File::open(&copied.source).unwrap(),
                &copied.destination,
                |path| {
                    if first.swap(false, Ordering::AcqRel) {
                        entered_tx.send(()).unwrap();
                        wait(&worker_validated);
                    }
                    crate::node_apps::source_ownership::require_delivery_location_unclaimed(
                        copied.paths.data_root(),
                        path.parent().unwrap(),
                    )
                },
                |point, _| {
                    if matches!(point, Point::Bound) {
                        copy_tx.send(()).unwrap();
                        wait(&worker_copying);
                    }
                    Ok(())
                },
            )
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let copied = &fixture;
        let node = &node;
        let claimant = scope.spawn(move || {
            let result = crate::node_apps::source_ownership::claim_source_after_verified_migration(
                copied.paths.data_root(),
                copied.paths.workspace_root(),
                node,
                copied.destination.parent().unwrap(),
            );
            result_tx.send(result).unwrap();
        });
        release(&validated);
        copy_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(result_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .is_err());
        let unrelated = fixture
            .paths
            .data_root()
            .parent()
            .unwrap()
            .join("independent-node-B");
        fs::create_dir(&unrelated).unwrap();
        crate::node_apps::source_ownership::claim_source_after_verified_migration(
            fixture.paths.data_root(),
            fixture.paths.workspace_root(),
            node,
            &unrelated,
        )
        .unwrap();
        release(&copying);
        claimant.join().unwrap();
        publisher.join().unwrap().unwrap();
    });
    fixture.delete().await;
    fixture.stop().await;
    let fixture = Fixture::new().await;
    let node = uuid::Uuid::new_v4().to_string();
    let owner = crate::node_project_ownership::NodeProjectOwnership::new(
        fixture.paths.data_root().into(),
        fixture.paths.workspace_root().into(),
    );
    owner.initialize(&node).unwrap();
    crate::node_apps::source_ownership::claim_source_after_verified_migration(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &node,
        fixture.destination.parent().unwrap(),
    )
    .unwrap();
    let before = fixture.ownership().task_ids(&fixture.project.id).unwrap();
    assert!(publish_file(
        fixture.paths.data_root(),
        fixture.paths.workspace_root(),
        &fixture.project,
        File::open(&fixture.source).unwrap(),
        &fixture.destination,
        |path| crate::node_apps::source_ownership::require_delivery_location_unclaimed(
            fixture.paths.data_root(),
            path.parent().unwrap()
        )
    )
    .is_err());
    assert_eq!(
        before,
        fixture.ownership().task_ids(&fixture.project.id).unwrap()
    );
    assert!(fixture.stages().is_empty());
    assert!(!fixture.destination.exists());
    fixture.delete().await;
    fixture.stop().await;
}
