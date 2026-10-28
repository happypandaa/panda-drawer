//! Durable, bounded Node deletion plans. This module never stops processes or
//! removes registrations; its caller owns those responsibilities and admission.
//! Every destructive directory operation is confined to a fixed instance path
//! or the exact private staging directory recorded for that target.
#[cfg(target_os = "macos")]
mod macos {
    use crate::{file_identity::PersistedIdentity, ApiError};
    use serde::{Deserialize, Serialize};
    use serde_json::{json, Value};
    use std::{
        ffi::CString,
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
        path::{Component, Path, PathBuf},
    };
    use uuid::Uuid;

    type Result<T> = std::result::Result<T, ApiError>;
    const MAX_RECORD_BYTES: u64 = 1024 * 1024;
    const RECORD_NAME: &str = "task.json";
    const STAGING_NAME: &str = ".pending-deletions";

    fn error(message: impl Into<String>) -> ApiError {
        ApiError::new("NODE_DELETION_JOURNAL_UNCONFIRMED", message)
    }
    fn fs_error(value: impl std::fmt::Display) -> ApiError {
        error(value.to_string())
    }
    fn irreversible() -> ApiError {
        ApiError::new(
            "NODE_DELETION_CANNOT_ABANDON",
            "已有目录进入待清理位置，或移动结果尚未核实。不能放弃删除，请重试或继续。",
        )
    }

