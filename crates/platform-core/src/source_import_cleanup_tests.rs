use super::*;
use crate::builds;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    cleanup: Arc<CleanupManager>,
    source: String,
    owner: String,
    task: TaskRecord,
}
impl Fixture {
    fn new() -> Self {
        let (_temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            "test-import".into(),
        )
        .unwrap();
        let source = format!("src_{}", Uuid::new_v4().simple());
        let owner = Uuid::new_v4().to_string();
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            paths.data_root().into(),
            paths.workspace_root().into(),
        );
        ownership.initialize(&owner).unwrap();
        let task = tasks
            .create_node_operation(&owner, "sourceAcquire")
            .unwrap();
        tasks
            .record_node_source(&task.task_id, &owner, &source)
            .unwrap();
        let slot = paths
            .workspace_root()
            .join(".platform/sources")
            .join(&source);
        fs::create_dir_all(slot.join("repository/.git")).unwrap();
        fs::create_dir_all(slot.join(".acquiring/home")).unwrap();
        fs::write(slot.join("repository/app.js"), "disposable source").unwrap();
        fs::write(slot.join("repository/.git/keep"), "git history").unwrap();
        fs::write(
            slot.join(".acquiring/home/tmp"),
            "download temporary material",
        )
        .unwrap();
        builds::atomic_json(&slot.join("source.json"), &json!({"schemaVersion":1,"sourceId":source,"ownerAppId":owner,"taskId":task.task_id,"status":"ready","phase":"completed","error":null})).unwrap();
        ownership.record_source(&owner, &source).unwrap();
        tasks
            .finish(
                &task.task_id,
                Ok(json!({"sourceId":source,"ownerAppId":owner})),
            )
            .unwrap();
        Self {
            _temp,
            paths,
            store,
            tasks,
            cleanup,
            source,
            owner,
            task,
        }
    }
    fn source_path(&self) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/sources")
            .join(&self.source)
    }
    fn staging(&self, state: &str) -> PathBuf {
        let prep = self
            .tasks
            .create_node_operation(&self.owner, "nodeInstallPrepare")
            .unwrap();
        self.tasks
            .finish(
                &prep.task_id,
                Err(ApiError::new("DISPOSABLE_TEST", "artificial failure")),
            )
            .unwrap();
        let path = self
            .paths
            .data_root()
            .join("node-apps/.installations")
            .join(format!("install_{}", Uuid::new_v4().simple()));
        fs::create_dir_all(path.join("npm-cache")).unwrap();
        fs::write(path.join("npm-cache/payload"), "disposable npm cache").unwrap();
        builds::atomic_json(&path.join("install.json"), &json!({"version":1,"candidateId":path.file_name().unwrap().to_str().unwrap(),"sourceId":self.source,"prepareTaskId":prep.task_id,"applyTaskId":null,"state":state,"target":self.target(),"prepared":null,"registration":null,"error":null})).unwrap();
        let build = self
            .paths
            .data_root()
            .join("node-development-builds")
            .join(&prep.task_id);
        fs::create_dir_all(&build).unwrap();
        fs::write(build.join("source.js"), "isolated build input").unwrap();
        builds::atomic_json(
            &self
                .source_path()
                .join("install-tasks")
                .join(format!("{}.json", prep.task_id)),
            &json!({"sourceId":self.source,"prepareTaskId":prep.task_id,"targetAppId":null}),
        )
        .unwrap();
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        ownership
            .record_installation(&self.owner, path.file_name().unwrap().to_str().unwrap())
            .unwrap();
        ownership.record_build(&self.owner, &prep.task_id).unwrap();
        path
    }
    fn target(&self) -> crate::node_app_install::InstallTargetSnapshot {
        let mut target = crate::node_app_install::InstallTargetSnapshot::fresh();
        target.app_id = self.owner.clone();
        target
    }
    async fn stop(&self) {
        self.cleanup.shutdown().await;
        self.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn failed_import_cleans_only_exact_source_staging_and_build_and_is_idempotent() {
    let fixture = Fixture::new();
    let stage = fixture.staging("failed");
    let unrelated = fixture
        .paths
        .workspace_root()
        .join("external-author-source");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join("keep.txt"), "author content").unwrap();
    let result = fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .unwrap();
    assert_eq!(result["completed"], true, "{result}");
    assert_eq!(result["sourceRetained"], false);
    assert!(!fixture.source_path().exists());
    assert!(!stage.exists());
    assert_eq!(
        fs::read_to_string(unrelated.join("keep.txt")).unwrap(),
        "author content"
    );
    let repeated = fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .unwrap();
    assert_eq!(repeated["completed"], true);
    assert!(repeated["removedPaths"].as_array().unwrap().is_empty());
    assert!(!fixture.paths.data_root().join("cleanup/imports").exists());
    assert_eq!(
        fixture.tasks.task(&fixture.task.task_id).unwrap_err().code,
        "TASK_NOT_FOUND"
    );
    assert!(!fixture
        .paths
        .data_root()
        .join("node-project-ownership")
        .join(&fixture.owner)
        .exists());
    fixture.stop().await;
}

