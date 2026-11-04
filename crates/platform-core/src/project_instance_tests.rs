//! Disposable folders prove instance identity, lazy data and user ownership.
use super::*;
use crate::{
    builds::BuildManager, maintenance::MaintenanceManager,
    web_project_deletion::WebProjectDeletionManager, RuntimePaths, TaskStatus,
};
fn author(root: &Path, sdk: bool) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("index.html"), "<h1>Disposable</h1>").unwrap();
    let mut value = json!({"manifestVersion":1,"runtime":"web","app":{"name":"Disposable web"},"web":{"root":".","entry":"index.html"}});
    if sdk {
        value["platform"] = json!({"sdkApiVersion":1});
    }
    fs::write(root.join("panda.json"), serde_json::to_vec(&value).unwrap()).unwrap();
}
fn fixture() -> (tempfile::TempDir, RuntimePaths, Arc<ProjectStore>) {
    let (t, p) = RuntimePaths::for_test();
    let s = Arc::new(
        ProjectStore::with_paths(
            p.workspace_root().into(),
            p.data_root().into(),
            None,
            Default::default(),
        )
        .unwrap(),
    );
    (t, p, s)
}
#[tokio::test]
async fn local_plain_web_never_creates_data_or_source_control_and_readdition_gets_new_instance() {
    let (t, p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("user-web");
    author(&root, false);
    let info = store.register_local(&root).await.unwrap();
    assert_eq!(info.state, "ready");
    let project = store.project(&info.id).unwrap();
    let configuration = store.web_configuration(&info.id).unwrap();
    assert!(configuration.fields.is_empty());
    assert_eq!(configuration.configuration.revision, 0);
    assert!(configuration.configuration.values.is_empty());
    assert!(configuration.configuration.can_start);
    store.require_web_configuration_ready(&project).unwrap();
    assert!(store.web_configuration_values(&project).unwrap().is_empty());
    assert_eq!(info.ownership, ProjectOwnership::User);
    assert_eq!(
        project.private_dir,
        p.data_root().join("web-apps").join(&info.id)
    );
    assert!(!root.join("data").exists());
    assert!(!root.join(".platform").exists());
    assert_eq!(
        store.schema(&info.id, None).await.unwrap_err().code,
        "SDK_NOT_ENABLED"
    );
    assert!(!root.join("data").exists());
    assert!(store.register_local(&root).await.is_err());
    store.shutdown().await;
}
#[tokio::test]
async fn sdk_data_is_lazy_recreated_when_missing_and_corruption_is_preserved() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("sdk-web");
    author(&root, true);
    let info = store.register_local(&root).await.unwrap();
    assert!(!root.join("data").exists());
    store.schema(&info.id, None).await.unwrap();
    assert!(root.join("data/main.sqlite").is_file());
    assert!(!root.join("data/attachments").exists());
    store.close_database(&info.id).await.unwrap();
    fs::remove_file(root.join("data/main.sqlite")).unwrap();
    store.schema(&info.id, None).await.unwrap();
    store.close_database(&info.id).await.unwrap();
    fs::write(
        root.join("data/main.sqlite"),
        b"deliberately corrupt disposable database",
    )
    .unwrap();
    assert!(store.project(&info.id).is_ok());
    assert!(store.schema(&info.id, None).await.is_err());
    assert_eq!(
        fs::read(root.join("data/main.sqlite")).unwrap(),
        b"deliberately corrupt disposable database"
    );
    store.shutdown().await;
}
#[tokio::test]
async fn browser_sdk_snapshot_survives_declaration_edits_until_reopen() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("snapshot-web");
    author(&root, false);
    let info = store.register_local(&root).await.unwrap();
    let disabled = store.project(&info.id).unwrap();
    author(&root, true);
    assert_eq!(
        store
            .schema_snapshot(&disabled, Some(&info.document_epoch), SqlCaller::Browser)
            .await
            .unwrap_err()
            .code,
        "SDK_NOT_ENABLED"
    );
    let enabled = store.project(&info.id).unwrap();
    author(&root, false);
    store
        .schema_snapshot(&enabled, Some(&info.document_epoch), SqlCaller::Browser)
        .await
        .unwrap();
    assert_eq!(
        store.schema(&info.id, None).await.unwrap_err().code,
        "SDK_NOT_ENABLED"
    );
    store.shutdown().await;
}
#[tokio::test]
async fn web_open_and_build_are_atomic_exclusions_but_delete_can_close_an_open_window() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("leased-web");
    author(&root, false);
    let info = store.register_local(&root).await.unwrap();
    let opened = store.reserve_web_open(&info.id).unwrap();
    assert!(!store.has_active_tasks());
    assert_eq!(
        store
            .reserve_web_build(&info.id, "task_build")
            .err()
            .unwrap()
            .code,
        "WEB_APP_OPEN"
    );
    let deleting = store.reserve_delete_task(&info.id, "task_delete").unwrap();
    drop(opened);
    assert_eq!(store.active_task(&info.id).as_deref(), Some("task_delete"));
    drop(deleting);
    let building = store.reserve_web_build(&info.id, "task_build").unwrap();
    assert!(store.reserve_web_open(&info.id).is_err());
    drop(building);
    assert!(store.reserve_web_open(&info.id).is_ok());
    store.shutdown().await;
}
#[tokio::test]
async fn local_delete_keeps_git_and_data_removes_instance_then_readd_gets_fresh_identity() {
    let (t, p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("user-delete-web");
    author(&root, true);
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/author.bin"), b"user data").unwrap();
    let info = store.register_local(&root).await.unwrap();
    let tasks = BuildManager::new(p.clone(), store.clone()).unwrap();
    let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
    let deletion =
        WebProjectDeletionManager::new(p.clone(), store.clone(), tasks.clone(), maintenance)
            .unwrap();
    let task = deletion
        .submit_project(
            &info.id,
            "delete",
            json!({"expectedDocumentEpoch":info.document_epoch}),
        )
        .await
        .unwrap();
    let mut completed = None;
    for _ in 0..500 {
        match deletion.task_for_project(&task.task_id, &info.id, "delete") {
            Ok(t) if t.status.is_final() => {
                completed = Some(t);
                break;
            }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let completed = completed.expect("disposable deletion timed out");
    assert_eq!(completed.status, TaskStatus::Succeeded, "{completed:?}");
    assert_eq!(
        fs::read(root.join("data/author.bin")).unwrap(),
        b"user data"
    );
    assert!(root.join(".git/HEAD").is_file());
    assert!(!p.data_root().join("web-apps").join(&info.id).exists());
    let fresh = store.register_local(&root).await.unwrap();
    assert_ne!(fresh.id, info.id);
    assert_ne!(fresh.document_epoch, info.document_epoch);
    deletion.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn managed_import_transfers_exact_source_rights_and_refuses_postcommit_rollback() {
    let (_t, p, store) = fixture();
    let root = p
        .workspace_root()
        .join("project-sources/disposable-transfer");
    author(&root, false);
    let identity = FileIdentity::capture(&root).unwrap();
    let import = Uuid::new_v4().to_string();
    let node = crate::node_project_ownership::NodeProjectOwnership::new(
        p.data_root().into(),
        p.workspace_root().into(),
    );
    node.initialize(&import).unwrap();
    node.reserve_published_source_identity(&import, "disposable-transfer", &identity)
        .unwrap();
    let id = format!("prj_{}", Uuid::new_v4().simple());
    let task = format!("task_{}", Uuid::new_v4().simple());
    store
        .reserve_managed_source(&id, &task, &root, &identity)
        .unwrap();
    assert_eq!(
        store
            .web_import_state(&id, &task, &root, &identity)
            .unwrap(),
        WebImportState::Reserved
    );
    node.release_imported_source(&import, &root, &identity)
        .unwrap();
    node.release_imported_source(&import, &root, &identity)
        .unwrap();
    assert!(!node.owned_paths(&import).unwrap().contains(&root));
    let info = store.register_managed(&id, &task, &root).await.unwrap();
    assert_eq!(info.ownership, ProjectOwnership::Platform);
    assert_eq!(
        store
            .web_import_state(&id, &task, &root, &identity)
            .unwrap(),
        WebImportState::Registered
    );
    assert_eq!(
        store
            .discard_unpublished_project(&id, &task, &root, &identity)
            .unwrap_err()
            .code,
        "PROJECT_ALREADY_REGISTERED"
    );
    assert!(root.join("index.html").is_file());
    store.shutdown().await;
}
#[tokio::test]
async fn cancelled_managed_import_removes_only_its_precisely_reserved_folder() {
    let (t, p, store) = fixture();
    let root = p.workspace_root().join("project-sources/disposable-cancel");
    author(&root, false);
    let identity = FileIdentity::capture(&root).unwrap();
    let original = t.path().join("user-original");
    author(&original, false);
    let id = format!("prj_{}", Uuid::new_v4().simple());
    let task = format!("task_{}", Uuid::new_v4().simple());
    store
        .reserve_managed_source(&id, &task, &root, &identity)
        .unwrap();
    store
        .discard_unpublished_project(&id, &task, &root, &identity)
        .unwrap();
    assert!(!root.exists());
    assert!(original.join("index.html").is_file());
    assert!(!p.data_root().join("web-apps").join(&id).exists());
    assert!(!crate::web_project_ownership::WebProjectOwnership::new(
        p.data_root().into(),
        p.workspace_root().into()
    )
    .path(&id)
    .unwrap()
    .exists());
    store.shutdown().await;
}
#[tokio::test]
async fn local_confirmation_rechecks_manifest_before_claiming_user_source() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("fresh-local");
    author(&root, false);
    let identity = FileIdentity::capture(&root).unwrap();
    let inspected = PandaManifest::load(&root).unwrap();
    author(&root, true);
    assert_eq!(
        store
            .register_local_checked(&root, &identity, &inspected.source_digest)
            .await
            .unwrap_err()
            .code,
        "PROJECT_SOURCE_CHANGED"
    );
    assert!(store.register_local(&root).await.is_ok());
    store.shutdown().await;
}

#[tokio::test]
async fn idle_cli_database_close_and_a_new_window_share_one_io_admission_gate() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("close-race");
    author(&root, true);
    let info = store.register_local(&root).await.unwrap();
    store.schema(&info.id, None).await.unwrap();
    assert!(!store.fixture_has_database_worker(&info.id));
    let project = store.project(&info.id).unwrap();
    store
        .schema_snapshot(&project, Some(&info.document_epoch), SqlCaller::Browser)
        .await
        .unwrap();
    let read = store.gate(&info.id).read_owned().await;
    let closer_store = store.clone();
    let id = info.id.clone();
    let closer = tokio::spawn(async move { closer_store.close_idle_database(&id).await });
    tokio::task::yield_now().await;
    let opened = store.reserve_web_open(&info.id).unwrap();
    let reader_store = store.clone();
    let epoch = info.document_epoch.clone();
    let sql = tokio::spawn(async move {
        reader_store
            .schema_snapshot(&project, Some(&epoch), SqlCaller::Browser)
            .await
    });
    drop(read);
    closer.await.unwrap().unwrap();
    sql.await.unwrap().unwrap();
    assert!(store.fixture_has_database_worker(&info.id));
    store.drain_and_close_database(&info.id).await.unwrap();
    drop(opened);
    assert!(!store.fixture_has_database_worker(&info.id));
    store.shutdown().await;
}

#[tokio::test]
async fn changing_registered_runtime_never_rebinds_the_instance_or_touches_data() {
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("changed-runtime");
    author(&root, true);
    let info = store.register_local(&root).await.unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/author.bin"), b"preserved runtime data").unwrap();
    fs::create_dir(root.join("backend")).unwrap();
    fs::write(
        root.join("backend/main.mjs"),
        "throw Error('never execute')",
    )
    .unwrap();
    fs::write(root.join("panda.json"),serde_json::to_vec(&json!({"manifestVersion":1,"runtime":"node","app":{"name":"Changed"},"web":{"root":".","entry":"index.html"},"backend":{"root":"backend","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}})).unwrap()).unwrap();
    assert_eq!(
        store.project(&info.id).err().unwrap().code,
        "APP_RUNTIME_CHANGED"
    );
    assert_eq!(
        store.registered_epoch(&info.id).unwrap().as_deref(),
        Some(info.document_epoch.as_str())
    );
    assert_eq!(
        fs::read(root.join("data/author.bin")).unwrap(),
        b"preserved runtime data"
    );
    store.shutdown().await;
}
#[tokio::test]
async fn rollback_can_finish_after_private_reservation_disappears_without_source_recreation() {
    let (_t, p, store) = fixture();
    let root = p
        .workspace_root()
        .join("project-sources/disposable-partial-rollback");
    author(&root, false);
    let identity = FileIdentity::capture(&root).unwrap();
    let id = format!("prj_{}", Uuid::new_v4().simple());
    let task = format!("task_{}", Uuid::new_v4().simple());
    store
        .reserve_managed_source(&id, &task, &root, &identity)
        .unwrap();
    crate::history_cleanup::remove_owned_directory(&root, &identity).unwrap();
    let private = store.private_dir(&id).unwrap();
    let private_identity = FileIdentity::capture(&private).unwrap();
    crate::history_cleanup::remove_owned_directory(&private, &private_identity).unwrap();
    assert_eq!(
        store
            .web_import_state(&id, &task, &root, &identity)
            .unwrap(),
        WebImportState::Reserved
    );
    store
        .discard_unpublished_project(&id, &task, &root, &identity)
        .unwrap();
    assert_eq!(
        store
            .web_import_state(&id, &task, &root, &identity)
            .unwrap(),
        WebImportState::Absent
    );
    assert!(!root.exists());
    store.shutdown().await;
}
#[tokio::test]
async fn managed_git_provenance_survives_reopening_without_author_manifest_identity() {
    let (_t, p, store) = fixture();
    let root = p
        .workspace_root()
        .join("project-sources/disposable-provenance");
    author(&root, false);
    let identity = FileIdentity::capture(&root).unwrap();
    let id = format!("prj_{}", Uuid::new_v4().simple());
    let task = format!("task_{}", Uuid::new_v4().simple());
    store
        .reserve_managed_source(&id, &task, &root, &identity)
        .unwrap();
    store
        .register_managed_git(
            &id,
            &task,
            &root,
            "https://github.com/disposable/web",
            Some("main"),
            None,
        )
        .await
        .unwrap();
    store.shutdown().await;
    let reopened = ProjectStore::with_paths(
        p.workspace_root().into(),
        p.data_root().into(),
        None,
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        reopened.info(&id).await.unwrap().git_source,
        Some(json!({"url":"https://github.com/disposable/web","branch":"main"}))
    );
    reopened.shutdown().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn source_registration_survives_device_renumbering_without_rewriting_metadata() {
    let (t, p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("stable-web-root");
    author(&root, false);
    let info = store.register_local(&root).await.unwrap();
    let path = p
        .data_root()
        .join("web-apps")
        .join(&info.id)
        .join("registration.json");
    let mut registration: Registration = read_json(&path).unwrap();
    assert!(!registration.root_identity.is_legacy());
    registration.root_identity.device += 1;
    let bytes = serde_json::to_vec_pretty(&registration).unwrap();
    fs::write(&path, &bytes).unwrap();
    store.project(&info.id).unwrap().check().unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
    store.shutdown().await;
}
#[tokio::test]
async fn a_broken_registration_is_a_diagnostic_row_and_never_hides_healthy_instances() {
    let (t, p, store) = fixture();
    let a = t.path().canonicalize().unwrap().join("healthy-web");
    let b = t
        .path()
        .canonicalize()
        .unwrap()
        .join("damaged-registration-web");
    author(&a, false);
    author(&b, false);
    let healthy = store.register_local(&a).await.unwrap();
    let damaged = store.register_local(&b).await.unwrap();
    let path = p
        .data_root()
        .join("web-apps")
        .join(&damaged.id)
        .join("registration.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schemaVersion"] = 0.into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let rows = store.list().await.unwrap();
    assert!(rows
        .iter()
        .any(|r| r.id == healthy.id && r.state == "ready"));
    let row = rows.iter().find(|r| r.id == damaged.id).unwrap();
    assert_eq!(row.state, "error");
    assert_eq!(row.project_root, path);
    assert!(b.join("index.html").is_file());
    store.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn sdk_bootstrap_preserves_existing_user_data_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (t, _p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("permissions-web");
    author(&root, true);
    let data = root.join("data");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
    let info = store.register_local(&root).await.unwrap();
    store.schema(&info.id, None).await.unwrap();
    store
        .project(&info.id)
        .unwrap()
        .ensure_attachments_dir()
        .unwrap();
    assert_eq!(
        fs::metadata(data).unwrap().permissions().mode() & 0o777,
        0o755
    );
    store.shutdown().await;
}

async fn delete_web_instance(
    paths: &RuntimePaths,
    store: Arc<ProjectStore>,
    info: &ProjectInfo,
) -> crate::TaskRecord {
    let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
    let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
    let manager =
        WebProjectDeletionManager::new(paths.clone(), store, tasks.clone(), maintenance).unwrap();
    let task = manager
        .submit_project(
            &info.id,
            "delete",
            json!({"expectedDocumentEpoch":info.document_epoch}),
        )
        .await
        .unwrap();
    let mut completed = None;
    for _ in 0..500 {
        if let Ok(task) = manager.task_for_project(&task.task_id, &info.id, "delete") {
            if task.status.is_final() {
                completed = Some(task);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    manager.shutdown().await;
    tasks.shutdown().await;
    completed.expect("disposable project deletion timed out")
}
#[tokio::test]
async fn deleting_a_user_instance_with_missing_or_replaced_source_never_adopts_or_removes_either_folder(
) {
    for replacement in [false, true] {
        let (t, p, store) = fixture();
        let root = t
            .path()
            .canonicalize()
            .unwrap()
            .join("user-container/replaced-user-web");
        author(&root, true);
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data/original.bin"), b"original user data").unwrap();
        let info = store.register_local(&root).await.unwrap();
        let held_parent = t.path().canonicalize().unwrap().join("held-user-container");
        fs::rename(root.parent().unwrap(), &held_parent).unwrap();
        let held = held_parent.join("replaced-user-web");
        if replacement {
            author(&root, true);
            fs::create_dir(root.join("data")).unwrap();
            fs::write(root.join("data/replacement.bin"), b"replacement user data").unwrap();
        }
        let result = delete_web_instance(&p, store.clone(), &info).await;
        assert_eq!(result.status, TaskStatus::Succeeded, "{result:?}");
        assert_eq!(
            fs::read(held.join("data/original.bin")).unwrap(),
            b"original user data"
        );
        if replacement {
            assert_eq!(
                fs::read(root.join("data/replacement.bin")).unwrap(),
                b"replacement user data"
            );
            assert!(store.register_local(&root).await.is_ok());
        } else {
            assert!(!root.exists());
        }
        assert!(!p.data_root().join("web-apps").join(&info.id).exists());
        store.shutdown().await;
    }
}
#[tokio::test]
async fn deleting_external_node_with_replaced_author_folder_retires_only_the_instance() {
    let (t, p, store) = fixture();
    let root = t.path().canonicalize().unwrap().join("replaced-user-node");
    author(&root, false);
    fs::create_dir(root.join("backend")).unwrap();
    fs::write(
        root.join("backend/main.mjs"),
        "throw Error('never execute')",
    )
    .unwrap();
    let manifest = json!({"manifestVersion":1,"runtime":"node","app":{"name":"Disposable external Node"},"web":{"root":".","entry":"index.html"},"backend":{"root":"backend","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}});
    fs::write(
        root.join("panda.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/original.bin"), b"original Node user data").unwrap();
    let apps = crate::node_apps::NodeApps::new(
        p.data_root().into(),
        "/disposable/nonexistent-node".into(),
    )
    .unwrap();
    apps.set_source_workspace(p.workspace_root().into())
        .unwrap();
    let registered = apps.register(root.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    let held = root.with_extension("original");
    fs::rename(&root, &held).unwrap();
    author(&root, false);
    fs::create_dir(root.join("data")).unwrap();
    fs::write(
        root.join("data/replacement.bin"),
        b"replacement Node user data",
    )
    .unwrap();
    let fresh_web = store.register_local(&root).await.unwrap();
    let preview = apps.delete_preview(id).await.unwrap();
    let result = apps
        .delete(
            id,
            crate::node_apps::DeleteNodeAppInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                force: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(result["sourcePreserved"], true);
    assert_eq!(
        fs::read(held.join("data/original.bin")).unwrap(),
        b"original Node user data"
    );
    assert_eq!(
        fs::read(root.join("data/replacement.bin")).unwrap(),
        b"replacement Node user data"
    );
    assert!(!p.data_root().join("node-apps").join(id).exists());
    assert!(store.project(&fresh_web.id).is_ok());
    assert!(p
        .data_root()
        .join("web-apps")
        .join(&fresh_web.id)
        .join("registration.json")
        .exists());
    apps.stop_all().await.unwrap();
    store.shutdown().await;
}
