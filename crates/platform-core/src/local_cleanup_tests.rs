//! Artificial TempDir roots only. These tests never resolve the installed app's
//! default data root or touch existing development projects.
use super::*;
const CONTROL: &str = ".local-cleanup";
use crate::{CreateProjectInput, TaskStatus};
use std::os::unix::{fs::symlink, fs::MetadataExt};

async fn fixture() -> (tempfile::TempDir, RuntimePaths, RuntimeService) {
    let (temporary, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    (temporary, paths, service)
}

async fn owned_export_fixture() -> (
    tempfile::TempDir,
    RuntimePaths,
    RuntimeService,
    crate::web_project_ownership::WebProjectOwnership,
    String,
    PathBuf,
    PersistedIdentity,
) {
    let (temporary, paths, service) = fixture().await;
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable uninstall export owner".into(),
            description: None,
        })
        .await
        .unwrap();
    let ownership = crate::web_project_ownership::WebProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    let id = project.id;
    let task = format!("task_{}", Uuid::new_v4().simple());
    ownership.record_task(&id, &task).unwrap();
    let parent = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-external-exports");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("owned-snapshot.zip");
    ownership
        .reserve_export_destination(&id, &task, &path)
        .unwrap();
    let stage = parent.join(format!(".export-{task}.partial"));
    ownership.reserve_export_stage(&id, &task, &stage).unwrap();
    fs::write(&stage, "disposable platform ZIP").unwrap();
    let identity = PersistedIdentity::capture(&stage).unwrap();
    ownership
        .capture_export_stage(&id, &task, &stage, &identity)
        .unwrap();
    ownership
        .bind_export_destination(&id, &task, &path, &stage)
        .unwrap();
    fs::rename(&stage, &path).unwrap();
    ownership.commit_export_destination(&id, &task).unwrap();
    ownership.forget_export_stage(&id, &task).unwrap();
    (temporary, paths, service, ownership, id, path, identity)
}

#[tokio::test]
async fn cleanup_removes_owned_external_export_before_owner_index_and_leaves_no_stage_or_claim() {
    let (_temporary, paths, service, ownership, id, path, _) = owned_export_fixture().await;
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(plan
        .web_owned_files
        .iter()
        .any(|file| file.path == path && file.project_id == id));
    assert!(service.quiesce().await.is_complete());
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap()["operationOutcome"],
        "deleted"
    );
    assert!(!path.exists());
    assert!(!ownership.directory(&id).unwrap().exists());
    assert!(!paths.data_root().join("web-export-owners").exists());
    assert!(fs::read_dir(path.parent().unwrap())
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
async fn cleanup_public_export_replacement_after_capture_is_preserved_and_keeps_all_owner_evidence()
{
    let (temporary, paths, service, ownership, id, path, _) = owned_export_fixture().await;
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    let held = temporary
        .path()
        .join("disposable-captured-original-kept.zip");
    assert!(service.quiesce().await.is_complete());
    let error = plan
        .execute_with(&service, |owner, project, candidate, expected| {
            if candidate == path {
                crate::web_file_deletion::fixture_remove_after_capture(
                    owner,
                    project,
                    candidate,
                    expected,
                    || {
                        fs::rename(&path, &held).unwrap();
                        fs::write(&path, "foreign replacement must remain").unwrap();
                        Ok(())
                    },
                )
            } else {
                crate::web_file_deletion::remove_owned_file(owner, project, candidate, expected)
            }
        })
        .unwrap_err();
    assert_eq!(error.code, "LOCAL_CLEANUP_INCOMPLETE");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "foreign replacement must remain"
    );
    assert_eq!(fs::read_to_string(held).unwrap(), "disposable platform ZIP");
    assert!(ownership.path(&id).unwrap().is_file());
    assert!(paths.workspace_root().exists());
    assert!(paths.data_root().join("settings.json").exists());
    assert!(paths.data_root().join("web-export-owners").is_dir());
    let plans = ownership.file_deletion_plans(&id).unwrap();
    assert_eq!(plans.len(), 1);
    assert!(plans[0].staging_path.is_dir());
    assert!(error.details["remainingPaths"]
        .as_array()
        .unwrap()
        .contains(&json!(plans[0].staging_path)));
}

