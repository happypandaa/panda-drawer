//! Artificial TempDir roots only. These tests never resolve the installed app's
//! default data root or touch existing development projects.
use super::*;
const CONTROL: &str = ".local-cleanup";
use crate::{CreateProjectInput, TaskStatus};
use std::os::unix::{fs::symlink, fs::MetadataExt};
use std::{ffi::CString, os::unix::ffi::OsStrExt};

async fn fixture() -> (tempfile::TempDir, RuntimePaths, RuntimeService) {
    let (temporary, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    (temporary, paths, service)
}

#[tokio::test]
async fn native_store_receipts_must_be_cleared_before_uninstall_can_remove_disk_authority() {
    let (_temporary, paths, service) = fixture().await;
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable native uninstall owner".into(),
            description: None,
        })
        .await
        .unwrap();
    let ledger = crate::webview_storage::StoreLedger::new(paths.data_root().into());
    ledger
        .record(crate::project_cleanup::ProjectKind::Web, &project.id)
        .unwrap();
    let blocked = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    let after_receipt = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    blocked.verify_before_native_cleanup(&service).unwrap();
    assert_eq!(
        blocked.execute_after_shutdown(&service).unwrap_err().code,
        "WEBVIEW_STORE_CLEANUP_REQUIRED"
    );
    assert!(project.project_root.exists());
    assert!(ledger
        .contains(crate::project_cleanup::ProjectKind::Web, &project.id)
        .unwrap());
    // No WK store is created by this core fixture; this simulates only the
    // separately tested native receipt, while exercising the real disk plan.
    ledger
        .forget_confirmed(crate::project_cleanup::ProjectKind::Web, &project.id)
        .unwrap();
    after_receipt.execute_after_shutdown(&service).unwrap();
    assert!(!project.project_root.exists());
    assert!(!ledger.directory().exists());
}

async fn owned_partial_fixture() -> (
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
            name: "Disposable uninstall partial owner".into(),
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
    let path = parent.join(format!(".export-{task}.partial"));
    let stage = path.clone();
    ownership.reserve_export_stage(&id, &task, &stage).unwrap();
    fs::write(&stage, []).unwrap();
    let identity = PersistedIdentity::capture(&stage).unwrap();
    ownership
        .capture_export_stage(&id, &task, &stage, &identity)
        .unwrap();
    fs::write(&stage, "disposable platform ZIP").unwrap();
    (temporary, paths, service, ownership, id, path, identity)
}

#[tokio::test]
async fn cleanup_preserves_a_delivered_user_zip_and_does_not_inspect_its_replacement() {
    let (_temporary, paths, service, ownership, id, stage, _) = owned_partial_fixture().await;
    let task = stage
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix(".export-")
        .unwrap()
        .strip_suffix(".partial")
        .unwrap();
    let delivered = stage.with_file_name("user-delivered.zip");
    fs::rename(&stage, &delivered).unwrap();
    ownership.forget_export_stage(&id, task).unwrap();
    let owner_json = fs::read_to_string(ownership.path(&id).unwrap()).unwrap();
    assert!(!owner_json.contains(delivered.to_str().unwrap()));
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert!(!plan
        .web_owned_files
        .iter()
        .any(|file| file.path == delivered));
    assert!(!plan.targets.iter().any(|target| target.path == delivered));
    fs::write(
        &delivered,
        b"user replacement stays outside platform cleanup",
    )
    .unwrap();
    assert!(service.quiesce().await.is_complete());
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap()["operationOutcome"],
        "deleted"
    );
    assert_eq!(
        fs::read(&delivered).unwrap(),
        b"user replacement stays outside platform cleanup"
    );
    assert!(!ownership.directory(&id).unwrap().exists());
}

