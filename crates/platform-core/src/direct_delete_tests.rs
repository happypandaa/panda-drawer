//! Permanent deletion evidence uses only the parent fixture's disposable files.
use super::*;

#[tokio::test]
async fn moving_a_completed_user_export_folder_never_blocks_or_expands_project_deletion() {
    let fixture = Fixture::new().await;
    let folder = fixture
        .paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-user-exports");
    fs::create_dir(&folder).unwrap();
    let archive = folder.join("user-archive.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let owner = crate::web_project_ownership::WebProjectOwnership::new(
        fixture.paths.data_root().into(),
        fixture.paths.workspace_root().into(),
    );
    let owner_json = fs::read_to_string(owner.path(&fixture.id).unwrap()).unwrap();
    assert!(!owner_json.contains(archive.to_str().unwrap()));
    let preview_paths =
        crate::web_project_cleanup::project_material_paths(&fixture.paths, &fixture.id).unwrap();
    assert!(!preview_paths.contains(&archive));
    assert!(!preview_paths.iter().any(|path| path.starts_with(&folder)));
    let moved = folder.with_file_name("disposable-user-renamed-exports");
    fs::rename(&folder, &moved).unwrap();
    let delivered = moved.join("user-archive.zip");
    let bytes = fs::read(&delivered).unwrap();
    assert!(!archive.exists());
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert_eq!(fs::read(&delivered).unwrap(), bytes);
    assert!(!owner.directory(&fixture.id).unwrap().exists());
    fixture.stop().await;
}

#[tokio::test]
async fn permanent_delete_removes_records_but_preserves_user_exports_and_external_originals() {
    let fixture = Fixture::new().await;
    let original = fixture.store.project(&fixture.id).unwrap();
    let archive = fixture.export_path("disposable-before-delete.zip");
    let exported = fixture
        .project("export", json!({"destinationPath":archive}))
        .await;
    good(&exported);
    assert!(archive.is_file());
    let archive_bytes = fs::read(&archive).unwrap();
    let history = fixture
        .paths
        .workspace_root()
        .join("backups")
        .join(&fixture.id)
        .join("retained-evidence.txt");
    let recovery = fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&exported.task_id)
        .join("original/keep.txt");
    let external = fixture.paths.data_root().join("external-source/keep.txt");
    for path in [&history, &recovery, &external] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"disposable fixture material").unwrap();
    }
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert_eq!(good(&deleted)["restoreAvailable"], false);
    assert!(good(&deleted).get("trashId").is_none());
    assert!(!original.root.exists());
    assert!(original.check().is_err());
    assert!(!fixture
        .store
        .list()
        .await
        .unwrap()
        .iter()
        .any(|project| project.id == fixture.id));
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_none());
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
    assert!(!fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .exists());
    for path in [&history, &recovery] {
        assert!(!path.exists());
    }
    assert_eq!(fs::read(&external).unwrap(), b"disposable fixture material");
    assert!(fixture
        .store
        .deleted_project_identity_absent(&fixture.id)
        .unwrap());
    assert!(fixture.tasks.task(&deleted.task_id).is_err());
    assert!(fixture
        .lifecycle
        .operations
        .lock()
        .unwrap()
        .values()
        .all(|operation| operation.project_id != fixture.id));
    assert!(!fixture
        .lifecycle
        .operation(&deleted.task_id)
        .join("delete-progress.ndjson")
        .exists());
    fixture.stop().await;
}

