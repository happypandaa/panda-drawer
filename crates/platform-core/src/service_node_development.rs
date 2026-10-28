use super::*;
use crate::node_apps::{NodeApps, NodeDevelopmentSource};

pub enum NodeGitAction {
    Status,
    Versions(u32),
    RestorePreview(crate::node_development_git::NodeRestorePreviewInput),
    RestoreApply(crate::node_development_git::NodeRestoreApplyInput),
    RestoreStatus,
    RestoreRecover(crate::node_development_git::NodeRestoreRecoverInput),
}
pub struct NodeDevelopmentLease<'a> {
    _source_lease: crate::node_apps::NodeDevelopmentSourceLease<'a>,
    pub source: NodeDevelopmentSource,
    pub instance_id: String,
    _task: Arc<crate::projects::ProjectTaskLease>,
    active: Active,
}
impl NodeDevelopmentLease<'_> {
    // Retain only actual per-operation admission. Active/global I/O belong to
    // the earlier service drain and must never be captured by a process G.
    fn keep_alive(&self) -> Arc<dyn Send + Sync> {
        Arc::new((self._task.clone(), self._source_lease.keep_alive()))
    }
    pub fn validate(&self) -> std::result::Result<(), ApiError> {
        if self.active.0.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        self.active.0.store.check_workspace_available()?;
        self._source_lease.validate()?;
        self.source.validate()
    }
}
// Post-action admission may change after the Git/filesystem commit point.
// Preserve that actual receipt while retaining the validation error itself.
fn finalize_node_git_result(
    lease: &NodeDevelopmentLease<'_>,
    result: Value,
) -> std::result::Result<Value, ApiError> {
    if let Err(issue) = lease.validate() {
        let issue = if result["operationOutcome"] == "committed" {
            issue.outcome("committed")
        } else {
            issue
        };
        return Err(issue.detail("result", result));
    }
    Ok(result)
}

impl RuntimeHandle {
    pub async fn node_development_lease<'a>(
        &self,
        apps: &'a NodeApps,
        app_id: &str,
        editable: bool,
    ) -> std::result::Result<NodeDevelopmentLease<'a>, ApiError> {
        let state = self.current();
        apps.set_source_workspace(state.info.workspace_root.clone())?;
        state.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        state.node_development.require_idle()?;
        state.node_install.require_idle()?;
        let task = state.store.reserve_transient_operation(
            &format!("node-development:{app_id}"),
            "node-development",
        )?;
        let source_lease = apps.development_source_lease(app_id).await?;
        source_lease.validate()?;
        let source = source_lease.source().clone();
        if editable {
            apps.require_no_source_restore(app_id)?;
            crate::node_development_git::require_source_restore_clear(&source.source_root)?;
        }
        let lease = NodeDevelopmentLease {
            _source_lease: source_lease,
            source,
            instance_id: state.info.instance_id.clone(),
            _task: Arc::new(task),
            active,
        };
        lease.validate()?;
        Ok(lease)
    }
    async fn node_source_restore_lease<'a>(
        &self,
        apps: &'a NodeApps,
        app_id: &str,
    ) -> std::result::Result<NodeDevelopmentLease<'a>, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        state.node_development.require_idle()?;
        state.node_install.require_idle()?;
        let task = state.store.reserve_transient_operation(
            &format!("node-development:{app_id}"),
            "node-source-restore",
        )?;
        let source_lease = apps.development_restore_source_lease(app_id).await?;
        let source = source_lease.source().clone();
        let lease = NodeDevelopmentLease {
            _source_lease: source_lease,
            source,
            instance_id: state.info.instance_id.clone(),
            _task: Arc::new(task),
            active,
        };
        lease.validate()?;
        Ok(lease)
    }

    pub async fn node_development_info(
        &self,
        apps: &NodeApps,
        app_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let lease = self.node_development_lease(apps, app_id, false).await?;
        Ok(
            json!({"sourceRoot":lease.source.source_root,"runtimeIsSource":lease.source.runtime_is_source,"instanceId":lease.instance_id}),
        )
    }
    pub async fn node_development_git(
        &self,
        apps: &NodeApps,
        app_id: &str,
        action: NodeGitAction,
    ) -> std::result::Result<Value, ApiError> {
        let restore = matches!(
            action,
            NodeGitAction::RestorePreview(_)
                | NodeGitAction::RestoreApply(_)
                | NodeGitAction::RestoreStatus
                | NodeGitAction::RestoreRecover(_)
        );
        let lease = if restore {
            self.node_source_restore_lease(apps, app_id).await?
        } else {
            self.node_development_lease(apps, app_id, false).await?
        };
        let git = crate::node_development_git::NodeGit::new(lease.active.0.git_environment.clone())
            .with_stop(lease.active.0.stop.subscribe())
            .with_source_restore_ownership(
                crate::node_project_ownership::NodeProjectOwnership::new(
                    lease.active.0.paths.data_root().to_path_buf(),
                    lease.active.0.paths.workspace_root().to_path_buf(),
                ),
            )
            .with_process_scope(
                lease.active.0.store.processes().with_writer_owner(
                    crate::durable_writer::WriterOwner::node(
                        lease.active.0.paths.data_root(),
                        app_id,
                    )?,
                ),
                lease.keep_alive(),
            );
        let root = &lease.source.source_root;
        let scope = crate::node_development_git::NodeRestoreScope {
            app_id: app_id.into(),
            registration_digest: lease._source_lease.registration_digest().into(),
            control_path: lease._source_lease.source_restore_control_path(),
        };
        if matches!(action, NodeGitAction::Versions(_)) {
            apps.require_no_source_restore(app_id)?;
        }
        // Keep native requests bounded even when a source transaction is
        // reached through several service/operation futures on the default stack.
        let result = match action {
            NodeGitAction::Status => {
                let mut status = Box::pin(git.status(root)).await?;
                if let Err(issue) = apps.require_no_source_restore(app_id) {
                    status.source_restore.issues.push(issue.clone());
                    status.issues.push(issue);
                    status.can_list_versions = false;
                }
                json!(status)
            }
            NodeGitAction::Versions(limit) => json!(Box::pin(git.versions(root, limit)).await?),
            NodeGitAction::RestorePreview(input) => {
                Box::pin(git.restore_preview(root, &scope, input)).await?
            }
            NodeGitAction::RestoreApply(input) => {
                Box::pin(git.restore_apply(root, &scope, input)).await?
            }
            NodeGitAction::RestoreStatus => git.restore_status(root, &scope)?,
            NodeGitAction::RestoreRecover(input) => {
                Box::pin(git.restore_recover(root, &scope, input)).await?
            }
        };
        finalize_node_git_result(&lease, result)
    }
    pub async fn node_development_handoff(
        &self,
        apps: &NodeApps,
        app_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let lease = self.node_development_lease(apps, app_id, true).await?;
        let root = &lease.source.source_root;
        crate::node_development_git::require_source_restore_clear(root)?;
        let text = node_handoff_text(
            root,
            &lease._source_lease.control_path(),
            lease.source.runtime_is_source,
        );
        Ok(json!({"text":text,"projectRoot":root,"instanceId":lease.instance_id}))
    }
    pub async fn node_development_build(
        &self,
        apps: Arc<NodeApps>,
        app_id: String,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        if self.shared.closing.load(Ordering::Acquire) || state.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        state.node_install.require_idle()?;
        Ok(json!(
            state
                .node_development
                .build(apps, app_id, state.stop.subscribe())
                .await?
        ))
    }
    pub fn node_development_cancel(&self, task_id: &str) -> std::result::Result<Value, ApiError> {
        Ok(json!(self.current().node_development.cancel(task_id)?))
    }
}

