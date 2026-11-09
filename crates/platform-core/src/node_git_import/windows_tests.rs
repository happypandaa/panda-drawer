//! Windows coordinator tests use only RuntimePaths' owned TempDir. Network,
//! official MinGit, npm and a running Node app require the explicit ignored tests.
use super::*;
use crate::{
    git_environment::GitEnvironmentManager,
    node_app_configuration::{SaveConfigurationRequest, SecretAction},
    owned_process::ProcessPhase,
    TaskStatus,
};
use std::time::Duration;

const PUBLIC_URL: &str = "https://github.com/happypandaa/panda-drawer-demo-node.git";
const PUBLIC_BRANCH: &str = "codex/manual-demo";
const RUNTIME_PUBLIC_URL: &str = "https://github.com/happypandaa/radio-bob.git";
const RUNTIME_PUBLIC_BRANCH: &str = "main";
const RUNTIME_PUBLIC_COMMIT: &str = "163923d99f4b8cce188e3372dc135d4ae41bee78";
const ARTIFICIAL_SECRET: &str = "artificial-disposable-w4-local-status-only-key";
const IMPORT_WAIT: Duration = Duration::from_secs(900);
const RESOURCE_WAIT: Duration = Duration::from_secs(30);

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<NodeGitImportManager>,
    apps: Arc<ProjectStore>,
    input: StartNodeGitImportInput,
    stop: watch::Sender<bool>,
    runtime: tokio::sync::Mutex<Option<Arc<TestRuntime>>>,
}

fn source_unavailable() -> ApiError {
    ApiError::new(
        "DISPOSABLE_SOURCE_UNAVAILABLE",
        "No source process in this test",
    )
    .detail("retainedMaterial", "disposable source capability evidence")
}

fn fixture() -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(
        ProjectStore::with_paths(
            paths.workspace_root().into(),
            paths.data_root().into(),
            None,
            Default::default(),
        )
        .unwrap(),
    );
    let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
    let manager = NodeGitImportManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        Err(source_unavailable()),
    )
    .unwrap();
    let apps = store.clone();
    let (stop, _) = watch::channel(false);
    Fixture {
        _temporary: temporary,
        paths,
        store,
        tasks,
        manager,
        apps,
        input: StartNodeGitImportInput {
            selected_directory: None,
            alternate_url: None,
            catalog_id: None,
            import_id: Uuid::new_v4().to_string(),
            url: PUBLIC_URL.into(),
            branch: Some(PUBLIC_BRANCH.into()),
            name: Some("可丢弃 Windows Git 导入 中文 space".into()),
        },
        stop,
        runtime: tokio::sync::Mutex::new(None),
    }
}

impl Fixture {
    fn reopen(&self, sources: Result<Arc<SourceAcquisitionManager>>) -> Arc<NodeGitImportManager> {
        NodeGitImportManager::new(
            self.paths.clone(),
            self.store.clone(),
            self.tasks.clone(),
            sources,
        )
        .unwrap()
    }

    async fn runtime(&self) -> Arc<TestRuntime> {
        let mut current = self.runtime.lock().await;
        if let Some(runtime) = &*current {
            return runtime.clone();
        }
        let runtime = Arc::new(TestRuntime::open(&self.paths, official_node()).await);
        *current = Some(runtime.clone());
        runtime
    }

    async fn close(&self) {
        // Attempt every actual resource closure before reporting any failed
        // receipt; an earlier assertion must not skip a live app's stop.
        let imports = self.manager.shutdown().await;
        if let Some(runtime) = self.runtime.lock().await.take() {
            runtime.shutdown().await.unwrap();
        }
        let tasks = self.tasks.shutdown().await;
        let processes = self.store.processes().shutdown().await;
        let snapshots = self.store.processes().snapshot();
        assert!(imports.is_empty(), "{imports:?}");
        assert!(tasks.is_empty(), "{tasks:?}");
        assert!(processes.is_empty(), "{processes:?}");
        assert!(
            snapshots.iter().all(|process| process.resource_closed()),
            "{snapshots:?}"
        );
    }
}