#[tokio::test]
async fn cleanup_fresh_preview_resumes_bound_external_file_crash_snapshot_and_removes_every_plan_and_claim(
) {
    use crate::web_file_deletion::{FileDeletionPlan, FileDeletionState};
    use std::os::unix::fs::PermissionsExt;
    let (_temporary, paths, service, ownership, id, path, expected) = owned_export_fixture().await;
    let old_preview = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    drop(old_preview);
    let parent = path.parent().unwrap();
    let staging = parent.join(format!(".panda-delete-file-{}", Uuid::new_v4()));
    let mut interrupted = FileDeletionPlan {
        original: path.clone(),
        expected,
        parent_identity: PersistedIdentity::capture(parent).unwrap(),
        staging_path: staging.clone(),
        staging_identity: None,
        state: FileDeletionState::Preparing,
    };
    ownership
        .save_file_deletion_plan(&id, &interrupted)
        .unwrap();
    fs::create_dir(&staging).unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).unwrap();
    interrupted.staging_identity = Some(PersistedIdentity::capture(&staging).unwrap());
    interrupted.state = FileDeletionState::RenameIntent;
    ownership
        .save_file_deletion_plan(&id, &interrupted)
        .unwrap();
    fs::rename(&path, staging.join("payload")).unwrap();
    interrupted.state = FileDeletionState::Staged;
    ownership
        .save_file_deletion_plan(&id, &interrupted)
        .unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(plan.targets.iter().any(|target| target.path == staging));
    assert!(service.quiesce().await.is_complete());
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap()["operationOutcome"],
        "deleted"
    );
    assert!(!path.exists());
    assert!(!staging.exists());
    assert!(!ownership.directory(&id).unwrap().exists());
    assert!(!paths.data_root().join("web-export-owners").exists());
    assert!(fs::read_dir(parent).unwrap().next().is_none());
}

fn external_registration(paths: &RuntimePaths, source: &Path) -> PathBuf {
    let registration = paths
        .data_root()
        .join("node-apps")
        .join(Uuid::new_v4().to_string())
        .join("registration.json");
    fs::create_dir_all(registration.parent().unwrap()).unwrap();
    fs::write(&registration, serde_json::to_vec(&json!({"version":1,"id":registration.parent().unwrap().file_name().unwrap().to_str().unwrap(),
        "sourcePath":source,"manifestDigest":"a".repeat(64)})).unwrap()).unwrap();
    registration
}

fn development_record(source: &Path) -> Value {
    let text = serde_json::to_string(
        &json!({"runtime":"node-service","app":{"id":"disposable-author-app"}}),
    )
    .unwrap();
    json!({"sourceRoot":source,"manifestId":"disposable-author-app","rootIdentity":PersistedIdentity::capture(source).unwrap(),
        "registeredManifestText":text,"registeredManifestDigest":format!("{:x}",Sha256::digest(text.as_bytes()))})
}

fn source_record(source: &Path, id: &str) -> Value {
    json!({"schemaVersion":1,"sourceId":id,"taskId":format!("task_{}",Uuid::new_v4().simple()),
        "name":"Disposable source capture","status":"ready","phase":"completed","url":"","branch":null,
        "provider":"local","localSourcePath":source,"capturePolicy":"local-source-v1","createdAt":1,
        "repository":"repository","shallow":false})
}

