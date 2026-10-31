//! Original cleanup admission, task, candidate and execution policy. Native
//! identity, inventory and unlink operations live in Windows adapters.
use super::*;
impl CleanupManager {
    pub(crate) fn new(
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        tasks: Arc<BuildManager>,
        instance_id: String,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            paths,
            store,
            tasks,
            instance_id,
            admission: Mutex::new(()),
            plans: Mutex::new(BTreeMap::new()),
            coordinator: Coordinator::new(),
        }))
    }

    fn ensure_running(&self) -> Result<(), ApiError> {
        if self.coordinator.stopping.load(Ordering::Acquire) {
            Err(ApiError::new("SERVICE_STOPPING", "Platform is stopping"))
        } else if !self.coordinator.deleting.lock().unwrap().is_empty() {
            Err(ApiError::new(
                "PROJECT_DELETION_PENDING",
                "项目正在删除，请在删除结束后重新预览清理范围。",
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) fn preview(self: &Arc<Self>) -> Result<TaskRecord, ApiError> {
        let mut running = self.coordinator.running.lock().unwrap();
        self.ensure_running()?;
        self.store.check_workspace_available()?;
        let task = self
            .tasks
            .create_workspace_operation("workspaceCleanupPreview")?;
        let id = task.task_id.clone();
        let manager = self.clone();
        let runner = tokio::spawn(async move {
            let worker = manager.clone();
            let task_id = id.clone();
            let result = tokio::task::spawn_blocking(move || {
                worker.tasks.phase(&task_id, "inspecting")?;
                let items = worker.candidates()?;
                let plan = Plan {
                    schema_version: 1,
                    plan_id: format!("cleanup_plan_{}", Uuid::new_v4().simple()),
                    instance_id: worker.instance_id.clone(),
                    workspace_root: worker.paths.workspace_root().into(),
                    workspace_identity: directory_identity(worker.paths.workspace_root())?,
                    expires_at: now_ms() + PLAN_TTL,
                    items,
                    task_id: None,
                    selected_ids: vec![],
                    preview_task_id: task_id.clone(),
                };
                let public = plan.public();
                let mut plans = worker.plans.lock().unwrap();
                plans.retain(|_, plan| plan.expires_at >= now_ms() || plan.task_id.is_some());
                plans.insert(plan.plan_id.clone(), plan);
                Ok(public)
            })
            .await
            .map_err(error)
            .and_then(|result| result);
            let _ = manager.tasks.finish(&id, result);
        });
        running.push(runner);
        Ok(task)
    }
    pub(crate) fn submit(self: &Arc<Self>, input: Value) -> Result<TaskRecord, ApiError> {
        let mut running = self.coordinator.running.lock().unwrap();
        self.ensure_running()?;
        let input: SubmitInput = serde_json::from_value(input)?;
        if !input.plan_id.starts_with("cleanup_plan_")
            || !valid_id(&input.plan_id)
            || input.confirmation != CONFIRMATION
            || input.item_ids.is_empty()
        {
            return Err(ApiError::new(
                "INVALID_CLEANUP_CONFIRMATION",
                "Select specific preview items and enter the exact permanent-cleanup confirmation",
            ));
        }
        let _admission = self.admission.lock().unwrap();
        let mut plans = self.plans.lock().unwrap();
        let mut plan = plans.get(&input.plan_id).cloned().ok_or_else(|| {
            ApiError::new(
                "CLEANUP_PLAN_EXPIRED",
                "清理预览已结束或软件已重开，请重新扫描后确认。",
            )
        })?;
        let selected: BTreeSet<_> = input.item_ids.into_iter().collect();
        let selected_ids = selected.iter().cloned().collect::<Vec<_>>();
        if plan.schema_version != 1 || plan.plan_id != input.plan_id {
            return Err(error("Invalid cleanup plan identity"));
        }
        if let Some(id) = &plan.task_id {
            if plan.selected_ids != selected_ids {
                return Err(ApiError::new(
                    "CLEANUP_PLAN_CONSUMED",
                    "This plan was consumed with a different selection",
                ));
            }
            return self.tasks.task(id);
        }
        if plan.instance_id != self.instance_id
            || plan.expires_at < now_ms()
            || plan.workspace_root != self.paths.workspace_root()
            || plan.workspace_identity != directory_identity(self.paths.workspace_root())?
        {
            return Err(ApiError::new(
                "CLEANUP_PLAN_EXPIRED",
                "The plan expired or its runtime/workspace changed; preview again",
            ));
        }
        if selected.iter().any(|id| {
            !plan
                .items
                .iter()
                .any(|item| &item.id == id && item.can_clean)
        }) {
            return Err(error("Selection includes an unknown or protected item"));
        }
        self.store.begin_workspace_cleanup()?;
        let guard = WorkspaceGuard(self.store.clone());
        if self.store.has_active_tasks() {
            return Err(ApiError::new(
                "WORKSPACE_BUSY",
                "Wait for active project operations before cleanup",
            ));
        }
        let task = self.tasks.create_workspace_operation("workspaceCleanup")?;
        plan.task_id = Some(task.task_id.clone());
        plan.selected_ids = selected_ids;
        plans.insert(plan.plan_id.clone(), plan.clone());
        drop(plans);
        *self.coordinator.latest.lock().unwrap() = Some(task.task_id.clone());
        let manager = self.clone();
        let id = task.task_id.clone();
        let runner = tokio::spawn(async move {
            let worker = manager.clone();
            let task_id = id.clone();
            let failure_items = plan
                .items
                .iter()
                .filter(|item| plan.selected_ids.contains(&item.id))
                .map(Candidate::public)
                .collect::<Vec<_>>();
            let result = tokio::task::spawn_blocking(move || {
                let _guard = guard;
                worker.execute(&task_id, &plan)
            })
            .await
            .map_err(error)
            .and_then(|result| result);
            match result {
                Ok(value) if value["completed"] == true => {
                    let _ = manager.tasks.finish(&id, Ok(value));
                }
                Ok(value) => {
                    let _ = manager.tasks.finish_partial(
                        &id,
                        value,
                        ApiError::new(
                            "WORKSPACE_CLEANUP_INCOMPLETE",
                            "Some selected content was retained; inspect actual item results",
                        ),
                    );
                }
                Err(failure) => {
                    let value = json!({"completed":false,"error":failure,"items":failure_items});
                    let _ = manager.tasks.finish_partial(&id, value, failure);
                }
            }
        });
        running.push(runner);
        Ok(task)
    }
    pub(crate) fn latest_task(&self) -> Result<Option<TaskRecord>, ApiError> {
        self.coordinator
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .map(|id| self.tasks.task(id))
            .transpose()
    }
    pub(crate) async fn forget_deleted_project(
        &self,
        id: &str,
        source_ids: &[String],
        roots: &[PathBuf],
    ) -> Result<Vec<String>, ApiError> {
        if !(id.starts_with("prj_") && valid_id(id)
            || Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id))
            || source_ids
                .iter()
                .any(|source| !valid_id(source) || !source.starts_with("src_"))
            || roots.iter().any(|root| !normalized(root))
        {
            return Err(error("Invalid project cleanup ownership"));
        }
        self.coordinator.deleting.lock().unwrap().insert(id.into());
        let running = std::mem::take(&mut *self.coordinator.running.lock().unwrap());
        let issues = crate::shutdown::join_all("history-cleanup-deletion", running).await;
        if !issues.is_empty() {
            return Err(error("History inspection or cleanup has not stopped")
                .detail("issues", json!(issues)));
        }
        // Only typed candidate ownership and known scope paths are inspected.
        // Nested strings in another project's registration are never references.
        let mut ids = vec![];
        let mut plan_ids = vec![];
        for (plan_id, plan) in self.plans.lock().unwrap().iter() {
            let matches = plan.items.iter().any(|item| {
                deleted_scope_matches(
                    item.project_id.as_deref(),
                    &item.id,
                    &item.path,
                    id,
                    source_ids,
                    roots,
                )
            });
            if matches {
                plan_ids.push(plan_id.clone());
                ids.push(plan.preview_task_id.clone());
                if let Some(task) = &plan.task_id {
                    ids.push(task.clone());
                }
            }
        }
        for task in self.tasks.history_cleanup_tasks() {
            if task
                .result
                .and_then(|result| serde_json::from_value::<DerivedCleanupResult>(result).ok())
                .is_some_and(|result| {
                    result.items.iter().any(|item| {
                        normalized(&item.path)
                            && deleted_scope_matches(
                                item.project_id.as_deref(),
                                &item.id,
                                &item.path,
                                id,
                                source_ids,
                                roots,
                            )
                    })
                })
            {
                ids.push(task.task_id);
            }
        }
        ids.sort();
        ids.dedup();
        crate::workspace::local_cleanup_roots(&self.paths.selected_for_cleanup().map_err(error)?)?;
        self.tasks.forget_owned_tasks(&ids)?;
        self.plans
            .lock()
            .unwrap()
            .retain(|key, _| !plan_ids.contains(key));
        let mut latest = self.coordinator.latest.lock().unwrap();
        if latest.as_ref().is_some_and(|task| ids.contains(task)) {
            *latest = None;
        }
        Ok(ids)
    }
    pub(crate) fn finish_deleted_project(&self, id: &str) {
        self.coordinator.finish_deleted_project(id);
    }
    pub(crate) fn request_stop(&self) {
        self.coordinator.request_stop();
    }
    pub async fn shutdown(&self) -> Vec<crate::shutdown::ShutdownIssue> {
        self.coordinator.shutdown().await
    }
    fn candidates(&self) -> Result<Vec<Candidate>, ApiError> {
        let mut items = vec![];
        // Migration sources are finalized automatically after a verified
        // reopen, with retries bound to their migration ID. History owns only
        // completed source/data restore originals.
        self.restore_candidates(&mut items)?;
        items.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.id.cmp(&b.id)));
        Ok(items)
    }
    /// Retained originals of finished Git data/source restores. Unfinished
    /// operations are listed as protected so the user can see why they stay.
    fn restore_candidates(&self, items: &mut Vec<Candidate>) -> Result<(), ApiError> {
        let workspace = self.paths.workspace_root();
        for scope in ["data", "source"] {
            let projects = if scope == "data" {
                crate::data_restore_guard::operation_projects(workspace)?
            } else {
                crate::source_restore_guard::operation_projects(workspace)?
            };
            for project in projects {
                let name = self
                    .store
                    .project(&project)
                    .ok()
                    .map(|handle| handle.manifest.name.clone());
                let directory = workspace
                    .join(format!(".platform/project-state/{scope}-restore"))
                    .join(&project);
                let mut tasks = vec![];
                for entry in fs::read_dir(&directory)? {
                    let entry = entry?;
                    if let Some(task) = entry.file_name().to_str() {
                        if task.starts_with("task_") && valid_id(task) {
                            tasks.push((task.to_owned(), entry.path()));
                        }
                    }
                }
                tasks.sort();
                for (task, path) in tasks {
                    let label = if scope == "data" {
                        "数据取回前的原件"
                    } else {
                        "源码取回前的原件"
                    };
                    let mut item = Candidate::new(
                        format!("{scope}restore_{task}"),
                        "restoreOriginal",
                        label,
                        path,
                    );
                    item.project_id = Some(project.clone());
                    item.name = name.clone();
                    self.inspect(&mut item, |item| {
                        self.validate_restore(item, scope, &project)
                    });
                    items.push(item);
                }
            }
        }
        Ok(())
    }
    fn validate_restore(
        &self,
        item: &mut Candidate,
        scope: &str,
        project: &str,
    ) -> Result<(), ApiError> {
        let workspace = self.paths.workspace_root();
        let terminal = if scope == "data" {
            crate::data_restore_guard::terminal_operations(workspace, project)
        } else {
            crate::source_restore_guard::terminal_operations(workspace, project)
        }
        .map_err(|_| protected("这个项目还有未完成或无法核对的取回操作，全部取回现场继续保留"))?;
        if !terminal.contains(&item.path) {
            return Err(protected("这次取回尚未结束，原件仍用于继续或撤回"));
        }
        self.check_references(Some(project), None)
            .map_err(|_| protected("这个项目还有未完成的操作，取回现场继续保留"))
    }
    fn inspect(
        &self,
        item: &mut Candidate,
        validate: impl FnOnce(&mut Candidate) -> Result<(), ApiError>,
    ) {
        let result = (|| {
            self.validate_location(item)?;
            item.root_identity = Some(directory_identity(&item.path)?);

            item.parents = parent_identities(&item.path)?;
            item.entries = scan(&item.path)?;
            item.total_bytes = Some(item.entries.iter().try_fold(0u64, |total, entry| {
                total.checked_add(entry.bytes).ok_or_else(|| {
                    error("Cleanup logical byte total exceeds the supported integer range")
                })
            })?);
            item.file_count = Some(
                item.entries
                    .iter()
                    .filter(|entry| entry.kind != "directory")
                    .count() as u64,
            );
            validate(item)
        })();
        match result {
            Ok(()) => item.can_clean = true,
            Err(failure) => item
                .blocked_reasons
                .push(format!("{}: {}", failure.code, failure.message)),
        }
    }
    fn validate_location(&self, item: &Candidate) -> Result<(), ApiError> {
        let path = &item.path;
        if !normalized(path)
            || path.parent().is_none()
            || path
                .components()
                .any(|part| part.as_os_str().to_string_lossy().ends_with(".app"))
        {
            return Err(error("Unsafe cleanup location"));
        }
        let workspace = self.paths.workspace_root();
        let allowed = match item.kind.as_str() {
            "restoreOriginal" => [
                workspace.join(".platform/project-state/data-restore"),
                workspace.join(".platform/project-state/source-restore"),
            ],
            _ => return Err(error("Unknown historical cleanup category")),
        };
        if !allowed
            .iter()
            .any(|root| path.starts_with(root) && path != root)
        {
            return Err(error("Payload is outside its recorded owner directory"));
        }
        check_no_symlinks(path, false)
    }
    fn check_references(
        &self,
        project_id: Option<&str>,
        except: Option<&str>,
    ) -> Result<(), ApiError> {
        match project_id {
            Some(id) => crate::project_git::check_data_operations_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::project_git::check_data_workspace_available(self.paths.workspace_root())?
            }
        }
        match project_id {
            Some(id) => crate::source_restore_guard::check_development_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::source_restore_guard::check_workspace_available(self.paths.workspace_root())?
            }
        }
        match project_id {
            Some(id) => crate::data_restore_guard::check_development_available(
                self.paths.workspace_root(),
                id,
            )?,
            None => {
                crate::data_restore_guard::check_workspace_available(self.paths.workspace_root())?
            }
        }
        let operations =
            match fs::read_dir(self.paths.workspace_root().join(".platform/operations")) {
                Ok(entries) => Some(entries),
                Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => None,
                Err(failure) => return Err(failure.into()),
            };
        for directory in operations.into_iter().flatten() {
            let directory = directory?;
            if directory.file_name().to_str() == except {
                continue;
            }
            for file in ["journal.json"] {
                let path = directory.path().join(file);
                if !path.try_exists()? {
                    continue;
                }
                let journal = read_json(&path)?;
                if journal["resolved"] != true
                    && project_id.is_none_or(|id| {
                        journal["projectId"] == id || journal["sourceProjectId"] == id
                    })
                {
                    return Err(error(
                        "An unresolved operation still protects this historical content",
                    ));
                }
            }
        }
        Ok(())
    }
    fn execute(&self, task: &str, plan: &Plan) -> Result<Value, ApiError> {
        self.tasks.phase(task, "revalidating")?;
        let fresh = self.candidates()?;
        let mut selected = vec![];
        for id in &plan.selected_ids {
            let planned = plan
                .items
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| error("Unknown planned item"))?;
            let current = fresh
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| error("Planned content disappeared; preview again"))?;
            if !current.can_clean
                || current.path != planned.path
                || current.root_identity != planned.root_identity
                || current.parents != planned.parents
                || current.entries != planned.entries
                || current.guards != planned.guards
            {
                return Err(ApiError::new("CLEANUP_PLAN_CHANGED", "Selected content or protection evidence changed after preview; no deletion started").detail("itemId", id.clone()).detail("blockedReasons",json!(current.blocked_reasons)));
            }
            selected.push(planned);
        }
        let mut results = vec![];
        let mut stopped = false;
        for item in selected {
            if stopped {
                results.push(json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"skipped","completed":false,"reason":"A preceding item needs attention"}));
                continue;
            }
            self.tasks.phase(task, &format!("cleaning-{}", item.kind))?;
            let operation = remove_tree(item);
            let result = match operation {
                Ok(removed) => {
                    json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"removed","completed":true,"removed":removed,"completedAt":now_ms()})
                }
                Err(failure) => {
                    stopped = true;
                    json!({"id":item.id,"projectId":item.project_id,"kind":item.kind,"path":item.path,"status":"failed","completed":false,"error":failure})
                }
            };
            results.push(result);
        }
        let result = json!({"planId":plan.plan_id,"workspaceRoot":plan.workspace_root,"completed":!stopped,"items":results,"completedAt":now_ms()});
        Ok(result)
    }
}
