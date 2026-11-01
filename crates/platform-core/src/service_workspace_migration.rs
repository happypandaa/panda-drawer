use super::*;

#[path = "service_workspace_migration/cleanup_admission.rs"]
mod cleanup_admission;

/// The native receipt belongs to this migration, never to a path supplied by a
/// cleanup request. Native callbacks retain their own reversible pause tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceMigrationStage {
    Prepare,
    Reopened,
    Aborted,
    Startup,
}
#[derive(Clone, Debug)]
pub struct WorkspaceMigrationNotice {
    pub stage: WorkspaceMigrationStage,
    pub migration_id: String,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
}
pub type NativeWorkspaceMigration = Arc<
    dyn Fn(
            WorkspaceMigrationNotice,
        ) -> futures_util::future::BoxFuture<'static, std::result::Result<(), ApiError>>
        + Send
        + Sync,
>;

async fn native_stage(
    shared: &RuntimeSwitch,
    journal: &crate::workspace::MigrationJournal,
    stage: WorkspaceMigrationStage,
) -> std::result::Result<(), ApiError> {
    let hook = shared.native_migration.read().unwrap().clone();
    if let Some(hook) = hook {
        hook(WorkspaceMigrationNotice {
            stage,
            migration_id: journal.task_id.clone(),
            source_path: journal.source_path.clone(),
            destination_path: journal.destination_path.clone(),
        })
        .await
    } else if matches!(
        stage,
        WorkspaceMigrationStage::Prepare | WorkspaceMigrationStage::Startup
    ) {
        crate::node_apps::NodeApps::verify_workspace_migration_data_root_clear(
            shared.snapshot().paths.data_root(),
        )
    } else {
        Ok(())
    }
}

fn retain_runner(shared: &RuntimeSwitch, runner: JoinHandle<()>) {
    if let Some(previous) = shared.migration_runner.lock().unwrap().replace(runner) {
        shared.migration_previous.lock().unwrap().push(previous);
    }
}
struct MigrationLease(Arc<RuntimeSwitch>);
impl Drop for MigrationLease {
    fn drop(&mut self) {
        self.0.migration_active.store(false, Ordering::Release);
    }
}

/// Startup metadata recovery has no delete authority. This worker runs only
/// after the selected service (and native startup, when present) is ready.
pub(super) fn submit_selected_cleanup(
    shared: Arc<RuntimeSwitch>,
) -> std::result::Result<(), ApiError> {
    let state = shared.snapshot();
    cleanup_admission::with_selected(
        crate::workspace::selected_cleanup(&state.paths)?,
        |journal| {
            if let Err(error) = submit_cleanup(shared, state.clone(), &journal.task_id) {
                let _ = crate::workspace::retain_cleanup_reopen_failure(
                    &state.paths,
                    &journal.task_id,
                    &error,
                );
                return Err(error);
            }
            Ok(())
        },
    )
}

pub(super) fn submit_cleanup(
    shared: Arc<RuntimeSwitch>,
    state: Arc<HttpState>,
    id: &str,
) -> std::result::Result<TaskRecord, ApiError> {
    let _submission = shared.migration_submission.lock().unwrap();
    if shared.closing.load(Ordering::Acquire) {
        return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
    }
    let selected = crate::workspace::selected_cleanup(&state.paths)?;
    let journal = selected.filter(|j| j.task_id == id).ok_or_else(|| {
        ApiError::new(
            "MIGRATION_CLEANUP_NOT_AUTHORIZED",
            "只能重试当前已登记迁移的原目录收尾。",
        )
    })?;
    cleanup_admission::require_cleanup()?;
    if shared.migration_active.load(Ordering::Acquire) {
        if let Some(task) = journal
            .cleanup_task_id
            .as_ref()
            .and_then(|id| state.builds.task(id).ok())
            .filter(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running))
        {
            return Ok(task);
        }
        return Err(ApiError::new(
            "WORKSPACE_MIGRATING",
            "工作区正在切换或收尾。",
        ));
    }
    // This is an ordinary task lease, not a whole-workspace pause. Current
    // projects remain usable while the fixed old source is being removed.
    let task = state
        .builds
        .create_workspace_operation("workspaceMigrationCleanup")?;
    let lease = match state
        .store
        .reserve_transient_operation("workspace-source-cleanup", &task.task_id)
    {
        Ok(lease) => lease,
        Err(error) => {
            let _ = state.builds.finish(&task.task_id, Err(error.clone()));
            return Err(error);
        }
    };
    let mut saved = journal.clone();
    saved.cleanup_task_id = Some(task.task_id.clone());
    if let Err(error) = crate::workspace::save_journal(&state.paths, &saved) {
        let _ = state.builds.finish(&task.task_id, Err(error.clone()));
        return Err(error);
    }
    shared.migration_active.store(true, Ordering::Release);
    let owned = shared.clone();
    let task_id = task.task_id.clone();
    let runner = tokio::spawn(async move {
        let _active = MigrationLease(owned.clone());
        let _task = lease;
        let result = async {
            if journal
                .source_cleanup
                .as_ref()
                .is_some_and(|cleanup| cleanup.needs_reopen())
            {
                state.builds.phase(&task_id, "reopening")?;
                let retained = owned.migration_retained_state.lock().unwrap().clone();
                if let Some(old) = retained {
                    let issues = shutdown_coordinator::drain_managers(&old).await;
                    if !issues.is_empty() {
                        return Err(ApiError::new(
                            "WORKSPACE_DRAIN_FAILED",
                            "Old workspace managers still require attention",
                        ));
                    }
                }
                native_stage(&owned, &journal, WorkspaceMigrationStage::Startup).await?;
                *owned.migration_retained_state.lock().unwrap() = None;
                crate::workspace::acknowledge_reopened(&state.paths, &journal.task_id)?;
            }
            state.builds.phase(&task_id, "cleaning-old-workspace")?;
            cleanup_old_source(&owned, &state.paths, &journal.task_id).await
        }
        .await;
        finish_cleanup_task(&state, &task_id, &journal.task_id, result);
    });
    retain_runner(&shared, runner);
    Ok(task)
}

