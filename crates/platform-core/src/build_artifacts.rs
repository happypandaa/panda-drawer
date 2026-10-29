//! Project-exclusive immutable build generations. Window pins outlive requests;
//! reader pins belong to the actual file producer, never just its HTTP response.
use crate::{
    builds::{self, ArtifactGeneration},
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, checked_relative},
    ApiError, ProjectHandle, ProjectStore, RuntimePaths,
};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
};
use tokio::{sync::Notify, task::JoinHandle};
use uuid::Uuid;

#[path = "build_window_views.rs"]
mod views;
use views::{valid_view_id, view_error};

static HOST: OnceLock<String> = OnceLock::new();
static SCOPES: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();
fn host() -> &'static str {
    HOST.get_or_init(|| Uuid::new_v4().to_string())
}
fn scopes() -> &'static Mutex<BTreeMap<String, bool>> {
    SCOPES.get_or_init(Default::default)
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Instance {
    project: String,
    epoch: String,
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    project: String,
    epoch: String,
    root: PathBuf,
    build: String,
}
impl Key {
    fn new(project: &ProjectHandle, build: &str) -> Self {
        Self {
            project: project.manifest.id.clone(),
            epoch: project.document_epoch.clone(),
            root: project.root.clone(),
            build: build.into(),
        }
    }
    fn instance(&self) -> Instance {
        Instance {
            project: self.project.clone(),
            epoch: self.epoch.clone(),
            root: self.root.clone(),
        }
    }
}
#[derive(Default)]
struct State {
    sealed: bool,
    pins: BTreeMap<Key, usize>,
    readers: usize,
    reader_instances: BTreeMap<Instance, usize>,
    deleting: BTreeSet<Key>,
    cleaning: BTreeSet<String>,
    sealed_instances: BTreeSet<Instance>,
    known_instances: BTreeSet<Instance>,
    stopping: bool,
}
pub(crate) struct PublicationGuard<'a> {
    _guard: std::sync::MutexGuard<'a, State>,
}
struct Inner {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    state: Mutex<State>,
    changed: Notify,
    runtime: tokio::runtime::Handle,
    jobs: Mutex<Vec<(Instance, JoinHandle<()>)>>,
    scope: String,
    coordinated: AtomicBool,
    job_issues: Mutex<Vec<crate::shutdown::ShutdownIssue>>,
    #[cfg(test)]
    registration_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    retirement_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    capture_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}