// Capture bytes and full native identities, rather than using mtime to claim
// that rejected admission or an unreadable journal left our materials alone.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Value, Option<Vec<u8>>)> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, (Value, Option<Vec<u8>>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!platform_native::filesystem::is_reparse(&metadata));
        let bytes = if metadata.is_dir() {
            None
        } else {
            assert!(metadata.is_file());
            Some(fs::read(path).unwrap())
        };
        let identity = capture(path).unwrap();
        assert!(identity.windows_file_id.is_some());
        result.insert(
            path.strip_prefix(root).unwrap().into(),
            (serde_json::to_value(identity).unwrap(), bytes),
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn queued(f: &Fixture) -> Journal {
    let app_id = Uuid::new_v4().to_string();
    let ownership = f.manager.ownership();
    ownership.initialize(&app_id).unwrap();
    let task = f
        .tasks
        .create_node_operation(&app_id, "nodeGitImport")
        .unwrap();
    let journal = Journal {
        selected_root_identity: None,
        download_url: None,
        version: 1,
        input: f.input.clone(),
        app_id: app_id.clone(),
        task_id: task.task_id,
        source_id: format!("src_{}", Uuid::new_v4().simple()),
        workspace: f.paths.workspace_root().into(),
        workspace_identity: capture(f.paths.workspace_root()).unwrap(),
        owner_identity: Some(capture(&ownership.directory(&app_id).unwrap()).unwrap()),
        has_backend: Some(true),
        web_project_id: None,
        build_confirmation: None,
        agent_placement: None,
        retiring_control: false,
        phase: "queued".into(),
        registration_attempted: false,
        commit_state: CommitState::NotCommitted,
        source_path: None,
        source_identity: None,
        manifest_digest: None,
        temporary: BTreeMap::new(),
        cleanup_pending: false,
        remaining_paths: vec![],
        app: None,
        final_task: None,
    };
    f.manager.save(&journal).unwrap();
    journal
}

fn published_without_running(f: &Fixture, j: &mut Journal) -> PathBuf {
    let source = f
        .paths
        .workspace_root()
        .join("project-sources/disposable-w4-registration");
    private_dir(&source).unwrap();
    private_dir(&source.join("web")).unwrap();
    private_dir(&source.join("server")).unwrap();
    fs::write(
        source.join("web/index.html"),
        "disposable registration only",
    )
    .unwrap();
    fs::write(
        source.join("server/main.mjs"),
        "throw Error('registration-only fixture must never run')",
    )
    .unwrap();
    atomic_json(
        &source.join("panda.json"),
        &json!({
            "manifestVersion":2,
            "app":{"name":"登记三态","version":"1"},
            "web":{"root":"web","entry":"index.html"},
            "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        }),
    )
    .unwrap();
    let identity = capture(&source).unwrap();
    f.manager
        .ownership()
        .reserve_published_source_identity(&j.app_id, "disposable-w4-registration", &identity)
        .unwrap();
    j.source_path = Some(source.clone());
    j.source_identity = Some(identity);
    j.manifest_digest = Some(format!(
        "{:x}",
        Sha256::digest(fs::read(source.join("panda.json")).unwrap())
    ));
    j.phase = "registering".into();
    f.manager.save(j).unwrap();
    source
}

#[tokio::test]
async fn admission_rejects_invalid_requests_and_actual_source_failure_before_new_material() {
    let f = fixture();
    let before = snapshot(f._temporary.path());
    let mut bad = f.input.clone();
    bad.import_id = "not-a-request-uuid".into();
    let issue = f
        .manager
        .start(f.apps.clone(), bad, f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.code, "NODE_GIT_IMPORT_INVALID");
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    assert_eq!(snapshot(f._temporary.path()), before);
    let issue = f
        .manager
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.code, "DISPOSABLE_SOURCE_UNAVAILABLE");
    assert_eq!(
        issue.details["retainedMaterial"],
        "disposable source capability evidence"
    );
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(issue.details["operationOutcome"], "not_started");
    assert_eq!(snapshot(f._temporary.path()), before);
    assert!(f.manager.records.lock().unwrap().is_empty());
    assert!(f.store.processes().snapshot().is_empty());
    f.close().await;
}

#[tokio::test]
async fn journal_roundtrip_deduplicates_before_unavailable_sources_without_reaccepting() {
    let f = fixture();
    let j = queued(&f);
    let before = snapshot(f._temporary.path());
    let restarted = f.reopen(Err(source_unavailable()));
    let loaded = restarted.status(&f.input.import_id).unwrap();
    assert_eq!(loaded.app_id, j.app_id);
    assert_eq!(loaded.commit_state, CommitState::NotCommitted);
    let duplicate = restarted
        .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(duplicate.task_id, j.task_id);
    let mut changed = f.input.clone();
    changed.branch = Some("another-disposable-branch".into());
    let issue = restarted
        .start(f.apps.clone(), changed, f.stop.subscribe())
        .await
        .unwrap_err();
    assert_eq!(issue.code, "NODE_GIT_IMPORT_INVALID");
    assert_eq!(issue.details["accepted"], false);
    assert_eq!(snapshot(f._temporary.path()), before);
    assert_eq!(restarted.records.lock().unwrap().len(), 1);
    assert!(restarted.active.lock().unwrap().is_empty());
    assert!(f.store.processes().snapshot().is_empty());
    assert!(restarted.shutdown().await.is_empty());
    f.close().await;
}

#[tokio::test]
async fn known_cancel_signals_only_its_accepted_task_without_starting_a_child() {
    let f = fixture();
    let j = queued(&f);
    let (sender, mut receiver) = watch::channel(false);
    f.manager
        .active
        .lock()
        .unwrap()
        .insert(j.task_id.clone(), sender);
    let before = snapshot(f._temporary.path());
    let result = f.manager.cancel(&j.task_id).unwrap();
    tokio::time::timeout(RESOURCE_WAIT, receiver.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(*receiver.borrow());
    assert_eq!(result.task_id, j.task_id);
    assert_eq!(result.status, TaskStatus::Queued);
    // This is signal admission, not a claim that a running writer has closed.
    assert!(!f
        .manager
        .status(&f.input.import_id)
        .unwrap()
        .task
        .status
        .is_final());
    assert_eq!(snapshot(f._temporary.path()), before);
    assert!(f.store.processes().snapshot().is_empty());
    f.manager.active.lock().unwrap().remove(&j.task_id);
    f.close().await;
}

#[tokio::test]
async fn interrupted_admission_recovers_only_its_actual_owner_and_task_without_a_child() {
    let f = fixture();
    let ownership = f.manager.ownership();
    let app_id = Uuid::new_v4().to_string();
    ownership.initialize(&app_id).unwrap();
    let owner = ownership.directory(&app_id).unwrap();
    let task = f
        .tasks
        .create_node_operation(&app_id, "nodeGitImport")
        .unwrap();
    atomic_json(
        &owner.join("import-request.json"),
        &Admission {
            selected_root_identity: None,
            version: 1,
            input: f.input.clone(),
            app_id,
            workspace: f.paths.workspace_root().into(),
            workspace_identity: capture(f.paths.workspace_root()).unwrap(),
            owner_identity: capture(&owner).unwrap(),
        },
    )
    .unwrap();
    let retained = f._temporary.path().join("unrelated-disposable-original");
    fs::create_dir(&retained).unwrap();
    fs::write(retained.join("keep"), "unrelated disposable evidence").unwrap();
    let before = snapshot(&retained);
    let restarted = f.reopen(Err(source_unavailable()));
    assert_eq!(restarted.unaccepted.lock().unwrap().len(), 1);
    assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
    assert!(!owner.exists());
    assert!(f.tasks.task(&task.task_id).is_err());
    assert_eq!(snapshot(&retained), before);
    restarted.require_migration_clear().unwrap();
    assert!(f.store.processes().snapshot().is_empty());
    assert!(restarted.shutdown().await.is_empty());
    f.close().await;
}

#[tokio::test]
async fn invalid_journal_bytes_or_version_block_recovery_and_preserve_all_actual_material() {
    for bad_version in [false, true] {
        let f = fixture();
        let mut j = queued(&f);
        let path = f.manager.journal_path(&j).unwrap();
        if bad_version {
            j.version = 999;
            fs::write(&path, serde_json::to_vec(&j).unwrap()).unwrap();
        } else {
            fs::write(&path, b"invalid disposable import journal").unwrap();
        }
        let before = snapshot(f._temporary.path());
        let restarted = f.reopen(Err(source_unavailable()));
        assert_eq!(restarted.issues().len(), 1);
        assert!(restarted.status(&f.input.import_id).is_err());
        assert_eq!(
            restarted.require_migration_clear().unwrap_err().code,
            "NODE_GIT_IMPORT_RECOVERY_REQUIRED"
        );
        assert_eq!(
            restarted.recover(f.apps.clone()).await.unwrap_err().code,
            "NODE_GIT_IMPORT_RECOVERY_REQUIRED"
        );
        assert_eq!(snapshot(f._temporary.path()), before);
        assert!(f.store.processes().snapshot().is_empty());
        assert!(restarted.shutdown().await.is_empty());
        f.close().await;
    }
}

#[tokio::test]
async fn actual_registration_reconciliation_distinguishes_all_three_commit_states_without_child() {
    let f = fixture();
    let mut j = queued(&f);
    let source = published_without_running(&f, &mut j);
    j.commit_state = CommitState::Unknown;
    f.manager.reconcile(&f.apps, &mut j).await.unwrap();
    assert_eq!(j.commit_state, CommitState::NotCommitted);
    j.registration_attempted = true;
    j.commit_state = CommitState::Unknown;
    f.manager.reconcile(&f.apps, &mut j).await.unwrap();
    assert_eq!(j.commit_state, CommitState::NotCommitted);
    let identity = capture(&source).unwrap();
    f.store
        .reserve_import_source(
            &j.app_id,
            &j.task_id,
            &source,
            &identity,
            crate::projects::ProjectOwnership::Platform,
        )
        .unwrap();
    let app = f
        .store
        .register_imported_git(
            &j.app_id,
            &j.task_id,
            &source,
            &j.input.url,
            j.input.branch.as_deref(),
            j.input.catalog_id.as_deref(),
            crate::projects::ProjectOwnership::Platform,
        )
        .await
        .unwrap();
    j.web_project_id = Some(j.app_id.clone());
    assert_eq!(app.id, j.app_id);
    f.manager.reconcile(&f.apps, &mut j).await.unwrap();
    assert_eq!(j.commit_state, CommitState::Committed);
    assert_eq!(j.app.as_ref().unwrap()["id"], j.app_id);
    let source_before = snapshot(&source);
    let registration = f
        .paths
        .data_root()
        .join("apps")
        .join(&j.app_id)
        .join("registration.json");
    fs::write(&registration, b"invalid disposable registration").unwrap();
    assert!(f.manager.reconcile(&f.apps, &mut j).await.is_err());
    assert_eq!(j.commit_state, CommitState::Unknown);
    assert_eq!(
        fs::read(&registration).unwrap(),
        b"invalid disposable registration"
    );
    assert_eq!(snapshot(&source), source_before);
    assert!(f.manager.ownership().directory(&j.app_id).unwrap().is_dir());
    let persisted: Journal =
        serde_json::from_slice(&fs::read(f.manager.journal_path(&j).unwrap()).unwrap()).unwrap();
    assert_eq!(persisted.commit_state, CommitState::Unknown);
    assert!(f.store.processes().snapshot().is_empty());
    f.close().await;
}

#[tokio::test]
async fn publishing_recovery_and_retry_keep_reserved_source_unknown_when_receipt_is_missing_or_bad()
{
    for malformed in [false, true] {
        let mut f = fixture();
        let mut j = queued(&f);
        let ownership = f.manager.ownership();
        let receipt = f
            .paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&j.app_id);
        ownership.record_publication(&j.app_id).unwrap();
        private_dir(&receipt).unwrap();
        ownership.record_publication(&j.app_id).unwrap();
        let payload = receipt.join(format!("payload_{}", Uuid::new_v4().simple()));
        private_dir(&payload).unwrap();
        fs::write(
            payload.join("retained-source"),
            "actual unfinished disposable source bytes",
        )
        .unwrap();
        let payload_identity = capture(&payload).unwrap();
        private_dir(&f.paths.workspace_root().join("project-sources")).unwrap();
        ownership
            .reserve_published_source_identity(
                &j.app_id,
                "disposable-w4-pending-publication",
                &payload_identity,
            )
            .unwrap();
        let receipt_file = receipt.join("publication.json");
        if malformed {
            fs::write(&receipt_file, b"{invalid disposable publication receipt").unwrap();
        }
        j.phase = "publishing".into();
        j.temporary
            .insert(receipt.clone(), capture(&receipt).unwrap());
        f.manager.save(&j).unwrap();
        // Construction is lazy and performs no Git lookup or spawn. Recovery
        // must inspect actual ownership/receipt evidence even without source.json.
        let sources = SourceAcquisitionManager::new(
            f.paths.clone(),
            f.store.clone(),
            f.tasks.clone(),
            GitEnvironmentManager::with_processes(
                f.paths.data_root().into(),
                None,
                f.store.processes(),
            ),
        )
        .unwrap();
        f.manager = f.reopen(Ok(sources));
        let owner = ownership.directory(&j.app_id).unwrap();
        let owner_identity = capture(&owner).unwrap();
        let ownership_bytes = fs::read(owner.join("ownership.json")).unwrap();
        let before = snapshot(&receipt);
        let recovered = f.manager.recover(f.apps.clone()).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].app_id, j.app_id);
        assert_eq!(recovered[0].task.task_id, j.task_id);
        assert_eq!(recovered[0].commit_state, CommitState::Unknown);
        assert!(recovered[0].cleanup_pending);
        assert!(recovered[0].source_path.is_none());
        let issue = recovered[0].task.error.as_ref().unwrap();
        assert_eq!(issue.details["sourcePublicationPending"], true);
        assert_eq!(issue.details["operationOutcome"], "unknown");
        assert!(recovered[0].remaining_paths.contains(&receipt));
        let retry = f
            .manager
            .retry_cleanup(f.apps.clone(), &j.task_id)
            .await
            .unwrap_err();
        assert_eq!(retry.details["sourcePublicationPending"], true);
        assert_eq!(retry.details["operationOutcome"], "unknown");
        let duplicate = f
            .manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        assert_eq!(duplicate.task_id, j.task_id);
        assert_eq!(f.manager.records.lock().unwrap().len(), 1);
        let retained = f.manager.status(&f.input.import_id).unwrap();
        assert_eq!(retained.app_id, j.app_id);
        assert_eq!(retained.commit_state, CommitState::Unknown);
        assert!(retained.cleanup_pending);
        assert!(owner_identity.matches_path(&owner).unwrap());
        assert_eq!(
            fs::read(owner.join("ownership.json")).unwrap(),
            ownership_bytes
        );
        assert_eq!(snapshot(&receipt), before);
        assert!(payload_identity.matches_path(&payload).unwrap());
        assert!(!f
            .paths
            .workspace_root()
            .join("project-sources/disposable-w4-pending-publication")
            .exists());
        assert!(f.store.processes().snapshot().is_empty());
        f.close().await;
    }
}

