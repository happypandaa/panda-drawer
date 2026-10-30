//! A reversible Node pause for a whole-workspace switch. This is deliberately
//! separate from permanent platform shutdown and from recovery PID signalling.
use super::*;
use std::collections::BTreeSet;

/// Retain this receipt until the new runtime has reopened or the migration has
/// aborted. Dropping it only releases admission; it never restarts applications
/// or performs filesystem work, and cannot undo permanent shutdown flags.
pub struct NodeWorkspacePause {
    apps: NodeApps,
    gate: operations::MigrationGate,
}

impl NodeWorkspacePause {
    /// Native migration coordination calls this after the RuntimeHandle points
    /// to its fully initialized new workspace, before releasing the pause.
    pub fn validate_reopened(&self, workspace: &Path) -> Result<()> {
        ordinary_path(workspace, true)?;
        if let Some(runtime) = &self.apps.runtime {
            if runtime.info().workspace_root != workspace {
                return Err(ApiError::new(
                    "NODE_WORKSPACE_REOPEN_FAILED",
                    "Node 管理器尚未绑定本次迁移的新工作区。",
                ));
            }
            self.apps.set_deletion_store(runtime.store())?;
        }
        self.apps.set_source_workspace(workspace.to_path_buf())
    }
}

fn recovery(id: &str) -> ApiError {
    ApiError::new(
        "NODE_MIGRATION_RUN_RECOVERY_REQUIRED",
        "仍有应用运行或写入尚未确认结束。请先在平台处理运行状态，再迁移工作区；原目录已保留。",
    )
    .detail("appId", id)
}

fn ordinary_optional_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            ordinary_path(path, true)?;
            Ok(true)
        }
        Ok(_) => Err(fail(
            "Node 运行或归属记录目录的类型已变化，旧工作区已保留。",
        )),
    }
}