async fn cleanup_old_source(
    shared: &RuntimeSwitch,
    paths: &RuntimePaths,
    id: &str,
) -> std::result::Result<Value, ApiError> {
    let source_scope = shared.project_cleanup.try_migration_source_cleanup()?;
    let paths = paths.clone();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        let _source_scope = source_scope;
        crate::workspace::cleanup_source(&paths, &id)
    })
    .await
    .map_err(|e| ApiError::new("WORKSPACE_SOURCE_CLEANUP_FAILED", e.to_string()))?
}
fn finish_cleanup_task(
    state: &HttpState,
    task: &str,
    migration: &str,
    result: std::result::Result<Value, ApiError>,
) {
    match result {
        Ok(value) if value["completed"] == true => {
            let _ = state.builds.finish(task, Ok(value));
        }
        Ok(value) => {
            let error = value
                .get("error")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_else(|| {
                    ApiError::new(
                        "WORKSPACE_SOURCE_CLEANUP_PENDING",
                        "新工作区可继续使用，原目录收尾需要重试。",
                    )
                });
            let _ = state.builds.finish_partial(task, value, error);
        }
        Err(error) => {
            let _ =
                crate::workspace::retain_cleanup_reopen_failure(&state.paths, migration, &error);
            let journal = crate::workspace::read_journal(&state.paths, migration).ok();
            let result = json!({"migrationId":migration,"cleanupPending":true,"sourceRetained":true,"sourcePath":journal.map(|j|j.source_path)});
            let _ = state.builds.finish_partial(task, result, error);
        }
    }
}