fn official_node() -> PathBuf {
    let node = PathBuf::from(
        std::env::var_os("PANDA_W3_OFFICIAL_NODE").expect("explicit pinned official Node required"),
    );
    assert!(node.is_absolute());
    let release: Value = serde_json::from_str(crate::node_tool_layout::RUNTIME_RELEASE).unwrap();
    assert_eq!(release["version"], "24.22.0");
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&node).unwrap())),
        release["binary"]["sha256"].as_str().unwrap()
    );
    node
}

async fn official_fixture(
    url: &str,
    branch: &str,
    name: &str,
) -> (Fixture, Arc<SourceAcquisitionManager>, PathBuf) {
    let node = official_node();
    let git = PathBuf::from(
        std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT")
            .expect("explicit disposable official MinGit sample required"),
    );
    assert!(git.is_absolute());
    assert!(git.components().any(|component| component
        .as_os_str()
        .to_string_lossy()
        .starts_with("disposable-w4-git-")));
    crate::managed_git::verify(git.clone()).await.unwrap();
    let mut f = fixture();
    f.input.url = url.into();
    f.input.branch = Some(branch.into());
    f.input.name = Some(name.into());
    let environment = f
        .tasks
        .environment()
        .configure_node(Some(node.clone()))
        .await
        .unwrap();
    assert!(environment.ready, "{environment:?}");
    let git_environment = GitEnvironmentManager::with_processes(
        f.paths.data_root().into(),
        Some(git.clone()),
        f.store.processes(),
    );
    let sources = SourceAcquisitionManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        git_environment,
    )
    .unwrap();
    f.manager = f.reopen(Ok(sources.clone()));
    (f, sources, git)
}

