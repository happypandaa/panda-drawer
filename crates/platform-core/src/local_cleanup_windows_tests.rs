//! Real runtime/lock and public Node onboarding, only RuntimePaths::for_test.
//! No WebView or Node process is launched: absent native ledgers and no-child
//! closure are required, with no fake successful browser-purge callback.
use super::*;
use crate::{
    durable_writer::WriterOwner,
    node_apps::{DirectoryInitializeInput, NodeApps},
    CreateProjectInput,
};
use platform_native::windows::files::{self as native, FileIdentity};

async fn fixture() -> (tempfile::TempDir, RuntimePaths, RuntimeService) {
    let (temporary, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    (temporary, paths, service)
}

async fn quiesce(service: &RuntimeService) {
    let report = tokio::time::timeout(Duration::from_secs(45), service.quiesce())
        .await
        .expect("actual runtime workers and registry must join within the fixture deadline");
    assert!(report.is_complete(), "{report:?}");
}

fn identity(path: &Path) -> FileIdentity {
    native::open_no_reparse(path).unwrap().identity
}

fn prepare_error(paths: &RuntimePaths, service: &RuntimeService) -> ApiError {
    match LocalCleanupPlan::prepare(paths, service) {
        Ok(_) => panic!("cleanup preview unexpectedly accepted an unconfirmed writer"),
        Err(error) => error,
    }
}

fn require_held(path: &Path) {
    let contender = native::open_read_file(path).unwrap();
    let error = fs2::FileExt::try_lock_exclusive(&contender).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        fs2::lock_contended_error().raw_os_error()
    );
    assert!(io::lock_is_contended(&error));
}