impl NodeApps {
    pub(super) fn current_processes(&self) -> crate::owned_process::OwnedProcessRegistry {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.store().processes())
            .unwrap_or_else(|| self.processes.clone())
    }

    /// Seal synchronously admitted work before spawning the independently owned
    /// drain. Cancelling the waiter cannot release admission while that worker
    /// still owns a start, backend, proxy or file reader.
    pub async fn begin_workspace_migration(&self) -> Result<NodeWorkspacePause> {
        let gate = self.operations.pause_for_migration()?;
        let apps = self.clone();
        tokio::spawn(async move {
            let pause = NodeWorkspacePause {
                apps: apps.clone(),
                gate,
            };
            pause.gate.wait_drained().await;
            let ids = {
                let inner = apps.inner.lock().await;
                if inner.shutting_down || apps.directory_stopping.load(Ordering::Acquire) {
                    return Err(fail("平台正在关闭，不能切换工作区。"));
                }
                inner.running.keys().cloned().collect::<Vec<_>>()
            };
            let results = futures_util::future::join_all(
                ids.iter()
                    .map(|id| apps.stop_runtime_with_reason(id, RuntimeStopReason::UserRequest)),
            )
            .await;
            results.into_iter().collect::<Result<Vec<_>>>()?;
            apps.wait_deletion_workers().await?;
            if !apps.inner.lock().await.running.is_empty() {
                return Err(fail("仍有 Node 后端或页面文件工作尚未确认停止。"));
            }
            apps.verify_workspace_migration_runs_clear()?;
            Ok(pause)
        })
        .await
        .map_err(|_| fail("Node 工作区暂停任务异常结束；未取得停止收据。"))?
    }

    /// Startup may have an empty `inner.running` after a crash while durable
    /// markers still name live or unknown workers. This check never signals a
    /// process. Only a verified stopped marker is retired by its original owner.
    /// Pending-cleanup retries after a successful reopen must not call this:
    /// those retries must leave newly running applications alone.
    pub fn verify_workspace_migration_runs_clear(&self) -> Result<()> {
        if self.operations.has_pending_deletion() {
            return Err(ApiError::new(
                "NODE_DELETION_PENDING",
                "仍有应用删除或收尾待处理，暂不能确认旧工作区已停止使用。",
            ));
        }
        Self::verify_workspace_migration_data_root_clear(&self.data_root)
    }

    /// A deferred reopen check must not stop applications already launched in
    /// the new workspace. Seal new admissions, drain real admitted owners, then
    /// exempt only exact live runs retained by this manager in the current root.
    /// Unknown old markers and independent durable writers still fail closed.
    pub async fn verify_previous_workspace_drained(&self, source: &Path) -> Result<()> {
        if !source.is_absolute()
            || source
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(fail("旧工作区路径无效，未取得停止收据。"));
        }
        let gate = self.operations.pause_for_migration()?;
        let apps = self.clone();
        let source = source.to_path_buf();
        tokio::spawn(async move {
            let _gate = gate;
            _gate.wait_drained().await;
            let current = apps
                .runtime
                .as_ref()
                .ok_or_else(|| fail("当前 Node 管理器未绑定重开的运行环境。"))?
                .info()
                .workspace_root;
            ordinary_path(&current, true)?;
            if current.starts_with(&source) {
                return Err(fail("当前运行环境仍指向旧工作区，不能清理。"));
            }
            let inner = apps.inner.lock().await;
            if inner.shutting_down || apps.directory_stopping.load(Ordering::Acquire) {
                return Err(fail("平台正在关闭，不能核对工作区。"));
            }
            let mut current_runs = BTreeSet::new();
            for (id, entry) in &inner.running {
                if entry.workspace_root.as_deref() != Some(current.as_path())
                    || entry
                        .source_root
                        .as_ref()
                        .is_none_or(|root| root.starts_with(&source))
                    || entry.phase() != RUNTIME_RUNNING
                    || has_start_intent_cleanup(&apps.root.join(id))?
                    || node_registration::registration_digest(&apps.root.join(id))?.as_deref()
                        != Some(entry.registration_digest.as_str())
                {
                    return Err(recovery(id));
                }
                let resources = entry.resources.try_lock().map_err(|_| recovery(id))?;
                let runtime = &resources.as_ref().ok_or_else(|| recovery(id))?.runtime;
                let process = runtime.process_identity().map_err(|_| recovery(id))?;
                let intent = inner.markers.get(id).ok_or_else(|| recovery(id))?;
                intent
                    .require_current(&apps.root.join(id).join("active.json"))
                    .map_err(|_| recovery(id))?;
                let marker: Value =
                    serde_json::from_slice(&intent.contents).map_err(|_| recovery(id))?;
                if marker["version"] != 2
                    || marker["appId"] != *id
                    || marker["hostPid"] != std::process::id()
                    || marker["phase"] != "running"
                    || marker["process"] != serde_json::to_value(process)?
                    || marker["runDirectory"].as_str().is_none_or(|path| {
                        Path::new(path).parent() != Some(apps.root.join(id).join("runs").as_path())
                    })
                {
                    return Err(recovery(id));
                }
                current_runs.insert(id.clone());
            }
            Self::verify_workspace_migration_data_root_except_current(
                &apps.data_root,
                &current_runs,
            )
        })
        .await
        .map_err(|_| fail("Node 工作区核对任务异常结束；未取得停止收据。"))?
    }

    /// Headless startup must apply the same crash-surviving check even when no
    /// native NodeApps object was constructed. This does not load or resume
    /// deletion tasks, retire unknown files, or signal old process identifiers.
    pub(crate) fn verify_workspace_migration_data_root_clear(data_root: &Path) -> Result<()> {
        Self::verify_workspace_migration_data_root_except_current(data_root, &BTreeSet::new())
    }

    fn verify_workspace_migration_data_root_except_current(
        data_root: &Path,
        current_runs: &BTreeSet<String>,
    ) -> Result<()> {
        ordinary_path(data_root, true)?;
        let deletions = data_root.join("node-deletion-tasks");
        if ordinary_optional_directory(&deletions)? && fs::read_dir(&deletions)?.next().is_some() {
            return Err(ApiError::new(
                "NODE_DELETION_PENDING",
                "仍有应用删除或收尾记录，旧工作区暂时保留。",
            ));
        }
        let root = data_root.join("node-apps");
        let mut ids = BTreeSet::new();
        let mut slots = BTreeMap::new();
        if ordinary_optional_directory(&root)? {
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if Uuid::parse_str(&id).is_err() {
                    continue;
                }
                let slot = entry.path();
                ordinary_path(&slot, true).map_err(|_| recovery(&id))?;
                ids.insert(id.clone());
                slots.insert(id, slot);
            }
        }
        let owners = data_root.join("node-project-ownership");
        if ordinary_optional_directory(&owners)? {
            for entry in fs::read_dir(&owners)? {
                let entry = entry?;
                if let Some(id) = entry
                    .file_name()
                    .to_str()
                    .filter(|id| Uuid::parse_str(id).is_ok())
                {
                    ordinary_path(&entry.path(), true).map_err(|_| recovery(id))?;
                    ids.insert(id.to_owned());
                }
            }
        }
        for id in ids {
            let owner = crate::durable_writer::WriterOwner::node(data_root, &id)?;
            crate::durable_writer::require_owner_clear(&owner)?;
        }
        for (id, slot) in slots {
            if current_runs.contains(&id) {
                continue;
            }
            let inspected = run_marker::inspect(&slot, &id, run_marker::boot_time_or_unknown())
                .map_err(|_| recovery(&id))?;
            match &inspected.state {
                run_marker::RunState::Absent => (),
                run_marker::RunState::Stopped => {
                    inspected.retire(&slot).map_err(|_| recovery(&id))?
                }
                run_marker::RunState::Orphaned(_) | run_marker::RunState::Unconfirmed => {
                    return Err(recovery(&id))
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_migration_check_preserves_unknown_deletion_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().canonicalize().unwrap();
        let evidence = data.join("node-deletion-tasks/disposable-incomplete-task/task.json");
        fs::create_dir_all(evidence.parent().unwrap()).unwrap();
        fs::write(&evidence, b"unknown pending deletion evidence").unwrap();
        let error = NodeApps::verify_workspace_migration_data_root_clear(&data).unwrap_err();
        assert_eq!(error.code, "NODE_DELETION_PENDING");
        assert_eq!(
            fs::read(&evidence).unwrap(),
            b"unknown pending deletion evidence"
        );
    }

    #[cfg(unix)]
    #[test]
    fn headless_migration_check_rejects_dangling_record_directory_links() {
        for name in ["node-apps", "node-project-ownership", "node-deletion-tasks"] {
            let temporary = tempfile::tempdir().unwrap();
            let data = temporary.path().canonicalize().unwrap();
            let path = data.join(name);
            std::os::unix::fs::symlink(data.join("disposable-missing-link-target"), &path).unwrap();
            assert!(NodeApps::verify_workspace_migration_data_root_clear(&data).is_err());
            assert!(fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn headless_migration_check_requires_unregistered_node_writer_receipts() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().canonicalize().unwrap();
        let workspace = data.join("disposable-workspace");
        fs::create_dir(&workspace).unwrap();
        let id = Uuid::new_v4().to_string();
        crate::node_project_ownership::NodeProjectOwnership::new(data.clone(), workspace)
            .initialize(&id)
            .unwrap();
        let marker = crate::durable_writer::WriterOwner::node(&data, &id)
            .unwrap()
            .fixture_mark_unconfirmed()
            .unwrap();
        assert!(!data.join("node-apps").exists());
        let error = NodeApps::verify_workspace_migration_data_root_clear(&data).unwrap_err();
        assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED");
        assert!(marker.is_file());
    }
}
