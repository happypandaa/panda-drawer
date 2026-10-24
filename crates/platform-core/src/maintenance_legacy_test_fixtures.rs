//! Explicit historical files for isolated tests. This is not an alternate
//! admission path: production `submit` always rejects retired operations,
//! including when the library is compiled with `cfg(test)`.
use super::*;
use crate::backups::{PendingBuild, SnapshotOptions};

/// Materialize a pre-retirement task, archive and optional maintenance control.
/// Callers own disposable RuntimePaths and must opt into this named fixture.
pub(crate) async fn historical_operation(
    manager: &Arc<MaintenanceManager>,
    id: &str,
    kind: &str,
    input: OperationInput,
) -> TaskRecord {
    assert!(matches!(kind, "backup" | "maintenanceBegin"));
    let (task, lease) = manager.tasks.create_operation(id, kind).unwrap();
    manager
        .tasks
        .phase(&task.task_id, "legacy-fixture")
        .unwrap();
    let result = async {
        let control = manager.store.control(id)?;
        if kind == "maintenanceBegin" && control.get().mode != "maintenance" {
            let project = manager.store.project(id)?;
            let info = manager.tasks.info(id).await?;
            control.begin(
                input.reason.unwrap_or_else(|| "Historical maintenance fixture".into()),
                Some(builds::input_digest(&project)?),
                info["sourceState"] != "unbuilt",
            )?;
        }
        let project = manager.store.project(id)?;
        let state = control.get();
        let info = builds::build_info(&manager.paths, &project)?;
        let pending = state.pending_build
            || (state.mode == "maintenance"
                && info["sourceState"] != "matched"
                && state.baseline_input_digest.as_ref().is_some_and(|digest| {
                    builds::input_digest(&project).ok().as_ref() != Some(digest)
                }));
        let backup = format!("backup_{}", Uuid::new_v4().simple());
        let candidate = manager.paths.workspace_root()
            .join(".platform/legacy-test-fixtures")
            .join(&task.task_id);
        let prepared = backups::prepare_snapshot(
            &project.root,
            &project.manifest,
            &candidate,
            SnapshotOptions {
                backup_id: backup.clone(),
                kind: if kind == "maintenanceBegin" { "maintenanceBaseline" } else { "manual" }.into(),
                note: input.note,
                snapshot_state: if state.mode == "maintenance" { SnapshotState::Maintenance } else { SnapshotState::Ready },
                pending_build: PendingBuild {
                    required: pending,
                    reason: pending.then(|| "Historical unfinished build inputs".into()),
                },
                build_provenance: backups::capture_provenance(&project)?,
            },
        )?;
        backups::publish_snapshot(&prepared, &manager.directory(id)?.join(format!("{backup}.zip")))?;
        if kind == "maintenanceBegin" {
            let mut current = control.get();
            current.baseline_backup_id = Some(backup.clone());
            current.protected_backup_ids.push(backup.clone());
            current.ready_for_changes = true;
            current.last_error = None;
            control.set(current)?;
        }
        Ok::<Value, ApiError>(json!({"backupId":backup,"baselineBackupId":if kind=="maintenanceBegin" {Some(&backup)} else {None},"maintenance":manager.status(id)?}))
    }.await;
    if let Err(error) = &result {
        let control = manager.store.control(id).unwrap();
        let mut current = control.get();
        if current.mode == "maintenance" {
            current.last_error = Some(error.clone());
            control.set(current).unwrap();
        }
    }
    manager.tasks.finish(&task.task_id, result).unwrap();
    drop(lease);
    manager.tasks.task(&task.task_id).unwrap()
}
