use super::*;
use crate::node_apps::{NodeApps, NodeDevelopmentSource};

pub enum NodeGitAction {
    Status,
    Versions(u32),
    Initialize(crate::project_git::GitInitInput),
    Save(crate::project_git::GitSaveInput),
    Branch(crate::project_git::GitCreateBranchInput),
}
pub struct NodeDevelopmentLease<'a> {
    _source_lease: crate::node_apps::NodeDevelopmentSourceLease<'a>,
    pub source: NodeDevelopmentSource,
    pub instance_id: String,
    _task: crate::projects::ProjectTaskLease,
    active: Active,
}
impl NodeDevelopmentLease<'_> {
    pub fn validate(&self) -> std::result::Result<(), ApiError> {
        if self.active.0.stopping.load(Ordering::Acquire) {
            return Err(ApiError::new("SERVICE_STOPPING", "平台正在退出。"));
        }
        self.active.0.store.check_workspace_available()?;
        self._source_lease.validate()?;
        self.source.validate()
    }
}
impl RuntimeHandle {
    pub async fn node_development_lease<'a>(
        &self,
        apps: &'a NodeApps,
        app_id: &str,
        editable: bool,
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
            "node-development",
        )?;
        let source_lease = apps.development_source_lease(app_id).await?;
        source_lease.validate()?;
        let source = source_lease.source().clone();
        if editable && source.runtime_is_source {
            return Err(ApiError::new(
                "NODE_DEVELOPMENT_RUNTIME_IS_SOURCE",
                "请先构建并更新应用，再开发源码。",
            ));
        }
        let lease = NodeDevelopmentLease {
            _source_lease: source_lease,
            source,
            instance_id: state.info.instance_id.clone(),
            _task: task,
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
        let editable = matches!(action, NodeGitAction::Branch(_));
        let lease = self.node_development_lease(apps, app_id, editable).await?;
        let git = crate::node_development_git::NodeGit::new(lease.active.0.git_environment.clone())
            .with_stop(lease.active.0.stop.subscribe());
        let root = &lease.source.source_root;
        let result = match action {
            NodeGitAction::Status => json!(git.status(root).await?),
            NodeGitAction::Versions(limit) => json!(git.versions(root, limit).await?),
            NodeGitAction::Initialize(input) => git.initialize(root, input).await?,
            NodeGitAction::Save(input) => git.save(root, input).await?,
            NodeGitAction::Branch(input) => git.create_branch(root, input).await?,
        };
        lease.validate()?;
        Ok(result)
    }
    pub async fn node_development_handoff(
        &self,
        apps: &NodeApps,
        app_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let lease = self.node_development_lease(apps, app_id, true).await?;
        let root = &lease.source.source_root;
        let text = format!("请继续开发这个 Node 应用。\n源码仓库：{}\n\n以仓库 local-app.json 的 development 声明为准。一个应用只使用这个 Git 仓库。不要修改已安装的运行副本、平台登记、真实数据库、附件、配置或密钥。\n在源码中修改并测试后，通过平台的“构建并更新”准备新版本，用户确认后才发布。Git 只保存源码，不能代替数据库或密钥备份。请先阅读仓库中的 AGENTS.md、README 和开发文档，再按用户需求开发。", root.display());
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