#[tokio::test]
async fn cleanup_removes_project_code_data_node_configuration_and_history_but_preserves_external_sources_and_lock(
) {
    let (temporary, paths, service) = fixture().await;
    let original = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("external-original-source");
    fs::create_dir(&original).unwrap();
    fs::write(original.join("author.txt"), b"keep original author source").unwrap();
    let registration = external_registration(&paths, &original);
    let node_slot = registration.parent().unwrap();
    for name in [
        "generations",
        "configuration/credentials",
        "data",
        "cache",
        "logs",
        "runs",
    ] {
        fs::create_dir_all(node_slot.join(name)).unwrap();
        fs::write(
            node_slot.join(name).join("owned.txt"),
            b"disposable owned payload",
        )
        .unwrap();
    }
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable cleanup project".into(),
            description: None,
        })
        .await
        .unwrap();
    fs::write(
        project.project_root.join("app/disposable.txt"),
        b"disposable project source",
    )
    .unwrap();
    symlink(
        &original,
        project.project_root.join("data/attachments/external-link"),
    )
    .unwrap();
    for name in [
        "backups/retained",
        ".platform/trash/legacy",
        ".platform/operations/recovery",
    ] {
        fs::create_dir_all(paths.workspace_root().join(name)).unwrap();
        fs::write(
            paths.workspace_root().join(name).join("owned.txt"),
            b"disposable retained evidence",
        )
        .unwrap();
    }
    let lock_before = fs::metadata(paths.data_root().join("instance.lock"))
        .unwrap()
        .ino();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(plan.preview()["confirmationText"], CONFIRMATION);
    assert_eq!(plan.preview()["preservedPaths"], json!([original]));
    assert!(
        !paths.data_root().join(CONTROL).exists(),
        "preview is read-only"
    );
    assert!(service.quiesce().await.is_complete());
    let result = plan.execute_after_shutdown(&service).unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    assert_eq!(
        fs::read(original.join("author.txt")).unwrap(),
        b"keep original author source"
    );
    assert_eq!(
        fs::metadata(paths.data_root().join("instance.lock"))
            .unwrap()
            .ino(),
        lock_before
    );
    assert!(!project.project_root.exists());
    assert!(!registration.exists());
    assert!(!paths.data_root().join("settings.json").exists());
    let names = fs::read_dir(paths.data_root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, RETAINED.into_iter().map(String::from).collect());
    assert!(!paths.data_root().join(CONTROL).exists());
    let reinitialized = paths.reinitialize_for_test().unwrap();
    assert_eq!(
        reinitialized.workspace_root(),
        paths.data_root().join("workspace")
    );
    assert!(
        reinitialized.acquire_lock().is_err(),
        "the original instance lock remains held"
    );
}

#[tokio::test]
async fn cleanup_requires_actual_runtime_shutdown_and_matching_service() {
    let (_temporary, paths, service) = fixture().await;
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(plan.execute_after_shutdown(&service).is_err());
    assert!(paths.workspace_root().exists());
    assert!(!paths.data_root().join(CONTROL).exists());
    let (_other_temp, _other_paths, other) = fixture().await;
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(other.quiesce().await.is_complete());
    assert!(plan.execute_after_shutdown(&other).is_err());
    assert!(service.quiesce().await.is_complete());
}

#[tokio::test]
async fn cleanup_removes_owned_generated_roots_and_external_restore_material_but_keeps_author_git()
{
    use crate::node_project_ownership::NodeProjectOwnership;
    let (temporary, paths, service) = fixture().await;
    let ownership =
        NodeProjectOwnership::new(paths.data_root().into(), paths.workspace_root().into());
    let generated = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-generated-outside-workspace");
    fs::create_dir(&generated).unwrap();
    let registration = external_registration(&paths, &generated);
    let generated_id = registration
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    ownership.initialize(generated_id).unwrap();
    ownership
        .record_generated_source(generated_id, &generated)
        .unwrap();
    fs::create_dir(generated.join(".git")).unwrap();
    fs::write(generated.join(".git/HEAD"), "generated Git is deleted").unwrap();
    fs::write(generated.join("uncommitted.js"), "AI changes are deleted").unwrap();
    let author = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-original-outside-workspace");
    fs::create_dir_all(author.join(".git")).unwrap();
    fs::write(author.join(".git/HEAD"), "ordinary author Git remains").unwrap();
    fs::write(author.join("source.js"), "original source remains").unwrap();
    let registration = external_registration(&paths, &author);
    let id = registration
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    ownership.initialize(id).unwrap();
    crate::node_apps::development::claim_source_after_verified_migration(
        paths.data_root(),
        paths.workspace_root(),
        id,
        &author,
    )
    .unwrap();
    ownership.record_source_restore(id, &author).unwrap();
    for name in [
        "local-projects-node-restores",
        "local-projects-node-restore-preparing",
    ] {
        let path = author.join(".git").join(name);
        fs::create_dir(&path).unwrap();
        ownership
            .capture_source_restore_directory(
                id,
                &author,
                name,
                &PersistedIdentity::capture(&path).unwrap(),
            )
            .unwrap();
        fs::write(path.join("payload"), "platform recovery material").unwrap();
    }
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(plan.preview()["preservedPaths"], json!([author]));
    assert!(plan.targets.iter().any(|target| target.path == generated));
    assert!(plan
        .targets
        .iter()
        .any(|target| target.path == author.join(".git/local-projects-node-restores")));
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!generated.exists());
    assert!(!author.join(".git/local-projects-node-restores").exists());
    assert!(!author
        .join(".git/local-projects-node-restore-preparing")
        .exists());
    assert_eq!(
        fs::read_to_string(author.join(".git/HEAD")).unwrap(),
        "ordinary author Git remains"
    );
    assert_eq!(
        fs::read_to_string(author.join("source.js")).unwrap(),
        "original source remains"
    );
}

