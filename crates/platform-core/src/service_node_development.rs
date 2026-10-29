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
        // Read the display name before taking the development lease. A missing
        // name only drops one line from the prompt.
        let app_name = apps
            .status(app_id)
            .await
            .ok()
            .and_then(|detail| detail["name"].as_str().map(str::to_owned));
        let lease = self.node_development_lease(apps, app_id, true).await?;
        let root = &lease.source.source_root;
        crate::node_development_git::require_source_restore_clear(root)?;
        let text = node_handoff_text(
            root,
            &lease._source_lease.control_path(),
            lease.source.runtime_is_source,
            app_name.as_deref(),
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
    app_name: Option<&str>,
) -> String {
    use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE, SECRETS, STDOUT};
    let delivery = if runtime_is_source {
        "这个应用直接运行此文件夹的代码。修改前提醒我先关闭应用窗口；改完后我重新进入项目即可生效。如果改完打不开，请先读日志排查。"
    } else {
        "当前应用运行的是平台准备的安装副本，修改源码不会自动生效。改完后提醒我关闭应用，在应用详情的项目文件夹区点击“应用修改”，按提示核对并确认。"
    };
    let name = app_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!("应用：{name}\n"))
        .unwrap_or_default();
    format!(
        "请先阅读并理解这个 Node 应用，暂时不要修改任何文件。\n\n{name}项目文件夹：{root}\n\n请阅读项目里的 local-app.json、AGENTS.md、README 和开发文档，再看一下最近的运行日志（如果有）：\n- 本次运行：{current}\n- 上一次运行：{previous}\n\n读完后用通俗易懂的中文告诉我：\n1. 这个应用是做什么的，主要有哪些功能\n2. 代码大致怎么组织：界面、后端、数据存在哪里\n3. 你注意到的问题或风险，例如日志里的报错、过旧的依赖\n然后等我提出具体需求，再开始修改。\n\n—— 之后修改时请遵守 ——\n- 只修改上面的项目文件夹；不修改平台登记、私有配置和密钥文件。{SECRETS}配置项由平台表单保存并在启动时注入，不要让我另建 .env。日志已按平台持有的密钥值脱敏。\n- {GIT_SAVE}\n- {DATA_CONSENT}\n- {STDOUT}\n- 不要改动 local-app.json 里的应用身份、配置声明和数据声明，也不要绕过平台检查。\n- {delivery}\n- 完成后列出修改了哪些文件。",
        root = root.display(),
        current = control.join("logs/current.log").display(),
        previous = control.join("logs/previous.log").display(),
    )
}

#[cfg(test)]
mod handoff_tests {
    use super::node_handoff_text;
    use std::path::Path;

    #[test]
    fn node_handoff_asks_for_understanding_first_with_real_log_paths() {
        for direct in [true, false] {
            let text = node_handoff_text(
                Path::new("/disposable/author"),
                Path::new("/disposable/private/node-apps/instance"),
                direct,
                Some("Radio Bob"),
            );
            assert!(text.starts_with("请先阅读并理解这个 Node 应用，暂时不要修改任何文件。"));
            for expected in [
                "应用：Radio Bob",
                "/disposable/author",
                "/disposable/private/node-apps/instance/logs/current.log",
                "/disposable/private/node-apps/instance/logs/previous.log",
                "等我提出具体需求，再开始修改",
                crate::ai_prompt_rules::GIT_SAVE,
                crate::ai_prompt_rules::DATA_CONSENT,
                crate::ai_prompt_rules::SECRETS,
                crate::ai_prompt_rules::STDOUT,
                "修改前先用 Git 保存当前版本",
                "我要求回退时，用 Git 恢复到之前的版本",
                "console.error",
                "协议错误并终止应用",
            ] {
                assert!(text.contains(expected), "Missing instruction: {expected}");
            }
            for retired in [
                "继续开发",
                "Codex",
                "构建并更新",
                "由平台准备新版本",
                "普通话",
            ] {
                assert!(!text.contains(retired), "Retired wording: {retired}");
            }
            if direct {
                assert!(text.contains("重新进入项目"));
                assert!(!text.contains("点击“应用修改”"));
            } else {
                assert!(text.contains("点击“应用修改”"));
            }
        }
    }

    #[test]
    fn node_handoff_omits_a_missing_application_name() {
        let text = node_handoff_text(Path::new("/a"), Path::new("/c"), true, Some("  "));
        assert!(!text.contains("应用："));
        assert!(text.contains("项目文件夹：/a"));
        assert_eq!(
            text,
            node_handoff_text(Path::new("/a"), Path::new("/c"), true, None)
        );
    }
}

#[cfg(test)]
#[path = "service_node_git_owner_tests.rs"]
mod owned_tests;
