//! Web admission uses the same downloaded directory until registration commits.
use super::*;
use crate::{
    panda_manifest::BuildDecl,
    projects::WebImportState,
    web_build::{WebBuildContext, WebBuildRunner, WebBuildTarget},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitBuildConfirmation {
    pub revision: String,
    pub source_path: PathBuf,
    pub source_url: String,
    pub source_dir: String,
    pub script: String,
}

impl NodeGitImportManager {
    pub fn confirm_build(&self, import_id: &str, expected_revision: &str) -> Result<TaskRecord> {
        let records = self.records.lock().unwrap();
        let j = records
            .get(import_id)
            .ok_or_else(|| invalid("添加任务不存在。"))?;
        let task = self.tasks.task(&j.task_id)?;
        let confirmation = j.build_confirmation.as_ref().ok_or_else(|| {
            ApiError::new("GIT_BUILD_NOT_WAITING", "这次添加当前没有等待构建确认。")
        })?;
        if j.phase != "awaiting-build-confirmation"
            || task.status != crate::TaskStatus::WaitingConfirmation
            || j.commit_state != CommitState::NotCommitted
            || j.runtime != Some(PandaRuntime::Web)
            || task.project_id != j.web_project_id
        {
            return Err(ApiError::new(
                "GIT_BUILD_NOT_WAITING",
                "这次添加当前没有等待构建确认。",
            ));
        }
        if confirmation.revision != expected_revision {
            return Err(ApiError::new(
                "GIT_BUILD_CONFIRMATION_CHANGED",
                "构建声明已经变化，请重新查看后确认。",
            ));
        }
        let senders = self.confirmations.lock().unwrap();
        let sender = senders.get(import_id).ok_or_else(|| {
            ApiError::new(
                "GIT_BUILD_INTERRUPTED",
                "等待构建的添加已中断，请先完成清理再重试。",
            )
        })?;
        sender
            .send(true)
            .map_err(|_| ApiError::new("GIT_BUILD_INTERRUPTED", "等待构建的添加已中断。"))?;
        Ok(task)
    }

    pub(super) async fn run_web(
        &self,
        j: &mut Journal,
        manifest: PandaManifest,
        mut stop: watch::Receiver<bool>,
        mut cancel: watch::Receiver<bool>,
        scope: Arc<ImportScope>,
    ) -> Result<()> {
        let root = j
            .source_path
            .clone()
            .ok_or_else(|| invalid("项目目录缺失。"))?;
        let identity = j
            .source_identity
            .clone()
            .ok_or_else(|| invalid("项目目录身份缺失。"))?;
        let inspection = manifest
            .inspect_local(&root)
            .map_err(|e| invalid(&e.to_string()))?;
        if inspection["entryMissing"] == true && inspection["needsBuild"] != true {
            return Err(ApiError::new(
                "SOURCE_NEEDS_ADAPTATION",
                "web.entry 不存在，且未声明 development.build；尚未登记，请让 AI 准备入口后重试。",
            ));
        }
        let web_id = format!(
            "prj_{}",
            Uuid::parse_str(&j.app_id)
                .map_err(|_| invalid("导入标识无效。"))?
                .simple()
        );
        *scope._web_lease.lock().unwrap() = Some(self.store.reserve_task(&web_id, &j.task_id)?);
        j.web_project_id = Some(web_id.clone());
        // Persist the exact transfer intent before either ledger changes. A
        // reservation, even if a later write fails, prevents Node rollback.
        self.phase(j, "web-reserving")?;
        self.store
            .reserve_managed_source(&web_id, &j.task_id, &root, &identity)?;
        self.tasks.bind_web_operation(&j.task_id, &web_id)?;
        self.ownership()
            .release_imported_source(&j.app_id, &root, &identity)?;
        check(&stop, &cancel)?;
        if inspection["entryMissing"] == true {
            let development = manifest
                .development
                .as_ref()
                .ok_or_else(|| invalid("构建声明缺失。"))?;
            let BuildDecl::Npm { script } = &development.build;
            let revision = j
                .manifest_digest
                .clone()
                .ok_or_else(|| invalid("应用说明摘要缺失。"))?;
            j.build_confirmation = Some(GitBuildConfirmation {
                revision,
                source_path: root.clone(),
                source_url: j.input.url.clone(),
                source_dir: development.source_dir.clone(),
                script: script.clone(),
            });
            let (sender, mut confirmation) = watch::channel(false);
            self.confirmations
                .lock()
                .unwrap()
                .insert(j.input.import_id.clone(), sender);
            j.phase = "awaiting-build-confirmation".into();
            self.save(j)?;
            self.tasks.set_waiting_confirmation(&j.task_id)?;
            loop {
                check(&stop, &cancel)?;
                if *confirmation.borrow() {
                    break;
                }
                tokio::select! {
                    result = confirmation.changed() => {
                        result.map_err(|_| ApiError::new("GIT_BUILD_INTERRUPTED", "构建确认已中断。"))?;
                    }
                    _ = stop.changed() => {}
                    _ = cancel.changed() => {}
                }
            }
            self.confirmations
                .lock()
                .unwrap()
                .remove(&j.input.import_id);
            self.verify_web_source(j)?;
            j.build_confirmation = None;
            self.phase(j, "building")?;
            let operation = self
                .paths
                .data_root()
                .join("onboarding/imports")
                .join(&j.task_id);
            self.ownership().record_onboarding(&j.app_id, &j.task_id)?;
            private_dir(&operation)?;
            self.ownership().record_onboarding(&j.app_id, &j.task_id)?;
            j.temporary.insert(operation.clone(), capture(&operation)?);
            self.save(j)?;
            let task = j.task_id.clone();
            let tasks = self.tasks.clone();
            WebBuildRunner::run(
                WebBuildTarget {
                    project_root: root.clone(),
                    manifest,
                },
                WebBuildContext {
                    environment: self.tasks.environment().clone(),
                    operation_root: operation.join("operation"),
                    cache_root: operation.join("npm"),
                    processes: self.store.processes().with_writer_owner(
                        crate::durable_writer::WriterOwner::web(self.paths.data_root(), &web_id)?,
                    ),
                    owner: scope,
                    stop: stop.clone(),
                    cancel: cancel.clone(),
                    progress: Arc::new(move |phase| tasks.phase(&task, phase)),
                },
            )
            .await?;
        }
        check(&stop, &cancel)?;
        self.verify_web_source(j)?;
        j.registration_attempted = true;
        j.commit_state = CommitState::Unknown;
        self.phase(j, "registering")?;
        let info = self
            .store
            .register_managed_git(
                &web_id,
                &j.task_id,
                &root,
                &j.input.url,
                j.input.branch.as_deref(),
            )
            .await?;
        j.app = Some(serde_json::to_value(info)?);
        j.commit_state = CommitState::Committed;
        self.save(j)
    }

    fn verify_web_source(&self, j: &Journal) -> Result<()> {
        let root = j
            .source_path
            .as_ref()
            .ok_or_else(|| invalid("项目目录缺失。"))?;
        if !j
            .source_identity
            .as_ref()
            .is_some_and(|i| i.matches_path(root).unwrap_or(false))
        {
            return Err(invalid("下载目录的身份已变化，未执行构建或登记。"));
        }
        let bytes = read_manifest(root)?;
        if j.manifest_digest.as_deref() != Some(format!("{:x}", Sha256::digest(bytes)).as_str()) {
            return Err(ApiError::new(
                "GIT_BUILD_CONFIRMATION_CHANGED",
                "panda.json 已变化，未执行后续构建或登记。",
            ));
        }
        Ok(())
    }

    pub(super) async fn reconcile_web(&self, j: &mut Journal) -> Result<()> {
        let id = j
            .web_project_id
            .clone()
            .ok_or_else(|| invalid("网页实例标识缺失。"))?;
        let root = j
            .source_path
            .clone()
            .ok_or_else(|| invalid("项目目录缺失。"))?;
        let identity = j
            .source_identity
            .clone()
            .ok_or_else(|| invalid("项目目录身份缺失。"))?;
        let state = match self
            .store
            .web_import_state(&id, &j.task_id, &root, &identity)
        {
            Ok(state) => state,
            Err(issue) => {
                j.commit_state = CommitState::Unknown;
                self.save(j)?;
                return Err(issue);
            }
        };
        match state {
            WebImportState::Registered => {
                j.commit_state = CommitState::Committed;
                self.ownership()
                    .release_imported_source(&j.app_id, &root, &identity)?;
                if j.app.is_none() {
                    self.save(j)?;
                    j.app = Some(serde_json::to_value(self.store.info(&id).await?)?);
                }
            }
            WebImportState::Reserved => {
                j.commit_state = CommitState::NotCommitted;
                // The durable reservation also covers interrupted ledger writes.
                // Do not recreate a source already removed by partial rollback.
                self.ownership()
                    .release_imported_source(&j.app_id, &root, &identity)?;
                j.app = None;
            }
            WebImportState::Absent => {
                j.commit_state = CommitState::NotCommitted;
                j.app = None;
            }
        }
        self.save(j)
    }

    pub(super) fn cleanup_web(&self, j: &Journal) -> Result<bool> {
        let Some(id) = &j.web_project_id else {
            return Ok(false);
        };
        let root = j
            .source_path
            .as_ref()
            .ok_or_else(|| invalid("项目目录缺失。"))?;
        let identity = j
            .source_identity
            .as_ref()
            .ok_or_else(|| invalid("项目目录身份缺失。"))?;
        crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::web(
            self.paths.data_root(),
            id,
        )?)?;
        match self
            .store
            .web_import_state(id, &j.task_id, root, identity)?
        {
            WebImportState::Registered => Ok(true),
            WebImportState::Reserved => {
                self.ownership()
                    .release_imported_source(&j.app_id, root, identity)?;
                self.store
                    .discard_unpublished_project(id, &j.task_id, root, identity)?;
                if self.tasks.task(&j.task_id)?.project_id.as_deref() == Some(id.as_str()) {
                    self.tasks.release_web_operation(&j.task_id, id)?;
                }
                Ok(true)
            }
            WebImportState::Absent => {
                self.store.discard_empty_managed_reservation(id)?;
                if self.tasks.task(&j.task_id)?.project_id.as_deref() == Some(id.as_str()) {
                    self.tasks.release_web_operation(&j.task_id, id)?;
                }
                Ok(false)
            }
        }
    }

    pub(super) fn load_completed_web_imports(&self) -> Result<()> {
        let ownership = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        for task in self.tasks.operation_tasks("nodeGitImport") {
            let Some(id) = task.project_id.clone().filter(|id| id.starts_with("prj_")) else {
                continue;
            };
            if !self.store.is_registered(&id).unwrap_or(false) {
                continue;
            }
            if !task.status.is_final() {
                continue;
            }
            let Some(result) = task.result.as_ref() else {
                continue;
            };
            if result["commitState"] != "committed"
                || result["appId"] != id
                || result["webProjectId"] != id
                || result["runtime"] != "web"
                || result["cleanupPending"] != false
            {
                continue;
            }
            if !ownership
                .task_ids(&id)
                .unwrap_or_default()
                .contains(&task.task_id)
            {
                continue;
            }
            let Some(import_owner) = result["importOwnerId"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
            else {
                continue;
            };
            if id != format!("prj_{}", import_owner.simple()) {
                continue;
            }
            let Some(input) = result.get("input").and_then(|value| {
                serde_json::from_value::<StartNodeGitImportInput>(value.clone()).ok()
            }) else {
                continue;
            };
            // Terminal metadata never scans or adopts author source. Isolate a
            // damaged unrelated registration from the onboarding coordinator.
            let source_path = crate::projects::resolve_project_root(
                self.paths.data_root(),
                self.paths.workspace_root(),
                &id,
            )
            .ok()
            .or_else(|| result["sourcePath"].as_str().map(PathBuf::from));
            // An existing recovery journal takes precedence over terminal cache.
            let mut records = self.records.lock().unwrap();
            if records.contains_key(&input.import_id) {
                continue;
            }
            let j = Journal {
                version: 1,
                input,
                app_id: import_owner.to_string(),
                task_id: task.task_id.clone(),
                source_id: format!("src_{}", Uuid::nil().simple()),
                workspace: self.paths.workspace_root().into(),
                workspace_identity: capture(self.paths.workspace_root())?,
                owner_identity: None,
                runtime: Some(PandaRuntime::Web),
                web_project_id: Some(id),
                build_confirmation: None,
                retiring_control: false,
                phase: "completed".into(),
                registration_attempted: true,
                commit_state: CommitState::Committed,
                source_path,
                source_identity: None,
                manifest_digest: None,
                temporary: BTreeMap::new(),
                cleanup_pending: false,
                remaining_paths: vec![],
                app: result.get("app").filter(|value| !value.is_null()).cloned(),
                final_task: Some(task),
            };
            records.insert(j.input.import_id.clone(), j);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