#[tokio::test]
async fn expired_preview_does_not_remove_data_or_create_cleanup_records() {
    let (_temporary, paths, service) = fixture().await;
    let mut plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    plan.created = Instant::now() - TTL;
    assert!(service.quiesce().await.is_complete());
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap_err().code,
        "LOCAL_CLEANUP_EXPIRED"
    );
    assert!(paths.workspace_root().exists());
    assert!(!paths.data_root().join(CONTROL).exists());
}

#[tokio::test]
async fn changed_root_or_new_top_level_content_invalidates_the_reviewed_scope() {
    for replace in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
        assert!(service.quiesce().await.is_complete());
        if replace {
            fs::rename(
                paths.workspace_root(),
                temporary.path().join("original-workspace"),
            )
            .unwrap();
            fs::create_dir_all(paths.workspace_root().join("projects")).unwrap();
            fs::create_dir_all(paths.workspace_root().join("backups")).unwrap();
            fs::create_dir_all(paths.workspace_root().join(".platform")).unwrap();
        } else {
            fs::write(paths.data_root().join("new-unreviewed-file"), b"retain").unwrap();
        }
        assert!(plan.execute_after_shutdown(&service).is_err());
        assert!(paths.workspace_root().exists());
        assert!(!paths.data_root().join(CONTROL).exists());
    }
}

#[tokio::test]
async fn changed_external_registration_or_development_binding_invalidates_scope() {
    for development in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("author-source");
        fs::create_dir(&source).unwrap();
        let registration = external_registration(&paths, &source);
        let target = if development {
            registration
                .parent()
                .unwrap()
                .join("development-source.json")
        } else {
            registration.clone()
        };
        if development {
            fs::write(
                &target,
                serde_json::to_vec(&development_record(&source)).unwrap(),
            )
            .unwrap();
        }
        let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
        assert!(service.quiesce().await.is_complete());
        fs::write(
            &target,
            serde_json::to_vec(&json!({"sourcePath":source,"changed":true})).unwrap(),
        )
        .unwrap();
        assert!(plan.execute_after_shutdown(&service).is_err());
        assert!(registration.exists());
        assert!(paths.workspace_root().exists());
    }
}