#[tokio::test]
async fn permanent_delete_accepts_a_corrupt_database_and_unlinks_external_symlinks_only() {
    let fixture = Fixture::new().await;
    let legacy_trash = fixture.paths.workspace_root().join(".platform/trash");
    assert!(!legacy_trash.exists());
    let root = fixture.store.project(&fixture.id).unwrap().root;
    fs::write(
        root.join("data/main.sqlite"),
        b"corrupt disposable database",
    )
    .unwrap();
    let external = fixture.paths.data_root().join("external-original");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("keep.txt"), b"external original").unwrap();
    std::os::unix::fs::symlink(&external, root.join("external-link")).unwrap();
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!root.exists());
    assert!(!legacy_trash.exists());
    assert_eq!(
        fs::read(external.join("keep.txt")).unwrap(),
        b"external original"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn permanent_delete_refuses_a_replaced_registered_root_without_touching_either_directory() {
    let fixture = Fixture::new().await;
    let root = fixture.store.project(&fixture.id).unwrap().root;
    let retained = fixture.paths.data_root().join("retained-original-project");
    fs::rename(&root, &retained).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep.txt"), b"replacement must survive").unwrap();
    let error = fixture
        .lifecycle
        .submit_project(&fixture.id, "delete", json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "PROJECT_IDENTITY_CHANGED");
    assert_eq!(
        fs::read(root.join("keep.txt")).unwrap(),
        b"replacement must survive"
    );
    assert!(retained.join("data/main.sqlite").is_file());
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn permanent_delete_requires_confirmation_and_reimport_uses_an_independent_identity() {
    let fixture = Fixture::new().await;
    let displayed = fixture.store.project(&fixture.id).unwrap();
    let missing = fixture
        .lifecycle
        .submit_project(&fixture.id, "delete", json!({}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "PROJECT_DELETE_CONFIRMATION_REQUIRED");
    let archive = fixture.export_path("disposable-stale-delete-confirmation.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    good(
        &fixture
            .project(
                "delete",
                json!({"expectedDocumentEpoch":displayed.document_epoch}),
            )
            .await,
    );
    assert!(archive.is_file());
    let imported = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive}))
        .await
        .unwrap();
    let imported = fixture.wait(&imported).await;
    let new_id = good(&imported)["projectId"].as_str().unwrap();
    assert_ne!(new_id, fixture.id);
    let current = fixture.store.project(new_id).unwrap();
    assert_ne!(current.document_epoch, displayed.document_epoch);
    let before = fs::read(current.root.join("data/attachments/original.txt")).unwrap();
    let stale = fixture
        .lifecycle
        .submit_project(
            &fixture.id,
            "delete",
            json!({"expectedDocumentEpoch":displayed.document_epoch}),
        )
        .await
        .unwrap();
    assert_eq!(good(&stale)["operationOutcome"], "deleted");
    assert_eq!(good(&stale)["alreadyAbsent"], true);
    assert_eq!(
        fs::read(current.root.join("data/attachments/original.txt")).unwrap(),
        before
    );
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn permanent_delete_keeps_registered_confirmation_readable_when_manifest_and_database_are_missing(
) {
    let fixture = Fixture::new().await;
    let original = fixture.store.project(&fixture.id).unwrap();
    fs::remove_file(original.root.join("project.json")).unwrap();
    fs::remove_file(original.root.join("data/main.sqlite")).unwrap();
    let info = fixture.store.info(&fixture.id).await.unwrap();
    assert_eq!(info.document_epoch, original.document_epoch);
    assert_eq!(info.state, "error");
    let deleted = fixture
        .project(
            "delete",
            json!({"expectedDocumentEpoch":info.document_epoch}),
        )
        .await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!original.root.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn interrupted_direct_delete_quarantines_residue_without_replaying_removal() {
    let fixture = Fixture::new().await;
    let (root, expected) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let mut journal = LifecycleManager::operation_state(&task, "delete", &fixture.id);
    journal.directory_name = Some(root_directory_name(&root).unwrap());
    journal.directory_identity = Some(expected.clone());
    journal.control = Some(fixture.store.control(&fixture.id).unwrap().get());
    fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &journal.target_epoch, &expected)
        .unwrap();
    journal.phase = "delete-intent".into();
    fixture.lifecycle.remember(&journal).unwrap();
    fs::remove_file(root.join("app/README.md")).unwrap();
    let remaining = root.join("data/attachments/original.txt");
    let bytes = fs::read(&remaining).unwrap();
    drop(lease);
    let fixture = fixture.restart_task(task.clone()).await;
    let recovered = fixture.tasks.task(&task.task_id).unwrap();
    assert_eq!(recovered.status, TaskStatus::Interrupted);
    assert!(recovered.result.is_none());
    assert_eq!(fs::read(&remaining).unwrap(), bytes);
    assert_eq!(
        fixture.store.project(&fixture.id).err().unwrap().code,
        "PROJECT_DELETE_INCOMPLETE"
    );
    assert_eq!(
        fixture
            .store
            .check_development_available(&fixture.id)
            .unwrap_err()
            .code,
        "PROJECT_DELETE_INCOMPLETE"
    );
    assert_eq!(
        fixture
            .store
            .check_identity_available(&fixture.id, None)
            .unwrap_err()
            .code,
        "PROJECT_ID_CONFLICT"
    );
    assert_eq!(
        fixture.store.begin_workspace_migration().unwrap_err().code,
        "PROJECT_DELETE_INCOMPLETE"
    );
    assert!(!fixture.store.workspace_paused());
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!root.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn unstarted_direct_delete_recovery_preserves_the_active_project() {
    let fixture = Fixture::new().await;
    let project = fixture.store.project(&fixture.id).unwrap();
    let before = fs::read(project.root.join("data/attachments/original.txt")).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let mut journal = LifecycleManager::operation_state(&task, "delete", &fixture.id);
    journal.directory_name = Some(root_directory_name(&project.root).unwrap());
    journal.directory_identity = Some(persistent_identity(&project.root).unwrap());
    fixture.lifecycle.remember(&journal).unwrap();
    drop(lease);
    let fixture = fixture.restart_task(task.clone()).await;
    assert_eq!(
        fixture.tasks.task(&task.task_id).unwrap().status,
        TaskStatus::Interrupted
    );
    assert!(fixture.store.project(&fixture.id).is_ok());
    assert_eq!(
        fs::read(project.root.join("data/attachments/original.txt")).unwrap(),
        before
    );
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn removed_payload_requires_explicit_finalization_before_a_later_import() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("disposable-reimport-after-delete.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let (root, expected) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let mut journal = LifecycleManager::operation_state(&task, "delete", &fixture.id);
    journal.directory_name = Some(root_directory_name(&root).unwrap());
    journal.directory_identity = Some(expected.clone());
    journal.control = Some(fixture.store.control(&fixture.id).unwrap().get());
    fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &journal.target_epoch, &expected)
        .unwrap();
    journal.phase = "delete-intent".into();
    fixture.lifecycle.remember(&journal).unwrap();
    fs::remove_dir_all(&root).unwrap();
    drop(lease);
    let fixture = fixture.restart_task(task.clone()).await;
    assert_eq!(
        fixture.tasks.task(&task.task_id).unwrap().status,
        TaskStatus::Interrupted
    );
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_some());
    assert_eq!(
        good(&fixture.project("delete", json!({})).await)["operationOutcome"],
        "deleted"
    );
    assert!(archive.is_file());
    let imported = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive}))
        .await
        .unwrap();
    let imported = fixture.wait(&imported).await;
    let new_id = good(&imported)["projectId"].as_str().unwrap();
    assert_ne!(new_id, fixture.id);
    let project = fixture.store.project(new_id).unwrap();
    let epoch = project.document_epoch.clone();
    let identity = persistent_identity(&project.root).unwrap();
    let control = fixture.store.control(new_id).unwrap().get();
    assert!(!fixture
        .lifecycle
        .operation(&task.task_id)
        .join("lifecycle.json")
        .exists());
    let current = fixture.store.project(new_id).unwrap();
    assert_eq!(current.document_epoch, epoch);
    assert!(identity.matches_path(&current.root).unwrap());
    assert_eq!(
        json!(fixture.store.control(new_id).unwrap().get()),
        json!(control)
    );
    fixture.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_partial_delete_reports_failure_blocks_runtime_and_requires_explicit_retry() {
    let fixture = Fixture::new().await;
    let original = fixture.store.project(&fixture.id).unwrap();
    let removed_first = original.root.join("app/src/disposable-before-failure.txt");
    fs::write(&removed_first, b"deleted before blocked top-level file").unwrap();
    let blocked = original.root.join("zz-disposable-blocked.txt");
    fs::write(&blocked, b"retained until explicit retry").unwrap();
    let immutable = ImmutableFile::new(&blocked);
    let failed = fixture.project("delete", json!({})).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap().details["operationOutcome"],
        "delete_incomplete"
    );
    assert!(!removed_first.exists());
    assert!(blocked.exists());
    assert_eq!(
        original.check().unwrap_err().code,
        "PROJECT_DELETE_INCOMPLETE"
    );
    let fixture = fixture.restart_task(failed).await;
    assert_eq!(
        fs::read(&blocked).unwrap(),
        b"retained until explicit retry"
    );
    drop(immutable);
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!original.root.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn deleted_task_receipt_is_recovered_after_response_loss_without_leaving_any_task() {
    let fixture = Fixture::new().await;
    let root = fixture.store.project(&fixture.id).unwrap().root;
    let gate = fixture.store.gate(&fixture.id);
    let writer = gate.read_owned().await;
    let task = fixture
        .lifecycle
        .submit_project(
            &fixture.id,
            "delete",
            json!({"expectedDocumentEpoch":fixture.store.registered_epoch(&fixture.id).unwrap()}),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    assert!(root.exists());
    let waiting = fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .unwrap();
    assert!(!waiting.status.is_final());
    let manager = fixture.lifecycle.clone();
    let task_id = task.task_id.clone();
    let project_id = fixture.id.clone();
    let waiter = tokio::spawn(async move {
        loop {
            if manager
                .task_for_project(&task_id, &project_id, "delete")
                .unwrap()
                .status
                .is_final()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
    waiter.abort();
    drop(writer);
    let done = fixture.wait(&task).await;
    assert_eq!(good(&done)["operationOutcome"], "deleted");
    assert!(fixture.tasks.task(&task.task_id).is_err());
    assert!(!root.exists());
    let receipt = fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .unwrap();
    assert_eq!(good(&receipt)["alreadyAbsent"], true);
    assert!(fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "build")
        .is_err());
    let fixture = fixture.restart_saved().await;
    let receipt = fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .unwrap();
    assert_eq!(good(&receipt)["alreadyAbsent"], true);
    fixture.stop().await;
}

#[tokio::test]
async fn finalizing_success_is_hidden_and_cleanup_failure_is_retryable() {
    let fixture = Fixture::new().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *fixture.lifecycle.delete_cleanup_checkpoint.lock().unwrap() =
        Some((entered_tx, release_rx, true));
    let task = fixture
        .lifecycle
        .submit_project(
            &fixture.id,
            "delete",
            json!({"expectedDocumentEpoch":fixture.store.registered_epoch(&fixture.id).unwrap()}),
        )
        .await
        .unwrap();
    entered_rx.await.unwrap();
    assert_eq!(
        fixture.tasks.task(&task.task_id).unwrap().status,
        TaskStatus::Succeeded
    );
    let pending = fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .unwrap();
    assert_eq!(pending.status, TaskStatus::Running);
    assert!(pending.result.is_none());
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_some());
    release_tx.send(()).unwrap();
    let failed = fixture.wait(&task).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap().details["finalCleanupPending"],
        true
    );
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(fixture.tasks.task(&task.task_id).is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn persisted_success_before_purge_is_failure_after_restart_until_explicit_finalization() {
    let fixture = Fixture::new().await;
    let (root, identity) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let epoch = Uuid::new_v4().to_string();
    fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &epoch, &identity)
        .unwrap();
    crate::history_cleanup::remove_owned_directory(&root, &identity).unwrap();
    fixture
        .tasks
        .finish(
            &task.task_id,
            Ok(json!({"projectId":fixture.id,"operationOutcome":"deleted"})),
        )
        .unwrap();
    drop(lease);
    let fixture = fixture.restart_saved().await;
    let projected = fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .unwrap();
    assert_eq!(projected.status, TaskStatus::Failed);
    assert_eq!(projected.error.unwrap().code, "PROJECT_DELETE_INCOMPLETE");
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(fixture.tasks.task(&task.task_id).is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn existing_task_context_must_match_and_pending_recovery_is_deleted_as_material() {
    let fixture = Fixture::new().await;
    let root = fixture.store.project(&fixture.id).unwrap().root;
    let task = fixture
        .tasks
        .create_readonly_operation(&fixture.id, "storageScan")
        .unwrap();
    fixture.tasks.finish(&task.task_id, Ok(json!({}))).unwrap();
    assert_eq!(
        fixture
            .lifecycle
            .task_for_project(&task.task_id, &fixture.id, "delete")
            .unwrap_err()
            .code,
        "TASK_IDENTITY_CHANGED"
    );
    assert_eq!(
        fixture
            .lifecycle
            .task_for_project(
                &task.task_id,
                "822f8111-8b3e-555d-a894-82c65df0c867",
                "storageScan"
            )
            .unwrap_err()
            .code,
        "TASK_IDENTITY_CHANGED"
    );
    for relative in [
        ".platform/project-state/source-restore",
        ".platform/project-state/data-restore",
    ] {
        let path = fixture
            .paths
            .workspace_root()
            .join(relative)
            .join(&fixture.id)
            .join("pending.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"disposable pending recovery material").unwrap();
    }
    fixture
        .store
        .control(&fixture.id)
        .unwrap()
        .mark_recovery_required(ApiError::new("DISPOSABLE_RECOVERY", "pending fixture"));
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!root.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn cleanup_only_retry_removes_a_terminal_task_after_identity_and_owner_are_gone() {
    let fixture = Fixture::new().await;
    let (root, identity) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let epoch = Uuid::new_v4().to_string();
    let marker = fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &epoch, &identity)
        .unwrap();
    crate::history_cleanup::remove_owned_directory(&root, &identity).unwrap();
    fixture
        .tasks
        .finish(
            &task.task_id,
            Ok(json!({"projectId":fixture.id,"operationOutcome":"deleted"})),
        )
        .unwrap();
    drop(lease);
    crate::web_project_cleanup::remove_project_materials(
        &fixture.paths,
        &fixture.store,
        &fixture.tasks,
        &fixture.id,
        &task.task_id,
    )
    .unwrap();
    fixture
        .store
        .complete_deleted_project_identity(&marker)
        .unwrap();
    crate::web_project_cleanup::finish_project_ownership(&fixture.paths, &fixture.id).unwrap();
    let fixture = fixture.restart_saved().await;
    assert!(fixture
        .store
        .deleted_project_identity_absent(&fixture.id)
        .unwrap());
    assert_eq!(
        fixture
            .lifecycle
            .task_for_project(&task.task_id, &fixture.id, "delete")
            .unwrap()
            .status,
        TaskStatus::Failed
    );
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(fixture.tasks.task(&task.task_id).is_err());
    assert!(crate::web_project_cleanup::project_materials_absent(
        &fixture.paths,
        &fixture.tasks,
        &fixture.id
    )
    .unwrap());
    fixture.stop().await;
}

#[tokio::test]
async fn partial_own_task_directory_is_removed_after_restart_using_accepted_owner_identity() {
    let fixture = Fixture::new().await;
    let (root, identity) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let epoch = Uuid::new_v4().to_string();
    let marker = fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &epoch, &identity)
        .unwrap();
    crate::history_cleanup::remove_owned_directory(&root, &identity).unwrap();
    fixture
        .tasks
        .finish(
            &task.task_id,
            Ok(json!({"projectId":fixture.id,"operationOutcome":"deleted"})),
        )
        .unwrap();
    drop(lease);
    crate::web_project_cleanup::remove_project_materials(
        &fixture.paths,
        &fixture.store,
        &fixture.tasks,
        &fixture.id,
        &task.task_id,
    )
    .unwrap();
    fixture
        .store
        .complete_deleted_project_identity(&marker)
        .unwrap();
    let task_dir = fixture
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&task.task_id);
    fs::remove_file(task_dir.join("task.json")).unwrap();
    fs::write(
        task_dir.join("partial-tail"),
        b"remaining accepted task directory",
    )
    .unwrap();
    let fixture = fixture.restart_saved().await;
    assert!(fixture.tasks.task(&task.task_id).is_err());
    assert!(fixture
        .lifecycle
        .task_for_project(&task.task_id, &fixture.id, "delete")
        .is_err());
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!task_dir.exists());
    assert!(crate::web_project_cleanup::project_materials_absent(
        &fixture.paths,
        &fixture.tasks,
        &fixture.id
    )
    .unwrap());
    fixture.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn owner_last_removal_failure_restores_failed_task_and_can_retry_after_restart() {
    let fixture = Fixture::new().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    *fixture.lifecycle.delete_cleanup_checkpoint.lock().unwrap() =
        Some((entered_tx, release_rx, false));
    let task = fixture
        .lifecycle
        .submit_project(
            &fixture.id,
            "delete",
            json!({"expectedDocumentEpoch":fixture.store.registered_epoch(&fixture.id).unwrap()}),
        )
        .await
        .unwrap();
    entered_rx.await.unwrap();
    let owner_dir = crate::web_project_ownership::WebProjectOwnership::new(
        fixture.paths.data_root().into(),
        fixture.paths.workspace_root().into(),
    )
    .directory(&fixture.id)
    .unwrap();
    let blocked = owner_dir.join("zz-disposable-owner-tail");
    fs::write(&blocked, b"blocked only during this fixture").unwrap();
    let immutable = ImmutableFile::new(&blocked);
    release_tx.send(()).unwrap();
    let failed = fixture.wait(&task).await;
    assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
    assert_eq!(
        failed.error.as_ref().unwrap().details["finalCleanupPending"],
        true
    );
    assert!(fixture
        .store
        .deleted_project_identity_absent(&fixture.id)
        .unwrap());
    assert!(owner_dir.exists());
    assert!(fixture.tasks.task(&task.task_id).is_ok());
    drop(immutable);
    let fixture = fixture.restart_saved().await;
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!owner_dir.exists());
    assert!(fixture.tasks.task(&task.task_id).is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn corrupt_maintenance_json_and_database_do_not_block_confirmed_full_deletion() {
    let fixture = Fixture::new().await;
    let project = fixture.store.project(&fixture.id).unwrap();
    let root = project.root.clone();
    let control = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/maintenance")
        .join(format!("{}.json", fixture.id));
    fs::create_dir_all(control.parent().unwrap()).unwrap();
    fs::write(&control, b"disposable unreadable maintenance JSON").unwrap();
    fs::write(
        root.join("data/main.sqlite"),
        b"disposable corrupt database",
    )
    .unwrap();
    let fixture = fixture.restart_saved().await;
    assert_eq!(
        fixture.store.control(&fixture.id).err().unwrap().code,
        "INVALID_JSON"
    );
    let volatile = fixture.store.control_for_deletion(&fixture.id).unwrap();
    assert_eq!(volatile.get().mode, "unknown");
    assert_eq!(
        fixture.store.control(&fixture.id).err().unwrap().code,
        "INVALID_JSON"
    );
    let deleted = fixture
        .project(
            "delete",
            json!({"expectedDocumentEpoch":project.document_epoch}),
        )
        .await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!root.exists());
    assert!(!control.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn same_boot_unconfirmed_web_writer_blocks_root_deletion_without_guessing_pid_liveness() {
    let fixture = Fixture::new().await;
    let root = fixture.store.project(&fixture.id).unwrap().root;
    let owner =
        crate::durable_writer::WriterOwner::web(fixture.paths.data_root(), &fixture.id).unwrap();
    let marker = owner.fixture_mark_unconfirmed().unwrap();
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(deleted.status, TaskStatus::Failed);
    assert_eq!(
        deleted.error.as_ref().unwrap().code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    assert!(root.exists());
    assert!(marker.exists());
    assert!(fixture
        .store
        .pending_deletion(&fixture.id)
        .unwrap()
        .is_none());
    fixture.stop().await;
}