#[tokio::test]
async fn cleanup_removes_owned_external_partial_before_owner_index_and_leaves_no_stage() {
    let (_temporary, paths, service, ownership, id, path, _) = owned_partial_fixture().await;
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
async fn cleanup_public_partial_replacement_after_capture_is_preserved_and_keeps_all_owner_evidence(
) {
    let (temporary, paths, service, ownership, id, path, _) = owned_partial_fixture().await;
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
    let plans = ownership.file_deletion_plans(&id).unwrap();
    assert_eq!(plans.len(), 1);
    assert!(plans[0].staging_path.is_dir());
    assert!(error.details["remainingPaths"]
        .as_array()
        .unwrap()
        .contains(&json!(plans[0].staging_path)));
}

#[tokio::test]
async fn cleanup_fresh_preview_resumes_bound_external_partial_crash_and_removes_every_plan() {
    use crate::web_file_deletion::{FileDeletionPlan, FileDeletionState};
    use std::os::unix::fs::PermissionsExt;
    let (_temporary, paths, service, ownership, id, path, expected) = owned_partial_fixture().await;
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

fn prepare_node_source(source: &Path) {
    fs::create_dir_all(source.join("web")).unwrap();
    fs::create_dir_all(source.join("backend")).unwrap();
    fs::write(
        source.join("web/index.html"),
        "<p>Disposable cleanup fixture</p>",
    )
    .unwrap();
    fs::write(
        source.join("backend/main.mjs"),
        "throw Error('inspection must not execute')",
    )
    .unwrap();
    fs::write(
        source.join("panda.json"),
        serde_json::to_vec(&json!({
            "manifestVersion":1,"runtime":"node","app":{"name":"Disposable cleanup source"},
            "web":{"root":"web","entry":"index.html"},
            "backend":{"root":"backend","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        }))
        .unwrap(),
    )
    .unwrap();
}

async fn external_registration(paths: &RuntimePaths, source: &Path) -> PathBuf {
    prepare_node_source(source);
    let apps = crate::node_apps::NodeApps::new(
        paths.data_root().into(),
        "/disposable/nonexistent-node".into(),
    )
    .unwrap();
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let registered = apps.register(source.into()).await.unwrap();
    paths
        .data_root()
        .join("node-apps")
        .join(registered["id"].as_str().unwrap())
        .join("registration.json")
}

fn source_record(id: &str, owner: &str) -> Value {
    json!({"schemaVersion":1,"sourceId":id,"ownerAppId":owner,"taskId":format!("task_{}",Uuid::new_v4().simple()),
        "name":"Disposable source capture","status":"ready","phase":"completed","url":"https://github.com/disposable/source","branch":null,
        "provider":"github","createdAt":1,"completedAt":1,"repository":"repository","shallow":false,
        "git":null,"commit":null,"fileCount":0,"totalBytes":0,"contentDigest":null,"inspection":null,"error":null})
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
    let registration = external_registration(&paths, &original).await;
    let node_slot = registration.parent().unwrap();
    for name in ["configuration/credentials", "cache", "logs", "runs"] {
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
        project.project_root.join("disposable.txt"),
        b"disposable project source",
    )
    .unwrap();
    service
        .store()
        .project(&project.id)
        .unwrap()
        .ensure_attachments_dir()
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
    fs::create_dir(original.join("data")).unwrap();
    fs::write(original.join("data/business.txt"), b"keep business file").unwrap();
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
        fs::read(original.join("data/business.txt")).unwrap(),
        b"keep business file"
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
async fn cleanup_removes_platform_downloaded_source_and_preserves_user_starter_and_author_git() {
    let (temporary, paths, service) = fixture().await;
    let apps = crate::node_apps::NodeApps::new(
        paths.data_root().into(),
        "/disposable/nonexistent-node".into(),
    )
    .unwrap();
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let downloaded = paths
        .workspace_root()
        .join("project-sources/disposable-downloaded");
    prepare_node_source(&downloaded);
    fs::create_dir(downloaded.join(".git")).unwrap();
    fs::write(
        downloaded.join(".git/HEAD"),
        "platform-downloaded Git is deleted",
    )
    .unwrap();
    let id = Uuid::new_v4().to_string();
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    ownership.initialize(&id).unwrap();
    let identity = PersistedIdentity::capture(&downloaded).unwrap();
    ownership
        .reserve_published_source_identity(&id, "disposable-downloaded", &identity)
        .unwrap();
    apps.register_published_source(
        &id,
        crate::node_git_import::VerifiedPublishedSource {
            source_root: downloaded.clone(),
            source_identity: identity,
            manifest_digest: format!(
                "{:x}",
                Sha256::digest(fs::read(downloaded.join("panda.json")).unwrap())
            ),
            source_url: "https://github.com/disposable/source".into(),
            source_branch: None,
        },
    )
    .await
    .unwrap();
    let author = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-user-starter");
    fs::create_dir(&author).unwrap();
    let preview = apps.inspect_directory(&author).unwrap();
    apps.initialize_and_register_directory(
        author.clone(),
        crate::node_apps::DirectoryInitializeInput {
            expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
            name: "Disposable user starter".into(),
        },
    )
    .await
    .unwrap();
    fs::create_dir(author.join(".git")).unwrap();
    fs::write(author.join(".git/HEAD"), "ordinary author Git remains").unwrap();
    fs::create_dir(author.join("data")).unwrap();
    fs::write(author.join("data/business.txt"), "user data remains").unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(plan.preview()["preservedPaths"], json!([author]));
    assert!(service.quiesce().await.is_complete());
    plan.execute_after_shutdown(&service).unwrap();
    assert!(!downloaded.exists());
    assert_eq!(
        fs::read_to_string(author.join(".git/HEAD")).unwrap(),
        "ordinary author Git remains"
    );
    assert_eq!(
        fs::read_to_string(author.join("data/business.txt")).unwrap(),
        "user data remains"
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
async fn changed_external_registration_or_source_binding_invalidates_scope() {
    for development in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("author-source");
        fs::create_dir(&source).unwrap();
        let registration = external_registration(&paths, &source).await;
        let target = if development {
            registration.parent().unwrap().join("source-binding.json")
        } else {
            registration.clone()
        };
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
async fn cleanup_includes_only_current_after_automatic_migration_source_finalization() {
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
    assert!(!plan.targets.iter().any(|target| target.path == destination));
    assert!(!destination.exists());
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
    // Keep this particular source pending to exercise whole-installation
    // accounting before its registered automatic cleanup has completed.
    service
        .handle()
        .set_workspace_migration_hook(std::sync::Arc::new(|notice| {
            Box::pin(async move {
                if notice.stage == crate::WorkspaceMigrationStage::Reopened {
                    fs::write(
                        notice.source_path.join("disposable-unknown-new-file"),
                        b"keep pending source",
                    )
                    .unwrap();
                }
                Ok(())
            })
        }));
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
async fn real_user_source_within_migrated_workspace_blocks_cleanup() {
    let (temporary, paths, service) = fixture().await;
    let destination = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("migrated-workspace");
    let selected = migrate(&paths, &service, &destination).await;
    let author = destination.join("author-original-source");
    fs::create_dir(&author).unwrap();
    external_registration(&selected, &author).await;
    assert!(LocalCleanupPlan::prepare(&selected, &service).is_err());
    assert!(author.exists());
    assert!(service.quiesce().await.is_complete());
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
    fs::write(original.join("author.txt"), b"preserve original").unwrap();
    external_registration(&selected, &original).await;
    let plan = LocalCleanupPlan::prepare(&selected, &service).unwrap();
    assert!(service.quiesce().await.is_complete());
    let moved = original.with_file_name("retained-original");
    fs::rename(&original, &moved).unwrap();
    symlink(&destination, &original).unwrap();
    assert!(plan.execute_after_shutdown(&service).is_err());
    assert!(destination.exists());
    assert_eq!(
        fs::read(moved.join("author.txt")).unwrap(),
        b"preserve original"
    );
    assert!(!selected.data_root().join(CONTROL).exists());
}

#[tokio::test]
async fn current_git_capture_metadata_and_real_user_source_binding_are_bound_to_preview() {
    let (temporary, paths, service) = fixture().await;
    let original = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("source-original");
    fs::create_dir(&original).unwrap();
    let registration = external_registration(&paths, &original).await;
    let owner = registration
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let source_id = format!("src_{}", Uuid::new_v4().simple());
    let source_path = paths
        .workspace_root()
        .join(".platform/sources")
        .join(&source_id);
    fs::create_dir_all(&source_path).unwrap();
    let source_record_path = source_path.join("source.json");
    fs::write(
        &source_record_path,
        serde_json::to_vec(&source_record(&source_id, owner)).unwrap(),
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
    for variant in 0..6 {
        let (_temporary, paths, service) = fixture().await;
        let slot = paths
            .data_root()
            .join("node-apps")
            .join(Uuid::new_v4().to_string());
        let source_id = format!("src_{}", Uuid::new_v4().simple());
        let path = match variant {
            0..=2 => slot.join("registration.json"),
            3 => slot.join("source-binding.json"),
            _ => paths
                .workspace_root()
                .join(".platform/sources")
                .join(&source_id)
                .join("source.json"),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let value = match variant {
            1 => {
                json!({"version":3,"id":slot.file_name().unwrap().to_str().unwrap(),"manifestDigest":"a".repeat(64)})
            }
            2 => {
                json!({"version":999,"id":Uuid::new_v4().to_string()})
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
async fn live_owner_index_preserves_user_source_when_current_registration_and_binding_are_missing()
{
    for overlaps in [false, true] {
        let (temporary, paths, service) = fixture().await;
        let selected = if overlaps {
            let destination = temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("disposable-migrated-owner-workspace");
            migrate(&paths, &service, &destination).await
        } else {
            paths.clone()
        };
        let source = if overlaps {
            selected
                .workspace_root()
                .join("custom/disposable-author-original")
        } else {
            temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("disposable-author-original")
        };
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("original-code"), b"preserve author source").unwrap();
        let registration = external_registration(&selected, &source).await;
        let slot = registration.parent().unwrap().to_path_buf();
        fs::remove_file(&registration).unwrap();
        fs::remove_file(slot.join("source-binding.json")).unwrap();
        let plan = LocalCleanupPlan::prepare(&selected, &service);
        assert!(service.quiesce().await.is_complete());
        if overlaps {
            assert!(plan.is_err());
        } else {
            let plan = plan.unwrap();
            assert_eq!(plan.preview()["preservedPaths"], json!([source]));
            plan.execute_after_shutdown(&service).unwrap();
            assert!(!slot.exists());
        }
        assert_eq!(
            fs::read(source.join("original-code")).unwrap(),
            b"preserve author source"
        );
    }
}

#[tokio::test]
async fn owner_index_cannot_treat_a_replaced_author_directory_as_the_recorded_original() {
    let (temporary, paths, service) = fixture().await;
    let source = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-claimed-original");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("original-code"), b"preserve original").unwrap();
    let id = Uuid::new_v4().to_string();
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    ownership.initialize(&id).unwrap();
    crate::node_apps::source_ownership::claim_source_after_verified_migration(
        paths.data_root(),
        paths.workspace_root(),
        &id,
        &source,
    )
    .unwrap();
    let moved = source.with_file_name("disposable-moved-original");
    fs::rename(&source, &moved).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("foreign-file"), b"preserve replacement").unwrap();
    assert!(LocalCleanupPlan::prepare(&paths, &service).is_err());
    assert!(service.quiesce().await.is_complete());
    assert_eq!(
        fs::read(moved.join("original-code")).unwrap(),
        b"preserve original"
    );
    assert_eq!(
        fs::read(source.join("foreign-file")).unwrap(),
        b"preserve replacement"
    );
}