impl Drop for Inner {
    fn drop(&mut self) {
        scopes().lock().unwrap().insert(self.scope.clone(), false);
    }
}
/// Bound to one runtime state. A migration must retain this exact handle while
/// sealing and draining the old state; querying the new state is not a receipt.
#[derive(Clone)]
pub struct ArtifactAccess(Arc<Inner>);
#[derive(Clone)]
pub(crate) struct WeakArtifactAccess(Weak<Inner>);
impl WeakArtifactAccess {
    pub(crate) fn upgrade(&self) -> Option<ArtifactAccess> {
        self.0.upgrade().map(ArtifactAccess)
    }
}
/// Kept by an actual native-window receipt or a real HTTP file producer.
pub struct ArtifactLease {
    owner: ArtifactAccess,
    key: Key,
    reader: bool,
    root: PathBuf,
}
impl ArtifactLease {
    pub fn build_id(&self) -> &str {
        &self.key.build
    }
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }
}
impl Drop for ArtifactLease {
    fn drop(&mut self) {
        let mut state = self.owner.0.state.lock().unwrap();
        if self.reader {
            state.readers -= 1;
            if let Some(count) = state.reader_instances.get_mut(&self.key.instance()) {
                *count -= 1;
                if *count == 0 {
                    state.reader_instances.remove(&self.key.instance());
                }
            }
        }
        let last = if let Some(pins) = state.pins.get_mut(&self.key) {
            *pins -= 1;
            if *pins == 0 {
                state.pins.remove(&self.key);
                true
            } else {
                false
            }
        } else {
            false
        };
        let clean =
            last && !state.stopping && !state.sealed_instances.contains(&self.key.instance());
        drop(state);
        self.owner.0.changed.notify_waiters();
        if clean {
            self.owner.retirement_checkpoint();
            if valid_view_id(&self.key.build) {
                self.owner.schedule_view_cleanup(&self.key);
            } else {
                self.owner.schedule_cleanup(&self.key);
            }
        }
    }
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupReport {
    pub removed_paths: Vec<PathBuf>,
    pub cleanup_pending: bool,
    pub remaining_paths: Vec<PathBuf>,
    pub issues: Vec<ApiError>,
}
impl CleanupReport {
    fn issue(&mut self, path: PathBuf, issue: ApiError) {
        self.cleanup_pending = true;
        self.remaining_paths.push(path);
        self.issues.push(issue);
    }
}
impl ArtifactAccess {
    pub(crate) fn new(paths: RuntimePaths, store: Arc<ProjectStore>) -> Self {
        let scope = Uuid::new_v4().to_string();
        scopes().lock().unwrap().insert(scope.clone(), true);
        Self(Arc::new(Inner {
            paths,
            store,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            runtime: tokio::runtime::Handle::current(),
            jobs: Mutex::new(vec![]),
            scope,
            coordinated: AtomicBool::new(false),
            job_issues: Mutex::new(vec![]),
            #[cfg(test)]
            registration_pause: Mutex::new(None),
            #[cfg(test)]
            retirement_pause: Mutex::new(None),
            #[cfg(test)]
            capture_pause: Mutex::new(None),
        }))
    }
    pub(crate) fn downgrade(&self) -> WeakArtifactAccess {
        WeakArtifactAccess(Arc::downgrade(&self.0))
    }
    pub(crate) fn coordinated(&self) {
        self.0.coordinated.store(true, Ordering::Release);
    }
    pub(crate) fn publication(&self) -> Result<PublicationGuard<'_>, ApiError> {
        let guard = self.0.state.lock().unwrap();
        require_open(&guard)?;
        Ok(PublicationGuard { _guard: guard })
    }
    pub fn seal_access(&self) {
        self.0.state.lock().unwrap().sealed = true;
        self.0.changed.notify_waiters();
    }
    pub fn resume_access(&self) {
        self.0.state.lock().unwrap().sealed = false;
    }
    pub async fn wait_readers_drained(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.0.state.lock().unwrap().readers == 0 {
                return;
            }
            changed.await;
        }
    }
    pub(crate) fn current(
        &self,
        project: &ProjectHandle,
        reader: bool,
    ) -> Result<(Option<ArtifactGeneration>, Option<ArtifactLease>), ApiError> {
        let mut capture_admission = None;
        let mut _capture_read = None;
        loop {
            let mut state = self.0.state.lock().unwrap();
            require_open(&state)?;
            project.check()?;
            if state
                .sealed_instances
                .contains(&Key::new(project, "").instance())
            {
                return Err(ApiError::new(
                    "PROJECT_REMOVED",
                    "This project window admission has been sealed",
                ));
            }
            let generation = builds::artifact_generation(project)?;
            if generation.is_none() && !reader && capture_admission.is_none() {
                // Only a new window snapshot writes control material. Register
                // its short operation before taking the selection lock again;
                // deletion/migration can then account for the actual copy.
                drop(state);
                capture_admission = Some(
                    self.0
                        .store
                        .reserve_transient_operation(&project.manifest.id, "捕获网页窗口入口")?,
                );
                _capture_read = Some(project.operation_gate.clone().try_read_owned().map_err(
                    |_| {
                        ApiError::new(
                            "PROJECT_BUSY",
                            "The project is changing; reopen it after the operation completes",
                        )
                    },
                )?);
                continue;
            }
            let generation = match generation {
                None if !reader => Some(self.capture_view(project)?),
                generation => generation,
            };
            let lease = if let Some(generation) = &generation {
                Some(self.pin(
                    &mut state,
                    project,
                    &generation.build_id,
                    reader,
                    generation.root.clone(),
                )?)
            } else {
                let root = checked_relative(&project.root, &project.manifest.output_dir, true)?;
                Some(self.pin(&mut state, project, "", reader, root)?)
            };
            return Ok((generation, lease));
        }
    }
    pub(crate) fn reader(
        &self,
        project: &ProjectHandle,
        build: &str,
    ) -> Result<ArtifactLease, ApiError> {
        let mut state = self.0.state.lock().unwrap();
        require_open(&state)?;
        project.check()?;
        let root = if valid_view_id(build) {
            let (root, receipt) = self.view_receipt(&project.manifest.id, build)?;
            if receipt.scope != self.0.scope
                || receipt.host != host()
                || receipt.epoch != project.document_epoch
                || receipt.phase != "ready"
            {
                return Err(view_error());
            }
            root.join("dist")
        } else {
            builds::version_root(project, build)?
        };
        self.pin(&mut state, project, build, true, root)
    }
    fn pin(
        &self,
        state: &mut State,
        project: &ProjectHandle,
        build: &str,
        reader: bool,
        root: PathBuf,
    ) -> Result<ArtifactLease, ApiError> {
        let key = Key::new(project, build);
        if state.sealed_instances.contains(&key.instance()) || state.deleting.contains(&key) {
            return Err(ApiError::new(
                "BUILD_ARTIFACT_MISSING",
                "This unoccupied build generation has been retired; reopen the project",
            ));
        }
        *state.pins.entry(key.clone()).or_default() += 1;
        state.known_instances.insert(key.instance());
        if reader {
            state.readers += 1;
            *state.reader_instances.entry(key.instance()).or_default() += 1;
        }
        Ok(ArtifactLease {
            owner: self.clone(),
            key,
            reader,
            root,
        })
    }
    fn schedule_cleanup(&self, key: &Key) {
        // This admission is created before queueing I/O. Migration/deletion can
        // neither abandon the worker nor wait on a task it cannot account for.
        let Ok(project) = self.0.store.project(&key.project) else {
            return;
        };
        if project.root != key.root
            || project.document_epoch != key.epoch
            || project.check().is_err()
        {
            return;
        }
        let Ok(operation) = self
            .0
            .store
            .reserve_transient_operation(&key.project, "清理未占用的旧网页构建")
        else {
            return;
        };
        let mut state = self.0.state.lock().unwrap();
        if state.sealed
            || state.stopping
            || state.sealed_instances.contains(&key.instance())
            || !state.cleaning.insert(key.project.clone())
        {
            return;
        }
        let owner = self.clone();
        let id = key.project.clone();
        self.scheduling_checkpoint();
        let job = self.0.runtime.spawn_blocking(move || {
            let _operation = operation;
            if let Ok(_read) = project.operation_gate.clone().try_read_owned() {
                let report = owner.cleanup(&project, &[]);
                if !report.removed_paths.is_empty() && project.check().is_ok() {
                    if let Err(issue) = crate::maintenance::MaintenanceManager::invalidate_storage(
                        owner.0.paths.workspace_root(),
                        &project.manifest.id,
                    ) {
                        eprintln!("Build storage invalidation pending: {}", issue.code);
                    }
                }
                for issue in report.issues {
                    eprintln!("Build generation cleanup pending: {}", issue.code);
                }
            }
            owner.0.state.lock().unwrap().cleaning.remove(&id);
        });
        self.register_job(key.instance(), job);
        drop(state);
    }
    pub(crate) async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.seal_access();
        if !self.0.coordinated.load(Ordering::Acquire) {
            self.0.state.lock().unwrap().stopping = true;
        }
        let jobs = std::mem::take(&mut *self.0.jobs.lock().unwrap());
        let mut issues = std::mem::take(&mut *self.0.job_issues.lock().unwrap());
        for (_, job) in jobs {
            if let Err(error) = job.await {
                issues.push(crate::shutdown::ShutdownIssue::join(
                    "build-artifacts",
                    error,
                ));
            }
        }
        issues
    }
    fn register_job(&self, instance: Instance, job: JoinHandle<()>) {
        let mut jobs = self.0.jobs.lock().unwrap();
        let mut retained = vec![];
        for (key, mut previous) in jobs.drain(..) {
            let joined = if previous.is_finished() {
                (&mut previous).now_or_never()
            } else {
                None
            };
            match joined {
                Some(Ok(())) => (),
                Some(Err(error)) => {
                    self.0
                        .job_issues
                        .lock()
                        .unwrap()
                        .push(crate::shutdown::ShutdownIssue::join(
                            "build-artifacts",
                            error,
                        ))
                }
                None => retained.push((key, previous)),
            }
        }
        retained.push((instance, job));
        *jobs = retained;
    }
    fn scheduling_checkpoint(&self) {
        #[cfg(test)]
        if let Some((entered, release)) = self.0.registration_pause.lock().unwrap().take() {
            let _ = entered.send(());
            let _ = release.recv();
        }
    }
    fn retirement_checkpoint(&self) {
        #[cfg(test)]
        if let Some((entered, release)) = self.0.retirement_pause.lock().unwrap().take() {
            let _ = entered.send(());
            let _ = release.recv();
        }
    }
    pub(crate) async fn shutdown_all(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.0.state.lock().unwrap().stopping = true;
        self.shutdown().await
    }
    pub(crate) async fn prepare_project_deletion(&self, id: &str) -> Result<(), ApiError> {
        let instances = {
            let mut state = self.0.state.lock().unwrap();
            let mut instances = state
                .known_instances
                .iter()
                .filter(|key| key.project == id)
                .cloned()
                .collect::<BTreeSet<_>>();
            instances.extend(
                self.0
                    .jobs
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(key, _)| key.project == id)
                    .map(|(key, _)| key.clone()),
            );
            state.sealed_instances.extend(instances.iter().cloned());
            instances
        };
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self
                .0
                .state
                .lock()
                .unwrap()
                .reader_instances
                .iter()
                .any(|(key, count)| instances.contains(key) && *count > 0)
            {
                break;
            }
            changed.await;
        }
        let jobs = {
            let mut all = self.0.jobs.lock().unwrap();
            let mut selected = vec![];
            let mut retained = vec![];
            for (instance, job) in all.drain(..) {
                if instances.contains(&instance) {
                    selected.push(job);
                } else {
                    retained.push((instance, job));
                }
            }
            *all = retained;
            selected
        };
        for job in jobs {
            job.await
                .map_err(|error| ApiError::new("BUILD_CLEANUP_JOIN_FAILED", error.to_string()))?;
        }
        Ok(())
    }
    /// Caller owns startup admission or the project's existing task/read gate.
    /// Unknown writers, current generations and pending publications are never
    /// cleanup candidates. Every removal rechecks the captured directory object.
    pub(crate) fn cleanup(
        &self,
        project: &ProjectHandle,
        completed_tasks: &[String],
    ) -> CleanupReport {
        let mut report = CleanupReport::default();
        let operation = (|| -> Result<(), ApiError> {
            project.check()?;
            self.0
                .store
                .check_development_available(&project.manifest.id)?;
            crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::web(
                self.0.paths.data_root(),
                &project.manifest.id,
            )?)?;
            let journal = self
                .0
                .paths
                .workspace_root()
                .join(".platform/project-state/builds")
                .join(format!("{}.publication.json", project.manifest.id));
            if journal.try_exists()? {
                return Err(ApiError::new(
                    "BUILD_RECOVERY_REQUIRED",
                    "A publication must be recovered before retiring generations",
                ));
            }
            let Some(current) = builds::artifact_generation(project)? else {
                self.cleanup_views(project, &mut report)?;
                return Ok(());
            };
            let artifacts = checked_relative(&project.root, ".platform/artifacts", false)?;
            for entry in fs::read_dir(&artifacts)? {
                let entry = entry?;
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if id == current.build_id || !builds::valid_generation_id(&id) {
                    continue;
                }
                let key = Key::new(project, &id);
                {
                    let mut state = self.0.state.lock().unwrap();
                    if state.sealed
                        || state.pins.contains_key(&key)
                        || !state.deleting.insert(key.clone())
                    {
                        continue;
                    }
                    // A new publication may have committed before we obtained
                    // this reservation. Only its current immutable ID is live.
                    let live = match builds::artifact_generation(project) {
                        Ok(live) => live,
                        Err(issue) => {
                            state.deleting.remove(&key);
                            return Err(issue);
                        }
                    };
                    if live.is_none_or(|live| live.build_id == id) {
                        state.deleting.remove(&key);
                        continue;
                    }
                }
                let path = entry.path();
                let result = remove_directory(project, &path);
                self.0.state.lock().unwrap().deleting.remove(&key);
                match result {
                    Ok(()) => report.removed_paths.push(path),
                    Err(issue) => report.issue(path, issue),
                }
            }
            self.cleanup_views(project, &mut report)?;
            for task in completed_tasks {
                if !builds::valid_task_id(task) {
                    continue;
                }
                let path =
                    checked_relative(&project.root, &format!(".platform/builds/{task}"), true)?;
                if !path.try_exists()? {
                    continue;
                }
                match remove_directory(project, &path) {
                    Ok(()) => report.removed_paths.push(path),
                    Err(issue) => report.issue(path, issue),
                }
            }
            Ok(())
        })();
        if let Err(issue) = operation {
            report.issue(project.root.clone(), issue);
        }
        report
    }
}
fn require_open(state: &State) -> Result<(), ApiError> {
    if state.sealed {
        Err(ApiError::new(
            "WORKSPACE_MIGRATING",
            "Old webpage resource access has been revoked",
        ))
    } else {
        Ok(())
    }
}
fn remove_directory(project: &ProjectHandle, path: &std::path::Path) -> Result<(), ApiError> {
    project.check()?;
    check_no_symlinks(path, false)?;
    let identity = PersistedIdentity::capture(path)
        .map_err(|error| ApiError::new("FILESYSTEM_ERROR", error.to_string()))?;
    project.check()?;
    crate::history_cleanup::remove_owned_directory(path, &identity).map(|_| ())
}

#[cfg(test)]
#[path = "build_artifacts_tests.rs"]
mod tests;