#[tokio::test]
async fn top_level_symlink_and_special_file_refuse_preview_without_following_them() {
    for special in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let external = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("keep-external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("keep"), b"original").unwrap();
        let bad = paths.data_root().join("unknown-owned-entry");
        if special {
            let path = CString::new(bad.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        } else {
            symlink(&external, &bad).unwrap();
        }
        assert!(LocalCleanupPlan::prepare(&paths, &service).is_err());
        assert_eq!(fs::read(external.join("keep")).unwrap(), b"original");
        fs::remove_file(bad).unwrap();
        assert!(service.quiesce().await.is_complete());
    }
}

async fn migrate(
    paths: &RuntimePaths,
    service: &RuntimeService,
    destination: &Path,
) -> RuntimePaths {
    fs::create_dir(destination).unwrap();
    let handle = service.handle();
    let task = handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let id = task["taskId"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let task: crate::TaskRecord =
                serde_json::from_value(handle.task(id).await.unwrap()).unwrap();
            if task.status.is_final() {
                assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    paths.reinitialize_for_test().unwrap()
}

#[tokio::test]
async fn cleanup_includes_current_and_retained_completed_migration_roots() {
    let (temporary, paths, service) = fixture().await;
    fs::write(
        paths.workspace_root().join("original-project-code"),
        b"disposable original",
    )
    .unwrap();
    let destination = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("migrated-workspace");
    let selected = migrate(&paths, &service, &destination).await;
    let second = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("second-migrated-workspace");
    let selected = migrate(&selected, &service, &second).await;
    let plan = LocalCleanupPlan::prepare(&selected, &service).unwrap();
    assert!(plan.targets.iter().any(|target| target.path == destination));
    assert!(plan.targets.iter().any(|target| target.path == second));
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!destination.exists());
    assert!(!second.exists());
    assert!(!paths.workspace_root().exists());
    assert!(selected.reinitialize_for_test().is_ok());
}

#[tokio::test]
async fn absent_same_volume_root_is_rescanned_but_replaced_or_offline_root_blocks_cleanup() {
    let (temporary, paths, service) = fixture().await;
    let destination = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("migrated-workspace");
    let selected = migrate(&paths, &service, &destination).await;
    let saved = temporary.path().join("saved-retained-source");
    fs::rename(paths.workspace_root(), &saved).unwrap();
    let plan = LocalCleanupPlan::prepare(&selected, &service).unwrap();
    assert!(!plan.targets.iter().any(|target| target.path == saved));
    assert!(saved.exists());
    fs::create_dir(paths.workspace_root()).unwrap();
    assert!(LocalCleanupPlan::prepare(&selected, &service).is_err());
    fs::remove_dir(paths.workspace_root()).unwrap();
    let journal_path = fs::read_dir(selected.migrations_dir())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("journal.json");
    let mut journal: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    journal["sourceIdentity"]["volumeUuid"] = json!(Uuid::new_v4().to_string());
    crate::builds::atomic_json(&journal_path, &journal).unwrap();
    assert!(LocalCleanupPlan::prepare(&selected, &service).is_err());
    assert!(destination.exists());
    assert!(saved.exists());
    assert!(!selected.data_root().join(CONTROL).exists());
    assert!(service.quiesce().await.is_complete());
}

#[tokio::test]
async fn external_author_source_within_migrated_workspace_or_symlink_alias_blocks_cleanup() {
    for alias in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let destination = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("migrated-workspace");
        let selected = migrate(&paths, &service, &destination).await;
        let author = destination.join("author-original-source");
        fs::create_dir(&author).unwrap();
        let source = if alias {
            let alias = temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("alias-to-author");
            symlink(&author, &alias).unwrap();
            alias
        } else {
            author.clone()
        };
        external_registration(&selected, &source);
        assert!(LocalCleanupPlan::prepare(&selected, &service).is_err());
        assert!(author.exists());
        assert!(service.quiesce().await.is_complete());
    }
}

#[tokio::test]
async fn unfinished_migration_is_not_deleted_as_an_unknown_external_root() {
    let (temporary, paths, service) = fixture().await;
    let destination = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("empty-candidate");
    fs::create_dir(&destination).unwrap();
    crate::workspace::create(&paths, &destination).unwrap();
    assert!(LocalCleanupPlan::prepare(&paths, &service).is_err());
    assert!(destination.exists());
    assert!(paths.workspace_root().exists());
    assert!(service.quiesce().await.is_complete());
}

#[tokio::test]
async fn moved_external_alias_after_preview_cannot_turn_preserved_source_into_deleted_content() {
    let (temporary, paths, service) = fixture().await;
    let destination = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("migrated-workspace");
    let selected = migrate(&paths, &service, &destination).await;
    let original = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("external-original");
    fs::create_dir(&original).unwrap();
    let alias = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("source-alias");
    symlink(&original, &alias).unwrap();
    external_registration(&selected, &alias);
    let plan = LocalCleanupPlan::prepare(&selected, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    fs::remove_file(&alias).unwrap();
    symlink(&destination, &alias).unwrap();
    assert!(plan.execute_after_shutdown(&service).is_err());
    assert!(destination.exists());
    assert!(!selected.data_root().join(CONTROL).exists());
}

#[tokio::test]
async fn original_source_capture_and_separate_development_bindings_are_preserved_and_bound() {
    let (temporary, paths, service) = fixture().await;
    let original = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("source-original");
    fs::create_dir(&original).unwrap();
    let source_id = format!("src_{}", Uuid::new_v4().simple());
    let source_record_path = paths
        .workspace_root()
        .join(".platform/sources")
        .join(&source_id)
        .join("source.json");
    fs::create_dir_all(source_record_path.parent().unwrap()).unwrap();
    fs::write(
        &source_record_path,
        serde_json::to_vec(&source_record(&original, &source_id)).unwrap(),
    )
    .unwrap();
    let binding = paths
        .data_root()
        .join("node-apps")
        .join(Uuid::new_v4().to_string())
        .join("development-source.json");
    fs::create_dir_all(binding.parent().unwrap()).unwrap();
    fs::write(
        &binding,
        serde_json::to_vec(&development_record(&original)).unwrap(),
    )
    .unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(plan.preview()["preservedPaths"], json!([original]));
    assert!(service.quiesce().await.is_complete());
    fs::write(source_record_path, b"{}").unwrap();
    assert!(plan.execute_after_shutdown(&service).is_err());
    assert!(original.exists());
}

#[tokio::test]
async fn partial_cleanup_reports_remaining_paths_without_writing_failure_records() {
    let (_temporary, paths, service) = fixture().await;
    let bad = paths.workspace_root().join("special-unhandled-file");
    let name = CString::new(bad.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    let error = plan.execute_after_shutdown(&service).unwrap_err();
    assert_eq!(error.code, "LOCAL_CLEANUP_INCOMPLETE");
    assert_eq!(error.details["operationOutcome"], "delete_incomplete");
    assert!(error.details["remainingPaths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == &json!(paths.workspace_root())));
    assert!(bad.exists());
    assert!(!paths.data_root().join(CONTROL).exists());
    assert!(paths.data_root().join("instance.lock").exists());
    fs::remove_file(bad).unwrap();
}

#[tokio::test]
async fn explicitly_reviewed_legacy_cleanup_folder_is_removed_without_creating_a_new_receipt() {
    let (_temporary, paths, service) = fixture().await;
    let control = paths.data_root().join(CONTROL);
    fs::create_dir(&control).unwrap();
    fs::write(control.join("receipt.json"), b"legacy cleanup receipt").unwrap();
    fs::write(control.join("old.ndjson"), b"old per-file project names").unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!control.exists());
}

#[tokio::test]
async fn malformed_or_unknown_reference_records_are_never_treated_as_having_no_external_source() {
    for variant in 0..7 {
        let (_temporary, paths, service) = fixture().await;
        let slot = paths
            .data_root()
            .join("node-apps")
            .join(Uuid::new_v4().to_string());
        let source_id = format!("src_{}", Uuid::new_v4().simple());
        let path = match variant {
            0..=2 => slot.join("registration.json"),
            3 => slot.join("development-source.json"),
            4..=5 => paths
                .workspace_root()
                .join(".platform/sources")
                .join(&source_id)
                .join("source.json"),
            _ => paths
                .data_root()
                .join("node-apps/.installations")
                .join(format!("install_{}", Uuid::new_v4().simple()))
                .join("install.json"),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let value = match variant {
            1 => {
                json!({"version":1,"id":slot.file_name().unwrap().to_str().unwrap(),"manifestDigest":"a".repeat(64)})
            }
            2 => {
                json!({"version":2,"kind":"managed","id":slot.file_name().unwrap().to_str().unwrap()})
            }
            5 => json!({"schemaVersion":1,"sourceId":source_id,"provider":"local"}),
            _ => json!({}),
        };
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            LocalCleanupPlan::prepare(&paths, &service).is_err(),
            "variant {variant}"
        );
        assert!(path.exists());
        assert!(paths.workspace_root().exists());
        assert!(!paths.data_root().join(CONTROL).exists());
        assert!(service.quiesce().await.is_complete());
    }
}

#[tokio::test]
async fn historical_installation_snapshots_still_protect_external_original_code_after_registration_is_removed(
) {
    for overlaps in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let destination = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("migrated-workspace");
        let selected = migrate(&paths, &service, &destination).await;
        let original = if overlaps {
            destination.join("historical-author-source")
        } else {
            temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("historical-author-source")
        };
        fs::create_dir(&original).unwrap();
        fs::write(original.join("original-code"), b"preserve original").unwrap();
        let binding = development_record(&original);
        let app_id = Uuid::new_v4().to_string();
        let previous = json!({"version":1,"id":app_id,"sourcePath":original,"manifestDigest":binding["registeredManifestDigest"]});
        let install_id = format!("install_{}", Uuid::new_v4().simple());
        let path = selected
            .data_root()
            .join("node-apps/.installations")
            .join(&install_id)
            .join("install.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = json!({"appId":app_id,"registrationDigest":"a".repeat(64),"generation":null,
            "manifest":serde_json::from_str::<Value>(binding["registeredManifestText"].as_str().unwrap()).unwrap(),
            "configurationRevision":0,"externalRegistration":previous,"developmentSource":binding});
        fs::write(&path,serde_json::to_vec(&json!({"version":1,"candidateId":install_id,"prepareTaskId":format!("task_{}",Uuid::new_v4().simple()),
            "applyTaskId":null,"state":"committed","target":target,"prepared":null,"registration":null,"error":null})).unwrap()).unwrap();
        let plan = LocalCleanupPlan::prepare(&selected, &service);
        assert!(service.quiesce().await.is_complete());
        if overlaps {
            assert!(plan.is_err());
        } else {
            let plan = plan.unwrap();
            assert_eq!(plan.preview()["preservedPaths"], json!([original]));
            plan.execute_after_shutdown(&service).unwrap();
        }
        assert_eq!(
            fs::read(original.join("original-code")).unwrap(),
            b"preserve original"
        );
    }
}

#[tokio::test]
async fn valid_managed_v2_registration_remains_owned_cleanup_content() {
    let (_temporary, paths, service) = fixture().await;
    let app_id = Uuid::new_v4().to_string();
    let generation = format!("gen_{}", Uuid::new_v4().simple());
    let slot = paths.data_root().join("node-apps").join(&app_id);
    fs::create_dir_all(slot.join("generations").join(&generation)).unwrap();
    let value = json!({"version":2,"id":app_id,"kind":"managed","generation":generation,"manifestDigest":"a".repeat(64),"programDigest":"b".repeat(64),
        "provenance":{"sourceId":format!("src_{}",Uuid::new_v4().simple()),"sourceDigest":"c".repeat(64),"sourceCommit":null,
            "node":{"path":"/disposable/node","version":"24.0.0"},"npm":{"path":"/disposable/npm","version":"11.0.0"},"preparedAt":1}});
    fs::write(
        slot.join("registration.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(plan.preview()["preservedPaths"], json!([]));
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!slot.exists());
}

#[tokio::test]
async fn retained_managed_install_history_is_cleanable_when_its_slot_and_generation_were_already_deleted(
) {
    let (_temporary, paths, service) = fixture().await;
    let app_id = Uuid::new_v4().to_string();
    let generation = format!("gen_{}", Uuid::new_v4().simple());
    let registration = json!({"version":2,"id":app_id,"kind":"managed","generation":generation,"manifestDigest":"a".repeat(64),"programDigest":"b".repeat(64),
        "provenance":{"sourceId":format!("src_{}",Uuid::new_v4().simple()),"sourceDigest":"c".repeat(64),"sourceCommit":null,
            "node":{"path":"/disposable/node","version":"24.0.0"},"npm":{"path":"/disposable/npm","version":"11.0.0"},"preparedAt":1}});
    let install_id = format!("install_{}", Uuid::new_v4().simple());
    let path = paths
        .data_root()
        .join("node-apps/.installations")
        .join(&install_id)
        .join("install.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path,serde_json::to_vec(&json!({"version":1,"candidateId":install_id,"prepareTaskId":format!("task_{}",Uuid::new_v4().simple()),
        "applyTaskId":null,"state":"committed","target":{"appId":app_id,"registrationDigest":null,"generation":null,"manifest":null,"configurationRevision":0},
        "prepared":null,"registration":registration,"error":null})).unwrap()).unwrap();
    assert!(!paths.data_root().join("node-apps").join(&app_id).exists());
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!path.exists());
}
