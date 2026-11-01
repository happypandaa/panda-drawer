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
            "skills/local-projects-app/SKILL.md",
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

    pub fn handoff(&self, root: &Path, id: &str, handoff_dir: &Path) -> Value {
        let manifest = PandaManifest::load(root).ok();
        let skill = self.root.join("skills/local-projects-app/SKILL.md");
        let constraints = self.root.join(WINDOW_CONSTRAINTS_RESOURCE);
        let constraints = (check_no_symlinks(&constraints, false).is_ok() && constraints.is_file())
            .then_some(constraints);
        let schema = self.manifest_schema_path();
        let schema =
            (check_no_symlinks(&schema, false).is_ok() && schema.is_file()).then_some(schema);
        let sdk_types = self.sdk_types_path();
        let sdk_types = (check_no_symlinks(&sdk_types, false).is_ok() && sdk_types.is_file())
            .then_some(sdk_types);
        let generated = handoff_dir.join("handoff.md");
        let generated = (check_no_symlinks(&generated, false).is_ok() && generated.is_file())
            .then_some(generated);
        let declared_docs = manifest
            .as_ref()
            .and_then(|manifest| manifest.documentation.as_ref())
            .map(|docs| {
                [
                    docs.readme.as_ref(),
                    docs.development.as_ref(),
                    docs.database.as_ref(),
                ]
                .into_iter()
                .flatten()
                .map(|path| root.join(path).display().to_string())
                .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let docs_line = if declared_docs.is_empty() {
            "尚未声明项目文档；理解代码后请补写用户说明和开发说明。".into()
        } else {
            format!("清单声明的项目文档：{}", declared_docs.join("、"))
        };
        let build_line = manifest.as_ref().and_then(|manifest| manifest.development.as_ref()).map(|development| {
            let crate::panda_manifest::BuildDecl::Npm { script } = &development.build;
            format!("构建目录是项目内的 {}；自行运行 npm run {}，或请用户点击平台的“构建”。平台会先 npm ci，再执行这个脚本，不添加输出目录参数。构建失败不会回滚项目文件。", development.source_dir, script)
        }).unwrap_or_else(|| "没有声明平台构建；直接修改现有网页文件，或由 AI 按项目自己的工具构建。".into());
        let cli = shell_quote(&self.cli_path.to_string_lossy());
        let project_id = shell_quote(id);
        use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE};
        let text = format!(
            "请先阅读并理解这个网页应用，暂时不要修改任何文件。\n\n项目 ID：{id}\n项目文件夹：{}\n平台 CLI：{}\n\n请阅读项目根的 panda.json、现有 AGENTS.md 和 README（如果有），以及下面的实际文档：\n- 接管 skill：{}\n- 窗口约束：{}\n- JSON Schema：{}\n- SDK 类型：{}\n- {docs_line}\n{}\n读完后用通俗易懂的中文告诉我：应用做什么，界面、代码和数据怎样组织，注意到哪些问题或风险。然后等我提出具体需求，再开始修改。\n\n—— 之后修改时请遵守 ——\n- 项目文件夹就是正在运行的那一份。只修改这里的代码和项目文档，不修改平台源码、登记、配置值或秘密。可以修改 panda.json 的名称、版本、图标、入口及声明；关闭再打开生效，runtime 改变须删除后重新添加。\n- 修改代码或构建前提醒我先关闭应用窗口。{build_line} 改好后关闭再打开即使用当前文件，不需要平台另存副本。\n- 网页通过普通脚本 <script src=\"/_platform/sdk.js\"></script> 使用 window.panda，API 1 固定访问 /_platform/api/v1/，不要从 URL 解析实例 ID，不内置 SDK 包。只有声明 platform.sdkApiVersion:1 才能调用 SQL／附件；未启用时接口明确拒绝且不创建数据库。\n- 用户配置从 /_platform/config.json 或 window.panda.config() 读取，返回字符串／整数／布尔值；配置不依赖 SDK 开关，选填未填项省略。网页配置公开，不得保存 secret。保存后重新打开或刷新生效，不需构建。\n- {GIT_SAVE}\n- 文件数据在项目 data/，SDK 数据库固定 data/main.sqlite，附件在 data/attachments；未启用 SDK 时平台不创建数据库或附件。数据库修改通过平台 CLI 的 db 命令和事务完成。{DATA_CONSENT}\n- 浏览器数据（localStorage、IndexedDB、Cookie）在平台窗口存储中，删除应用时总会清除。用户本地文件夹及 data/ 删除后保留；平台新建或 Git 下载的项目整根删除。\n- 请遵守当前安装的窗口约束。不向前端、日志、说明或截图写入真实密钥，不自行借用宿主环境或其他应用的配置。\n- 完成后列出改动、数据是否变化、构建结果与实际验证，并更新项目说明。\n\n常用命令：\n```sh\n{cli} project info --project {project_id} --json\n{cli} db schema --project {project_id} --json\n{cli} db execute --project {project_id} --input request.json --json\n{cli} project build --project {project_id} --json\n{cli} task wait --task TASK_ID --json\n```\n",
            root.display(), self.cli_path.display(), skill.display(),
            constraints.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "安装资源缺失，请先修复平台资源；不引用不存在的路径。".into()),
            schema.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "安装资源缺失，请先修复平台资源；不引用不存在的路径。".into()),
            sdk_types.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "安装资源缺失，请先修复平台资源；不引用不存在的路径。".into()),
            generated.as_ref().map(|path| format!("平台接管记录：{}\n", path.display())).unwrap_or_default(),
        );
        json!({"projectId":id,"projectRoot":root,"generatedHandoffPath":generated,
            "readmePath":manifest.as_ref().and_then(|manifest|manifest.documentation.as_ref()).and_then(|docs|docs.readme.as_ref()).map(|path|root.join(path)),
            "skillPath":skill,"skillSource":"installation","runtimeConstraintsPath":constraints,"schemaPath":schema,"sdkTypesPath":sdk_types,"sdkScriptUrl":"/_platform/sdk.js","cliPath":self.cli_path,"text":text})
    }
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
        for expected in [
            "panda.json",
            "npm run bundle",
            "先关闭应用窗口",
            "data/main.sqlite",
            "浏览器数据",
            "Git",
            "db schema",
            "JSON Schema",
            "/_platform/sdk.js",
            "window.panda.config()",
            "SDK 类型",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        for retired in [
            "project.json",
            "local-app.json",
            ".platform",
            "发布目录",
            "版本面板",
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
