use super::*;
use crate::node_apps::NodeApps;

impl RuntimeHandle {
    pub async fn node_app_handoff(
        &self,
        apps: &NodeApps,
        app_id: &str,
    ) -> std::result::Result<Value, ApiError> {
        let state = self.current();
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        state.store.check_workspace_available()?;
        let _admission = apps.admit_operation(app_id, "准备项目说明")?;
        let _workspace = state
            .store
            .reserve_transient_operation(&format!("node-handoff:{app_id}"), "node-handoff")?;
        let root = apps.project_source(app_id).await?;
        let detail = apps.status(app_id).await?;
        let text = node_handoff_text(
            &root,
            &state.paths.data_root().join("node-apps").join(app_id),
            detail["name"].as_str(),
        );
        Ok(json!({"text":text,"projectRoot":root,"instanceId":state.info.instance_id}))
    }
}

fn node_handoff_text(
    root: &std::path::Path,
    control: &std::path::Path,
    app_name: Option<&str>,
) -> String {
    use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE, SECRETS, STDOUT};
    let delivery = "这个应用直接运行此文件夹的代码。修改前提醒我先关闭应用窗口；改完后我重新进入项目即可生效。如果改完打不开，请先读日志排查。";
    let name = app_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!("应用：{name}\n"))
        .unwrap_or_default();
    format!(
        "请先阅读并理解这个 Node 应用，暂时不要修改任何文件。\n\n{name}项目文件夹：{root}\n\n请阅读项目里的 local-app.json、AGENTS.md、README 和开发文档，再看一下最近的运行日志（如果有）：\n- 本次运行：{current}\n- 上一次运行：{previous}\n\n读完后用通俗易懂的中文告诉我：\n1. 这个应用是做什么的，主要有哪些功能\n2. 代码大致怎么组织：界面、后端、数据存在哪里\n3. 你注意到的问题或风险，例如日志里的报错、过旧的依赖\n然后等我提出具体需求，再开始修改。\n\n—— 之后修改时请遵守 ——\n- 只修改上面的项目文件夹；不修改平台登记、私有配置和密钥文件。{SECRETS}配置项由平台表单保存并在启动时注入，不要让我另建 .env。日志已按平台持有的密钥值脱敏。\n- {GIT_SAVE}\n- {DATA_CONSENT}\n- {STDOUT}\n- 可以修改 local-app.json（包括 app.id、版本号和配置项），重启后生效；修改后请确认清单有效、入口文件存在。不修改平台控制文件绕过检查。\n- {delivery}\n- 完成后列出修改了哪些文件。",
        root = root.display(),
        current = control.join("logs").join("current.log").display(),
        previous = control.join("logs").join("previous.log").display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handoff_describes_direct_run_and_mutable_manifest() {
        let text = node_handoff_text(
            std::path::Path::new("/disposable/source"),
            std::path::Path::new("/disposable/control"),
            Some("Disposable"),
        );
        for expected in [
            "/disposable/source",
            "app.id",
            "重启后生效",
            "关闭应用窗口",
            crate::ai_prompt_rules::GIT_SAVE,
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        for name in ["current.log", "previous.log"] {
            let path = std::path::Path::new("/disposable/control")
                .join("logs")
                .join(name);
            assert!(text.contains(path.to_str().unwrap()), "{}", path.display());
        }
        for retired in ["安装副本", "应用修改", "不要改动 local-app.json"] {
            assert!(!text.contains(retired), "{retired}");
        }
    }
}