pub(super) async fn submit_migration(
    shared: Arc<RuntimeSwitch>,
    state: Arc<HttpState>,
    input: crate::workspace::MigrationInput,
) -> std::result::Result<TaskRecord, ApiError> {
    // Stop/seal and full admission-to-registration share one critical section.
    // The outer submit has no await while this guard is held.
    let _submission = shared.migration_submission.lock().unwrap();
    if shared.closing.load(Ordering::Acquire) {
        return Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"));
    }
    crate::workspace::require_cleanup_before_migration(&state.paths)?;
    state.node_git_import.require_migration_clear()?;
    if shared.migration_active.swap(true, Ordering::AcqRel) {
        return Err(ApiError::new(
            "WORKSPACE_MIGRATING",
            "A migration is already finishing or running",
        ));
    }
    if let Err(error) = state.store.begin_workspace_migration() {
        shared.migration_active.store(false, Ordering::Release);
        return Err(error);
    }
    let submitted = match crate::workspace::create(&state.paths, &input.destination_path) {
        Ok(task) => task,
        Err(error) => {
            state.store.resume_workspace();
            state.storage.resume();
            shared.migration_active.store(false, Ordering::Release);
            return Err(error);
        }
    };
    let id = submitted.task_id.clone();
    let weak = Arc::downgrade(&shared);
    let migration_shared = shared.clone();
    let runner = tokio::spawn(async move {
        struct Lease(Arc<RuntimeSwitch>);
        impl Drop for Lease {
            fn drop(&mut self) {
                self.0.migration_active.store(false, Ordering::Release);
            }
        }
        let _lease = Lease(migration_shared);
        let mut committed = false;
        let mut prepared = false;
        let mut io_pause = None;
        let outcome: std::result::Result<Value, ApiError> = async {
            crate::workspace::phase(&state.paths, &id, "waiting-for-project-tasks")?;
            while state.store.has_active_tasks() {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            state.node_git_import.require_migration_clear()?;
            crate::projects::check_pending_deletions(state.paths.workspace_root())?;
            // Reject unresolved recovery before any filesystem copying.
            for project in state.store.list().await? {
                if crate::projects::valid_id(&project.id)
                    && state.store.control(&project.id)?.get().recovery_required
                {
                    return Err(ApiError::new(
                        "RECOVERY_REQUIRED",
                        "Resolve pending project recovery before moving the workspace",
                    )
                    .detail("projectId", project.id));
                }
            }
            // Source-only recovery does not stop runtime use. Reject a retained
            // operation before migration would pause that still-usable runtime.



            let journal = crate::workspace::read_journal(&state.paths, &id)?;
            crate::workspace::phase(&state.paths, &id, "waiting-for-native-drain")?;
            prepared = true;
            if let Some(runtime) = &state.web_runtime { runtime.pause(); }
            native_stage(&_lease.0, &journal, WorkspaceMigrationStage::Prepare).await?;
            state.stopping.store(true, Ordering::SeqCst);
            if let Some(runtime) = &state.web_runtime { runtime.close_all().await?; }
            io_pause = Some(state.io_lifetime.pause_for_migration()?);
            crate::workspace::phase(&state.paths, &id, "waiting-for-old-io")?;
            state.io_lifetime.drain().await;
            if !state.io_lifetime.shutdown_issues().is_empty() {
                return Err(ApiError::new("WORKSPACE_DRAIN_FAILED", "Old workspace I/O did not close cleanly"));
            }
            let issues = state.storage.quiesce().await;
            if !issues.is_empty() {
                if let Some(shared) = state.shared.upgrade() { shared.shutdown_issues.lock().unwrap().extend(issues); }
                return Err(ApiError::new("WORKSPACE_DRAIN_FAILED", "Storage did not close cleanly; migration is stopped"));
            }
            state.store.pause_workspace().await?;
            let mut guards = vec![];
            for project in state.store.list().await? {
                if crate::projects::valid_id(&project.id) {
                    let _ = state.events.send(Change {
                        project_id: project.id.clone(),
                        request_id: id.clone(),
                        source: "migration".into(),
                        event: "project.state.changed".into(),
                    });
                    guards.push(state.store.gate(&project.id).write_owned().await);
                }
            }
            let paths = state.paths.clone();
            let store = state.store.clone();
            let task_id = id.clone();
            let (selected, mut journal) = tokio::task::spawn_blocking(move || {
                crate::workspace::copy_and_commit(&paths, &store, &task_id)
            })
            .await
            .map_err(|e| ApiError::new("WORKSPACE_MIGRATION_FAILED", e.to_string()))??;
            committed = true;
            *_lease.0.migration_retained_state.lock().unwrap() = Some(state.clone());
            #[cfg(test)]
            if let Some(error) = _lease.0.migration_reopen_failure.lock().unwrap().take() { return Err(error); }
            let store = Arc::new(ProjectStore::with_paths(
                selected.workspace_root().into(),
                selected.data_root().into(),
                state.resources.clone(),
                state.store.processes(),
            )?);
            crate::workspace::initialize_selected(&selected, &store, &mut journal)?;
            let Some(shared) = weak.upgrade() else {
                return Err(ApiError::new(
                    "SERVICE_STOPPING",
                    "Runtime closed after committing the new workspace",
                ));
            };
            let new = make_state(
                selected.clone(),
                state.info.base_url.clone(),
                state.resources.clone(),
                Arc::downgrade(&shared),
                Some(store),
            )
            .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            let port = url::Url::parse(&new.info.base_url)
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?
                .port()
                .unwrap();
            selected
                .save_port(port)
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            selected
                .publish_runtime(
                    &Discovery {
                        api_version: API_VERSION,
                        instance_id: new.info.instance_id.clone(),
                        pid: std::process::id(),
                        base_url: new.info.base_url.clone(),
                        workspace_root: new.info.workspace_root.clone(),
                        environment: ENVIRONMENT.into(),
                        identifier: APP_IDENTIFIER.into(),
                    },
                    &new.token,
                )
                .map_err(|e| ApiError::new("WORKSPACE_REOPEN_FAILED", e.to_string()))?;
            shared.install(new.clone());
            let _ = state.stop.send(true);
            let issues = shutdown_coordinator::drain_managers(&state).await;
            if !issues.is_empty() {
                shared.shutdown_issues.lock().unwrap().extend(issues);
                return Err(ApiError::new("WORKSPACE_DRAIN_FAILED", "Old workspace managers did not close cleanly; retained evidence requires attention"));
            }
            crate::workspace::phase(&selected, &id, "reopening")?;
            native_stage(&shared, &journal, WorkspaceMigrationStage::Reopened).await?;
            prepared = false;
            *shared.migration_retained_state.lock().unwrap() = None;
            crate::workspace::acknowledge_reopened(&selected, &id)?;
            for project in new.store.list().await? {
                if let Some(resources) = &new.resources {
                    if let Ok(project) = new.store.project(&project.id) {
                        let handoff_dir = project.private_dir.join("handoff");
                        builds::ensure_dir(&handoff_dir)?;
                        let handoff_path = handoff_dir.join("handoff.md");
                        crate::projects::check_no_symlinks(&handoff_path, true)?;
                        if let Ok(text) = std::fs::read_to_string(&handoff_path) {
                            if text.starts_with("# Platform-managed project handoff\n") {
                                let updated = text
                                    .lines()
                                    .map(|line| {
                                        if line.starts_with("Project root: ") {
                                            format!("Project root: `{}`", project.root.display())
                                        } else {
                                            line.to_owned()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                                    + "\n";
                                let temp = handoff_path.with_file_name(format!(
                                    ".handoff-{}",
                                    Uuid::new_v4().simple()
                                ));
                                let mut file = platform_native::filesystem::create_new_private_file(&temp, false)?;
                                std::io::Write::write_all(&mut file, updated.as_bytes())?;
                                file.sync_all()?;
                                std::fs::rename(&temp, &handoff_path)?;
                            }
                        }
                        builds::atomic_json(
                            &handoff_dir.join("current.json"),
                            &resources.handoff(&project.root, &project.id, &handoff_dir),
                        )?;
                    }
                }
            }
            drop(guards);
            // The selected runtime is ready. Source cleanup must not leave its
            // current business admission paused, including on deletion failure.
            drop(io_pause.take());
            let cleanup_task = new.builds.create_workspace_operation("workspaceMigrationCleanup")?;
            let mut saved = crate::workspace::read_journal(&selected, &id)?;
            saved.cleanup_task_id = Some(cleanup_task.task_id.clone());
            crate::workspace::save_journal(&selected, &saved)?;
            new.builds.phase(&cleanup_task.task_id, "cleaning-old-workspace")?;
            let cleanup = cleanup_old_source(&shared, &selected, &id).await;
            finish_cleanup_task(&new, &cleanup_task.task_id, &id, cleanup);
            journal = crate::workspace::read_journal(&selected, &id)?;
            Ok(crate::workspace::completed_result(&journal))
        }
        .await;
        if let Err(error) = &outcome {
            // If settings committed but a final journal write failed, selection
            // remains authoritative. Never reopen or write into the source.
            committed =
                committed || crate::workspace::is_committed(&state.paths, &id).unwrap_or(true);
            if prepared && !committed {
                if let Ok(journal) = crate::workspace::read_journal(&state.paths, &id) {
                    let _ =
                        native_stage(&_lease.0, &journal, WorkspaceMigrationStage::Aborted).await;
                }
            }
            if !committed {
                state.stopping.store(false, Ordering::SeqCst);
                if let Some(runtime) = &state.web_runtime {
                    runtime.resume();
                }
                drop(io_pause.take());
                state.store.resume_workspace();
                state.storage.resume();
                if let Ok(projects) = state.store.list().await {
                    for project in projects {
                        let _ = state.events.send(Change {
                            project_id: project.id,
                            request_id: id.clone(),
                            source: "migration".into(),
                            event: "project.state.changed".into(),
                        });
                    }
                }
            } else {
                if let Err(close_error) = state.store.pause_workspace().await {
                    if let Some(shared) = state.shared.upgrade() {
                        shared.shutdown_issues.lock().unwrap().push(
                            crate::shutdown::ShutdownIssue::new(
                                "migrationDatabase",
                                "DATABASE_CLOSE_FAILED",
                                close_error.to_string(),
                                false,
                                false,
                            ),
                        );
                    }
                }
            }
            if committed {
                let _ = crate::workspace::retain_cleanup_reopen_failure(&state.paths, &id, error);
                drop(io_pause.take());
            }
            let error = error
                .clone()
                .outcome(if committed {
                    "committed_needs_attention"
                } else {
                    "not_committed"
                })
                .detail("sourceRetained", true)
                .detail("sourcePath", json!(state.paths.workspace_root()))
                .detail("destinationPath", json!(input.destination_path));
            let _ = crate::workspace::finish(&state.paths, &id, Err(error));
        } else {
            let _ = crate::workspace::finish(&state.paths, &id, outcome);
        }
    });
    #[cfg(test)]
    if let Some((entered, release)) = shared.migration_registration_pause.lock().unwrap().take() {
        let _ = entered.send(());
        let _ = release.recv();
    }
    if let Some(previous) = shared.migration_runner.lock().unwrap().replace(runner) {
        // Completion is not success proof. Keep every earlier outcome joinable.
        shared.migration_previous.lock().unwrap().push(previous);
    }
    Ok(submitted)
}
