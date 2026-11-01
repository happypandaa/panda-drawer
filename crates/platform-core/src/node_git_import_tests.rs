use super::*;
use crate::git_environment::GitEnvironmentManager;
use crate::TaskStatus;
use std::{os::unix::fs::PermissionsExt, process::Command, time::Duration};

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Disposable Import",
            "-c",
            "user.email=disposable@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn repository(root: &Path) {
    fs::create_dir_all(root.join("web")).unwrap();
    fs::create_dir(root.join("server")).unwrap();
    fs::write(root.join("web/index.html"), "disposable import page").unwrap();
    fs::write(
        root.join("server/main.mjs"),
        "throw Error('application must never run during import')",
    )
    .unwrap();
    atomic_json(&root.join("panda.json"),&json!({"manifestVersion":1,"runtime":"node","app":{"name":"可丢弃·单目录导入","version":"1.0.0"},"web":{"root":"web","entry":"index.html"},"backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}})).unwrap();
}
struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    sources: Arc<SourceAcquisitionManager>,
    manager: Arc<NodeGitImportManager>,
    apps: Arc<NodeApps>,
    input: StartNodeGitImportInput,
    stop: watch::Sender<bool>,
}
async fn fixture(kind: &str) -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let base = temporary.path().canonicalize().unwrap();
    let author = base.join("disposable-import-author");
    repository(&author);
    let script_marker = base.join("must-never-run");
    let package = json!({"name":"disposable-import","version":"1.0.0","scripts":{"preinstall":format!("touch {}",script_marker.display()),"build":format!("touch {}",script_marker.display())}});
    if [
        "missing-lock",
        "dual-root",
        "npmrc",
        "locked-dependencies",
        "prebuilt",
    ]
    .contains(&kind)
    {
        atomic_json(&author.join("package.json"), &package).unwrap();
    }
    if ["dual-root", "npmrc", "locked-dependencies", "prebuilt"].contains(&kind) {
        atomic_json(&author.join("package-lock.json"),&json!({"name":"disposable-import","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-import","version":"1.0.0"}}})).unwrap();
    }
    if kind == "dual-root" {
        atomic_json(&author.join("server/package.json"), &package).unwrap();
        atomic_json(
            &author.join("server/package-lock.json"),
            &json!({"lockfileVersion":3,"packages":{"":{}}}),
        )
        .unwrap();
    }
    if kind == "npmrc" {
        fs::write(
            author.join(".npmrc"),
            format!(
                "cache={}\nignore-scripts=false\n",
                base.join("must-not-create-npm-cache").display()
            ),
        )
        .unwrap();
    }
    if kind == "missing-build" {
        // Node has no development declaration: required runtime entry files
        // must be present before dependency preparation or registration.
        fs::remove_file(author.join("server/main.mjs")).unwrap();
        atomic_json(&author.join("package.json"), &package).unwrap();
    }
    git(&author, &["init", "-b", "main"]);
    git(&author, &["add", "."]);
    git(&author, &["commit", "-m", "disposable import input"]);
    let bin = base.join("disposable-import-tools");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper,format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'git version 2.56.0\\n'; exit 0; fi\nis_clone=\nlast=\nfor argument do\n if [ \"$argument\" = clone ]; then is_clone=yes; fi\n last=$argument\ndone\nif [ \"$is_clone\" = yes ]; then GIT_ALLOW_PROTOCOL=file; export GIT_ALLOW_PROTOCOL; exec /usr/bin/git -c protocol.file.allow=always clone --no-checkout --no-local --depth=1 --single-branch --no-tags -- 'file://{}' \"$last\"; fi\nexec /usr/bin/git \"$@\"\n",author.display())).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    if ["locked-dependencies", "prebuilt"].contains(&kind) {
        let node = bin.join("node");
        fs::write(&node,"#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(bin.join("npm"),"set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\ncase \"$*\" in *--ignore-scripts*) ;; *) exit 81;; esac\ncase \"$*\" in *--omit=dev*) ;; *) exit 82;; esac\nmkdir -p node_modules\nprintf 'in-place dependency fixture' > node_modules/disposable-import-marker\nprintf '%s\\n' \"$@\" > node_modules/disposable-npm-arguments\n").unwrap();
        tasks
            .environment()
            .configure_node(Some(node))
            .await
            .unwrap();
    }
    let environment =
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes());
    environment.configure_git(Some(wrapper)).await.unwrap();
    let sources =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), environment)
            .unwrap();
    let manager = NodeGitImportManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        Ok(sources.clone()),
    )
    .unwrap();
    let apps = Arc::new(
        NodeApps::new(
            paths.data_root().into(),
            base.join("must-not-run-runtime-node"),
        )
        .unwrap(),
    );
    apps.set_source_workspace(paths.workspace_root().into())
        .unwrap();
    let (stop, _) = watch::channel(false);
    let input = StartNodeGitImportInput {
        import_id: Uuid::new_v4().to_string(),
        url: "https://github.com/example/disposable-import.git".into(),
        branch: None,
        name: None,
    };
    Fixture {
        _temporary: temporary,
        paths,
        store,
        tasks,
        sources,
        manager,
        apps,
        input,
        stop,
    }
}
async fn completed(f: &Fixture) -> ImportSnapshot {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let value = f.manager.status(&f.input.import_id).unwrap();
            if value.task.status.is_final() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn direct_import_uses_one_uuid_one_project_and_deduplicates_lost_replies() {
    let f = fixture("zero-dependency").await;
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let duplicate = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(accepted.task_id, duplicate.task_id);
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert!(!result.cleanup_pending);
    let source = result.source_path.unwrap();
    assert!(source.join(".git").is_dir());
    let registration: Value = serde_json::from_slice(
        &fs::read(
            f.paths
                .data_root()
                .join("node-apps")
                .join(&result.app_id)
                .join("registration.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        registration["version"],
        crate::node_registration::DIRECT_NODE_REGISTRATION_VERSION
    );
    assert_eq!(registration["id"], result.app_id);
    assert_eq!(registration["sourcePath"], json!(source));
    assert!(!f
        .paths
        .data_root()
        .join("node-apps")
        .join(&result.app_id)
        .join("generations")
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-apps/.installations")
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&accepted.task_id)
        .exists());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/source-publications")
        .join(&result.app_id)
        .exists());
    assert!(!f
        .manager
        .journal_path(&f.manager.records.lock().unwrap()[&f.input.import_id])
        .unwrap()
        .exists());
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert_eq!(
        restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap()
            .task_id,
        accepted.task_id
    );
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().commit_state,
        CommitState::Committed
    );
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn invalid_dependency_or_missing_build_inputs_rollback_without_executing_scripts() {
    for kind in ["missing-lock", "dual-root", "npmrc", "missing-build"] {
        let f = fixture(kind).await;
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        let result = completed(&f).await;
        assert_eq!(
            result.commit_state,
            CommitState::NotCommitted,
            "{kind}: {:?}",
            result.task
        );
        assert_eq!(result.task.status, TaskStatus::Failed, "{kind}");
        let expected = if kind == "missing-build" {
            "SOURCE_NEEDS_ADAPTATION"
        } else {
            "NODE_DEPENDENCY_NEEDS_ADAPTATION"
        };
        assert_eq!(
            result.task.error.as_ref().unwrap().code,
            expected,
            "{kind}: {:?}",
            result.task
        );
        assert!(!result.cleanup_pending, "{kind}: {:?}", result.task);
        assert!(!f
            .paths
            .data_root()
            .join("node-project-ownership")
            .join(&result.app_id)
            .exists());
        assert!(!f._temporary.path().join("must-never-run").exists());
        assert!(!f
            ._temporary
            .path()
            .join("must-not-create-npm-cache")
            .exists());
        if let Some(source) = result.source_path {
            assert!(!source.exists(), "{kind}");
        }
        for dir in [
            f.paths.workspace_root().join(".platform/sources"),
            f.paths.workspace_root().join("project-sources"),
        ] {
            if dir.is_dir() {
                assert!(fs::read_dir(dir).unwrap().next().is_none(), "{kind}");
            }
        }
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_waits_for_the_actual_materializer_before_removing_any_source() {
    let f = fixture("zero-dependency").await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    f.sources.pause_import_writer(entered, blocked);
    let accepted = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    let j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    let slot = f
        .paths
        .workspace_root()
        .join(".platform/sources")
        .join(&j.source_id);
    let busy = f
        .manager
        .start(
            f.apps.clone(),
            StartNodeGitImportInput {
                import_id: Uuid::new_v4().to_string(),
                ..f.input.clone()
            },
            f.stop.subscribe(),
        )
        .await
        .unwrap_err();
    assert_eq!(busy.details["accepted"], false);
    assert_eq!(busy.details["operationOutcome"], "not_started");
    f.manager.cancel(&accepted.task_id).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.store.processes().snapshot().iter().any(|p| {
                p.component == "source-acquisition/materialize"
                    && p.phase == crate::owned_process::ProcessPhase::CleanupPending
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(slot.is_dir());
    assert!(f.store.has_active_tasks());
    assert!(!f
        .manager
        .status(&f.input.import_id)
        .unwrap()
        .task
        .status
        .is_final());
    release.send(()).unwrap();
    let result = completed(&f).await;
    assert_eq!(result.commit_state, CommitState::NotCommitted);
    assert!(!result.cleanup_pending, "{:?}", result.task);
    assert!(!slot.exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-project-ownership")
        .join(&j.app_id)
        .exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_reconciles_a_committed_registration_without_replaying_the_add() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    f.manager.shutdown().await;
    let mut j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    j.registration_attempted = true;
    j.commit_state = CommitState::Unknown;
    j.phase = "registering".into();
    j.final_task = None;
    j.cleanup_pending = true;
    f.manager.save(&j).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].commit_state, CommitState::Committed);
    assert!(!recovered[0].cleanup_pending);
    assert_eq!(recovered[0].app_id, result.app_id);
    assert!(result.source_path.unwrap().is_dir());
    assert_eq!(
        restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap()
            .task_id,
        result.task.task_id
    );
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn an_unreadable_registration_is_unknown_and_retry_never_deletes_the_project() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    f.manager.shutdown().await;
    let mut j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
    j.registration_attempted = true;
    j.commit_state = CommitState::Unknown;
    j.phase = "registering".into();
    j.final_task = None;
    j.cleanup_pending = true;
    f.manager.save(&j).unwrap();
    let registration = f
        .paths
        .data_root()
        .join("node-apps")
        .join(&j.app_id)
        .join("registration.json");
    let original = fs::read(&registration).unwrap();
    fs::write(&registration, b"invalid interrupted registration").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered[0].commit_state, CommitState::Unknown);
    assert!(recovered[0].cleanup_pending);
    assert!(j.source_path.as_ref().unwrap().is_dir());
    assert!(restarted.require_migration_clear().is_err());
    fs::write(&registration, original).unwrap();
    let resolved = restarted
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert_eq!(resolved.commit_state, CommitState::Committed);
    assert!(!resolved.cleanup_pending);
    assert!(j.source_path.unwrap().is_dir());
    f.tasks.shutdown().await;
}

fn interrupted_before_registration(f: &Fixture) -> Journal {
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let task = f.tasks.create_node_operation(&id, "nodeGitImport").unwrap();
    let source = f
        .paths
        .workspace_root()
        .join("project-sources/disposable-interrupted-import");
    private_dir(source.parent().unwrap()).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("partial"), "disposable interrupted project").unwrap();
    let identity = capture(&source).unwrap();
    ownership
        .reserve_published_source_identity(&id, "disposable-interrupted-import", &identity)
        .unwrap();
    let operation = f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&task.task_id);
    ownership.record_onboarding(&id, &task.task_id).unwrap();
    private_dir(&operation).unwrap();
    ownership.record_onboarding(&id, &task.task_id).unwrap();
    fs::write(operation.join("partial-cache"), "temporary cache").unwrap();
    let mut temporary = BTreeMap::new();
    temporary.insert(operation.clone(), capture(&operation).unwrap());
    let j = Journal {
        version: 1,
        input: StartNodeGitImportInput {
            import_id: Uuid::new_v4().to_string(),
            ..f.input.clone()
        },
        app_id: id.clone(),
        task_id: task.task_id,
        source_id: format!("src_{}", Uuid::new_v4().simple()),
        workspace: f.paths.workspace_root().into(),
        workspace_identity: capture(f.paths.workspace_root()).unwrap(),
        owner_identity: capture(&ownership.directory(&id).unwrap()).unwrap(),
        phase: "dependencies".into(),
        registration_attempted: false,
        commit_state: CommitState::NotCommitted,
        source_path: Some(source),
        source_identity: Some(identity),
        manifest_digest: None,
        temporary,
        cleanup_pending: false,
        remaining_paths: vec![],
        app: None,
        final_task: None,
    };
    f.manager.save(&j).unwrap();
    j
}
#[tokio::test]
async fn startup_rolls_back_uncommitted_material_and_ownership() {
    let f = fixture("zero-dependency").await;
    let j = interrupted_before_registration(&f);
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].commit_state, CommitState::NotCommitted);
    assert!(!recovered[0].cleanup_pending);
    assert!(!j.source_path.unwrap().exists());
    assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&j.task_id)
        .exists());
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn cleanup_retry_keeps_a_replacement_directory_until_the_original_identity_is_restored() {
    let f = fixture("zero-dependency").await;
    let j = interrupted_before_registration(&f);
    let source = j.source_path.clone().unwrap();
    let held = f._temporary.path().join("held-disposable-original");
    fs::rename(&source, &held).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("foreign"), "foreign directory must remain").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    let recovered = restarted.recover(f.apps.clone()).await.unwrap();
    assert!(recovered[0].cleanup_pending);
    assert_eq!(
        fs::read(source.join("foreign")).unwrap(),
        b"foreign directory must remain"
    );
    assert!(restarted.require_migration_clear().is_err());
    let foreign = f._temporary.path().join("held-disposable-foreign");
    fs::rename(&source, &foreign).unwrap();
    fs::rename(&held, &source).unwrap();
    let retried = restarted
        .retry_cleanup(f.apps.clone(), &j.task_id)
        .await
        .unwrap();
    assert!(!retried.cleanup_pending);
    assert!(!source.exists());
    assert_eq!(
        fs::read(foreign.join("foreign")).unwrap(),
        b"foreign directory must remain"
    );
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn imported_direct_source_is_owned_and_complete_deletion_removes_every_import_path() {
    let f = fixture("zero-dependency").await;
    let task = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let imported = completed(&f).await;
    assert_eq!(
        imported.commit_state,
        CommitState::Committed,
        "{:?}",
        imported.task
    );
    f.manager.shutdown().await;
    let source = imported.source_path.unwrap();
    let preview = f.apps.delete_preview(&imported.app_id).await.unwrap();
    assert_eq!(preview["ownership"], "managed");
    assert_eq!(preview["preservedPaths"], json!([]));
    assert!(preview["deletePaths"]
        .as_array()
        .unwrap()
        .contains(&json!(source)));
    let input =
        serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]})).unwrap();
    f.apps.delete(&imported.app_id, input).await.unwrap();
    for path in [
        source,
        f.paths.data_root().join("node-apps").join(&imported.app_id),
        f.paths
            .data_root()
            .join("node-project-ownership")
            .join(&imported.app_id),
        f.paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&imported.app_id),
        f.paths
            .data_root()
            .join("onboarding/imports")
            .join(&task.task_id),
        f.paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&task.task_id),
    ] {
        assert!(!path.exists(), "{}", path.display());
    }
    // The original author repository supplied by the disposable Git wrapper is
    // an external input, never inferred to belong to the imported project.
    assert!(f
        ._temporary
        .path()
        .join("disposable-import-author/.git")
        .is_dir());
    assert!(
        f.manager.status(&f.input.import_id).is_err(),
        "headless deletion must invalidate cached success before a native callback"
    );
    let rejected = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(rejected.details["accepted"], false);
    f.manager.forget_deleted_project(&imported.app_id);
    assert!(f.manager.status(&f.input.import_id).is_err());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn prebuilt_runtime_files_do_not_execute_repository_build_scripts() {
    let f = fixture("prebuilt").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    assert!(!f._temporary.path().join("must-never-run").exists());
    assert!(result
        .source_path
        .unwrap()
        .join("server/main.mjs")
        .is_file());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}