async fn completed(f: &Fixture) -> ImportSnapshot {
    // Node task phase/finish writes actual records but does not broadcast a
    // project BuildEvent. Poll the actual state; elapsed time is not evidence.
    tokio::time::timeout(IMPORT_WAIT, async {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        loop {
            tick.tick().await;
            let state = f.manager.status(&f.input.import_id).unwrap();
            if state.task.status.is_final() {
                return state;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Import did not finish: {:?}; processes: {:?}",
            f.manager.status(&f.input.import_id),
            f.store.processes().snapshot()
        )
    })
}

async fn wait_idle(f: &Fixture) {
    tokio::time::timeout(RESOURCE_WAIT, async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        loop {
            tick.tick().await;
            if f.manager.require_idle().is_ok() && !f.store.has_active_tasks() {
                return;
            }
        }
    })
    .await
    .expect("Actual import runner and project task leases must close");
}

async fn join_import_processes(f: &Fixture) -> Vec<crate::owned_process::ProcessSnapshot> {
    let before_join = f.store.processes().snapshot();
    assert!(
        before_join.iter().all(|process| {
            process.phase == ProcessPhase::Closed
                && process.leader_reaped
                && process.group_exited
                && process.io_joined
                && process.issues.is_empty()
        }),
        "Actual root/Job/I/O closure must precede the final driver join: {before_join:?}"
    );
    // wait_idle observes import leases, not each Registry driver's JoinHandle.
    // Explicitly join those drivers after actual root/Job/I/O closure. This
    // also seals the import Registry: subsequent dedup/recovery must not spawn.
    let issues = f.store.processes().shutdown().await;
    let processes = f.store.processes().snapshot();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(
        processes.iter().all(|process| process.resource_closed()),
        "{processes:?}"
    );
    processes
}

