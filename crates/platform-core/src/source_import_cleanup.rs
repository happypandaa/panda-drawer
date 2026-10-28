//! Cleanup of one explicitly completed import. Every attempt inspects current
//! source/install ownership; plans and detailed results are never written to disk.
use super::*;

#[derive(Clone)]
struct ImportTarget {
    path: PathBuf,
    identity: PersistedIdentity,
}

struct ImportPlan {
    owner_app_id: String,
    source_task_id: String,
    source_id: String,
    source_retained: bool,
    source_identity: PersistedIdentity,
    targets: Vec<ImportTarget>,
    removed_paths: Vec<PathBuf>,
    completed: bool,
    bindings: BTreeMap<String, Value>,
    installations: BTreeMap<String, Value>,
}

fn blocked(message: &str) -> ApiError {
    ApiError::new("SOURCE_IMPORT_CLEANUP_BLOCKED", message)
}

fn process_unconfirmed(value: &Value) -> bool {
    value["processResourceClosed"] == false
        || value["candidateOwnershipConfirmed"] == false
        || value["operationOutcome"] == "unknown"
}

impl CleanupManager {
    /// Native manager only. Its caller admits this blocking operation into the
    /// native I/O lifetime; no background deletion outlives that owner.
    pub(crate) fn source_import_cleanup(&self, source_task_id: &str) -> Result<Value, ApiError> {
        self.ensure_running()?;
        if !valid_id(source_task_id) || !source_task_id.starts_with("task_") {
            return Err(blocked("导入任务编号无效。"));
        }
        let task = match self.tasks.task(source_task_id) {
            Ok(task) => task,
            Err(error) if error.code == "TASK_NOT_FOUND" => {
                return Ok(
                    json!({"completed":true,"alreadyAbsent":true,"sourceId":null,"sourceRetained":false,"removedPaths":[],"failures":[]}),
                )
            }
            Err(error) => return Err(error),
        };
        if !matches!(task.task_type.as_str(), "sourceAcquire" | "sourceCapture")
            || !task.status.is_final()
            || task
                .error
                .as_ref()
                .is_some_and(|error| process_unconfirmed(&error.details))
        {
            return Err(blocked(
                "源码任务尚未停止或其进程归属仍需核对，请稍后重试清理。",
            ));
        }
        let _admission = self.admission.lock().unwrap();
        self.store.begin_workspace_cleanup()?;
        let _workspace = WorkspaceGuard(self.store.clone());
        if self.store.has_active_tasks() {
            return Err(blocked("相关任务或进程仍在收尾，请稍后重试清理。"));
        }
        let installer = self.installer()?;
        let _commit = installer.commit_lock()?;
        let result = task
            .result
            .as_ref()
            .ok_or_else(|| blocked("来源任务缺少受理归属。"))?;
        let source_id = result["sourceId"]
            .as_str()
            .filter(|id| id.starts_with("src_") && valid_id(id))
            .ok_or_else(|| blocked("来源任务缺少源码归属。"))?;
        let owner = result["ownerAppId"]
            .as_str()
            .filter(|id| Uuid::parse_str(id).is_ok())
            .ok_or_else(|| blocked("来源任务缺少项目归属。"))?;
        let registration = self.node_root().join(owner).join("registration.json");
        check_no_symlinks(&registration, true)?;
        if !registration.exists() {
            return self.remove_unregistered_import(owner, source_id);
        }
        let mut plan = self.import_plan(source_task_id)?;
        let source = self.sources_root().join(&plan.source_id);
        check_no_symlinks(&source, true)?;
        if source.try_exists()? && !plan.source_identity.matches_path(&source).map_err(error)? {
            return Err(blocked("导入源码目录的身份已变化，未继续清理。"));
        }
        if !plan.source_retained && self.import_source_referenced(&plan.source_id, &source)? {
            return Err(blocked("已有应用使用这份源码，未继续删除源码。"));
        }
        let bindings = source.join("install-tasks");
        check_no_symlinks(&bindings, true)?;
        if bindings.try_exists()? {
            for entry in fs::read_dir(&bindings)? {
                let live = read_json(&entry?.path())?;
                let task_id = live["prepareTaskId"]
                    .as_str()
                    .filter(|id| valid_id(id))
                    .ok_or_else(|| blocked("源码安装绑定无效，未继续清理。"))?;
                if plan.bindings.get(task_id) != Some(&live) {
                    return Err(blocked(
                        "这份源码的安装任务已变化，请先处理新任务，未继续清理。",
                    ));
                }
                self.import_task_stopped(task_id, "nodeInstallPrepare")?;
            }
        }
        self.validate_import_installations(&plan)?;
        let mut failures = vec![];
        for target in plan.targets.clone() {
            if plan.removed_paths.contains(&target.path) {
                continue;
            }
            match fs::symlink_metadata(&target.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    plan.removed_paths.push(target.path.clone());
                    continue;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if let Err(failure) = self.validate_import_target(&plan, &target) {
                failures.push(json!({"path":target.path,"error":failure}));
                break;
            }
            match remove_owned_directory(&target.path, &target.identity) {
                Ok(_) => plan.removed_paths.push(target.path.clone()),
                Err(failure) => failures.push(json!({"path":target.path,"error":failure})),
            }
            // Keep later targets, especially source metadata, for a safe retry.
            if !failures.is_empty() {
                break;
            }
        }
        plan.completed = failures.is_empty()
            && plan
                .targets
                .iter()
                .all(|target| plan.removed_paths.contains(&target.path) && matches!(fs::symlink_metadata(&target.path), Err(error) if error.kind() == std::io::ErrorKind::NotFound));
        Ok(import_result(&plan, failures))
    }

    fn remove_unregistered_import(&self, owner: &str, source: &str) -> Result<Value, ApiError> {
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let tasks = ownership.task_ids(owner)?;
        for id in &tasks {
            if self.tasks.task(id).is_ok_and(|task| {
                !task.status.is_final()
                    || task
                        .error
                        .as_ref()
                        .is_some_and(|error| process_unconfirmed(&error.details))
            }) {
                return Err(blocked("项目任务尚未结束，未清理材料。"));
            }
        }
        let paths = ownership.owned_paths(owner)?;
        let mut targets = vec![];
        add_target(&mut targets, self.node_root().join(owner))?;
        for path in &paths {
            add_target(&mut targets, path.clone())?;
        }
        // Capture and then recheck the same persisted index before mutation.
        if ownership.owned_paths(owner)? != paths {
            return Err(blocked("项目材料归属已变化，请重试。"));
        }
        let owner_directory = ownership.directory(owner)?;
        let mut removed = vec![];
        for target in targets
            .iter()
            .filter(|target| target.path != owner_directory)
        {
            remove_owned_directory(&target.path, &target.identity)?;
            removed.push(target.path.clone());
        }
        self.tasks.forget_owned_tasks(&tasks)?;
        if let Some(target) = targets.iter().find(|target| target.path == owner_directory) {
            remove_owned_directory(&target.path, &target.identity)?;
            removed.push(target.path.clone());
        }
        Ok(
            json!({"completed":true,"sourceId":source,"sourceRetained":false,"removedPaths":removed,"failures":[]}),
        )
    }

    fn import_plan(&self, task: &str) -> Result<ImportPlan, ApiError> {
        let task_record = self.tasks.task(task)?;
        let result = task_record
            .result
            .as_ref()
            .ok_or_else(|| blocked("来源任务缺少受理归属。"))?;
        let source_id = result["sourceId"]
            .as_str()
            .filter(|id| id.starts_with("src_") && valid_id(id))
            .ok_or_else(|| blocked("来源任务缺少源码归属。"))?
            .to_owned();
        let source = self.sources_root().join(&source_id);
        let record = read_json(&source.join("source.json"))?;
        let owner_app_id = result["ownerAppId"]
            .as_str()
            .ok_or_else(|| blocked("来源任务缺少项目归属。"))?
            .to_owned();
        if record["sourceId"] != source_id
            || record["ownerAppId"] != owner_app_id
            || record["taskId"] != task
            || record["schemaVersion"] != 1
            || !matches!(
                record["status"].as_str(),
                Some("ready" | "failed" | "cancelled" | "interrupted")
            )
            || process_unconfirmed(&record["error"]["details"])
            || record["phase"] == "process-unconfirmed"
        {
            return Err(blocked("源码记录或进程停止状态仍需核对，未删除文件。"));
        }
        let retained = self.import_source_referenced(&source_id, &source)?;
        let mut targets = vec![];
        let mut saved_bindings = BTreeMap::new();
        let mut installations = BTreeMap::new();
        let bindings = source.join("install-tasks");
        check_no_symlinks(&bindings, true)?;
        if bindings.try_exists()? {
            for entry in fs::read_dir(&bindings)? {
                let entry = entry?;
                let binding = read_json(&entry.path())?;
                let task_id = binding["prepareTaskId"]
                    .as_str()
                    .ok_or_else(|| blocked("安装任务记录无效。"))?;
                if binding["sourceId"] != source_id
                    || !valid_id(task_id)
                    || !task_id.starts_with("task_")
                {
                    return Err(blocked("安装任务与当前源码不一致。"));
                }
                self.import_task_stopped(task_id, "nodeInstallPrepare")?;
                saved_bindings.insert(task_id.to_owned(), binding.clone());
                add_target(
                    &mut targets,
                    self.paths
                        .data_root()
                        .join("node-development-builds")
                        .join(task_id),
                )?;
            }
        }
        let states = self.owned_import_installations(&owner_app_id)?;
        for state in states {
            if state.source_id.as_deref() != Some(source_id.as_str()) {
                continue;
            }
            let journal = read_json(&state.path.join("install.json"))?;
            self.validate_import_installation(&journal)?;
            installations.insert(state.candidate_id.clone(), journal);
            add_target(&mut targets, state.path)?;
        }
        // Source is last so interrupted staging removal retains its binding.
        add_target(
            &mut targets,
            if retained {
                source.join(".acquiring")
            } else {
                source.clone()
            },
        )?;
        Ok(ImportPlan {
            owner_app_id,
            source_task_id: task.into(),
            source_id,
            source_retained: retained,
            source_identity: PersistedIdentity::capture(&source).map_err(error)?,
            targets,
            removed_paths: vec![],
            completed: false,
            bindings: saved_bindings,
            installations,
        })
    }

    fn owned_import_installations(
        &self,
        owner: &str,
    ) -> Result<Vec<crate::node_app_install::InstallationState>, ApiError> {
        self.installer()?.owned_installation_states(owner)
    }

    fn import_source_referenced(&self, id: &str, slot: &Path) -> Result<bool, ApiError> {
        let source = read_json(&slot.join("source.json"))?;
        if source["sourceId"] != id {
            return Err(blocked("源码项目归属无效。"));
        }
        let owner = source["ownerAppId"]
            .as_str()
            .filter(|id| Uuid::parse_str(id).is_ok())
            .ok_or_else(|| blocked("源码缺少项目归属。"))?;
        let registration = self.node_root().join(owner).join("registration.json");
        check_no_symlinks(&registration, true)?;
        Ok(registration.is_file())
    }
    fn import_task_stopped(&self, id: &str, expected: &str) -> Result<(), ApiError> {
        let task = self.tasks.task(id)?;
        if task.task_type != expected
            || !task.status.is_final()
            || task
                .error
                .as_ref()
                .is_some_and(|error| process_unconfirmed(&error.details))
        {
            return Err(blocked("安装任务或其子进程尚未确认停止，未清理材料。"));
        }
        Ok(())
    }

    fn validate_import_installation(&self, journal: &Value) -> Result<(), ApiError> {
        if !matches!(
            journal["state"].as_str(),
            Some("prepared" | "committed" | "failed" | "interrupted" | "committing")
        ) || process_unconfirmed(&journal["error"]["details"])
            || journal["target"]["registrationDigest"].is_string() && journal["state"] == "prepared"
        {
            return Err(blocked("安装提交结果或更新候选仍需处理，未清理材料。"));
        }
        let prepare = journal["prepareTaskId"]
            .as_str()
            .ok_or_else(|| blocked("安装记录缺少任务编号。"))?;
        self.import_task_stopped(prepare, "nodeInstallPrepare")?;
        if let Some(apply) = journal["applyTaskId"].as_str() {
            self.import_task_stopped(apply, "nodeInstallApply")?;
            if journal["state"] != "committed" {
                let task = self.tasks.task(apply)?;
                let outcome = task
                    .result
                    .as_ref()
                    .and_then(|value| value["operationOutcome"].as_str())
                    .or_else(|| {
                        task.error
                            .as_ref()
                            .and_then(|error| error.details["operationOutcome"].as_str())
                    });
                if !matches!(outcome, Some("not-committed" | "not_committed")) {
                    return Err(blocked("安装曾提交但结果尚待核对，未清理材料。"));
                }
                // A terminal task alone is insufficient. Under the commit lock,
                // positively verify that registration still matches the old
                // pointer before abandoning an explicitly uncommitted import.
                let target: crate::node_app_install::InstallTargetSnapshot =
                    serde_json::from_value(journal["target"].clone())?;
                target.validate()?;
                if crate::node_app_install::registration_digest(
                    &self.node_root().join(&target.app_id),
                )? != target.registration_digest
                {
                    return Err(blocked("当前应用登记与未提交结果不一致，未清理材料。"));
                }
            }
        } else if journal["state"] == "committing" {
            return Err(blocked("安装提交缺少最终任务记录，未清理材料。"));
        }
        Ok(())
    }

    fn validate_import_installations(&self, plan: &ImportPlan) -> Result<(), ApiError> {
        for state in self.owned_import_installations(&plan.owner_app_id)? {
            if state.source_id.as_deref() == Some(plan.source_id.as_str()) {
                if !plan.targets.iter().any(|target| target.path == state.path) {
                    return Err(blocked(
                        "这份源码新增了安装候选，请先处理新候选，未继续清理。",
                    ));
                }
                self.validate_import_installation(&read_json(&state.path.join("install.json"))?)?;
            } else if !state.is_terminal() && state.source_id.is_none() {
                // A partial retry can have already removed its own journal.
                let owned = plan.targets.iter().any(|target| target.path == state.path)
                    && plan.installations.contains_key(&state.candidate_id);
                if !owned {
                    return Err(blocked("存在来源无法核对的安装记录，未删除源码。"));
                }
            }
        }
        Ok(())
    }

    fn validate_import_target(
        &self,
        plan: &ImportPlan,
        target: &ImportTarget,
    ) -> Result<(), ApiError> {
        let source = self.sources_root().join(&plan.source_id);
        let installation =
            target.path.parent() == Some(self.node_root().join(".installations").as_path());
        let build = target.path.parent()
            == Some(
                self.paths
                    .data_root()
                    .join("node-development-builds")
                    .as_path(),
            );
        let allowed = target.path == source && !plan.source_retained
            || target.path == source.join(".acquiring")
            || target.path.parent() == Some(self.node_root().join(".installations").as_path())
                && target
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("install_"))
            || target.path.parent()
                == Some(
                    self.paths
                        .data_root()
                        .join("node-development-builds")
                        .as_path(),
                )
                && target
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("task_"));
        if !allowed || !target.identity.matches_path(&target.path).map_err(error)? {
            return Err(blocked("清理目标或目录身份与本次导入不一致。"));
        }
        if installation {
            let id = target
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| blocked("安装目标标识无效。"))?;
            let archived = plan
                .installations
                .get(id)
                .ok_or_else(|| blocked("安装材料缺少本次检查的归属。"))?;
            let bound = archived["sourceId"]
                .as_str()
                .or_else(|| archived["prepared"]["provenance"]["sourceId"].as_str());
            if archived["candidateId"] != id || bound != Some(plan.source_id.as_str()) {
                return Err(blocked("安装材料与本次导入来源不一致。"));
            }
            self.validate_import_installation(archived)?;
            let live = target.path.join("install.json");
            if live.try_exists()? && &read_json(&live)? != archived {
                return Err(blocked("安装材料记录在清理后已变化，请保留材料。"));
            }
        } else if build {
            let id = target
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| blocked("构建任务标识无效。"))?;
            let binding = plan
                .bindings
                .get(id)
                .ok_or_else(|| blocked("构建材料缺少本次检查的归属。"))?;
            if binding["sourceId"] != plan.source_id || binding["prepareTaskId"] != id {
                return Err(blocked("构建材料与本次导入来源不一致。"));
            }
            self.import_task_stopped(id, "nodeInstallPrepare")?;
        } else {
            let record = source.join("source.json");
            if record.try_exists()? {
                let live = read_json(&record)?;
                if live["sourceId"] != plan.source_id || live["taskId"] != plan.source_task_id {
                    return Err(blocked("源码记录与本次导入不一致。"));
                }
            }
        }
        Ok(())
    }
}

fn add_target(targets: &mut Vec<ImportTarget>, path: PathBuf) -> Result<(), ApiError> {
    check_no_symlinks(&path, true)?;
    if path.try_exists()? {
        targets.push(ImportTarget {
            identity: PersistedIdentity::capture(&path).map_err(error)?,
            path,
        });
    }
    Ok(())
}

fn import_result(plan: &ImportPlan, failures: Vec<Value>) -> Value {
    json!({"completed":plan.completed,"sourceId":plan.source_id,"sourceRetained":plan.source_retained,
        "removedPaths":plan.removed_paths,"failures":failures})
}

#[cfg(test)]
#[path = "source_import_cleanup_tests.rs"]
mod tests;
