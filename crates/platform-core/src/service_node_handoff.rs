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
        let skill_path = state
            .resources
            .as_ref()
            .map(|resources| resources.skill_path());
        let cli_path = state
            .resources
            .as_ref()
            .map(|resources| resources.cli_path.clone());
        let instructions = crate::panda_manifest::PandaManifest::load(&root)
            .ok()
            .and_then(|manifest| manifest.agent)
            .map(|agent| agent.instructions);
        let text = node_handoff_text(
            &root,
            &state.paths.data_root().join("node-apps").join(app_id),
            detail["name"].as_str(),
            skill_path.as_deref(),
            cli_path.as_deref(),
            instructions.as_deref(),
        );
        Ok(
            json!({"text":text,"projectRoot":root,"skillPath":skill_path,"isAgent":instructions.is_some(),"instanceId":state.info.instance_id}),
        )
    }
}

fn node_handoff_text(
    root: &std::path::Path,
    control: &std::path::Path,
    app_name: Option<&str>,
    skill_path: Option<&std::path::Path>,
    cli_path: Option<&std::path::Path>,
    agent_instructions: Option<&str>,
) -> String {
    use crate::ai_prompt_rules::{render, TEMPLATE_MODIFY_AGENT_NODE, TEMPLATE_MODIFY_NODE};
    let unavailable = || "当前平台缺少安装资源，暂时无法提供".to_string();
    let skill = skill_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(unavailable);
    let cli = cli_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(unavailable);
    let name = app_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!("应用：{name}\n"))
        .unwrap_or_default();
    let root = root.display().to_string();
    let logs = control.join("logs").display().to_string();
    let mut values = vec![
        ("nameLine", name.as_str()),
        ("projectRoot", root.as_str()),
        ("skillPath", skill.as_str()),
        ("cliPath", cli.as_str()),
        ("logsPath", logs.as_str()),
    ];
    match agent_instructions {
        Some(instructions) => {
            values.push(("instructions", instructions));
            render(TEMPLATE_MODIFY_AGENT_NODE, &values)
        }
        None => render(TEMPLATE_MODIFY_NODE, &values),
    }
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
            Some(std::path::Path::new(
                "/disposable/install/skills/local-projects-app/SKILL.md",
            )),
            Some(std::path::Path::new("/disposable/install/bin/panda-cli")),
            None,
        );
        assert!(text.starts_with("请先读懂这个应用"));
        for expected in [
            "应用：Disposable",
            "/disposable/source",
            "/disposable/install/skills/local-projects-app/SKILL.md",
            "/disposable/install/bin/panda-cli",
            "panda.json",
            "想改哪里",
            "以平台说明为准",
            "重新加载",
            crate::ai_prompt_rules::GIT_SAVE,
            crate::ai_prompt_rules::DATA_CONSENT,
            crate::ai_prompt_rules::SECRETS,
            crate::ai_prompt_rules::STDOUT,
            crate::ai_prompt_rules::CHANGE_SIZE,
            crate::ai_prompt_rules::TOOLS,
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        assert!(!text.contains('{'), "{text}");
        let logs = std::path::Path::new("/disposable/control").join("logs");
        assert!(text.contains(logs.to_str().unwrap()));
        assert!(text.contains("current.log"));
        assert!(text.contains("previous.log"));
        assert!(text.chars().count() <= 820, "{}", text.chars().count());
        for retired in [
            "安装副本",
            "应用修改",
            "不要改动 panda.json",
            "实例 ID",
            "继续开发",
            "普通话",
            "重新打开即可生效",
            "关闭应用窗口",
            "这次是要修改应用本身",
        ] {
            assert!(!text.contains(retired), "{retired}");
        }
    }

    #[test]
    fn agent_node_apps_name_their_rules_file() {
        let text = node_handoff_text(
            std::path::Path::new("/disposable/source"),
            std::path::Path::new("/disposable/control"),
            Some("Disposable"),
            None,
            None,
            Some("AGENTS.md"),
        );
        assert!(text.contains("这次是要修改应用本身"));
        assert!(text.contains("AGENTS.md"));
        assert!(text.contains(crate::ai_prompt_rules::SCHEMA));
        assert!(text.contains("当前平台缺少安装资源，暂时无法提供"));
        assert!(!text.contains('{'), "{text}");
    }
}