/// Test-only runtime owner: imports and execution share the unified registration,
/// and the official Node binary is still verified against the compiled digest.
struct TestRuntime {
    service: tokio::sync::Mutex<Option<crate::RuntimeService>>,
    handle: crate::RuntimeHandle,
}
impl TestRuntime {
    async fn open(paths: &RuntimePaths, node: PathBuf) -> Self {
        let root = paths.data_root().join("disposable-runtime-resources");
        for relative in [
            "schemas/panda.schema.json",
            "sdk/sdk.js",
            "sdk/panda.d.ts",
            "template/template.json",
            "template/app/package.json",
            "template/app/package-lock.json",
            "template/app/README.md",
            "template/app/src/panda.d.ts",
            "skills/local-projects-app/SKILL.md",
            "skills/local-projects-app/references/app-window-runtime.md",
        ] {
            let path = root.join(relative);
            if !path.exists() {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"disposable runtime resource").unwrap();
            }
        }
        fs::write(
            root.join("sdk/sdk.js"),
            include_bytes!("../../../../packages/sdk/sdk.js"),
        )
        .unwrap();
        let binary = crate::resources::runtime_node_path(&root);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        if !binary.exists() {
            fs::copy(node, &binary).unwrap();
        }
        let cli = root.join("disposable-cli.exe");
        if !cli.exists() {
            fs::write(&cli, b"never executed").unwrap();
        }
        let service = crate::RuntimeService::start_with_resources(
            paths.clone(),
            paths.acquire_lock().unwrap(),
            root,
            cli,
        )
        .await
        .unwrap();
        let handle = service.handle();
        Self {
            service: tokio::sync::Mutex::new(Some(service)),
            handle,
        }
    }
    async fn configuration(&self, id: &str) -> Result<Value> {
        self.handle.web_configuration(id).await
    }
    async fn save(&self, id: &str, input: SaveConfigurationRequest) -> Result<Value> {
        Ok(json!(self
            .handle
            .store()
            .save_web_configuration(id, input)?))
    }
    async fn start(&self, id: &str) -> Result<Value> {
        self.handle.launch(id).await?;
        Ok(json!({"id": id, "state": self.handle.web_status(id)["state"]}))
    }
    async fn open_url(&self, id: &str) -> Result<String> {
        Ok(self.handle.launch(id).await?.launch_url)
    }
    async fn stop(&self, id: &str) -> Result<Value> {
        self.handle.close_web(id).await
    }
    async fn owned_process_ids(&self) -> Vec<String> {
        self.handle
            .store()
            .processes()
            .snapshot()
            .into_iter()
            .filter(|process| !process.resource_closed())
            .map(|process| process.id)
            .collect()
    }
    async fn shutdown(&self) -> Result<()> {
        if let Some(service) = self.service.lock().await.take() {
            service
                .shutdown()
                .await
                .map_err(|error| ApiError::new("DISPOSABLE_RUNTIME_SHUTDOWN", error.to_string()))?;
        }
        Ok(())
    }
}

struct PublicAppBrowser {
    client: reqwest::Client,
    origin: String,
    cookie: String,
}
impl PublicAppBrowser {
    async fn open(apps: &TestRuntime, app_id: &str) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let bootstrap = apps.open_url(app_id).await.unwrap();
        let origin = url::Url::parse(&bootstrap)
            .unwrap()
            .origin()
            .ascii_serialization();
        let response = client.get(bootstrap).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
        let cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let response = client
            .get(format!("{origin}/"))
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert!(!response.text().await.unwrap().is_empty());
        Self {
            client,
            origin,
            cookie,
        }
    }

    async fn get(&self, route: &str) -> Value {
        // These are the only permitted RadioBob routes in this acceptance test:
        // both inspect local state/configuration and do not invoke paid/remote
        // AI, music search, login, playback or speech operations.
        assert!(matches!(route, "/api/status" | "/api/state"));
        let response = self
            .client
            .get(format!("{}{route}", self.origin))
            .header(reqwest::header::COOKIE, &self.cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "Actual local API {route}");
        serde_json::from_str(&response.text().await.unwrap()).unwrap()
    }
}