#[tokio::test]
async fn referenced_import_keeps_formal_source_git_app_data_and_configuration() {
    let fixture = Fixture::new();
    let stage = fixture.staging("failed");
    let app = fixture
        .paths
        .data_root()
        .join("node-apps")
        .join(&fixture.owner);
    fs::create_dir_all(app.join("data")).unwrap();
    fs::create_dir_all(app.join("configuration")).unwrap();
    fs::write(app.join("data/keep.db"), "persistent business data").unwrap();
    fs::write(app.join("configuration/keep.json"), "private configuration").unwrap();
    builds::atomic_json(
        &app.join("registration.json"),
        &json!({"provenance":{"sourceId":fixture.source}}),
    )
    .unwrap();
    let result = fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .unwrap();
    assert_eq!(result["completed"], true, "{result}");
    assert_eq!(result["sourceRetained"], true);
    assert!(!fixture.source_path().join(".acquiring").exists());
    assert!(fixture.source_path().join("repository/.git/keep").is_file());
    assert!(!stage.exists());
    assert_eq!(
        fs::read_to_string(app.join("data/keep.db")).unwrap(),
        "persistent business data"
    );
    assert_eq!(
        fs::read_to_string(app.join("configuration/keep.json")).unwrap(),
        "private configuration"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn active_leases_block_and_stopped_unregistered_material_is_completely_removed() {
    let fixture = Fixture::new();
    let lease = fixture
        .store
        .reserve_transient_operation("source:disposable", "test-active")
        .unwrap();
    assert!(fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .is_err());
    assert!(fixture.source_path().join(".acquiring").exists());
    drop(lease);
    let stage = fixture.staging("committing");
    assert_eq!(
        fixture
            .cleanup
            .source_import_cleanup(&fixture.task.task_id)
            .unwrap()["completed"],
        true
    );
    assert!(!stage.exists());
    assert!(!fixture.source_path().exists());
    fixture.stop().await;
}

#[tokio::test]
async fn symlink_targets_and_task_identity_mismatch_are_not_followed() {
    let fixture = Fixture::new();
    let protected = fixture.paths.data_root().join("disposable-author-keep");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("keep"), "keep").unwrap();
    fs::remove_dir_all(fixture.source_path().join(".acquiring")).unwrap();
    std::os::unix::fs::symlink(&protected, fixture.source_path().join(".acquiring")).unwrap();
    // Descriptor-relative source-tree deletion unlinks the symlink and never
    // follows it; an installed source instead rejects the symlink temp target.
    let app = fixture
        .paths
        .data_root()
        .join("node-apps")
        .join(&fixture.owner);
    builds::atomic_json(
        &app.join("registration.json"),
        &json!({"sourceId":fixture.source}),
    )
    .unwrap();
    assert!(fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .is_err());
    assert_eq!(fs::read_to_string(protected.join("keep")).unwrap(), "keep");
    let other = fixture.tasks.create_workspace_operation("build").unwrap();
    fixture.tasks.finish(&other.task_id, Ok(json!({}))).unwrap();
    assert!(fixture
        .cleanup
        .source_import_cleanup(&other.task_id)
        .is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn partially_removed_material_is_cleared_using_current_project_ownership() {
    let fixture = Fixture::new();
    let stage = fixture.staging("failed");
    let mut item = Candidate::new(
        "disposable-partial".into(),
        "ownedDirectory",
        "disposable",
        stage.clone(),
    );
    item.root_identity = Some(directory_identity(&stage).unwrap());
    item.parents = parent_identities(&stage).unwrap();
    item.entries = scan(&stage).unwrap();
    let failed = remove_tree_with_hook(&item, |progress, _| {
        if progress
            .removed_paths
            .contains(&PathBuf::from("install.json"))
        {
            return Err(ApiError::new(
                "DISPOSABLE_PARTIAL",
                "artificial interruption after journal unlink",
            ));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(failed.code, "DISPOSABLE_PARTIAL");
    assert!(stage.exists());
    assert!(!stage.join("install.json").exists());
    assert_eq!(
        fixture
            .cleanup
            .source_import_cleanup(&fixture.task.task_id)
            .unwrap()["completed"],
        true
    );
    assert!(!stage.exists());
    assert!(!fixture.source_path().exists());
    fixture.stop().await;
}

#[tokio::test]
async fn known_uncommitted_apply_cleans_only_after_the_actual_pointer_is_checked() {
    let fixture = Fixture::new();
    let stage = fixture.staging("failed");
    let mut journal = read_json(&stage.join("install.json")).unwrap();
    let apply = fixture
        .tasks
        .create_node_operation(&fixture.owner, "nodeInstallApply")
        .unwrap();
    fixture
        .tasks
        .finish_partial(
            &apply.task_id,
            json!({"operationOutcome":"not-committed"}),
            ApiError::new("DISPOSABLE_COMMIT_FAILURE", "known uncommitted")
                .outcome("not-committed"),
        )
        .unwrap();
    let app = journal["target"]["appId"].as_str().unwrap().to_owned();
    let generation = format!("gen_{}", Uuid::new_v4().simple());
    let provenance = json!({"sourceId":fixture.source,"sourceDigest":"a".repeat(64),"sourceCommit":null,"node":{"path":"/disposable/node","version":"test"},"npm":{"path":"/disposable/npm","version":"test"},"preparedAt":1});
    journal["prepared"] = json!({"candidateId":journal["candidateId"],"appId":app,"generation":generation,"manifest":{},"programDigest":"b".repeat(64),"provenance":provenance});
    journal["registration"] = json!({"version":2,"id":app,"kind":"managed","generation":generation,"manifestDigest":"c".repeat(64),"programDigest":"b".repeat(64),"provenance":provenance});
    journal["applyTaskId"] = json!(apply.task_id);
    journal["state"] = json!("committing");
    journal["error"] = json!(
        ApiError::new("DISPOSABLE_COMMIT_FAILURE", "known uncommitted").outcome("not-committed")
    );
    builds::atomic_json(&stage.join("install.json"), &journal).unwrap();
    let pointer = fixture
        .paths
        .data_root()
        .join("node-apps")
        .join(&app)
        .join("registration.json");
    builds::atomic_json(&pointer, &json!({"disposable":"unexpected pointer"})).unwrap();
    assert!(fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .is_err());
    assert!(stage.exists());
    fs::remove_file(&pointer).unwrap();
    let result = fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .unwrap();
    assert_eq!(result["completed"], true, "{result}");
    assert!(!stage.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn fresh_attempt_checks_and_cleans_new_finished_candidates() {
    let fixture = Fixture::new();
    fixture.staging("failed");
    let new_stage = fixture.staging("failed");
    let result = fixture
        .cleanup
        .source_import_cleanup(&fixture.task.task_id)
        .unwrap();
    assert_eq!(result["completed"], true, "{result}");
    assert!(!new_stage.exists());
    assert!(!fixture.source_path().exists());
    fixture.stop().await;
}

#[tokio::test]
async fn corrupted_resume_target_cannot_delete_an_unrelated_installation() {
    let fixture = Fixture::new();
    let stage = fixture.staging("failed");
    let mut plan = fixture.cleanup.import_plan(&fixture.task.task_id).unwrap();
    let other = stage
        .parent()
        .unwrap()
        .join(format!("install_{}", Uuid::new_v4().simple()));
    fs::create_dir_all(other.join("npm-cache")).unwrap();
    fs::write(other.join("npm-cache/keep"), "other import").unwrap();
    let mut unrelated = read_json(&stage.join("install.json")).unwrap();
    unrelated["candidateId"] = json!(other.file_name().unwrap().to_str().unwrap());
    unrelated["sourceId"] = json!(format!("src_{}", Uuid::new_v4().simple()));
    builds::atomic_json(&other.join("install.json"), &unrelated).unwrap();
    plan.targets.insert(
        0,
        ImportTarget {
            identity: PersistedIdentity::capture(&other).unwrap(),
            path: other.clone(),
        },
    );
    let target = plan.targets.first().unwrap();
    assert!(fixture
        .cleanup
        .validate_import_target(&plan, target)
        .is_err());
    assert_eq!(
        fs::read_to_string(other.join("npm-cache/keep")).unwrap(),
        "other import"
    );
    assert!(fixture.source_path().join("repository/app.js").is_file());
    fixture.stop().await;
}
