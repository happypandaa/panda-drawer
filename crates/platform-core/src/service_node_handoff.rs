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
        let text = node_handoff_text(
            &root,
            &state.paths.data_root().join("node-apps").join(app_id),
            detail["name"].as_str(),
            skill_path.as_deref(),
        );
        Ok(
            json!({"text":text,"projectRoot":root,"skillPath":skill_path,"instanceId":state.info.instance_id}),
        )
    }
}

fn node_handoff_text(
    root: &std::path::Path,
    control: &std::path::Path,
    app_name: Option<&str>,
    skill_path: Option<&std::path::Path>,
) -> String {
    use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE, SECRETS, STDOUT};
    let skill = skill_path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "当前平台缺少安装资源，暂时无法提供".into());
    let name = app_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!("应用：{name}\n"))
        .unwrap_or_default();
    format!(
        "请先阅读并理解这个应用，暂不修改文件。\n\n{name}项目文件夹：{root}\n平台说明：{skill}\n日志目录：{logs}（current.log 本次、previous.log 上次）\n\n先读平台说明及 references、panda.json、README 与日志；说明不可读时先告诉我。用通俗中文说明应用功能、代码和数据组织及发现的问题，然后等我提出具体需求；先说明修改计划，等我同意再动手。\n\n修改规则：\n- 只修改项目文件夹，以平台说明为准。{SECRETS}\n- {GIT_SAVE}\n- {DATA_CONSENT}\n- 修改前提醒我关闭应用窗口；完成后说明改动、验证及后续操作。\n- {STDOUT}",
        root = root.display(),
        logs = control.join("logs").display(),
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
            Some(std::path::Path::new(
                "/disposable/install/skills/local-projects-app/SKILL.md",
            )),
        );
        assert!(text.starts_with("请先阅读并理解"));
        for expected in [
            "应用：Disposable",
            "/disposable/source",
            "/disposable/install/skills/local-projects-app/SKILL.md",
            "panda.json",
            "等我提出具体需求",
            "以平台说明为准",
            "关闭应用窗口",
            crate::ai_prompt_rules::GIT_SAVE,
            crate::ai_prompt_rules::DATA_CONSENT,
            crate::ai_prompt_rules::SECRETS,
            crate::ai_prompt_rules::STDOUT,
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        assert!(text.contains("/disposable/control/logs"));
        assert!(text.contains("current.log"));
        assert!(text.contains("previous.log"));
        assert!(text.chars().count() <= 800, "{}", text.chars().count());
        for retired in [
            "安装副本",
            "应用修改",
            "不要改动 panda.json",
            "实例 ID",
            "继续开发",
            "普通话",
            "重新打开即可生效",
        ] {
            assert!(!text.contains(retired), "{retired}");
        }
    }
}