fn public_source_commit(source: &Path, branch: &str) -> String {
    let git = source.join(".git");
    let head = fs::read_to_string(git.join("HEAD"))
        .unwrap()
        .trim()
        .to_owned();
    let commit = if let Some(reference) = head.strip_prefix("ref: ") {
        assert_eq!(reference, format!("refs/heads/{branch}"));
        match fs::read_to_string(git.join(reference)) {
            Ok(commit) => commit.trim().to_owned(),
            Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => {
                fs::read_to_string(git.join("packed-refs"))
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        let (commit, name) = line.split_once(' ')?;
                        (name == reference).then(|| commit.to_owned())
                    })
                    .expect("Actual public HEAD must have a recorded commit")
            }
            Err(issue) => panic!("Cannot read actual public HEAD reference: {issue}"),
        }
    } else {
        head
    };
    assert!(matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()));
    commit
}

fn original_source_snapshot(root: &Path) -> BTreeMap<PathBuf, (Value, Option<String>)> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (Value, Option<String>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(!platform_native::filesystem::is_reparse(&metadata));
        let digest = if metadata.is_dir() {
            None
        } else {
            assert!(metadata.is_file());
            Some(format!("{:x}", Sha256::digest(fs::read(path).unwrap())))
        };
        output.insert(
            path.strip_prefix(root).unwrap().into(),
            (
                serde_json::to_value(capture(path).unwrap()).unwrap(),
                digest,
            ),
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                let child = child.unwrap();
                if path == root
                    && matches!(child.file_name().to_str(), Some(".git" | "node_modules"))
                {
                    continue;
                }
                visit(root, &child.path(), output);
            }
        }
    }
    // Exclude Git's internal state and npm's generated dependency tree. Every
    // original author-source file remains byte/identity bound across both runs.
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[tokio::test]
#[ignore = "Explicit PANDA_W4_GIT_SAMPLE_ROOT, fixed PANDA_W3_OFFICIAL_NODE and public GitHub/npm required; original RadioBob source, artificial key, local page/status/state only"]
async fn official_public_import_runs_npm_one_source_and_public_nodeapps_then_recovers_without_replay(
) {
    let (f, sources, git) = official_fixture(
        RUNTIME_PUBLIC_URL,
        RUNTIME_PUBLIC_BRANCH,
        "可丢弃 RadioBob 原码运行 中文 space",
    )
    .await;
    let result = AssertUnwindSafe(async {
        let accepted = f
            .manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        assert_eq!(
            f.manager
                .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
                .await
                .unwrap()
                .task_id,
            accepted.task_id
        );
        let done = completed(&f).await;
        wait_idle(&f).await;
        assert_eq!(done.task.status, TaskStatus::Succeeded, "{:?}", done.task);
        assert!(done.task.error.is_none(), "{:?}", done.task);
        assert_eq!(done.commit_state, CommitState::Committed);
        assert!(!done.cleanup_pending, "{:?}", done.task);
        assert!(done.remaining_paths.is_empty());
        assert!(Uuid::parse_str(&done.app_id).is_ok());
        assert_eq!(done.task.task_id, accepted.task_id);
        assert_eq!(done.task.result.as_ref().unwrap()["appId"], done.app_id);
        assert_eq!(
            done.task.result.as_ref().unwrap()["importId"],
            f.input.import_id
        );
        let source = done.source_path.as_ref().unwrap();
        assert_eq!(
            source.parent(),
            Some(f.paths.workspace_root().join("project-sources").as_path())
        );
        assert!(source.join(".git").is_dir());
        assert_eq!(fs::read_dir(source.parent().unwrap()).unwrap().count(), 1);
        let manifest_bytes = read_manifest(source).unwrap();
        let manifest = PandaManifest::parse_runtime(&manifest_bytes).unwrap();
        let npm_root = dependency_root(source, &manifest)
            .unwrap()
            .expect("RadioBob must exercise actual npm dependency preparation");
        assert!(npm_root.join("package-lock.json").is_file());
        assert!(npm_root.join("node_modules").is_dir());
        // Successful shared import includes npm's actual zero-exit-status check.
        // Its actual Job/root/I/O and driver must also all close.
        let processes = join_import_processes(&f).await;
        assert!(
                processes
                    .iter()
                    .any(|process| process.component == "node-install/npm"
                        && process.resource_closed()),
                "{processes:?}"
            );
        assert!(
            processes.iter().all(|process| process.resource_closed()),
            "{processes:?}"
        );
        let slot = f.paths.data_root().join("apps").join(&done.app_id);
        let registration: Value =
            serde_json::from_slice(&fs::read(slot.join("registration.json")).unwrap()).unwrap();
        assert_eq!(registration["schemaVersion"], 3);
        assert_eq!(registration["id"], done.app_id);
        assert_eq!(registration["sourcePath"], json!(source));
        assert_eq!(registration["provenance"]["url"], RUNTIME_PUBLIC_URL);
        assert_eq!(registration["provenance"]["branch"], RUNTIME_PUBLIC_BRANCH);
        assert!(!slot.join("generations").exists());
        assert!(!f
            .paths
            .data_root()
            .join("apps/.installations")
            .exists());
        assert!(!f
            .paths
            .data_root()
            .join("onboarding/imports")
            .join(&accepted.task_id)
            .exists());
        let j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
        assert_eq!(j.app_id, done.app_id);
        assert!(j.source_id.starts_with("src_"));
        assert!(!f
            .paths
            .workspace_root()
            .join(".platform/sources")
            .join(&j.source_id)
            .exists());
        assert!(!f
            .paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&j.app_id)
            .exists());
        assert!(!f.manager.journal_path(&j).unwrap().exists());
        assert_eq!(public_source_commit(source, RUNTIME_PUBLIC_BRANCH), RUNTIME_PUBLIC_COMMIT);
        let original_source = original_source_snapshot(source);
        eprintln!("Actual public checkout: {RUNTIME_PUBLIC_URL} {RUNTIME_PUBLIC_BRANCH} {RUNTIME_PUBLIC_COMMIT}; original author-source receipts captured before runtime");
        let runtime = f.runtime().await;
        let configuration = runtime.configuration(&done.app_id).await.unwrap();
        assert_eq!(configuration["configuration"]["canStart"], false);
        let rejected = runtime.start(&done.app_id).await.unwrap_err();
        assert_eq!(rejected.code, "WEB_CONFIGURATION_REQUIRED");
        let missing = rejected.details["missingRequired"].as_array().unwrap();
        assert_eq!(missing, &vec![json!("ANTHROPIC_API_KEY")]);
        assert!(runtime.owned_process_ids().await.is_empty());
        assert!(!slot.join("active.json").exists());
        let current = runtime.configuration(&done.app_id).await.unwrap();
        let configured = runtime.save(
                &done.app_id,
                SaveConfigurationRequest {
                    revision: current["configuration"]["revision"].as_u64().unwrap(),
                    manifest_digest: Some(current["manifestDigest"].as_str().unwrap().into()),
                    values: BTreeMap::new(),
                    secrets: BTreeMap::from([(
                        "ANTHROPIC_API_KEY".into(),
                        SecretAction::Replace {
                            value: ARTIFICIAL_SECRET.into(),
                        },
                    )]),
                },
            )
            .await
            .unwrap();
        assert_eq!(configured["configuration"]["canStart"], true);
        assert_eq!(
            configured["configuration"]["secretStatus"]["ANTHROPIC_API_KEY"],
            "set"
        );
        assert!(!configured.to_string().contains(ARTIFICIAL_SECRET));
        assert_eq!(configured["configuration"]["secretStatus"]["FISH_API_KEY"], "missing");
        let running = runtime.start(&done.app_id).await.unwrap();
        assert_eq!(running["id"], done.app_id);
        assert_eq!(running["state"], "running");
        let browser = PublicAppBrowser::open(&runtime, &done.app_id).await;
        let status = browser.get("/api/status").await;
        // RadioBob's health reports configuration/local module availability,
        // not a successful request to an AI or music provider.
        assert_eq!(status["services"]["claude"]["configured"], true);
        assert_eq!(status["services"]["music"]["loggedIn"], false);
        assert!(!status.to_string().contains(ARTIFICIAL_SECRET));
        let initial_state = browser.get("/api/state").await;
        assert!(initial_state["messages"].is_array());
        assert!(initial_state["queue"].is_array());
        assert!(initial_state["taste"].is_string());
        assert!(!initial_state.to_string().contains(ARTIFICIAL_SECRET));
        assert_eq!(runtime.stop(&done.app_id).await.unwrap()["state"], "closed");
        assert!(runtime.owned_process_ids().await.is_empty());
        assert!(!slot.join("active.json").exists());
        runtime.shutdown().await.unwrap();
        let reopened_apps = TestRuntime::open(&f.paths, official_node()).await;
        let persisted = reopened_apps.configuration(&done.app_id).await.unwrap();
        assert_eq!(persisted["configuration"]["canStart"], true);
        assert_eq!(
            persisted["configuration"]["revision"],
            configured["configuration"]["revision"]
        );
        assert!(persisted["configuration"]["values"].as_object().unwrap().is_empty());
        assert_eq!(
            persisted["configuration"]["secretStatus"]["ANTHROPIC_API_KEY"],
            "set"
        );
        assert!(!persisted.to_string().contains(ARTIFICIAL_SECRET));
        assert!(reopened_apps.owned_process_ids().await.is_empty());
        let reopened_run = AssertUnwindSafe(async {
            let reopened_running = reopened_apps.start(&done.app_id).await.unwrap();
            assert_eq!(reopened_running["id"], done.app_id);
            assert_eq!(reopened_running["state"], "running");
            let reopened_browser = PublicAppBrowser::open(&reopened_apps, &done.app_id).await;
            let reopened_status = reopened_browser.get("/api/status").await;
            assert_eq!(reopened_status["services"]["claude"]["configured"], true);
            assert_eq!(reopened_browser.get("/api/state").await, initial_state);
            assert_eq!(reopened_apps.stop(&done.app_id).await.unwrap()["state"], "closed");
        }).catch_unwind().await;
        // This second manager owns a separate real Registry. Attempt its full
        // closure before propagating an assertion from its actual HTTP run.
        let reopened_stop = reopened_apps.shutdown().await;
        let reopened_owned = reopened_apps.owned_process_ids().await;
        reopened_stop.unwrap();
        assert!(reopened_owned.is_empty());
        assert!(!slot.join("active.json").exists());
        if let Err(panic) = reopened_run { std::panic::resume_unwind(panic); }
        assert!(f.manager.shutdown().await.is_empty());
        let before_source = capture(source).unwrap();
        let before_processes = f.store.processes().snapshot().len();
        let restarted = f.reopen(Ok(sources.clone()));
        assert!(restarted.recover(f.apps.clone()).await.unwrap().is_empty());
        let duplicate = restarted
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        assert_eq!(duplicate.task_id, accepted.task_id);
        assert_eq!(
            restarted.status(&f.input.import_id).unwrap().app_id,
            done.app_id
        );
        assert!(before_source.matches_path(source).unwrap());
        assert_eq!(f.store.processes().snapshot().len(), before_processes);
        // A real committed registration with an interrupted coordinator journal
        // is reconciled from disk. This is not an abrupt-host/reboot experiment.
        let mut interrupted = j;
        interrupted.registration_attempted = true;
        interrupted.commit_state = CommitState::Unknown;
        interrupted.phase = "registering".into();
        interrupted.final_task = None;
        interrupted.cleanup_pending = true;
        restarted.save(&interrupted).unwrap();
        let recovered_manager = f.reopen(Ok(sources.clone()));
        let recovered = recovered_manager.recover(f.apps.clone()).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].commit_state, CommitState::Committed);
        assert_eq!(recovered[0].app_id, done.app_id);
        assert!(!recovered[0].cleanup_pending);
        assert!(before_source.matches_path(source).unwrap());
        assert_eq!(f.store.processes().snapshot().len(), before_processes);
        assert_eq!(
            recovered_manager
                .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
                .await
                .unwrap()
                .task_id,
            accepted.task_id
        );
        assert_eq!(public_source_commit(source, RUNTIME_PUBLIC_BRANCH), RUNTIME_PUBLIC_COMMIT);
        assert_eq!(original_source_snapshot(source), original_source);
        assert_eq!(read_manifest(source).unwrap(), manifest_bytes);
        eprintln!("Original author-source FileIds/bytes and commit remain unchanged after both local HTTP runs and import recovery");
        assert!(recovered_manager.shutdown().await.is_empty());
        assert!(restarted.shutdown().await.is_empty());
    })
    .catch_unwind()
    .await;
    f.close().await;
    crate::managed_git::verify(git).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