#[tokio::test]
async fn actual_runtime_held_lock_allows_preview_and_cleanup_only_after_real_shutdown() {
    let (_temporary, paths, service) = fixture().await;
    let lock = paths.data_root().join("instance.lock");
    let before = identity(&lock);
    require_held(&lock);
    let preview = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(preview.preview()["confirmationText"], CONFIRMATION);
    assert!(!paths.data_root().join(".local-cleanup").exists());
    assert!(preview.execute_after_shutdown(&service).is_err());
    assert!(paths.workspace_root().exists());
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    quiesce(&service).await;
    plan.verify_before_native_cleanup(&service).unwrap();
    let result = plan.execute_after_shutdown(&service).unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    assert_eq!(result["remainingPaths"], json!([]));
    assert!(before.same_object(&identity(&lock)));
    require_held(&lock);
    let remaining = fs::read_dir(paths.data_root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    assert_eq!(remaining, RETAINED.map(std::ffi::OsString::from).into());
}

#[tokio::test]
async fn released_or_replaced_original_instance_lock_refuses_the_old_plan_without_deleting() {
    for replace in [false, true] {
        let (_temporary, paths, service) = fixture().await;
        let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
        let lock = paths.data_root().join("instance.lock");
        let before = identity(&lock);
        quiesce(&service).await;
        drop(service);
        // This actual independent handle proves the original owner released
        // its byte-range lock; no boolean is injected into cleanup authority.
        let contender = native::open_read_file(&lock).unwrap();
        fs2::FileExt::try_lock_exclusive(&contender).unwrap();
        fs2::FileExt::unlock(&contender).unwrap();
        drop(contender);
        let retained = paths.data_root().join("retained-original-instance.lock");
        if replace {
            fs::rename(&lock, &retained).unwrap();
            fs::write(&lock, b"disposable replacement must never be deleted").unwrap();
            assert!(!before.same_object(&identity(&lock)));
        }
        let error = plan.verify_controls().unwrap_err();
        assert_eq!(
            error.code,
            if replace {
                "LOCAL_CLEANUP_CHANGED"
            } else {
                "LOCAL_CLEANUP_UNAVAILABLE"
            }
        );
        assert!(paths.workspace_root().is_dir());
        assert!(paths.data_root().join("settings.json").is_file());
        if replace {
            assert!(before.same_object(&identity(&retained)));
            assert_eq!(
                fs::read(&lock).unwrap(),
                b"disposable replacement must never be deleted"
            );
        } else {
            assert!(before.same_object(&identity(&lock)));
        }
    }
}

#[tokio::test]
async fn real_unclosed_writer_blocks_both_preview_and_post_shutdown_native_cleanup() {
    let (_temporary, paths, service) = fixture().await;
    let owner = WriterOwner::platform(paths.data_root()).unwrap();
    let mut writer = owner
        .begin("disposable cleanup actual no-child guard")
        .unwrap();
    assert_eq!(
        prepare_error(&paths, &service).code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    writer.finish().unwrap();
    drop(writer);
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    let mut writer = owner
        .begin("disposable cleanup late unclosed guard")
        .unwrap();
    quiesce(&service).await;
    assert_eq!(
        plan.verify_before_native_cleanup(&service)
            .unwrap_err()
            .code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    assert!(paths.workspace_root().exists());
    assert!(paths.data_root().join("settings.json").exists());
    writer.finish().unwrap();
    // The closed guard still owns native directory pins until it is dropped.
    // Release those real handles before asking cleanup to remove their scope.
    drop(writer);
    plan.verify_before_native_cleanup(&service).unwrap();
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap()["operationOutcome"],
        "deleted"
    );
}

fn source_snapshot(root: &Path) -> BTreeMap<PathBuf, (FileIdentity, Vec<u8>)> {
    fn collect(
        base: &Path,
        directory: &Path,
        result: &mut BTreeMap<PathBuf, (FileIdentity, Vec<u8>)>,
    ) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().into(),
                    (identity(&path), fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}

#[tokio::test]
async fn public_node_cleanup_preserves_both_existing_and_selected_starter_sources_and_file_data() {
    let (temporary, paths, service) = fixture().await;
    let root = temporary.path().canonicalize().unwrap();
    let original = root.join("external-original 中文 space");
    let generated = root.join("selected-starter 中文 space");
    fs::create_dir(&original).unwrap();
    fs::create_dir(&generated).unwrap();
    fs::create_dir(original.join("web")).unwrap();
    fs::create_dir(original.join("backend")).unwrap();
    fs::write(
        original.join("web/index.html"),
        b"<p>External original remains</p>",
    )
    .unwrap();
    fs::write(
        original.join("backend/server.mjs"),
        b"throw new Error('registration must not execute this source');",
    )
    .unwrap();
    fs::write(
        original.join("panda.json"),
        serde_json::to_vec(&json!({
            "manifestVersion":1,"runtime":"node",
            "app":{"name":"External original","version":"1"},
            "web":{"root":"web","entry":"index.html"},
            "backend":{"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24}
        }))
        .unwrap(),
    )
    .unwrap();
    let original_id = identity(&original);
    let apps = NodeApps::with_runtime(
        paths.data_root().into(),
        root.join("never-executed-node.exe"),
        &service.handle(),
    )
    .unwrap();
    let preview = apps.inspect_directory(&original).unwrap();
    assert_eq!(preview["state"], "ready");
    let external = apps
        .register_directory(
            original.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(external["ownership"], "external");
    let preview = apps.inspect_directory(&generated).unwrap();
    assert_eq!(preview["state"], "empty");
    let initialized = apps
        .initialize_directory(
            &generated,
            DirectoryInitializeInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                name: "Disposable managed cleanup Node".into(),
            },
        )
        .unwrap();
    let pending = initialized["pendingAppId"].as_str().unwrap().to_owned();
    let preview = apps.inspect_directory(&generated).unwrap();
    let managed = apps
        .register_directory(
            generated.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(managed["id"], pending);
    assert_eq!(managed["ownership"], "external");
    for app in [&external, &managed] {
        let slot = paths
            .data_root()
            .join("node-apps")
            .join(app["id"].as_str().unwrap());
        let source = PathBuf::from(app["sourcePath"].as_str().unwrap());
        fs::create_dir(source.join("data")).unwrap();
        fs::write(
            source.join("data/retained-business.txt"),
            b"user business data remains",
        )
        .unwrap();
        for name in ["cache", "runs"] {
            fs::write(
                slot.join(name).join("disposable-owned-payload"),
                b"owned material must disappear",
            )
            .unwrap();
        }
    }
    let original_before = source_snapshot(&original);
    let generated_before = source_snapshot(&generated);
    let generated_id = identity(&generated);
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable cleanup SQLite project".into(),
            description: None,
        })
        .await
        .unwrap();
    let plan = LocalCleanupPlan::prepare(&paths, &service).unwrap();
    assert_eq!(
        plan.preview()["preservedPaths"],
        json!([original, generated])
    );
    assert!(!plan
        .targets
        .iter()
        .any(|target| generated.starts_with(&target.path)));
    apps.stop_all().await.unwrap();
    assert!(apps.owned_process_ids().await.is_empty());
    drop(apps);
    quiesce(&service).await;
    assert_eq!(
        plan.execute_after_shutdown(&service).unwrap()["operationOutcome"],
        "deleted"
    );
    assert!(generated_id.same_object(&identity(&generated)));
    let generated_after = source_snapshot(&generated);
    assert_eq!(generated_after.len(), generated_before.len());
    for (name, (expected_id, expected_bytes)) in generated_before {
        assert!(expected_id.same_object(&generated_after[&name].0));
        assert_eq!(generated_after[&name].1, expected_bytes);
    }
    assert!(!project.project_root.exists());
    assert!(!paths.data_root().join("node-apps").exists());
    assert!(!paths.data_root().join("node-project-ownership").exists());
    assert!(original_id.same_object(&identity(&original)));
    let after = source_snapshot(&original);
    assert_eq!(after.len(), original_before.len());
    for (name, (expected_id, expected_bytes)) in original_before {
        assert!(expected_id.same_object(&after[&name].0));
        assert_eq!(after[&name].1, expected_bytes);
    }
}
