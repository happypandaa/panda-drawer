use super::*;
use crate::file_identity::PersistedIdentity;

#[test]
fn windows_context_accepts_native_drive_and_command_environment_is_cleared() {
    let (stop, receiver) = watch::channel(false);
    let (cancel, cancelled) = watch::channel(false);
    let context = Context::new(receiver, cancelled).unwrap();
    assert!(context.root().is_absolute());
    let tools = GitToolchain {
        git: ExecutableInfo {
            path: context.root().join("disposable-git.exe"),
            version: "2.56.0.windows.4".into(),
        },
        path: "selected-disposable-PATH".into(),
        exec_path: None,
        template_path: None,
    };
    let mut command = git_command(&tools, context.root(), None);
    command.arg(format!(
        "--git-dir={}",
        context.root().join("candidate.git").display()
    ));
    let command = native_io::prepare_git_command(command).unwrap();
    let values = command.as_std().get_envs().collect::<BTreeMap<_, _>>();
    for name in ["SystemRoot", "USERPROFILE", "TEMP", "TMP"] {
        assert!(values
            .get(std::ffi::OsStr::new(name))
            .is_some_and(|value| value.is_some()));
    }
    for name in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM"] {
        assert_eq!(
            values[std::ffi::OsStr::new(name)],
            Some(std::ffi::OsStr::new("NUL"))
        );
    }
    assert_eq!(
        values[std::ffi::OsStr::new("GIT_ALLOW_PROTOCOL")],
        Some(std::ffi::OsStr::new(""))
    );
    assert!(native_io::supported_version("2.56.0.windows.4"));
    assert!(!native_io::supported_version("2.56.0.windows."));
    assert!(!native_io::supported_version("2.57.0.windows.4"));
    drop((stop, cancel));
}

#[tokio::test]
async fn real_manager_construction_does_not_open_unfinished_write_transactions() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().into(), None);
    let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
    let (stop, receiver) = watch::channel(false);
    let error = manager
        .source_restore_preview(
            "prj_missing",
            GitSourceRestorePreviewInput {
                target_commit: "a".repeat(40),
            },
            receiver,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    drop(stop);
    assert!(manager.shutdown().await.is_empty());
    tasks.shutdown().await;
    store.shutdown().await;
}

struct Official {
    _temporary: TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    environment: GitEnvironmentManager,
    tools: GitToolchain,
    context: Context,
    _stop: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
}
impl Official {
    async fn new() -> Self {
        let selected = PathBuf::from(
            std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT")
                .expect("explicit disposable pinned MinGit copy"),
        );
        crate::managed_git::verify(selected.clone()).await.unwrap();
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let created = store
            .create(crate::CreateProjectInput {
                name: "Disposable Windows readonly Git 中文".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&created.id).unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let environment = GitEnvironmentManager::with_processes(
            paths.data_root().into(),
            Some(selected),
            store.processes().clone(),
        );
        let manager = ProjectGitManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            environment.clone(),
        )
        .unwrap();
        let (stop, receiver) = watch::channel(false);
        let (cancel, cancelled) = watch::channel(false);
        let mut context = Context::new(receiver, cancelled).unwrap();
        context.bind_keep_alive(store.processes().clone(), Arc::new(()));
        let tools = context.resolve(&environment).await.unwrap();
        let mut fixture = Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            manager,
            project,
            environment,
            tools,
            context,
            _stop: stop,
            _cancel: cancel,
        };
        fixture
            .git(&["init", "--initial-branch=main"], vec![])
            .await;
        fs::write(
            fixture.project.root.join("app/disposable-script.sh"),
            b"disposable source body\n",
        )
        .unwrap();
        fixture
            .git(
                &["add", "--", "project.json", "app/disposable-script.sh"],
                vec![],
            )
            .await;
        fixture
            .git(
                &[
                    "update-index",
                    "--chmod=+x",
                    "--",
                    "app/disposable-script.sh",
                ],
                vec![],
            )
            .await;
        fixture
            .git(
                &[
                    "-c",
                    "user.name=Disposable",
                    "-c",
                    "user.email=disposable@example.invalid",
                    "commit",
                    "-m",
                    "disposable readonly seed",
                ],
                vec![],
            )
            .await;
        fixture
    }
    async fn git(&mut self, args: &[&str], input: Vec<u8>) -> Vec<u8> {
        let mut command = git_command(&self.tools, self.context.root(), Some(&self.project));
        command.args(args);
        self.context
            .run(command, input)
            .await
            .unwrap()
            .success()
            .unwrap()
    }
    async fn guard(&self) -> Result<BuildGitGuard, ApiError> {
        BuildGitGuard::capture_with_keep_alive(
            &self.project,
            &self.environment,
            self._stop.subscribe(),
            self.store.processes().clone(),
            Arc::new(()),
        )
        .await
    }
    async fn finish(self) {
        assert!(self.manager.shutdown().await.is_empty());
        self.tasks.shutdown().await;
        self.store.shutdown().await;
        assert!(self.store.processes().shutdown().await.is_empty());
    }
}
fn proof(path: &Path) -> (PersistedIdentity, Vec<u8>) {
    (
        PersistedIdentity::capture(path).unwrap(),
        fs::read(path).unwrap(),
    )
}
fn unchanged(path: &Path, proof: &(PersistedIdentity, Vec<u8>)) {
    assert!(proof.0.matches_path(path).unwrap());
    assert_eq!(fs::read(path).unwrap(), proof.1);
}