#[tokio::test]
async fn explicit_branch_is_persisted_and_a_missing_branch_rolls_back_the_same_import() {
    for branch in ["main", "missing-disposable-branch"] {
        let mut f = fixture("zero-dependency").await;
        f.input.branch = Some(branch.into());
        f.manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        let result = completed(&f).await;
        if branch == "main" {
            assert_eq!(
                result.commit_state,
                CommitState::Committed,
                "{:?}",
                result.task
            );
            let registration: Value = serde_json::from_slice(
                &fs::read(
                    f.paths
                        .data_root()
                        .join("node-apps")
                        .join(&result.app_id)
                        .join("registration.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(registration["provenance"]["sourceBranch"], "main");
        } else {
            assert_eq!(result.commit_state, CommitState::NotCommitted);
            assert_eq!(
                result.task.error.as_ref().unwrap().code,
                "SOURCE_BRANCH_UNAVAILABLE"
            );
            assert!(!result.cleanup_pending);
        }
        f.manager.shutdown().await;
        f.tasks.shutdown().await;
    }
}

#[tokio::test]
async fn dependencies_are_prepared_in_the_published_source_and_private_npm_state_is_removed() {
    let f = fixture("locked-dependencies").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let result = completed(&f).await;
    assert_eq!(
        result.commit_state,
        CommitState::Committed,
        "{:?}",
        result.task
    );
    let source = result.source_path.unwrap();
    assert_eq!(
        fs::read(source.join("node_modules/disposable-import-marker")).unwrap(),
        b"in-place dependency fixture"
    );
    let arguments =
        fs::read_to_string(source.join("node_modules/disposable-npm-arguments")).unwrap();
    for flag in ["--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund"] {
        assert!(arguments.lines().any(|line| line == flag), "{arguments}");
    }
    assert!(!f
        .paths
        .data_root()
        .join("onboarding/imports")
        .join(&result.task.task_id)
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("node-apps/.installations")
        .exists());
    assert!(!f._temporary.path().join("must-never-run").exists());
    f.manager.shutdown().await;
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn a_replaced_completed_source_cannot_block_startup_or_be_adopted() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let done = completed(&f).await;
    assert_eq!(done.commit_state, CommitState::Committed);
    f.manager.shutdown().await;
    let source = done.source_path.unwrap();
    let held = f
        ._temporary
        .path()
        .join("held-disposable-completed-original");
    fs::rename(&source, &held).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("foreign"), "must remain untouched").unwrap();
    std::os::unix::fs::symlink("/not-a-file-to-inspect", source.join("must-not-follow")).unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.issues().is_empty());
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().app_id,
        done.app_id
    );
    assert_eq!(
        fs::read(source.join("foreign")).unwrap(),
        b"must remain untouched"
    );
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    assert!(held.join("server/main.mjs").is_file());
    assert!(f.apps.list().await.is_ok());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn definite_rejections_and_failed_acceptance_do_not_leave_an_owner() {
    let f = fixture("zero-dependency").await;
    let mut invalid = f.input.clone();
    invalid.import_id = "invalid".into();
    let issue = f
        .manager
        .start(f.apps.clone(), invalid, f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    f.tasks.shutdown().await;
    let issue = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    assert!(
        issue.details.get("acceptanceCleanupError").is_none(),
        "{issue:?}"
    );
    let owners = f.paths.data_root().join("node-project-ownership");
    if owners.is_dir() {
        assert!(fs::read_dir(owners).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn a_bad_pending_journal_is_isolated_while_existing_projects_remain_available() {
    let f = fixture("zero-dependency").await;
    f.manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    let done = completed(&f).await;
    assert_eq!(done.commit_state, CommitState::Committed);
    f.manager.shutdown().await;
    let pending = interrupted_before_registration(&f);
    let journal = f.manager.journal_path(&pending).unwrap();
    fs::write(&journal, b"invalid disposable journal").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert_eq!(restarted.issues().len(), 1);
    assert_eq!(
        restarted.status(&f.input.import_id).unwrap().app_id,
        done.app_id
    );
    assert!(f.apps.list().await.is_ok());
    assert!(restarted.require_migration_clear().is_err());
    assert!(restarted.recover(f.apps.clone()).await.is_err());
    assert_eq!(fs::read(journal).unwrap(), b"invalid disposable journal");
    assert!(pending.source_path.unwrap().is_dir());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_cleans_a_task_created_before_the_initial_journal_was_published() {
    let f = fixture("zero-dependency").await;
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let task = f.tasks.create_node_operation(&id, "nodeGitImport").unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.require_migration_clear().is_err());
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    assert!(!ownership.directory(&id).unwrap().exists());
    assert!(f.tasks.task(&task.task_id).is_err());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(task.task_id)
        .exists());
    assert!(restarted.require_migration_clear().is_ok());
    f.tasks.shutdown().await;
}

#[tokio::test]
async fn startup_cleans_an_admission_interrupted_before_task_creation() {
    let f = fixture("zero-dependency").await;
    let ownership = f.manager.ownership();
    let id = Uuid::new_v4().to_string();
    ownership.initialize(&id).unwrap();
    let directory = ownership.directory(&id).unwrap();
    atomic_json(
        &directory.join("import-request.json"),
        &Admission {
            version: 1,
            input: f.input.clone(),
            app_id: id.clone(),
            workspace: f.paths.workspace_root().into(),
            workspace_identity: capture(f.paths.workspace_root()).unwrap(),
            owner_identity: capture(&directory).unwrap(),
        },
    )
    .unwrap();
    let restarted = NodeGitImportManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        Ok(f.sources.clone()),
    )
    .unwrap();
    assert!(restarted.require_migration_clear().is_err());
    restarted.recover(f.apps.clone()).await.unwrap();
    assert!(!directory.exists());
    assert!(restarted.require_migration_clear().is_ok());
    f.tasks.shutdown().await;
}
