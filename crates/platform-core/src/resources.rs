//! Installation resources: platform SDK and author template sources.
use crate::{
    panda_manifest::{AppDecl, PandaManifest},
    projects::check_no_symlinks,
    ApiError,
};
use serde_json::{json, Value};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_RESOURCE: &str = "skills/local-projects-app/SKILL.md";
const WINDOW_CONSTRAINTS_RESOURCE: &str =
    "skills/local-projects-app/references/app-window-runtime.md";

/// The pinned application execution engine is independent of development Node.
pub fn runtime_node_path(resource_root: &Path) -> PathBuf {
    crate::node_tool_layout::node(&resource_root.join("node-runtime"))
}

#[derive(Clone)]
pub struct ProjectResources {
    root: PathBuf,
    pub cli_path: PathBuf,
    installation_scope: Option<std::sync::Arc<crate::installation_scope::InstallationScope>>,
}
impl ProjectResources {
    pub fn new(root: PathBuf, cli_path: PathBuf) -> Result<Self, ApiError> {
        check_no_symlinks(&root, false)?;
        for relative in [
            "schemas/panda.schema.json",
            "sdk/sdk.js",
            "sdk/panda.d.ts",
            "template/template.json",
            "template/app/package.json",
            "template/app/package-lock.json",
            "template/app/README.md",
            "template/app/src/panda.d.ts",
            SKILL_RESOURCE,
            WINDOW_CONSTRAINTS_RESOURCE,
        ] {
            let file = root.join(relative);
            check_no_symlinks(&file, false)?;
            if !file.is_file() {
                return Err(ApiError::new(
                    "INSTALLATION_RESOURCES_MISSING",
                    format!("Missing installation resource: {relative}"),
                ));
            }
        }
        if !cli_path.is_absolute() || !cli_path.is_file() {
            return Err(ApiError::new(
                "INSTALLATION_RESOURCES_MISSING",
                "The bundled CLI is missing",
            ));
        }
        Ok(Self {
            root,
            cli_path,
            installation_scope: None,
        })
    }
    pub(crate) fn for_desktop(mut self) -> Self {
        self.installation_scope =
            crate::installation_scope::InstallationScope::for_desktop(&self.root, &self.cli_path);
        self
    }
    pub(crate) fn installation_scope(
        &self,
    ) -> Option<std::sync::Arc<crate::installation_scope::InstallationScope>> {
        self.installation_scope.clone()
    }
    /// Development tools are optional for running published apps. A missing or
    /// damaged bundle is reported by the build environment, not startup.
    pub fn manifest_schema_path(&self) -> PathBuf {
        self.root.join("schemas/panda.schema.json")
    }
    pub fn sdk_script_path(&self) -> PathBuf {
        self.root.join("sdk/sdk.js")
    }
    pub fn sdk_types_path(&self) -> PathBuf {
        self.root.join("sdk/panda.d.ts")
    }
    pub fn sdk_script(&self) -> Result<Vec<u8>, ApiError> {
        crate::independent_file_io::read_file(&self.sdk_script_path(), 1024 * 1024).map_err(
            |issue| {
                ApiError::new("INSTALLATION_RESOURCES_MISSING", "网页 SDK 安装资源不可用")
                    .detail("cause", issue.message)
            },
        )
    }
    pub fn development_node_root(&self) -> PathBuf {
        self.root.join("development-node")
    }
    /// Git is an optional development tool, independent of app execution.
    pub fn development_git_root(&self) -> PathBuf {
        self.root.join("development-git")
    }
    pub fn install(&self, staging: &Path, app: &AppDecl) -> Result<PandaManifest, ApiError> {
        crate::web_project_template::install(&self.root.join("template"), staging, app)
    }

    /// The installed platform skill: the single detailed source of platform
    /// rules that every copied AI prompt points to.
    pub fn skill_path(&self) -> PathBuf {
        self.root.join(SKILL_RESOURCE)
    }