fn node_handoff_text(
    root: &std::path::Path,
    control: &std::path::Path,
    runtime_is_source: bool,
) -> String {
    let delivery = if runtime_is_source {
        "这个应用直接运行此文件夹的代码。请先让用户关闭应用，再按需求修改；修改后提醒用户重新进入项目。修改可能使应用暂时无法启动。已登记 local-app.json 的应用身份、配置和数据契约仍受平台校验；不要直接改变这些声明或绕过登记检查。"
    } else {
        "AI 修改的是源码目录，当前应用运行独立安装副本。修改并测试完成后，提醒用户关闭应用，在应用详情的项目文件夹区点击“应用修改”，按提示核对并确认；修改源码不会自动影响当前运行程序。"
    };
    format!("请继续开发这个 Node 应用。\n源码仓库：{}\n\n以仓库 local-app.json 的 development 声明为准。一个应用只使用这个 Git 仓库。只修改指定源码目录，不修改平台登记、私有配置或密钥；不读取或输出真实 .env 和密钥。任何会删除或清空应用数据、数据库、上传文件的操作，先说明影响并取得用户明确同意。修改前先用 Git 保存当前版本，保留已有未提交改动；用户要求回退时，用 Git 恢复到之前的版本。完成后列出修改文件。\n{}\n\n排查问题先读取运行日志：\n本次运行：{}\n上一次运行：{}\n日志按平台持有的密钥值脱敏；不要读取私有配置或密钥文件。stdout 只能输出平台控制协议消息；普通日志一律写 stderr（使用 console.error，或把 console.log 重定向到 stderr），否则平台会判定协议错误并终止应用。\nGit 只保存源码，不能代替数据库或密钥备份。请先阅读仓库中的 AGENTS.md、README 和开发文档，再按用户需求开发。", root.display(), delivery, control.join("logs/current.log").display(), control.join("logs/previous.log").display())
}

#[cfg(test)]
mod handoff_tests {
    use super::node_handoff_text;
    use std::path::Path;

    #[test]
    fn node_handoff_uses_real_control_log_paths_and_external_git_workflow() {
        for direct in [true, false] {
            let text = node_handoff_text(
                Path::new("/disposable/author"),
                Path::new("/disposable/private/node-apps/instance"),
                direct,
            );
            for expected in [
                "/disposable/author",
                "/disposable/private/node-apps/instance/logs/current.log",
                "/disposable/private/node-apps/instance/logs/previous.log",
                "修改前先用 Git 保存当前版本",
                "用户要求回退时，用 Git 恢复到之前的版本",
                "stdout 只能输出平台控制协议消息",
                "console.error",
                "协议错误并终止应用",
            ] {
                assert!(text.contains(expected), "Missing instruction: {expected}");
            }
            assert!(!text.contains("Codex"));
            assert!(!text.contains("构建并更新"));
            assert!(!text.contains("由平台准备新版本"));
            if direct {
                assert!(text.contains("重新进入项目"));
                assert!(text.contains("不要直接改变这些声明"));
                assert!(!text.contains("点击“应用修改”"));
            } else {
                assert!(text.contains("点击“应用修改”"));
            }
        }
    }
}

#[cfg(test)]
#[path = "service_node_git_owner_tests.rs"]
mod owned_tests;
