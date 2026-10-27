//! Standard Git fixture setup for the retained restore/read-only tests.
//! These helpers are never compiled into the application and create no platform
//! Git-operation journals. Every caller owns a disposable test project.
use super::*;

impl ProjectGitManager {
    async fn fixture_git(
        &self,
        id: &str,
        args: &[&str],
        author: Option<&GitAuthor>,
    ) -> Result<Vec<u8>, ApiError> {
        let project = self.store.project(id)?;
        let tools = self.git.resolve().await?;
        let private = tempfile::tempdir()?;
        let mut command = git_command(&tools, private.path(), Some(&project));
        command.args(args);
        if let Some(author) = author {
            command
                .env("GIT_AUTHOR_NAME", &author.name)
                .env("GIT_AUTHOR_EMAIL", &author.email)
                .env("GIT_COMMITTER_NAME", &author.name)
                .env("GIT_COMMITTER_EMAIL", &author.email);
        }
        let output = command.output().await?;
        if !output.status.success() {
            return Err(git_failed());
        }
        Ok(output.stdout)
    }

    fn fixture_result(
        &self,
        id: &str,
        operation: &str,
        result: Value,
    ) -> Result<TaskRecord, ApiError> {
        let (task, lease) = self.tasks.create_operation(id, operation)?;
        self.tasks.finish(&task.task_id, Ok(result))?;
        drop(lease);
        self.tasks.task(&task.task_id)
    }

    pub(crate) async fn fixture_initialize(
        self: &Arc<Self>,
        id: &str,
        _input: GitInitInput,
        _stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        self.fixture_git(
            id,
            &[
                "init",
                "--quiet",
                "--initial-branch=main",
                "--object-format=sha1",
            ],
            None,
        )
        .await?;
        let project = self.store.project(id)?;
        let excluded = format!("/.platform/\n/data/\n/dist/\n**/node_modules/\n**/.env*\n!**/.env.example\n**/.npmrc\n/{}/\n/{}/\n", project.manifest.database, project.manifest.attachments_dir);
        fs::write(project.root.join(".git/info/exclude"), excluded)?;
        self.fixture_result(
            id,
            "externalGitFixture",
            json!({"operationOutcome":"committed"}),
        )
    }

    pub(crate) async fn fixture_save(
        self: &Arc<Self>,
        id: &str,
        input: GitSaveInput,
        _stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        self.fixture_git(id, &["add", "--all", "--", "."], None)
            .await?;
        self.fixture_git(
            id,
            &[
                "commit",
                "--quiet",
                "--no-gpg-sign",
                "--allow-empty",
                "-m",
                &input.message,
            ],
            Some(&input.author),
        )
        .await?;
        let bytes = self.fixture_git(id, &["rev-parse", "HEAD"], None).await?;
        let commit = one_oid(&bytes)?;
        self.fixture_result(
            id,
            "externalGitFixture",
            json!({"operationOutcome":"committed", "commit":commit}),
        )
    }

    pub(crate) async fn fixture_save_data(
        self: &Arc<Self>,
        id: &str,
        input: GitDataSaveInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        let project = self.store.project(id)?;
        let temporary = tempfile::tempdir()?;
        let destination = temporary.path().canonicalize()?.join("snapshot");
        let (_cancel, cancellation) = watch::channel(false);
        let lease = version_data::SnapshotLease::acquire(
            project.clone(),
            stop.clone(),
            cancellation.clone(),
        )
        .await?;
        let snapshot_stop = stop.clone();
        let candidate = tokio::task::spawn_blocking(move || {
            version_data::prepare_snapshot(
                lease,
                &destination,
                version_data::SnapshotLimits::default(),
                &snapshot_stop,
                &cancellation,
            )
        })
        .await
        .map_err(|_| recovery())??;
        let live = project.root.join(version_data::DIRECTORY);
        if live.exists() {
            fs::remove_dir_all(&live)?;
        }
        fs::rename(&candidate.root, &live)?;
        self.fixture_save(id, input.into(), stop).await
    }
}

// Only the process-lifetime tests need a retained file owner after Git writes
// moved out of the platform. This owns a disposable fixture, not a Git lock.
pub(super) struct OwnedFixtureFile {
    pub(super) retained: Arc<FixtureFile>,
}
pub(super) struct FixtureFile {
    path: PathBuf,
    device: u64,
    inode: u64,
    _file: File,
}
impl OwnedFixtureFile {
    pub(super) fn acquire(path: &Path) -> Result<Self, ApiError> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let meta = file.metadata()?;
        Ok(Self {
            retained: Arc::new(FixtureFile {
                path: path.into(),
                device: meta.dev(),
                inode: meta.ino(),
                _file: file,
            }),
        })
    }
}
impl Drop for FixtureFile {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.is_file() && m.dev() == self.device && m.ino() == self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(super) fn fixture_author_environment(command: &mut Command, author: &GitAuthor) {
    command
        .env("GIT_AUTHOR_NAME", &author.name)
        .env("GIT_AUTHOR_EMAIL", &author.email)
        .env("GIT_COMMITTER_NAME", &author.name)
        .env("GIT_COMMITTER_EMAIL", &author.email);
}
