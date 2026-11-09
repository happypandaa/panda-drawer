//! A temporary intent in the web-owned task outlives neutral control removal.
use super::*;
use crate::projects::WebImportState;
use std::io::Read;
const INTENT_FILE: &str = "import-retirement.json";

impl NodeGitImportManager {
    fn retirement_path(&self, j: &Journal) -> Result<PathBuf> {
        let id = j
            .web_project_id
            .as_ref()
            .ok_or_else(|| invalid("网页收尾标识缺失。"))?;
        let task = self.tasks.task(&j.task_id)?;
        if task.task_type != "nodeGitImport" || task.project_id.as_deref() != Some(id.as_str()) {
            return Err(invalid("网页收尾任务归属已变化。"));
        }
        let directory = self
            .paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&j.task_id);
        check_no_symlinks(&directory, false)?;
        let owner = crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        if !owner
            .task_directory_identity(id, &j.task_id)?
            .is_some_and(|identity| identity.matches_path(&directory).unwrap_or(false))
        {
            return Err(invalid("网页收尾任务目录身份已变化。"));
        }
        Ok(directory.join(INTENT_FILE))
    }
    fn validate_retirement(&self, j: &Journal) -> Result<PathBuf> {
        self.validate_record(j)?;
        if !j.retiring_control
            || j.commit_state != CommitState::Committed
            || j.owner_identity.is_none()
            || !j.temporary.is_empty()
        {
            return Err(invalid(
                "尚未完成项目登记与临时材料清理，不能退休导入控制目录。",
            ));
        }
        let id = j
            .web_project_id
            .as_ref()
            .ok_or_else(|| invalid("网页实例标识缺失。"))?;
        let source = j
            .source_path
            .as_ref()
            .ok_or_else(|| invalid("网页源码位置缺失。"))?;
        let identity = j
            .source_identity
            .as_ref()
            .ok_or_else(|| invalid("网页源码身份缺失。"))?;
        if self.store.web_import_state_with_ownership(
            id,
            &j.task_id,
            source,
            identity,
            self.import_ownership(j),
        )? != WebImportState::Registered
        {
            return Err(invalid("网页登记提交点尚未核对，未退休导入控制目录。"));
        }
        self.validate_owner(j, true)?;
        self.retirement_path(j)
    }
    fn persist_retirement(&self, j: &Journal) -> Result<PathBuf> {
        let path = self.validate_retirement(j)?;
        check_no_symlinks(&path, true)?;
        atomic_json(&path, &Self::recovery_record(j))?;
        self.records
            .lock()
            .unwrap()
            .insert(j.input.import_id.clone(), j.clone());
        Ok(path)
    }
    pub(super) fn begin_control_retirement(&self, j: &mut Journal) -> Result<()> {
        j.retiring_control = true;
        j.cleanup_pending = true;
        j.remaining_paths = vec![
            self.ownership().directory(&j.app_id)?,
            self.retirement_path(j)?,
        ];
        // A crash before the task intent write still has the original complete
        // owner. No removal starts until both durable writes have succeeded.
        self.save(j)?;
        self.persist_retirement(j)?;
        self.import_control_ownership().record_import_control(
            j.web_project_id.as_ref().unwrap(),
            &j.app_id,
            j.owner_identity.as_ref().unwrap(),
        )?;
        Ok(())
    }
    fn import_control_ownership(&self) -> crate::web_project_ownership::WebProjectOwnership {
        crate::web_project_ownership::WebProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        )
    }
    pub(super) fn finish_control_retirement(
        &self,
        j: &mut Journal,
        original: Option<ApiError>,
    ) -> Result<()> {
        let path = self.persist_retirement(j)?;
        let control = self.import_control_ownership();
        let owner = self.ownership().directory(&j.app_id)?;
        if owner.try_exists()? {
            control.record_import_control(
                j.web_project_id.as_ref().unwrap(),
                &j.app_id,
                j.owner_identity.as_ref().unwrap(),
            )?;
        }
        let outcome = (|| -> Result<()> {
            #[cfg(test)]
            if self.retirement_failure.swap(false, Ordering::AcqRel) {
                return Err(ApiError::new(
                    "DISPOSABLE_RETIREMENT_FAILURE",
                    "Disposable control retirement interruption",
                ));
            }
            crate::durable_writer::require_owner_clear(&crate::durable_writer::WriterOwner::node(
                self.paths.data_root(),
                &j.app_id,
            )?)?;
            if owner.try_exists()? {
                crate::history_cleanup::remove_owned_directory(
                    &owner,
                    j.owner_identity
                        .as_ref()
                        .ok_or_else(|| invalid("原导入目录身份缺失。"))?,
                )?;
            }
            control.release_import_control(
                j.web_project_id.as_ref().unwrap(),
                &j.app_id,
                j.owner_identity.as_ref().unwrap(),
            )?;
            j.cleanup_pending = false;
            j.remaining_paths.clear();
            let summary = Self::summary(j);
            match original {
                Some(issue) => self.tasks.finish_partial(&j.task_id, summary, issue)?,
                None => self.tasks.finish(&j.task_id, Ok(summary))?,
            }
            j.final_task = Some(self.tasks.task(&j.task_id)?);
            // The terminal task is durable before removing the last intent.
            let file = crate::files::read(
                &crate::files::root(path.parent().ok_or_else(|| invalid("任务目录缺失。"))?)?,
                INTENT_FILE,
            )?;
            let file_identity =
                PersistedIdentity::from_file(&file).map_err(|e| invalid(&e.to_string()))?;
            let parent = path.parent().ok_or_else(|| invalid("任务目录缺失。"))?;
            crate::cleanup_file_io::remove_file(
                &path,
                &file_identity,
                &[(parent.into(), capture(parent)?)],
            )?;
            j.retiring_control = false;
            Ok(())
        })();
        if let Err(issue) = outcome {
            j.cleanup_pending = true;
            j.final_task = None;
            j.remaining_paths = vec![owner, path];
            let _ = self.persist_retirement(j);
            self.records
                .lock()
                .unwrap()
                .insert(j.input.import_id.clone(), j.clone());
            return Err(issue);
        }
        self.blocked.lock().unwrap().remove(&j.app_id);
        self.records
            .lock()
            .unwrap()
            .insert(j.input.import_id.clone(), j.clone());
        Ok(())
    }
    pub(super) fn load_retirement_intents(&self) -> Result<()> {
        for task in self.tasks.operation_tasks("nodeGitImport") {
            if !task
                .project_id
                .as_deref()
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                continue;
            }
            let directory = self
                .paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&task.task_id);
            let path = directory.join(INTENT_FILE);
            match fs::symlink_metadata(&path) {
                Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => continue,
                Err(issue) => {
                    self.block(&task.task_id, issue.into());
                    continue;
                }
                Ok(_) => {}
            }
            let loaded = (|| -> Result<Journal> {
                let file = crate::files::read(&crate::files::root(&directory)?, INTENT_FILE)?;
                let mut bytes = vec![];
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 1024 * 1024 {
                    return Err(invalid("导入收尾记录过大。"));
                }
                let j: Journal = serde_json::from_slice(&bytes)?;
                self.validate_record(&j)?;
                if !j.retiring_control
                    || j.task_id != task.task_id
                    || j.web_project_id != task.project_id
                {
                    return Err(invalid("导入收尾记录与任务不符。"));
                }
                // Retain an otherwise valid intent even when the original
                // owner pathname was replaced: the user can restore its exact
                // directory and retry; no replacement is adopted or deleted.
                if let Err(issue) = self.validate_retirement(&j) {
                    self.block(&j.app_id, issue);
                }
                Ok(j)
            })();
            match loaded {
                Ok(j) => {
                    self.records
                        .lock()
                        .unwrap()
                        .insert(j.input.import_id.clone(), j);
                }
                Err(issue) => self.block(&task.task_id, issue),
            }
        }
        Ok(())
    }
}