    #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    enum TargetState {
        Missing,
        Planned,
        RenameIntent,
        Staged,
        Removing,
        Removed,
    }

    #[derive(Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct DirectoryTarget {
        name: String,
        original: PathBuf,
        staging_directory: PathBuf,
        staged: PathBuf,
        identity: Option<PersistedIdentity>,
        parent_identity: Option<PersistedIdentity>,
        staging_identity: Option<PersistedIdentity>,
        staging_parent_identity: Option<PersistedIdentity>,
        staging_parent_created: bool,
        state: TargetState,
    }

    #[derive(Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Record {
        version: u32,
        task_id: String,
        metadata: Value,
        data_root_identity: PersistedIdentity,
        journal_directory_identity: PersistedIdentity,
        phase: String,
        journal_revision: u64,
        renamed_any: bool,
        abandon_requested: bool,
        targets: Vec<DirectoryTarget>,
        error: Option<Value>,
    }

    /// A caller must serialize mutations of one journal. It may transfer this
    /// object into a blocking owner and serve previously captured snapshots while
    /// removal runs; no module-wide or platform-wide lock is required.
    pub(crate) struct NodeDeletionJournal {
        data_root: PathBuf,
        directory: PathBuf,
        record: Record,
        expected_bytes: Vec<u8>,
    }

    impl NodeDeletionJournal {
        pub(crate) fn create(data_root: &Path, metadata: Value) -> Result<Self> {
            Self::create_with(data_root, metadata, |_| Ok(()))
        }
        fn create_with(
            data_root: &Path,
            metadata: Value,
            observe: impl FnMut(JournalPoint) -> Result<()>,
        ) -> Result<Self> {
            Self::create_checked(data_root, metadata, observe, |parent, targets| {
                preflight_targets(parent, targets, known_mount_point)
            })
        }
        fn create_checked(
            data_root: &Path,
            metadata: Value,
            mut observe: impl FnMut(JournalPoint) -> Result<()>,
            check_targets: impl FnOnce(&PersistedIdentity, &[DirectoryTarget]) -> Result<()>,
        ) -> Result<Self> {
            validate_metadata(&metadata)?;
            ordinary(data_root, true)?;
            let task_id = Uuid::new_v4().to_string();
            let root = data_root.join("node-deletion-tasks");
            let directory = root.join(&task_id);
            let creating = root.join(format!(".creating-{task_id}"));
            let paths = metadata["ownedPaths"].as_array().unwrap();
            let mut targets = Vec::new();
            for (index, path) in paths.iter().enumerate() {
                let original = PathBuf::from(path.as_str().unwrap());
                validate_target(data_root, &metadata, &original)?;
                let identity = optional_identity(&original)?;
                let parent_identity = optional_identity(original.parent().unwrap())?;
                let name = format!("target-{index}");
                let staging = original.parent().unwrap().join(STAGING_NAME).join(&task_id);
                let staging_parent_identity = optional_identity(staging.parent().unwrap())?;
                let state = if identity.is_some() {
                    TargetState::Planned
                } else {
                    TargetState::Missing
                };
                targets.push(DirectoryTarget {
                    name: name.clone(),
                    original,
                    staging_directory: staging.clone(),
                    staged: staging.join(name),
                    identity,
                    parent_identity,
                    staging_identity: None,
                    staging_parent_identity,
                    staging_parent_created: false,
                    state,
                });
            }
            let data_root_identity = PersistedIdentity::capture(data_root).map_err(fs_error)?;
            check_targets(&data_root_identity, &targets)?;
            private_directory(&root, false)?;
            private_directory(&creating, true)?;
            let mut unpublished = UnpublishedTask {
                directory: creating.clone(),
                identity: PersistedIdentity::capture(&creating).map_err(fs_error)?,
                published: false,
            };
            observe(JournalPoint::TaskDirectoryCreated)?;
            let record = Record {
                version: 3,
                task_id,
                metadata,
                data_root_identity,
                journal_directory_identity: PersistedIdentity::capture(&creating)
                    .map_err(fs_error)?,
                phase: "waiting".into(),
                journal_revision: 1,
                renamed_any: false,
                abandon_requested: false,
                targets,
                error: None,
            };
            let bytes = serde_json::to_vec_pretty(&record)?;
            atomic_record(&creating, None, &bytes)?;
            File::open(&creating)?.sync_all()?;
            observe(JournalPoint::BeforeTaskPublication)?;
            let anchor = directory_file(&root)?;
            let from = c_name(&creating)?;
            let to = c_name(&directory)?;
            rename_at(&anchor, &from, &anchor, &to, RenameMode::Exclusive)?;
            unpublished.published = true;
            let task_id = record.task_id.clone();
            let app_id = record.metadata["appId"].as_str().unwrap().to_owned();
            let publication = (|| {
                observe(JournalPoint::TaskPublished)?;
                anchor.sync_all()?;
                File::open(data_root)?.sync_all()?;
                Ok::<(), ApiError>(())
            })();
            publication.map_err(|cause| publication_unknown(&task_id, &app_id, cause))?;
            let journal = Self {
                data_root: data_root.into(),
                directory,
                record,
                expected_bytes: bytes,
            };
            journal
                .validate()
                .map_err(|cause| publication_unknown(&task_id, &app_id, cause))?;
            Ok(journal)
        }

        pub(crate) fn load(data_root: &Path, task_id: &str) -> Result<Self> {
            canonical_uuid(task_id)?;
            let directory = data_root.join("node-deletion-tasks").join(task_id);
            check_private_directory(&directory)?;
            let bytes = read_record(&directory.join(RECORD_NAME))?.1;
            let record: Record = serde_json::from_slice(&bytes)?;
            let value = Self {
                data_root: data_root.into(),
                directory,
                record,
                expected_bytes: bytes,
            };
            value.validate()?;
            value.confirm_durable()?;
            Ok(value)
        }

        pub(crate) fn list(data_root: &Path) -> Result<Vec<Self>> {
            let root = data_root.join("node-deletion-tasks");
            if absent(&root)? {
                return Ok(Vec::new());
            }
            ordinary(&root, true)?;
            let mut result = Vec::new();
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| error("删除任务目录名无效。"))?;
                if let Some(task_id) = name.strip_prefix(".creating-") {
                    canonical_uuid(task_id)?;
                    remove_unpublished(&entry.path(), None)?;
                    continue;
                }
                canonical_uuid(&name)?;
                // Current terminal purge unlinks task.json before rmdir. Its
                // empty, private UUID container is no remaining delete plan.
                if empty_published_task(&entry.path())? {
                    remove_empty_published_task(&entry.path())?;
                    continue;
                }
                result.push(Self::load(data_root, &name)?);
            }
            result.sort_by(|a, b| a.task_id().cmp(b.task_id()));
            Ok(result)
        }

        pub(crate) fn task_id(&self) -> &str {
            &self.record.task_id
        }
        pub(crate) fn app_id(&self) -> &str {
            self.record.metadata["appId"].as_str().unwrap()
        }
        pub(crate) fn metadata(&self) -> &Value {
            &self.record.metadata
        }
        /// Admission binds the ledger directory before writer teardown. Only
        /// that same ledger may complete its list once every writer is joined.
        pub(crate) fn refresh_owned_paths(
            &mut self,
            paths: Vec<PathBuf>,
            task_ids: Vec<String>,
            generated_source: Option<PathBuf>,
            external_paths: Vec<PathBuf>,
        ) -> Result<()> {
            if self.record.renamed_any
                || self.record.targets.iter().any(|target| {
                    !matches!(target.state, TargetState::Planned | TargetState::Missing)
                })
            {
                return Ok(());
            }
            self.validate()?;
            let ledger = self
                .data_root
                .join("node-project-ownership")
                .join(self.app_id());
            let accepted = self
                .record
                .targets
                .iter()
                .find(|target| target.original == ledger)
                .and_then(|target| target.identity.as_ref())
                .ok_or_else(|| error("受理时未绑定项目归属索引。"))?;
            if !accepted.matches_path(&ledger).map_err(fs_error)? {
                return Err(error("项目归属索引身份已变化，未扩大删除范围。"));
            }
            let mut paths = paths;
            // A writer may retire an entry; retain every already accepted path.
            for target in &self.record.targets {
                if !paths.contains(&target.original) {
                    paths.push(target.original.clone());
                }
            }
            paths.retain(|path| *path != ledger);
            paths.push(ledger);
            let mut metadata = self.record.metadata.clone();
            metadata["ownedPaths"] = serde_json::to_value(&paths)?;
            metadata["generatedSource"] = serde_json::to_value(generated_source)?;
            let mut external: std::collections::BTreeSet<PathBuf> =
                serde_json::from_value(self.metadata()["externalOwnedPaths"].clone())?;
            external.extend(external_paths);
            metadata["externalOwnedPaths"] = serde_json::to_value(external)?;
            let mut ids: std::collections::BTreeSet<String> =
                serde_json::from_value(self.metadata()["ownedTaskIds"].clone())?;
            ids.extend(task_ids);
            metadata["ownedTaskIds"] = serde_json::to_value(ids)?;
            validate_metadata(&metadata)?;
            let mut targets = Vec::new();
            for (index, original) in paths.into_iter().enumerate() {
                validate_target(&self.data_root, &metadata, &original)?;
                let identity = optional_identity(&original)?;
                if let Some(previous) = self
                    .record
                    .targets
                    .iter()
                    .find(|target| target.original == original)
                    .and_then(|target| target.identity.as_ref())
                {
                    if identity
                        .as_ref()
                        .is_some_and(|current| !previous.matches_captured(current))
                    {
                        return Err(error("已受理删除目录被替换，未使用新的目录身份。"));
                    }
                    if identity.is_none() {
                        absent_with_identity(&original, Some(previous))?;
                    }
                }
                let parent_identity = optional_identity(original.parent().unwrap())?;
                if let Some(previous) = self
                    .record
                    .targets
                    .iter()
                    .find(|target| target.original == original)
                    .and_then(|target| target.parent_identity.as_ref())
                {
                    if parent_identity
                        .as_ref()
                        .is_some_and(|current| !previous.matches_captured(current))
                    {
                        return Err(error("已受理删除目标的父目录身份变化。"));
                    }
                    if parent_identity.is_none() {
                        absent_with_identity(original.parent().unwrap(), Some(previous))?;
                    }
                }
                let name = format!("target-{index}");
                let staging = original
                    .parent()
                    .unwrap()
                    .join(STAGING_NAME)
                    .join(self.task_id());
                let previous_staging = self
                    .record
                    .targets
                    .iter()
                    .find(|target| target.staging_directory == staging);
                let staging_identity =
                    previous_staging.and_then(|target| target.staging_identity.clone());
                let staging_parent_identity = match previous_staging {
                    Some(target) => target.staging_parent_identity.clone(),
                    None => optional_identity(staging.parent().unwrap())?,
                };
                let staging_parent_created =
                    previous_staging.is_some_and(|target| target.staging_parent_created);
                let state = if identity.is_some() {
                    TargetState::Planned
                } else {
                    TargetState::Missing
                };
                targets.push(DirectoryTarget {
                    name: name.clone(),
                    original,
                    staging_directory: staging.clone(),
                    staged: staging.join(name),
                    identity,
                    parent_identity,
                    staging_identity,
                    staging_parent_identity,
                    staging_parent_created,
                    state,
                });
            }
            preflight_targets(&self.record.data_root_identity, &targets, known_mount_point)?;
            self.record.metadata = metadata;
            self.record.targets = targets;
            self.persist()
        }
        pub(crate) fn phase(&self) -> &str {
            &self.record.phase
        }
        pub(crate) fn is_terminal(&self) -> bool {
            matches!(self.phase(), "deleted" | "abandoned")
        }
        pub(crate) fn abandon_requested(&self) -> bool {
            self.record.abandon_requested
        }
        pub(crate) fn set_phase(&mut self, phase: &str) -> Result<()> {
            if self.is_terminal()
                || !matches!(
                    phase,
                    "waiting" | "stopping" | "staging" | "removing" | "failed"
                )
            {
                return Err(error("删除任务阶段不允许此转换。"));
            }
            self.record.phase = phase.into();
            self.record.error = None;
            self.persist()
        }

        pub(crate) fn mark_failed(&mut self, failure: &ApiError) -> Result<()> {
            if self.is_terminal() {
                return Err(error("已结束的删除任务不能标记失败。"));
            }
            self.record.phase = "failed".into();
            self.record.error = Some(serde_json::to_value(failure)?);
            self.persist()
        }

        /// Resolve the crash interval between rename and its journal update before
        /// answering whether no target has moved. An unknown interval stays gated.
        pub(crate) fn reconcile(&mut self) -> Result<()> {
            self.validate()?;
            let mut changed = false;
            for index in 0..self.record.targets.len() {
                let target = &self.record.targets[index];
                if target.state == TargetState::Missing {
                    if !absent_with_identity(&target.original, target.parent_identity.as_ref())? {
                        return Err(error(format!(
                            "确认时不存在的目录已经出现，未扩大删除范围：{}",
                            target.original.display()
                        )));
                    }
                    continue;
                }
                if target.state == TargetState::Planned {
                    continue;
                }
                if target.state == TargetState::Removed
                    && absent_with_identity(
                        &target.staging_directory,
                        target.staging_identity.as_ref(),
                    )?
                {
                    // Final cleanup may remove the now-empty private staging
                    // directory before the terminal journal write commits.
                    if !absent_with_identity(&target.original, target.identity.as_ref())? {
                        return Err(error("已删除目标位置重新出现目录，未报告完成。"));
                    }
                    absent_with_identity(&target.staged, target.identity.as_ref())?;
                    continue;
                }
                self.verify_staging_parent(index)?;
                let source = optional_identity(&target.original)?;
                let staged = optional_identity(&target.staged)?;
                let expected = target
                    .identity
                    .as_ref()
                    .ok_or_else(|| error("删除目标身份缺失。"))?;
                let source_matches = source
                    .as_ref()
                    .is_some_and(|current| expected.matches_captured(current));
                let staged_matches = staged
                    .as_ref()
                    .is_some_and(|current| expected.matches_captured(current));
                if source.is_none() {
                    absent_with_identity(&target.original, Some(expected))?;
                }
                if staged.is_none() {
                    absent_with_identity(&target.staged, Some(expected))?;
                }
                match target.state {
                    TargetState::RenameIntent if source_matches && staged.is_none() => {
                        self.record.targets[index].state = TargetState::Planned;
                        changed = true;
                    }
                    TargetState::RenameIntent if source.is_none() && staged_matches => {
                        self.record.targets[index].state = TargetState::Staged;
                        self.record.renamed_any = true;
                        changed = true;
                    }
                    TargetState::Removing if source.is_none() && staged.is_none() => {
                        self.record.targets[index].state = TargetState::Removed;
                        self.record.renamed_any = true;
                        changed = true;
                    }
                    TargetState::Staged | TargetState::Removing
                        if source.is_none() && staged_matches =>
                    {
                        self.record.renamed_any = true;
                    }
                    TargetState::Removed if source.is_none() && staged.is_none() => (),
                    _ => {
                        return Err(error(
                            "删除目标在原位置和待清理位置的身份不一致，已保留现场。",
                        ))
                    }
                }
            }
            if changed {
                self.persist()?;
            }
            Ok(())
        }

        pub(crate) fn can_abandon(&mut self) -> Result<bool> {
            if self.is_terminal() || self.record.renamed_any {
                return Ok(false);
            }
            self.validate()?;
            // Before the first persisted rename intent no directory operation can
            // have run. Abandoning that plan is safe even if its source changed or
            // went offline; abandonment never touches the source or staging paths.
            if self
                .record
                .targets
                .iter()
                .all(|target| matches!(target.state, TargetState::Planned | TargetState::Missing))
            {
                return Ok(true);
            }
            self.reconcile()?;
            if self.record.renamed_any {
                return Ok(false);
            }
            for target in &self.record.targets {
                if !matches!(target.state, TargetState::Planned | TargetState::Missing) {
                    return Ok(false);
                }
                if !absent(&target.staged)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        pub(crate) fn request_abandon(&mut self) -> Result<()> {
            if self.phase() == "abandoned" {
                return self.confirm_durable();
            }
            if !self.can_abandon()? {
                return Err(irreversible());
            }
            self.record.abandon_requested = true;
            self.persist()
        }

        pub(crate) fn abandon(&mut self) -> Result<()> {
            if self.phase() == "abandoned" {
                return self.confirm_durable();
            }
            if !self.can_abandon()? {
                return Err(irreversible());
            }
            let previous_phase = self.record.phase.clone();
            self.record.phase = "abandoned".into();
            self.record.abandon_requested = true;
            self.record.error = None;
            if let Err(failure) = self.persist() {
                self.record.phase = previous_phase;
                return Err(failure);
            }
            Ok(())
        }

        pub(crate) fn stage_next(&mut self) -> Result<bool> {
            self.stage_next_with(|_| Ok(()))
        }
        fn stage_next_with(
            &mut self,
            mut observe: impl FnMut(JournalPoint) -> Result<()>,
        ) -> Result<bool> {
            self.ensure_executable()?;
            self.reconcile()?;
            let Some(index) = self
                .record
                .targets
                .iter()
                .position(|target| target.state == TargetState::Planned)
            else {
                return Ok(false);
            };
            self.prepare_staging(index)?;
            self.record.phase = "staging".into();
            self.record.targets[index].state = TargetState::RenameIntent;
            self.persist()?;
            observe(JournalPoint::RenameIntent)?;
            let target = &self.record.targets[index];
            self.verify_staging_parent(index)?;
            let expected = target.identity.as_ref().unwrap();
            if !expected.matches_path(&target.original).map_err(fs_error)? {
                self.record.targets[index].state = TargetState::Planned;
                self.persist()?;
                return Err(error("删除前目录身份已变化，尚未移动该目录。"));
            }
            rename_directory(
                &target.original,
                &target.staged,
                expected,
                target
                    .parent_identity
                    .as_ref()
                    .ok_or_else(|| error("删除父目录身份缺失。"))?,
                target.staging_identity.as_ref().unwrap(),
            )?;
            self.record.renamed_any = true;
            observe(JournalPoint::Renamed)?;
            self.record.targets[index].state = TargetState::Staged;
            self.persist()?;
            observe(JournalPoint::Staged)?;
            Ok(true)
        }
        pub(crate) fn stage_all(&mut self) -> Result<()> {
            while self.stage_next()? {}
            Ok(())
        }

        pub(crate) fn remove_next_staged(&mut self) -> Result<bool> {
            self.remove_next_with(|_| Ok(()))
        }
        fn remove_next_with(
            &mut self,
            mut observe: impl FnMut(JournalPoint) -> Result<()>,
        ) -> Result<bool> {
            self.ensure_executable()?;
            self.reconcile()?;
            let Some(index) = self.record.targets.iter().position(|target| {
                matches!(target.state, TargetState::Staged | TargetState::Removing)
            }) else {
                return Ok(false);
            };
            if self.record.targets.iter().any(|target| {
                matches!(
                    target.state,
                    TargetState::Planned | TargetState::RenameIntent
                )
            }) {
                return Err(error("尚有目录未完成暂存，未开始递归删除。"));
            }
            self.record.phase = "removing".into();
            self.record.targets[index].state = TargetState::Removing;
            self.record.renamed_any = true;
            self.persist()?;
            observe(JournalPoint::RemoveIntent)?;
            let target = &self.record.targets[index];
            self.verify_staging_parent(index)?;
            crate::history_cleanup::remove_owned_directory(
                &target.staged,
                target.identity.as_ref().unwrap(),
            )?;
            observe(JournalPoint::Removed)?;
            self.record.targets[index].state = TargetState::Removed;
            self.persist()?;
            Ok(true)
        }
        #[cfg(test)]
        pub(crate) fn remove_staged_all(&mut self) -> Result<()> {
            while self.remove_next_staged()? {}
            Ok(())
        }

        /// Call only after registration and other platform bookkeeping succeeded.
        pub(crate) fn mark_completed(&mut self) -> Result<()> {
            self.ensure_executable()?;
            self.reconcile()?;
            if self
                .record
                .targets
                .iter()
                .any(|target| !matches!(target.state, TargetState::Missing | TargetState::Removed))
            {
                return Err(error("删除目录尚未全部完成。"));
            }
            self.cleanup_staging()?;
            let previous_phase = self.record.phase.clone();
            self.record.phase = "deleted".into();
            self.record.error = None;
            if let Err(failure) = self.persist() {
                self.record.phase = previous_phase;
                return Err(failure);
            }
            Ok(())
        }

        pub(crate) fn remove_terminal(&self) -> Result<()> {
            if !self.is_terminal() {
                return Err(error("删除尚未结束，不能移除恢复任务。"));
            }
            if absent_with_identity(
                &self.directory,
                Some(&self.record.journal_directory_identity),
            )? {
                directory_file(&self.data_root.join("node-deletion-tasks"))?.sync_all()?;
                return Ok(());
            }
            self.confirm_durable()?;
            self.cleanup_staging()?;
            crate::history_cleanup::remove_owned_directory(
                &self.directory,
                &self.record.journal_directory_identity,
            )?;
            directory_file(&self.data_root.join("node-deletion-tasks"))?.sync_all()?;
            Ok(())
        }

        pub(crate) fn snapshot(&self) -> Value {
            let pending_intent = self
                .record
                .targets
                .iter()
                .any(|target| target.state == TargetState::RenameIntent);
            json!({
                "taskId":self.task_id(), "appId":self.app_id(), "name":self.record.metadata["name"],
                "phase":self.phase(), "renamedAny":self.record.renamed_any,
                "canAbandon":!self.is_terminal() && !self.record.renamed_any && !pending_intent && !self.record.abandon_requested,
                "canRetry":self.phase() == "failed" && !self.record.abandon_requested,
                "abandonRequested":self.record.abandon_requested,
                "journalRevision":self.record.journal_revision, "error":self.record.error,
                "remaining":self.record.targets.iter().filter(|target| !matches!(target.state, TargetState::Missing | TargetState::Removed)).map(|target| json!({"path":target.original,"stagedPath":target.staged,"state":target.state})).collect::<Vec<_>>(),
                "targets":self.record.targets.iter().map(|target| json!({"path":target.original,"stagedPath":target.staged,"state":target.state})).collect::<Vec<_>>(),
                "preservedPaths":self.record.metadata["preservedPaths"], "ownership":self.record.metadata["ownership"], "sourcePath":self.record.metadata["sourcePath"]
            })
        }

        fn ensure_executable(&self) -> Result<()> {
            if self.is_terminal() || self.record.abandon_requested {
                return Err(error("删除任务已结束或已申请放弃，不再执行文件删除。"));
            }
            Ok(())
        }
        fn persist(&mut self) -> Result<()> {
            self.validate()?;
            self.record.journal_revision = self
                .record
                .journal_revision
                .checked_add(1)
                .ok_or_else(|| error("删除任务修订溢出。"))?;
            let bytes = serde_json::to_vec_pretty(&self.record)?;
            if let Err(failure) = atomic_record(&self.directory, Some(&self.expected_bytes), &bytes)
            {
                // Publishing can succeed before a parent fsync fails. Keep a
                // matching visible revision usable for a later explicit retry,
                // while still reporting that this write was not confirmed.
                if read_record(&self.directory.join(RECORD_NAME))
                    .is_ok_and(|(_, current)| current == bytes)
                {
                    self.expected_bytes = bytes;
                }
                return Err(failure);
            }
            self.expected_bytes = bytes;
            self.confirm_durable()
        }
        fn confirm_durable(&self) -> Result<()> {
            self.validate()?;
            let (record, bytes) = read_record(&self.directory.join(RECORD_NAME))?;
            if bytes != self.expected_bytes {
                return Err(error("删除任务落盘核对时修订已变化。"));
            }
            record.sync_all()?;
            directory_file(&self.directory)?.sync_all()?;
            directory_file(&self.data_root.join("node-deletion-tasks"))?.sync_all()?;
            directory_file(&self.data_root)?.sync_all()?;
            Ok(())
        }
        fn verify_staging_parent(&self, index: usize) -> Result<()> {
            let target = &self.record.targets[index];
            let parent = target.staging_directory.parent().unwrap();
            let parent_identity = target
                .staging_parent_identity
                .as_ref()
                .ok_or_else(|| error("待清理容器身份尚未落盘。"))?;
            if !parent_identity.matches_path(parent).map_err(fs_error)? {
                return Err(error("待清理容器身份已变化。"));
            }
            let expected = target
                .staging_identity
                .as_ref()
                .ok_or_else(|| error("待清理目录身份尚未落盘。"))?;
            if !expected
                .matches_path(&target.staging_directory)
                .map_err(fs_error)?
            {
                return Err(error("待清理目录身份或所在卷已变化。"));
            }
            Ok(())
        }
        fn prepare_staging(&mut self, index: usize) -> Result<()> {
            if self.record.targets[index].staging_identity.is_some() {
                return self.verify_staging_parent(index);
            }
            let original_parent = self.record.targets[index]
                .original
                .parent()
                .unwrap()
                .to_path_buf();
            let expected_parent = self.record.targets[index]
                .parent_identity
                .as_ref()
                .ok_or_else(|| error("删除目标父目录身份缺失。"))?;
            if !expected_parent
                .matches_path(&original_parent)
                .map_err(fs_error)?
            {
                return Err(error("删除目标父目录身份已变化。"));
            }
            let parent = original_parent.join(STAGING_NAME);
            let (parent_identity, parent_created) =
                match self.record.targets[index].staging_parent_identity.clone() {
                    Some(identity) if !absent(&parent)? => {
                        if !identity.matches_path(&parent).map_err(fs_error)? {
                            return Err(error("受理的待清理容器身份已变化。"));
                        }
                        check_private_directory(&parent)?;
                        (identity, self.record.targets[index].staging_parent_created)
                    }
                    _ => {
                        // Exclusive creation never adopts a container that appeared
                        // after the accepted scope was captured.
                        private_directory(&parent, true)?;
                        (PersistedIdentity::capture(&parent).map_err(fs_error)?, true)
                    }
                };
            for target in &mut self.record.targets {
                if target.staging_directory.parent() == Some(parent.as_path()) {
                    target.staging_parent_identity = Some(parent_identity.clone());
                    target.staging_parent_created = parent_created;
                }
            }
            File::open(&original_parent)?.sync_all()?;
            self.persist()?;
            let directory = self.record.targets[index].staging_directory.clone();
            if absent(&directory)? {
                private_directory(&directory, true)?;
            } else {
                private_directory(&directory, false)?;
                if fs::read_dir(&directory)?.next().is_some() {
                    return Err(error("未绑定的待清理目录已有内容，已保留现场。"));
                }
            }
            let identity = PersistedIdentity::capture(&directory).map_err(fs_error)?;
            for target in &mut self.record.targets {
                if target.staging_directory == directory {
                    target.staging_identity = Some(identity.clone());
                }
            }
            File::open(&parent)?.sync_all()?;
            File::open(original_parent)?.sync_all()?;
            self.persist()
        }

        fn cleanup_staging(&self) -> Result<()> {
            let mut removed = std::collections::BTreeSet::new();
            for target in &self.record.targets {
                let Some(expected) = &target.staging_identity else {
                    continue;
                };
                if !removed.insert(target.staging_directory.clone())
                    || absent_with_identity(&target.staging_directory, Some(expected))?
                {
                    continue;
                }
                if !expected
                    .matches_path(&target.staging_directory)
                    .map_err(fs_error)?
                {
                    return Err(error("待清理目录身份变化，未删除收尾目录。"));
                }
                let parent = directory_file(
                    target
                        .staging_directory
                        .parent()
                        .ok_or_else(|| error("待清理目录父级缺失。"))?,
                )?;
                let directory = directory_file(&target.staging_directory)?;
                if !expected
                    .matches_captured(&PersistedIdentity::from_file(&directory).map_err(fs_error)?)
                    || fs::read_dir(&target.staging_directory)?.next().is_some()
                {
                    return Err(error(format!(
                        "待清理目录还有未确认内容，已保留：{}",
                        target.staging_directory.display()
                    )));
                }
                let name = c_name(&target.staging_directory)?;
                if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                    != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                parent.sync_all()?;
            }
            let mut parents = std::collections::BTreeSet::new();
            for target in &self.record.targets {
                if !target.staging_parent_created {
                    continue;
                }
                let path = target.staging_directory.parent().unwrap();
                let expected = target
                    .staging_parent_identity
                    .as_ref()
                    .ok_or_else(|| error("待清理容器归属缺失。"))?;
                if !parents.insert(path.to_path_buf())
                    || absent_with_identity(path, Some(expected))?
                {
                    continue;
                }
                if !expected.matches_path(path).map_err(fs_error)? {
                    return Err(error("待清理容器被替换，未删除其他目录。"));
                }
                let directory = directory_file(path)?;
                if !expected
                    .matches_captured(&PersistedIdentity::from_file(&directory).map_err(fs_error)?)
                {
                    return Err(error("待清理容器在收尾时被替换。"));
                }
                // Another accepted task or a third party may have added its
                // own material. Never remove that content or its container.
                if fs::read_dir(path)?.next().is_some() {
                    continue;
                }
                let parent = directory_file(path.parent().unwrap())?;
                let name = c_name(path)?;
                if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                    != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                parent.sync_all()?;
            }
            Ok(())
        }

        fn validate(&self) -> Result<()> {
            validate_metadata(&self.record.metadata)?;
            validate_identity(&self.record.data_root_identity)?;
            validate_identity(&self.record.journal_directory_identity)?;
            canonical_uuid(self.task_id())?;
            if self.record.version != 3
                || self.record.journal_revision == 0
                || !matches!(
                    self.phase(),
                    "waiting"
                        | "stopping"
                        | "staging"
                        | "removing"
                        | "failed"
                        | "deleted"
                        | "abandoned"
                )
            {
                return Err(error("删除任务格式或阶段无效。"));
            }
            if self.directory
                != self
                    .data_root
                    .join("node-deletion-tasks")
                    .join(self.task_id())
            {
                return Err(error("删除任务位置与记录不一致。"));
            }
            if !self
                .record
                .data_root_identity
                .matches_path(&self.data_root)
                .map_err(fs_error)?
                || !self
                    .record
                    .journal_directory_identity
                    .matches_path(&self.directory)
                    .map_err(fs_error)?
            {
                return Err(error("删除任务目录或数据卷身份已变化。"));
            }
            let paths = self.record.metadata["ownedPaths"].as_array().unwrap();
            if self.record.targets.len() != paths.len() {
                return Err(error("删除目标数量无效。"));
            }
            if self.phase() == "deleted"
                && self.record.targets.iter().any(|target| {
                    !matches!(target.state, TargetState::Missing | TargetState::Removed)
                })
            {
                return Err(error("删除任务的完成状态与尚未完成的目标不一致。"));
            }
            if self.phase() == "abandoned"
                && (self.record.renamed_any
                    || self.record.targets.iter().any(|target| {
                        !matches!(target.state, TargetState::Missing | TargetState::Planned)
                    }))
            {
                return Err(error("删除任务的放弃状态与不可逆移动现场不一致。"));
            }
            for (index, (target, path)) in self.record.targets.iter().zip(paths).enumerate() {
                let original = PathBuf::from(path.as_str().unwrap());
                validate_target(&self.data_root, &self.record.metadata, &original)?;
                let staging = original
                    .parent()
                    .unwrap()
                    .join(STAGING_NAME)
                    .join(self.task_id());
                let name = format!("target-{index}");
                if let Some(identity) = &target.identity {
                    validate_identity(identity)?;
                }
                if let Some(identity) = &target.parent_identity {
                    validate_identity(identity)?;
                }
                if target.identity.is_some() && target.parent_identity.is_none() {
                    return Err(error("删除父目录身份缺失。"));
                }
                if let Some(identity) = &target.staging_identity {
                    validate_identity(identity)?;
                }
                if let Some(identity) = &target.staging_parent_identity {
                    validate_identity(identity)?;
                }
                if (target.staging_identity.is_some() || target.staging_parent_created)
                    && target.staging_parent_identity.is_none()
                {
                    return Err(error("待清理容器归属不完整。"));
                }
                if target.name != name
                    || target.original != original
                    || target.staging_directory != staging
                    || target.staged != staging.join(&name)
                {
                    return Err(error("删除任务包含范围外目录。"));
                }
                if (target.state == TargetState::Missing) != target.identity.is_none() {
                    return Err(error("删除目标身份与状态不一致。"));
                }
                if matches!(
                    target.state,
                    TargetState::RenameIntent
                        | TargetState::Staged
                        | TargetState::Removing
                        | TargetState::Removed
                ) && target.staging_identity.is_none()
                {
                    return Err(error("移动中的删除目标缺少暂存身份。"));
                }
                if matches!(
                    target.state,
                    TargetState::Staged | TargetState::Removing | TargetState::Removed
                ) && !self.record.renamed_any
                {
                    return Err(error("删除任务的移动历史不一致。"));
                }
            }
            Ok(())
        }
    }

    /// Read current ownership for the explicitly confirmed uninstall plan.
    /// This never reconciles, removes, or adopts an unpublished task.
    pub(crate) fn local_cleanup_owned_paths(data_root: &Path) -> Result<Vec<PathBuf>> {
        let root = data_root.join("node-deletion-tasks");
        if absent(&root)? {
            return Ok(Vec::new());
        }
        ordinary(&root, true)?;
        let mut paths = std::collections::BTreeSet::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| error("删除任务目录名无效。"))?;
            if let Some(id) = name.strip_prefix(".creating-") {
                canonical_uuid(id)?;
                validate_unpublished(&entry.path())?;
                paths.insert(entry.path());
                continue;
            }
            canonical_uuid(&name)?;
            if empty_published_task(&entry.path())? {
                paths.insert(entry.path());
                continue;
            }
            let journal = NodeDeletionJournal::load(data_root, &name)?;
            paths.insert(journal.directory.clone());
            paths.insert(journal.directory.join(RECORD_NAME));
            let mut parents = std::collections::BTreeSet::new();
            for target in &journal.record.targets {
                for path in [&target.original, &target.staged] {
                    if absent_with_identity(
                        path,
                        target.identity.as_ref().or(target.parent_identity.as_ref()),
                    )? {
                        continue;
                    }
                    let expected = target
                        .identity
                        .as_ref()
                        .ok_or_else(|| error("删除任务中原本缺失的目标已出现，未扩大卸载范围。"))?;
                    if !expected.matches_path(path).map_err(fs_error)? {
                        return Err(error("删除任务目标身份变化，未纳入卸载范围。"));
                    }
                    paths.insert(path.clone());
                }
                if let Some(expected) = &target.staging_identity {
                    if !absent_with_identity(&target.staging_directory, Some(expected))? {
                        if !expected
                            .matches_path(&target.staging_directory)
                            .map_err(fs_error)?
                        {
                            return Err(error("删除任务暂存目录身份变化。"));
                        }
                        for entry in fs::read_dir(&target.staging_directory)? {
                            let entry = entry?;
                            if !journal.record.targets.iter().any(|known| {
                                known.staged == entry.path()
                                    && known.identity.as_ref().is_some_and(|id| {
                                        id.matches_path(&entry.path()).unwrap_or(false)
                                    })
                            }) {
                                return Err(error("删除任务暂存目录含未受理材料。"));
                            }
                        }
                        paths.insert(target.staging_directory.clone());
                    }
                }
                if !target.staging_parent_created {
                    continue;
                }
                let parent = target.staging_directory.parent().unwrap();
                let expected = target
                    .staging_parent_identity
                    .as_ref()
                    .ok_or_else(|| error("删除任务暂存容器归属缺失。"))?;
                if !parents.insert(parent.to_path_buf())
                    || absent_with_identity(parent, Some(expected))?
                {
                    continue;
                }
                if !expected.matches_path(parent).map_err(fs_error)? {
                    return Err(error("删除任务暂存容器身份变化。"));
                }
                let mut only_current_task = true;
                for entry in fs::read_dir(parent)? {
                    let entry = entry?;
                    if !journal.record.targets.iter().any(|known| {
                        known.staging_directory == entry.path()
                            && known
                                .staging_identity
                                .as_ref()
                                .is_some_and(|id| id.matches_path(&entry.path()).unwrap_or(false))
                    }) {
                        only_current_task = false;
                        break;
                    }
                }
                if only_current_task {
                    paths.insert(parent.to_path_buf());
                }
            }
        }
        Ok(paths.into_iter().collect())
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum JournalPoint {
        TaskDirectoryCreated,
        BeforeTaskPublication,
        TaskPublished,
        RenameIntent,
        Renamed,
        Staged,
        RemoveIntent,
        Removed,
    }
    fn publication_unknown(task_id: &str, app_id: &str, cause: ApiError) -> ApiError {
        ApiError::new(
            "NODE_DELETION_TASK_PUBLICATION_UNCONFIRMED",
            "删除任务已经发布，但落盘结果尚未确认。请保留并核对原任务，不要创建第二份删除。",
        )
        .detail("taskId", task_id)
        .detail("appId", app_id)
        .detail("publicationUnknown", true)
        .detail(
            "publicationCause",
            serde_json::to_value(cause).unwrap_or(Value::Null),
        )
    }
    fn empty_published_task(directory: &Path) -> Result<bool> {
        check_private_directory(directory)?;
        Ok(fs::read_dir(directory)?.next().is_none())
    }
    fn remove_empty_published_task(directory: &Path) -> Result<()> {
        let identity = PersistedIdentity::capture(directory).map_err(fs_error)?;
        let opened = directory_file(directory)?;
        if !identity.matches_captured(&PersistedIdentity::from_file(&opened).map_err(fs_error)?)
            || !empty_published_task(directory)?
        {
            return Err(error("终态任务容器已变化，未清理其他材料。"));
        }
        let parent = directory_file(directory.parent().unwrap())?;
        let name = c_name(directory)?;
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        Ok(())
    }
    struct UnpublishedTask {
        directory: PathBuf,
        identity: PersistedIdentity,
        published: bool,
    }
    impl Drop for UnpublishedTask {
        fn drop(&mut self) {
            if !self.published {
                let _ = remove_unpublished(&self.directory, Some(&self.identity));
            }
        }
    }
    fn remove_unpublished(directory: &Path, expected: Option<&PersistedIdentity>) -> Result<()> {
        validate_unpublished(directory)?;
        let identity = PersistedIdentity::capture(directory).map_err(fs_error)?;
        if expected.is_some_and(|expected| !expected.matches_captured(&identity)) {
            return Err(error("未发布任务身份变化，未删除替换目录。"));
        }
        crate::history_cleanup::remove_owned_directory(directory, &identity)?;
        directory_file(
            directory
                .parent()
                .ok_or_else(|| error("删除任务父目录缺失。"))?,
        )?
        .sync_all()?;
        Ok(())
    }
    fn validate_unpublished(directory: &Path) -> Result<()> {
        check_private_directory(directory)?;
        let mut count = 0usize;
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            count += 1;
            if count > 4 {
                return Err(error("未发布删除任务包含额外材料，已保留并停止自动核对。"));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| error("未发布任务中的文件名无效。"))?;
            if name != RECORD_NAME {
                let id = name
                    .strip_prefix(".write-")
                    .ok_or_else(|| error("未发布删除任务包含未知材料。"))?;
                canonical_uuid(id)?;
            }
            // This private, unpublished scaffold cannot authorize project
            // deletion. Only its bounded protocol files may be discarded.
            read_record(&entry.path())?;
        }
        Ok(())
    }

    fn validate_metadata(value: &Value) -> Result<()> {
        let object = value
            .as_object()
            .ok_or_else(|| error("删除任务元数据无效。"))?;
        canonical_uuid(
            object
                .get("appId")
                .and_then(Value::as_str)
                .ok_or_else(|| error("删除实例标识缺失。"))?,
        )?;
        if !matches!(
            value["ownership"].as_str(),
            Some("external" | "managed" | "absent")
        ) || value["name"]
            .as_str()
            .is_none_or(|name| name.trim().is_empty() || name.len() > 1024)
        {
            return Err(error("删除任务的名称或归属无效。"));
        }
        let revision = value["expectedRegistrationRevision"]
            .as_str()
            .ok_or_else(|| error("删除确认修订缺失。"))?;
        if revision.len() != 64 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error("删除确认修订无效。"));
        }
        normalized(Path::new(
            value["sourcePath"]
                .as_str()
                .ok_or_else(|| error("原源码位置缺失。"))?,
        ))?;
        normalized(Path::new(
            value["workspaceRoot"]
                .as_str()
                .ok_or_else(|| error("工作区归属缺失。"))?,
        ))?;
        let paths = value["ownedPaths"]
            .as_array()
            .ok_or_else(|| error("项目完整删除范围缺失。"))?;
        if paths.is_empty() || paths.len() > 4096 {
            return Err(error("项目删除范围数量无效。"));
        }
        let mut unique = std::collections::BTreeSet::new();
        for path in paths {
            let path = Path::new(path.as_str().ok_or_else(|| error("删除路径无效。"))?);
            normalized(path)?;
            if !unique.insert(path.to_path_buf()) {
                return Err(error("删除范围含重复路径。"));
            }
        }
        for a in &unique {
            for b in &unique {
                if a != b && a.starts_with(b) {
                    return Err(error("删除范围不能嵌套。"));
                }
            }
        }
        let external = value["externalOwnedPaths"]
            .as_array()
            .ok_or_else(|| error("精确外部归属范围缺失。"))?;
        if external.len() > 4096 {
            return Err(error("精确外部归属范围过大。"));
        }
        for path in external {
            normalized(Path::new(
                path.as_str()
                    .ok_or_else(|| error("精确外部归属路径无效。"))?,
            ))?;
        }
        let preserved = value["preservedPaths"]
            .as_array()
            .ok_or_else(|| error("保留范围缺失。"))?;
        for path in preserved {
            normalized(Path::new(
                path.as_str().ok_or_else(|| error("保留路径无效。"))?,
            ))?;
        }
        Ok(())
    }
    fn validate_target(data: &Path, metadata: &Value, path: &Path) -> Result<()> {
        normalized(path)?;
        let id = metadata["appId"].as_str().unwrap();
        let workspace = Path::new(metadata["workspaceRoot"].as_str().unwrap());
        if path == data.join("node-apps").join(id)
            || path == data.join("node-project-ownership").join(id)
            || path == workspace.join(".platform/source-publications").join(id)
            || metadata["externalOwnedPaths"]
                .as_array()
                .is_some_and(|paths| {
                    paths.iter().any(|source| {
                        source
                            .as_str()
                            .is_some_and(|source| path == Path::new(source))
                    })
                })
        {
            return Ok(());
        }
        let parent = path.parent().unwrap();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| error("删除目录名无效。"))?;
        let identifier = |prefix: &str| {
            name.strip_prefix(prefix).is_some_and(|s| {
                s.len() == 32
                    && s.bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        };
        if (parent == data.join("node-apps/.installations") && identifier("install_"))
            || (parent == data.join("node-development-builds") && identifier("task_"))
            || (parent == data.join("onboarding/imports") && identifier("task_"))
            || (parent == workspace.join(".platform/tasks") && identifier("task_"))
            || (parent == workspace.join(".platform/sources") && identifier("src_"))
            || (parent == data.join("node-source-owners")
                && name.len() == 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
            || (parent == workspace.join("project-sources")
                && !name.starts_with('.')
                && !name.is_empty())
        {
            return Ok(());
        }
        Err(error("项目删除包含固定管理范围之外的路径。"))
    }
    fn canonical_uuid(value: &str) -> Result<()> {
        if Uuid::parse_str(value)
            .ok()
            .is_none_or(|id| id.is_nil() || id.to_string() != value)
        {
            return Err(error("删除任务或实例标识无效。"));
        }
        Ok(())
    }
    fn validate_identity(identity: &PersistedIdentity) -> Result<()> {
        if identity.is_legacy() || !identity.matches_captured(identity) {
            return Err(error("删除任务缺少有效的持久卷身份，未继续删除。"));
        }
        Ok(())
    }
    fn normalized(path: &Path) -> Result<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(error("删除路径必须是规范的绝对路径。"));
        }
        Ok(())
    }
    fn ordinary(path: &Path, directory: bool) -> Result<()> {
        normalized(path)?;
        crate::projects::check_no_symlinks(path, false)?;
        let value = fs::symlink_metadata(path)?;
        if (directory && !value.is_dir()) || (!directory && !value.is_file()) {
            return Err(error("删除路径不是预期的普通对象。"));
        }
        Ok(())
    }
    fn absent_with_identity(path: &Path, expected: Option<&PersistedIdentity>) -> Result<bool> {
        match expected {
            Some(expected) => expected.absent_on_expected_volume(path).map_err(fs_error),
            None => absent(path),
        }
    }
    fn absent(path: &Path) -> Result<bool> {
        normalized(path)?;
        crate::projects::check_no_symlinks(path, true)?;
        match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e.into()),
            Ok(_) => Ok(false),
        }
    }
    fn optional_identity(path: &Path) -> Result<Option<PersistedIdentity>> {
        if absent(path)? {
            return Ok(None);
        }
        ordinary(path, true)?;
        PersistedIdentity::capture(path).map(Some).map_err(fs_error)
    }
    fn private_directory(path: &Path, exclusive: bool) -> Result<()> {
        normalized(path)?;
        crate::projects::check_no_symlinks(path, true)?;
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        match builder.create(path) {
            Ok(()) => (),
            Err(e) if !exclusive && e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        check_private_directory(path)
    }
    fn check_private_directory(path: &Path) -> Result<()> {
        ordinary(path, true)?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(error("删除任务目录不是当前用户的私有目录。"));
        }
        Ok(())
    }

    fn read_record(path: &Path) -> Result<(File, Vec<u8>)> {
        ordinary(path, false)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.len() > MAX_RECORD_BYTES
        {
            return Err(error("删除任务文件类型、归属或大小无效。"));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let current = fs::symlink_metadata(path)?;
        if metadata.dev() != current.dev()
            || metadata.ino() != current.ino()
            || bytes.len() as u64 != metadata.len()
            || current.nlink() != 1
        {
            return Err(error("读取删除任务时文件身份发生变化。"));
        }
        Ok((file, bytes))
    }

    fn directory_file(path: &Path) -> Result<File> {
        ordinary(path, true)?;
        let before = PersistedIdentity::capture(path).map_err(fs_error)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        if !before.matches_captured(&PersistedIdentity::from_file(&file).map_err(fs_error)?) {
            return Err(error("打开删除目录时归属变化。"));
        }
        Ok(file)
    }
    fn c_name(path: &Path) -> Result<CString> {
        let name = path
            .file_name()
            .ok_or_else(|| error("删除路径名称缺失。"))?;
        CString::new(name.as_bytes()).map_err(|_| error("删除路径名称无效。"))
    }

    /// Every project target stages beside its own parent, including sources on
    /// another workspace volume. The target itself cannot be a mount point.
    fn preflight_targets(
        _parent: &PersistedIdentity,
        targets: &[DirectoryTarget],
        mut mounted: impl FnMut(&Path) -> Result<bool>,
    ) -> Result<()> {
        for target in targets {
            let Some(identity) = &target.identity else {
                continue;
            };
            let parent = target
                .parent_identity
                .as_ref()
                .ok_or_else(|| error("删除父目录身份缺失。"))?;
            if identity.device != parent.device
                || identity.volume_uuid != parent.volume_uuid
                || mounted(&target.original)?
            {
                return Err(ApiError::new(
                    "NODE_DELETION_TARGET_VOLUME_UNSUPPORTED",
                    format!(
                        "平台私有目录位于独立卷或挂载点，本次未受理删除、未移动任何目录：{}",
                        target.original.display()
                    ),
                ));
            }
        }
        Ok(())
    }
    fn known_mount_point(path: &Path) -> Result<bool> {
        let name =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| error("挂载点核对路径无效。"))?;
        let mut info: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(name.as_ptr(), &mut info) } != 0 {
            return Err(error("无法核对删除目标的挂载边界，未受理删除。"));
        }
        let end = info
            .f_mntonname
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| error("挂载边界返回值不完整，未受理删除。"))?;
        let bytes = info.f_mntonname[..end]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        Ok(path.as_os_str().as_bytes() == bytes.as_slice())
    }

    fn rename_directory(
        from: &Path,
        to: &Path,
        expected: &PersistedIdentity,
        source_parent: &PersistedIdentity,
        staging_parent: &PersistedIdentity,
    ) -> Result<()> {
        let left = directory_file(from.parent().ok_or_else(|| error("删除目标父目录缺失。"))?)?;
        let right = directory_file(to.parent().ok_or_else(|| error("暂存父目录缺失。"))?)?;
        let before = PersistedIdentity::capture(from).map_err(fs_error)?;
        if !source_parent.matches_captured(&PersistedIdentity::from_file(&left).map_err(fs_error)?)
            || !staging_parent
                .matches_captured(&PersistedIdentity::from_file(&right).map_err(fs_error)?)
            || !expected.matches_captured(&before)
            || left.metadata()?.dev() != right.metadata()?.dev()
            || before.device != left.metadata()?.dev()
            || known_mount_point(from)?
        {
            return Err(error("删除目标身份变化、跨卷或是挂载点，未移动目录。"));
        }
        let from_name = c_name(from)?;
        let to_name = c_name(to)?;
        rename_at(&left, &from_name, &right, &to_name, RenameMode::Exclusive)?;
        left.sync_all()?;
        right.sync_all()?;
        let captured = PersistedIdentity::capture(to).map_err(fs_error)?;
        if !expected.matches_captured(&captured) {
            // A public source name can be swapped immediately before rename. The
            // unexpected object is retained and restored only without overwriting.
            let _ = rename_at(&right, &to_name, &left, &from_name, RenameMode::Exclusive);
            let _ = left.sync_all();
            let _ = right.sync_all();
            return Err(error("移动后的目录身份不一致，已保留或归还现场。"));
        }
        Ok(())
    }

    enum RenameMode {
        Exclusive,
        Swap,
    }
    fn rename_at(
        left: &File,
        from: &CString,
        right: &File,
        to: &CString,
        mode: RenameMode,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        let status = unsafe {
            libc::renameatx_np(
                left.as_raw_fd(),
                from.as_ptr(),
                right.as_raw_fd(),
                to.as_ptr(),
                match mode {
                    RenameMode::Exclusive => libc::RENAME_EXCL,
                    RenameMode::Swap => libc::RENAME_SWAP,
                },
            )
        };
        #[cfg(not(target_os = "macos"))]
        let status = {
            let _ = (left, from, right, to, mode);
            -1
        };
        if status != 0 {
            return Err(error(format!(
                "删除事务的安全原子移动未完成：{}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Swap captures the old public record before unlinking it. A concurrently
    /// replaced record is retained/restored; it is never silently overwritten.
    fn atomic_record(directory: &Path, expected: Option<&[u8]>, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(error("删除任务记录过大。"));
        }
        let anchor = directory_file(directory)?;
        let public = directory.join(RECORD_NAME);
        let previous = expected
            .map(|expected| {
                let (file, bytes) = read_record(&public)?;
                if bytes != expected {
                    return Err(error("删除任务已被其他写入更改。"));
                }
                Ok(file)
            })
            .transpose()?;
        let temporary = directory.join(format!(".write-{}", Uuid::new_v4()));
        let temporary_name = c_name(&temporary)?;
        let fd = unsafe {
            libc::openat(
                anchor.as_raw_fd(),
                temporary_name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)?;
        file.sync_all()?;
        let public_name = c_name(&public)?;
        rename_at(
            &anchor,
            &temporary_name,
            &anchor,
            &public_name,
            if previous.is_some() {
                RenameMode::Swap
            } else {
                RenameMode::Exclusive
            },
        )?;
        anchor.sync_all()?;
        if let Some(previous) = previous {
            let (captured, captured_bytes) = read_record(&temporary)?;
            let before = previous.metadata()?;
            let after = captured.metadata()?;
            if before.dev() != after.dev()
                || before.ino() != after.ino()
                || Some(captured_bytes.as_slice()) != expected
            {
                // Restore only while the new public object is still ours.
                if let Ok((current, current_bytes)) = read_record(&public) {
                    let owned = file.metadata()?;
                    let actual = current.metadata()?;
                    if owned.dev() == actual.dev()
                        && owned.ino() == actual.ino()
                        && current_bytes == bytes
                    {
                        let _ = rename_at(
                            &anchor,
                            &temporary_name,
                            &anchor,
                            &public_name,
                            RenameMode::Swap,
                        );
                        let _ = anchor.sync_all();
                    }
                }
                return Err(error("捕获到已替换的删除任务记录，现有文件均已保留。"));
            }
            let captured_now = fs::symlink_metadata(&temporary)?;
            if captured_now.dev() != after.dev() || captured_now.ino() != after.ino() {
                return Err(error("删除任务旧记录再次变化，已保留。"));
            }
            if unsafe { libc::unlinkat(anchor.as_raw_fd(), temporary_name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            anchor.sync_all()?;
        }
        let (published, published_bytes) = read_record(&public)?;
        if file.metadata()?.ino() != published.metadata()?.ino() || published_bytes != bytes {
            return Err(error("删除任务发布后身份发生变化。"));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        include!("node_deletion_journal_tests.rs");
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::{local_cleanup_owned_paths, NodeDeletionJournal};

#[cfg(not(target_os = "macos"))]
mod unsupported {
    use crate::ApiError;
    use serde_json::{json, Value};
    use std::path::Path;
    type Result<T> = std::result::Result<T, ApiError>;
    fn unsupported() -> ApiError {
        ApiError::new(
            "NODE_DELETION_PLATFORM_UNSUPPORTED",
            "持久删除事务目前仅验证 macOS，此平台不会开始文件删除。",
        )
    }
    pub(crate) struct NodeDeletionJournal {
        _private: (),
    }
    impl NodeDeletionJournal {
        pub(crate) fn create(_root: &Path, _metadata: Value) -> Result<Self> {
            Err(unsupported())
        }
        pub(crate) fn load(_root: &Path, _task_id: &str) -> Result<Self> {
            Err(unsupported())
        }
        pub(crate) fn list(_root: &Path) -> Result<Vec<Self>> {
            Err(unsupported())
        }
        pub(crate) fn task_id(&self) -> &str {
            ""
        }
        pub(crate) fn app_id(&self) -> &str {
            ""
        }
        pub(crate) fn metadata(&self) -> &Value {
            unreachable!("Unsupported journal cannot be constructed")
        }
        pub(crate) fn refresh_owned_paths(
            &mut self,
            _paths: Vec<std::path::PathBuf>,
            _task_ids: Vec<String>,
            _generated_source: Option<std::path::PathBuf>,
            _external_paths: Vec<std::path::PathBuf>,
        ) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn phase(&self) -> &str {
            "failed"
        }
        pub(crate) fn is_terminal(&self) -> bool {
            true
        }
        pub(crate) fn abandon_requested(&self) -> bool {
            false
        }
        pub(crate) fn set_phase(&mut self, _phase: &str) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn mark_failed(&mut self, _failure: &ApiError) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn reconcile(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn can_abandon(&mut self) -> Result<bool> {
            Err(unsupported())
        }
        pub(crate) fn request_abandon(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn abandon(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn stage_next(&mut self) -> Result<bool> {
            Err(unsupported())
        }
        pub(crate) fn stage_all(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn remove_next_staged(&mut self) -> Result<bool> {
            Err(unsupported())
        }
        #[cfg(test)]
        pub(crate) fn remove_staged_all(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn mark_completed(&mut self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn remove_terminal(&self) -> Result<()> {
            Err(unsupported())
        }
        pub(crate) fn snapshot(&self) -> Value {
            json!({"phase":"failed","error":unsupported(),"canAbandon":false,"canRetry":false})
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::NodeDeletionJournal;

#[cfg(not(target_os = "macos"))]
pub(crate) fn local_cleanup_owned_paths(
    _data_root: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, crate::ApiError> {
    Err(crate::ApiError::new(
        "UNSUPPORTED_PLATFORM",
        "Node 删除归属仅支持当前平台。",
    ))
}
