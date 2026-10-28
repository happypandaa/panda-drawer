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
        staging_identity: Option<PersistedIdentity>,
        state: TargetState,
    }

    #[derive(Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Record {
        version: u32,
        task_id: String,
        metadata: Value,
        data_root_identity: PersistedIdentity,
        instance_identity: PersistedIdentity,
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
            let app_id = metadata["appId"].as_str().unwrap();
            let instance = data_root.join("node-apps").join(app_id);
            ordinary(&instance, true)?;
            let task_id = Uuid::new_v4().to_string();
            let root = data_root.join("node-deletion-tasks");
            let directory = root.join(&task_id);
            let creating = root.join(format!(".creating-{task_id}"));
            let staging = instance.join(STAGING_NAME).join(&task_id);
            let mut names = vec!["configuration", "data", "cache", "logs"];
            if metadata["ownership"] == "managed" {
                names.push("generations");
            }
            let mut targets = Vec::new();
            for name in names {
                let original = instance.join(name);
                let identity = optional_identity(&original)?;
                let state = if identity.is_some() {
                    TargetState::Planned
                } else {
                    TargetState::Missing
                };
                targets.push(DirectoryTarget {
                    name: name.into(),
                    original,
                    staging_directory: staging.clone(),
                    staged: staging.join(name),
                    identity,
                    staging_identity: None,
                    state,
                });
            }
            // Validate the complete plan before publishing a task directory.
            // An invalid source must not leave an empty apparent pending task.
            let data_root_identity = PersistedIdentity::capture(data_root).map_err(fs_error)?;
            let instance_identity = PersistedIdentity::capture(&instance).map_err(fs_error)?;
            check_targets(&instance_identity, &targets)?;
            private_directory(&root, false)?;
            private_directory(&creating, true)?;
            observe(JournalPoint::TaskDirectoryCreated)?;
            let record = Record {
                version: 2,
                task_id,
                metadata,
                data_root_identity,
                instance_identity,
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
                    validate_unpublished(&entry.path())?;
                    continue;
                }
                canonical_uuid(&name)?;
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
        pub(crate) fn phase(&self) -> &str {
            &self.record.phase
        }
        pub(crate) fn is_terminal(&self) -> bool {
            matches!(self.phase(), "deleted" | "abandoned")
        }
        pub(crate) fn abandon_requested(&self) -> bool {
            self.record.abandon_requested
        }
        /// The manager defines and verifies this fixed-path capture binding.
        /// Restore bookkeeping must remain writable after abandonment has been
        /// requested, until the original registration is safely accounted for.
        pub(crate) fn bind_registration_removal(&mut self, binding: Value) -> Result<()> {
            if self.is_terminal() || !binding.is_object() {
                return Err(error("已结束任务或无效的登记收尾绑定不能写入。"));
            }
            self.record.metadata["registrationRemoval"] = binding;
            self.persist()
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
            self.check_instance()?;
            let mut changed = false;
            for index in 0..self.record.targets.len() {
                let target = &self.record.targets[index];
                if target.state == TargetState::Missing {
                    if !absent(&target.original)? {
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
                if target.state == TargetState::Removed && absent(&target.staging_directory)? {
                    // Final cleanup may remove the now-empty private staging
                    // directory before the terminal journal write commits.
                    if !absent(&target.original)? {
                        return Err(error("已删除目标位置重新出现目录，未报告完成。"));
                    }
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
            if self.is_terminal()
                || self.record.renamed_any
                || self.record.metadata["legacyIrreversible"] == true
            {
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
            self.check_instance()?;
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
                &self.record.instance_identity,
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

        pub(crate) fn snapshot(&self) -> Value {
            let pending_intent = self
                .record
                .targets
                .iter()
                .any(|target| target.state == TargetState::RenameIntent);
            json!({
                "taskId":self.task_id(), "appId":self.app_id(), "name":self.record.metadata["name"],
                "phase":self.phase(), "renamedAny":self.record.renamed_any,
                "canAbandon":!self.is_terminal() && !self.record.renamed_any && self.record.metadata["legacyIrreversible"] != true && !pending_intent && !self.record.abandon_requested,
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
        fn check_instance(&self) -> Result<()> {
            if !self
                .record
                .instance_identity
                .matches_path(&self.instance())
                .map_err(fs_error)?
            {
                return Err(error("项目实例目录身份已变化，未继续删除。"));
            }
            Ok(())
        }
        fn instance(&self) -> PathBuf {
            self.data_root.join("node-apps").join(self.app_id())
        }

        fn verify_staging_parent(&self, index: usize) -> Result<()> {
            self.check_instance()?;
            let target = &self.record.targets[index];
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
            self.check_instance()?;
            if self.record.targets[index].staging_identity.is_some() {
                return self.verify_staging_parent(index);
            }
            let parent = self.instance().join(STAGING_NAME);
            private_directory(&parent, false)?;
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
            File::open(self.instance())?.sync_all()?;
            self.persist()
        }

        fn cleanup_staging(&self) -> Result<()> {
            let mut removed = std::collections::BTreeSet::new();
            for target in &self.record.targets {
                let Some(expected) = &target.staging_identity else {
                    continue;
                };
                if !removed.insert(target.staging_directory.clone())
                    || absent(&target.staging_directory)?
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
            Ok(())
        }

        fn validate(&self) -> Result<()> {
            validate_metadata(&self.record.metadata)?;
            validate_identity(&self.record.data_root_identity)?;
            validate_identity(&self.record.instance_identity)?;
            validate_identity(&self.record.journal_directory_identity)?;
            canonical_uuid(self.task_id())?;
            if !matches!(self.record.version, 1 | 2)
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
            // Version 1 tasks predate application runtime logs. Keep their exact
            // reviewed targets so an upgrade can resume an existing deletion
            // without silently widening its destructive scope.
            let mut names = vec!["configuration", "data", "cache"];
            if self.record.version >= 2 {
                names.push("logs");
            }
            if self.record.metadata["ownership"] == "managed" {
                names.push("generations");
            }
            if self.record.targets.len() != names.len() {
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
                    || self.record.metadata["legacyIrreversible"] == true
                    || self.record.targets.iter().any(|target| {
                        !matches!(target.state, TargetState::Missing | TargetState::Planned)
                    }))
            {
                return Err(error("删除任务的放弃状态与不可逆移动或旧删除现场不一致。"));
            }
            let staging = self.instance().join(STAGING_NAME).join(self.task_id());
            for (target, name) in self.record.targets.iter().zip(names) {
                if let Some(identity) = &target.identity {
                    validate_identity(identity)?;
                }
                if let Some(identity) = &target.staging_identity {
                    validate_identity(identity)?;
                }
                if target.name != name
                    || target.original != self.instance().join(name)
                    || target.staging_directory != staging
                    || target.staged != staging.join(name)
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
            // Partial bounded temporary bytes are evidence, never an accepted
            // deletion authorization. Keep them without parsing or executing.
            read_record(&entry.path())?;
        }
        Ok(())
    }

    fn validate_metadata(value: &Value) -> Result<()> {
        let object = value
            .as_object()
            .ok_or_else(|| error("删除任务元数据无效。"))?;
        if object
            .get("legacyIrreversible")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(error("旧删除现场的不可逆状态无效。"));
        }
        canonical_uuid(
            object
                .get("appId")
                .and_then(Value::as_str)
                .ok_or_else(|| error("删除实例标识缺失。"))?,
        )?;
        if !matches!(value["ownership"].as_str(), Some("external" | "managed"))
            || value["name"]
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

    /// This version supports only the fixed private children of one instance.
    /// Independent managed author trees are outside this module's target set.
    fn preflight_targets(
        parent: &PersistedIdentity,
        targets: &[DirectoryTarget],
        mut mounted: impl FnMut(&Path) -> Result<bool>,
    ) -> Result<()> {
        for target in targets {
            let Some(identity) = &target.identity else {
                continue;
            };
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
pub(crate) use macos::NodeDeletionJournal;

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
        pub(crate) fn phase(&self) -> &str {
            "failed"
        }
        pub(crate) fn is_terminal(&self) -> bool {
            true
        }
        pub(crate) fn abandon_requested(&self) -> bool {
            false
        }
        pub(crate) fn bind_registration_removal(&mut self, _binding: Value) -> Result<()> {
            Err(unsupported())
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
        pub(crate) fn snapshot(&self) -> Value {
            json!({"phase":"failed","error":unsupported(),"canAbandon":false,"canRetry":false})
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::NodeDeletionJournal;