struct ReleaseWriter(Option<std::sync::mpsc::Sender<()>>);
impl ReleaseWriter {
    fn release(&mut self) {
        self.0.take().unwrap().send(()).unwrap();
    }
}
impl Drop for ReleaseWriter {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
#[ignore = "Explicit PANDA_W4_GIT_SAMPLE_ROOT, fixed PANDA_W3_OFFICIAL_NODE and public GitHub access required; actual materializer cancellation, not crash recovery"]
async fn official_public_import_cancel_preserves_source_until_actual_writer_and_job_close() {
    let (f, sources, git) = official_fixture(
        PUBLIC_URL,
        PUBLIC_BRANCH,
        "可丢弃 Windows Git 导入 中文 space",
    )
    .await;
    let result = AssertUnwindSafe(async {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let mut release = ReleaseWriter(Some(release));
        sources.pause_import_writer(entered, blocked);
        let accepted = f
            .manager
            .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
            .await
            .unwrap();
        tokio::time::timeout(IMPORT_WAIT, ready)
            .await
            .expect("Actual source materializer must enter")
            .unwrap();
        let j = f.manager.records.lock().unwrap()[&f.input.import_id].clone();
        let slot = f
            .paths
            .workspace_root()
            .join(".platform/sources")
            .join(&j.source_id);
        let slot_identity = capture(&slot).unwrap();
        let cancelled = f.manager.cancel(&accepted.task_id).unwrap();
        assert_eq!(cancelled.task_id, accepted.task_id);
        tokio::time::timeout(RESOURCE_WAIT, async {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            loop {
                tick.tick().await;
                if f.store.processes().snapshot().iter().any(|process| {
                    process.component == "source-acquisition/materialize"
                        && process.phase == ProcessPhase::CleanupPending
                }) {
                    return;
                }
            }
        })
        .await
        .expect("Actual cancellation must retain pending reader ownership");
        assert!(slot_identity.matches_path(&slot).unwrap());
        assert!(f.store.has_active_tasks());
        assert!(!f
            .manager
            .status(&f.input.import_id)
            .unwrap()
            .task
            .status
            .is_final());
        let busy = f
            .manager
            .start(
                f.apps.clone(),
                StartNodeGitImportInput {
                    selected_directory: None,
                    import_id: Uuid::new_v4().to_string(),
                    ..f.input.clone()
                },
                f.stop.subscribe(),
            )
            .await
            .unwrap_err();
        assert_eq!(busy.details["accepted"], false);
        release.release();
        let done = completed(&f).await;
        wait_idle(&f).await;
        assert_eq!(done.commit_state, CommitState::NotCommitted);
        assert!(!done.cleanup_pending, "{:?}", done.task);
        assert!(!slot.exists());
        assert!(!f.manager.ownership().directory(&j.app_id).unwrap().exists());
        assert!(!f.paths.data_root().join("apps").join(&j.app_id).exists());
        join_import_processes(&f).await;
        assert_eq!(
            f.manager
                .start(f.apps.clone(), f.input.clone(), f.stop.subscribe())
                .await
                .unwrap()
                .task_id,
            accepted.task_id
        );
    })
    .catch_unwind()
    .await;
    f.close().await;
    crate::managed_git::verify(git).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