    pub fn handoff(&self, root: &Path, id: &str, handoff_dir: &Path) -> Value {
        let manifest = PandaManifest::load(root).ok();
        let skill = self.skill_path();
        let existing = |path: PathBuf| {
            (check_no_symlinks(&path, false).is_ok() && path.is_file()).then_some(path)
        };
        let constraints = existing(self.root.join(WINDOW_CONSTRAINTS_RESOURCE));
        let schema = existing(self.manifest_schema_path());
        let sdk_types = existing(self.sdk_types_path());
        let generated = existing(handoff_dir.join("handoff.md"));
        let name = manifest
            .as_ref()
            .map(|manifest| manifest.app.name.trim())
            .filter(|name| !name.is_empty())
            .map(|name| format!("应用：{name}\n"))
            .unwrap_or_default();
        use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE, SECRETS};
        let text = format!(
            "请先阅读并理解这个网页应用，暂时不要修改任何文件。\n\n{name}项目文件夹：{root}\n平台说明：{skill}\n\n先读平台说明及其 references，再读项目的 panda.json、README 和开发文档；如有 AGENTS.md 或 agent.instructions 指定文件，也一并阅读；读不到说明时先告诉我。用通俗的中文说明应用做什么、代码和数据怎么组织、发现了哪些问题，然后等我提出具体需求。收到需求后，先说明修改计划，等我同意再动手。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{SECRETS}\n- {GIT_SAVE}\n- {DATA_CONSENT}\n- 改代码或构建前提醒我关闭应用窗口，按 panda.json 完成必要构建；改完说明后续操作和验证结果。\n",
            root = root.display(),
            skill = skill.display(),
        );
        json!({"projectId":id,"projectRoot":root,"generatedHandoffPath":generated,
            "readmePath":manifest.as_ref().and_then(|manifest|manifest.documentation.as_ref()).and_then(|docs|docs.readme.as_ref()).map(|path|root.join(path)),
            "skillPath":skill,"skillSource":"installation","runtimeConstraintsPath":constraints,"schemaPath":schema,"sdkTypesPath":sdk_types,"sdkScriptUrl":"/_platform/sdk.js","cliPath":self.cli_path,"text":text})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimePaths;
    fn installation(root: &Path) -> PathBuf {
        for relative in [
            "schemas",
            "sdk",
            "template/app/src",
            "skills/local-projects-app/references",
        ] {
            fs::create_dir_all(root.join(relative)).unwrap();
        }
        for relative in [
            "sdk/sdk.js",
            "sdk/panda.d.ts",
            "template/app/src/panda.d.ts",
        ] {
            fs::write(root.join(relative), "globalThis.panda = {};").unwrap();
        }
        for relative in [
            "template/template.json",
            "template/app/package.json",
            "template/app/package-lock.json",
        ] {
            fs::write(root.join(relative), "{}").unwrap();
        }
        fs::write(root.join("schemas/panda.schema.json"), "{}").unwrap();
        fs::write(root.join("sdk/README.md"), "Use platform.db.execute").unwrap();
        fs::write(root.join("skills/local-projects-app/SKILL.md"), "---\nname: local-projects-app\ndescription: Take over a local project.\n---\nRead the project README first.\n").unwrap();
        fs::write(
            root.join(WINDOW_CONSTRAINTS_RESOURCE),
            "Current installed single-window constraints. Never overwrite project-owned files.",
        )
        .unwrap();
        fs::write(root.join("template/app/src/App.tsx"), "template one").unwrap();
        fs::write(
            root.join("template/app/README.md"),
            "Template source instructions",
        )
        .unwrap();
        let cli = root.join("CLI with ' quote");
        fs::write(&cli, "fixture").unwrap();
        cli
    }
    #[test]
    fn handoff_uses_current_manifest_private_notes_and_actual_installation_paths_without_writes() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let resources_root = paths.data_root().join("installation");
        let cli = installation(&resources_root);
        let resources = ProjectResources::new(resources_root.clone(), cli).unwrap();
        let project = paths.workspace_root().join("disposable author's project");
        fs::create_dir_all(&project).unwrap();
        let manifest = br#"{"manifestVersion":1,"runtime":"web","app":{"name":"Disposable"},"web":{"root":".","entry":"index.html"},"development":{"build":{"kind":"npm","script":"bundle"}}}"#;
        fs::write(project.join("panda.json"), manifest).unwrap();
        fs::write(project.join("index.html"), "disposable").unwrap();
        let private = paths.data_root().join("web-apps/prj_current/handoff");
        fs::create_dir_all(&private).unwrap();
        fs::write(private.join("handoff.md"), "private control note").unwrap();
        let handoff = resources.handoff(&project, "prj_current", &private);
        let text = handoff["text"].as_str().unwrap();
        let skill = resources_root.join("skills/local-projects-app/SKILL.md");
        assert!(text.starts_with("请先阅读并理解"));
        for expected in [
            "应用：Disposable",
            "panda.json",
            "如有 AGENTS.md 或 agent.instructions 指定文件，也一并阅读",
            "关闭应用窗口",
            "等我提出具体需求",
            "以平台说明为准",
            crate::ai_prompt_rules::GIT_SAVE,
            crate::ai_prompt_rules::DATA_CONSENT,
            crate::ai_prompt_rules::SECRETS,
            skill.to_str().unwrap(),
            project.to_str().unwrap(),
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        assert_eq!(handoff["skillPath"], json!(skill));
        assert!(text.chars().count() <= 800, "{}", text.chars().count());
        for retired in [
            "project.json",
            "local-app.json",
            ".platform",
            "发布目录",
            "版本面板",
            "private control note",
            "继续开发",
            "普通话",
            "重新打开即可生效",
            "项目 ID：",
            "平台 CLI：",
        ] {
            assert!(!text.contains(retired), "{retired}");
        }
        assert_eq!(
            handoff["generatedHandoffPath"],
            json!(private.join("handoff.md"))
        );
        assert_eq!(
            handoff["schemaPath"],
            json!(resources_root.join("schemas/panda.schema.json"))
        );
        assert_eq!(fs::read(project.join("panda.json")).unwrap(), manifest);
        assert_eq!(fs::read_dir(&project).unwrap().count(), 2);
    }
    #[test]
    fn unavailable_reference_paths_are_omitted_without_reading_project_or_private_links() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let resources_root = paths.data_root().join("installation");
        let cli = installation(&resources_root);
        let resources = ProjectResources::new(resources_root.clone(), cli).unwrap();
        fs::remove_file(resources_root.join(WINDOW_CONSTRAINTS_RESOURCE)).unwrap();
        fs::remove_file(resources_root.join("schemas/panda.schema.json")).unwrap();
        let project = paths.workspace_root().join("disposable");
        fs::create_dir_all(&project).unwrap();
        let private = paths.data_root().join("handoff");
        fs::create_dir_all(&private).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/must-not-read", private.join("handoff.md")).unwrap();
        }
        let handoff = resources.handoff(&project, "prj_current", &private);
        assert!(handoff["runtimeConstraintsPath"].is_null());
        assert!(handoff["schemaPath"].is_null());
        assert!(handoff["generatedHandoffPath"].is_null());
        assert!(!project.join(".platform").exists());
    }
}