#[tokio::test]
#[ignore = "Opt-in pinned PANDA_W4_GIT_SAMPLE_ROOT; real offline disposable repository"]
async fn official_mingit_readonly_status_versions_and_dirty_build_preserve_live_git() {
    let fixture = Official::new().await;
    let index = fixture.project.root.join(".git/index");
    let config = fixture.project.root.join(".git/config");
    let head = fixture.project.root.join(".git/HEAD");
    let originals = [
        (&index, proof(&index)),
        (&config, proof(&config)),
        (&head, proof(&head)),
    ];
    let view = fixture
        .manager
        .status(&fixture.project.manifest.id, fixture._stop.subscribe())
        .await
        .unwrap();
    assert!(view.can_list_versions, "{:?}", view.issues);
    assert!(
        !view
            .changes
            .iter()
            .any(|change| change.path == "app/disposable-script.sh"),
        "Unix executable bit must not create a false Windows change"
    );
    assert!(!view.can_initialize && !view.can_save && !view.can_create_branch);
    let versions = fixture
        .manager
        .versions(&fixture.project.manifest.id, 20, fixture._stop.subscribe())
        .await
        .unwrap();
    assert_eq!(versions.versions.len(), 1);
    assert_eq!(versions.versions[0].subject, "disposable readonly seed");
    fs::write(
        fixture.project.root.join("app/disposable-script.sh"),
        b"dirty source remains buildable\n",
    )
    .unwrap();
    let guard = fixture.guard().await.unwrap();
    guard
        .recheck_with_keep_alive(
            &fixture.project,
            fixture._stop.subscribe(),
            fixture.store.processes().clone(),
            Arc::new(()),
        )
        .await
        .unwrap();
    for (path, before) in &originals {
        unchanged(path, before);
    }
    let mut changed = originals[1].1 .1.clone();
    changed.extend_from_slice(b"\n[gui]\n    disposable = changed\n");
    fs::write(&config, &changed).unwrap();
    let error = guard
        .recheck_with_keep_alive(
            &fixture.project,
            fixture._stop.subscribe(),
            fixture.store.processes().clone(),
            Arc::new(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "BUILD_GIT_STATE_CHANGED");
    assert_eq!(fs::read(&config).unwrap(), changed);
    unchanged(&index, &originals[0].1);
    assert!(!fixture.project.root.join(".git/index.lock").exists());
    assert!(!fixture
        .paths
        .data_root()
        .join("git-toolchain.json")
        .exists());
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "Opt-in pinned PANDA_W4_GIT_SAMPLE_ROOT; real busy and unmerged private-index checks"]
async fn official_mingit_busy_and_conflicting_indices_block_without_mutation() {
    let mut fixture = Official::new().await;
    let busy = fixture.project.root.join(".git/MERGE_HEAD");
    fs::write(&busy, format!("{}\n", "a".repeat(40))).unwrap();
    let error = fixture.guard().await.err().unwrap();
    assert_eq!(error.code, "BUILD_GIT_OPERATION_IN_PROGRESS");
    assert!(busy.is_file());
    fs::remove_file(&busy).unwrap();
    let oid = String::from_utf8(
        fixture
            .git(
                &["hash-object", "-w", "--stdin"],
                b"conflict base\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    fixture
        .git(
            &["update-index", "--index-info"],
            format!(
                "100644 {} 1\tdocs/outside-source-conflict.txt\n",
                oid.trim()
            )
            .into_bytes(),
        )
        .await;
    let index = fixture.project.root.join(".git/index");
    let original = proof(&index);
    let error = fixture.guard().await.err().unwrap();
    assert_eq!(error.code, "BUILD_GIT_CONFLICT");
    unchanged(&index, &original);
    assert!(!fixture.project.root.join(".git/index.lock").exists());
    fixture.finish().await;
}
