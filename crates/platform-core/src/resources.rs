//! Immutable installation resources are copied only when creating a project.
use crate::{projects::check_no_symlinks, ApiError, ProjectManifest};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

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
            "sdk/releases/local-projects-sdk-0.1.0.tgz",
            "template/template.json",
            "template/starter.json",
            "template/starter/index.html",
            "template/app/package.json",
            "template/app/package-lock.json",
            "template/app/README.md",
            "template/app/vendor/local-projects-sdk-0.1.0.tgz",
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
    pub fn development_node_root(&self) -> PathBuf {
        self.root.join("development-node")
    }
    /// Git is an optional development tool, independent of app execution.
    pub fn development_git_root(&self) -> PathBuf {
        self.root.join("development-git")
    }
    pub fn install(&self, staging: &Path, manifest: &ProjectManifest) -> Result<(), ApiError> {
        copy_tree(&self.root.join("template/app"), &staging.join("app"))?;
        crate::builds::install_starter(staging, manifest, &self.root.join("template"))?;
        copy_tree(
            &self.root.join("skills/local-projects-app"),
            &staging.join(".agents/skills/local-projects-app"),
        )?;
        let agent_entry = "# Project development instructions\n\nBefore changing this business application, read `.agents/skills/local-projects-app/SKILL.md`, then `README.md`, `project.json`, `docs/database.md` and `docs/platform.md`. All paths are relative to this project root. The README identifies this project and the platform CLI. Follow the user's request within this project; the skill explains data, SDK, build and verification rules.\n";
        fs::write(staging.join("AGENTS.md"), agent_entry)?;
        fs::write(staging.join("CLAUDE.md"), agent_entry)?;
        fs::write(staging.join("README.md"), self.readme(manifest))?;
        let template: Value =
            serde_json::from_slice(&fs::read(self.root.join("template/template.json"))?)?;
        let template_version = template["templateVersion"]
            .as_str()
            .ok_or_else(|| ApiError::new("INVALID_TEMPLATE", "Template version is missing"))?;
        let sdk_docs = fs::read_to_string(self.root.join("sdk/README.md"))?;
        fs::write(staging.join("docs/platform.md"), format!("# Platform integration\n\nCreation template: `{template_version}`. This project carries @local-projects/sdk 0.1.0 in app/vendor. API version: 1.\nUse the CLI commands and actual paths in ../README.md. Never cache the platform port, launch ticket, control token or browser cookie.\n\n## Pinned SDK 0.1.0 guide\n\nThe guide below ships unchanged with this SDK version. Any template version mentioned in it identifies the original SDK example; this project's creation template is the version stated above. Template and SDK versions are independent.\n\n{sdk_docs}\n"))?;
        fs::write(staging.join("docs/database.md"), "# Database contract\n\nDatabase: `data/main.sqlite`, independent SQLite database. Initially empty; no platform business tables.\n\n## Tables\n\nFor every table document its purpose, columns/types, nullability, primary/foreign keys, indexes, defaults, units, enums and timestamp convention. Keep this document synchronized with every schema change.\n\n## Attachments\n\nStore file logical paths, MIME types and lifecycle rules here. Files belong to `data/attachments/`, never the web build.\n\n## Optional template media sample\n\nThe media panel creates no schema when opening or building. Only its explicit Save action creates `media_samples`: `id TEXT PRIMARY KEY`, `attachment_path TEXT NOT NULL`, `mime_type TEXT NOT NULL`, `extension TEXT NOT NULL`, `size_bytes INTEGER NOT NULL`, `duration_ms INTEGER NULL` (measured milliseconds), `original_name TEXT NOT NULL`, `created_at TEXT NOT NULL` (ISO-8601 UTC). There are no additional indexes, defaults or foreign keys. IDs are UUIDs; attachment paths are project-relative logical references, never blob URLs. Deleting a row or clearing the preview does not remove its attachment. Define explicit retention/deletion and orphan recovery before adapting this optional example to business data. See app/BROWSER_TOOLS.md.\n\n## Changes and compatibility\n\nRecord every schema change, the applied SQL, data compatibility and validation. The platform enables foreign keys. Use CLI managed transactions; do not replace a registered database file or manipulate its WAL/SHM while running. Before changes inspect and save the current code using standard Git outside the platform. New backup creation and maintenance begin are retired; never request a ZIP baseline. Prefer compatible schema changes in managed transactions; candidate builds and necessary write pauses belong to the platform operation. Existing legacy maintenance must be inspected and ended explicitly only after its checks pass. The independent development environment still uses disposable test data.\n")?;
        Ok(())
    }
    fn readme(&self, manifest: &ProjectManifest) -> String {
        let cli = shell_quote(&self.cli_path.to_string_lossy());
        let id = &manifest.id;
        format!(
            r#"# {name}

{description}

## Project identity and purpose

- Project ID: `{id}`. Do not change it or the storage paths in `project.json`.
- Purpose and audience: describe the user's actual requirements here.
- This is a development project. Use disposable test data in this isolated development environment. Source and explicitly selected data versions use the supported Git workflow. New ZIP backups and maintenance entry are retired; existing archives and maintenance retain their compatibility and recovery protections.

## Start here (for an external AI)

First read the project-owned skill at `.agents/skills/local-projects-app/SKILL.md` and its `references/app-window-runtime.md`, then this README, `project.json`, `docs/database.md`, and `docs/platform.md`. Also read the latest installed platform constraints at the actual path returned by a fresh platform AI handoff; project-owned skills are preserved when the platform updates. The files can be read directly by any AI with file access; automatic discovery is not required. `AGENTS.md` and `CLAUDE.md` point to the same instructions. Implement ordinary usable webpage features: the user must be able to continue using the application when AI is offline.

This project starts with a ready-to-open welcome application, its matching React source and an empty independent database. Creating and opening it requires neither Node nor dependency downloads. The bundled template is recorded separately from user build tasks; after changing source, request a platform build to publish the new version. A failed first build preserves the initial application, but does not roll back any separately committed database changes.

For this static project-v1 type, the native platform runs the local service and owns SQLite and attachments. Node is needed only for builds (minimum 22.12, recommended 24 LTS, supported branches 22/24). The Mac internal bundle defaults to its separate managed Node/npm installation and preserves an existing explicit local choice; use the platform env doctor command to inspect the service-selected tools. An unavailable selected toolchain fails without silently switching sources. This selection does not change the fixed Node used to run registered applications. Do not start a separate backend or Vite development server. Open the published application from the platform in its single native application window. Opening it again only focuses the same window without reloading or losing drafts. Do not create popups, another WebView, an external browser window, or use window.open/target=_blank to extend the application. For a separately registered Node application, the platform starts its backend when opening its window and stops it when that application window closes; that rule does not turn this static project into a Node source slot.

Git is an optional development tool. The Mac internal bundle supplies a separate private Git; `env git doctor --json` reports the selected path/version and system SSH tool status. Tool readiness does not mean repository authentication is verified. `env git set --path /absolute/path/git` selects a local executable for this platform only; `env git clear` explicitly restores the bundled default. Broken selections do not silently fall back. These commands do not acquire repositories or provide commit/branch/restore operations, and published application execution does not require Git or SSH.

For an explicit public repository check, use `env git check --url https://github.com/owner/repository.git --json`; standard Gitee HTTPS repository addresses are also supported. This command anonymously reads only the remote HEAD within a 20-second execution budget, without reading account credentials, cloning/downloading a repository, pushing, or changing a project. The result is specific to that URL and check time; no returned HEAD does not prove an empty repository. It does not change the doctor's unverified authentication state and does not add SSH or private-repository authentication. Do not run a remote check as part of routine offline tool diagnosis.

The current Node configuration choice stores secrets in each application's private local slot under configuration/credentials/ (directory mode 0700, file mode 0600). These are plaintext files protected by the current user's file permissions, not an OS encrypted vault or macOS Keychain. The slot is separate from author source, shared packages and Git. Do not read/copy these secrets or touch older Keychain entries; the platform configuration flow manages them and injects only the target backend's environment. This does not add secret storage to this static project-v1 type.

Bundled CLI for this installation (rediscover after moving/reinstalling the app):

```sh
{cli} service status --json
{cli} env doctor --json
{cli} project info --project {id} --json
{cli} db schema --project {id} --json
{cli} db execute --project {id} --input request.json --json
{cli} db transaction --project {id} --input transaction.json --json
{cli} project build --project {id} --json
# Use the taskId from the response. Repeat wait if waitTimedOut is true.
{cli} task wait --task TASK_ID --json
{cli} project url --project {id} --json
```

Verify the result in the platform's native application window. The returned fresh `launchUrl` is only for necessary HTTP/browser diagnostics, not the standard application entry or a substitute for native-window acceptance. It is short lived; never save it or any credentials in source or documentation. After a platform restart, obtain a new URL if diagnostics are needed. `task wait` timing out does not cancel a build. A failed build keeps the previous webpage and saved data. Check the final task status and its error before claiming success; detailed build/npm output is not retained.

The platform no longer exposes task logs or platform Git init/save/save-data/branch-create commands, and it has no program rollback button. Use ordinary Git in the author repository for source versions. Platform builds, publication, installation and explicit updates remain available. If an older project-owned skill recommends a retired command, follow these current installed rules without overwriting the project documentation.

SQL input examples (save the JSON to the `--input` file):

```json
{{"sql":"SELECT ? AS example","params":["hello"]}}
```

```json
{{"statements":[{{"sql":"CREATE TABLE IF NOT EXISTS example (id INTEGER PRIMARY KEY, label TEXT NOT NULL)","params":[]}}]}}
```

## Files and dependencies

- `app/src/`: React + TypeScript source; `app/src/App.tsx` is the starting page.
- `app/public/`: build time static assets. Prefer the provided black and white reusable components; extend them as needed.
- `app/package.json`, `app/package-lock.json`: fixed dependencies. If intentionally changing dependencies, update and retain the lock file with npm before requesting a build.
- `app/vendor/local-projects-sdk-0.1.0.tgz`: project-owned fixed SDK. Import `platform` from `@local-projects/sdk`; consult `docs/platform.md`. Platform updates never silently replace this file or the project source.
- Build command: the `build` npm script must accept `--outDir ABSOLUTE_DIRECTORY`. The platform performs `npm ci` when dependencies change and builds into staging, validates inputs and output, then publishes atomically. Do not write `dist/` manually.
- `dist/`: current published webpage, initially copied from the bundled template. `.platform/`: platform-owned build and starter provenance. Do not edit either directory.
- `.agents/skills/local-projects-app/SKILL.md`: project-owned AI takeover instructions; keep it with project source and documentation. Platform updates do not silently replace it.
- `data/main.sqlite` and `data/attachments/`: persistent user data. Never delete these to fix a build, refresh a page, or clear caches.
- `project.json`: identity, metadata, paths and network/device declarations. No secrets or private API keys; first version supports local functionality and declared public interfaces.

## Metadata and storage

Use `project metadata get --project {id} --json` to read the current `metadataRevision`, then `project metadata set --project {id} --input metadata.json --json` with that value as `expectedRevision`. Change only name, description or icon; resolve a revision conflict by reading the current values again. Pass the current maintenance ID when applicable. Metadata updates do not require a build. Custom PNG icons live in persistent `meta/`, never in generated `dist/`; include them when explicitly exporting a complete project archive.

Use `storage info/scan/files/cleanup --project {id}` and `workspace storage` for platform accounting. `scan` and `cleanup` return tasks; wait for the final result. `files` and `cleanup` accept `--input` JSON according to CLI help. Measurements are logical file sizes with timestamps, completeness and stale indicators; a missing or unreadable category is unknown, not zero. Cleanup accepts only the platform's explicitly supported regenerable cache categories. Do not use it to remove databases, attachments, meta, backups, recycle-bin contents or retained operation evidence. Inspect the candidate and result before reporting reclaimed bytes.

## Browser capabilities and media

Read `app/BROWSER_TOOLS.md` and the independent `app/src/lib/browser-tools.js` helper before implementing recording, camera or file controls. It is copied only into new projects; SDK 0.1.0 remains fixed. Default runtimeRequirements are empty and no device is requested on load. Declare specific camera/microphone purposes and precise public HTTPS origins separately under network.connect/images/media. No credentials, remote script permission, localhost/LAN proxy or secret storage is provided. Document data sent and offline behavior. A declaration change needs explicit reload after handling drafts; declarations do not instantly revoke arbitrary old-page behavior.

The helper caps foreground capture at 60 seconds and media at min(10 MiB, context.limits.fileUploadMaxBytes), uses actual MIME detection, and releases devices on hidden/leave/maintenance/disconnect/reload states. These are internal test settings, not final product limits. File picking is the fallback. Media saving first writes the attachment and then the business association; only both successful acknowledgements mean saved. Do not replay unknown outcomes. If the optional media demo is used, its table is created only by the explicit Save button; record the media_samples schema and attachment retention policy in docs/database.md. Preview blob URLs are not persistent paths. Real signed-app/device/browser validation must be reported separately from mocks and builds.

## User flows

Document entry points, forms, validation, empty/loading/error states and saved data behavior. Handle errors visibly; do not report a failed save as success. Writes are not automatically retried. Reopen or explicitly refresh when the SDK reports that a new document is required; never automatically discard an unsaved form.

## Schema and data changes

Use the CLI to inspect existing schema and execute parameterized SQL or managed transactions. Keep `docs/database.md` complete. Use standard Git to preserve code before editing; code commits are not business-data backups. Prefer compatible schema changes and let the platform coordinate active SQL and uploads. Do not replace an open SQLite database or its WAL/SHM files. Source/data version retrieval and project ZIP operations are not provided.

## Management operations

Project deletion permanently removes platform-managed project folders, Git and file data. A user-selected local folder, including a generated local starter, is preserved. Browser data is cleared for every deleted instance. Completed downloads and attachment copies in user-selected external locations are preserved. Workspace location changes use the platform's available public workflow; do not edit control roots or manually remove retained locations.


## Verification and handoff

- Feature and acceptance criteria: pending.
- Source/dependency/schema changes: pending.
- Build task ID and outcome: pending.
- Native single-window, repeat-open, close/reopen and persistence validation: pending.
- Supplemental HTTP/browser diagnostics, if needed, and their limits: pending.
- Known limitations and next steps: pending.

Resource limits exposed by this development runtime are internal test parameters, not finalized product defaults.
"#,
            name = manifest.name,
            description = manifest.description
        )
    }
    pub fn handoff(&self, root: &Path, id: &str) -> Value {
        let readme = root.join("README.md");
        let project_skill = root.join(".agents/skills/local-projects-app/SKILL.md");
        let local_skill =
            check_no_symlinks(&project_skill, false).is_ok() && project_skill.is_file();
        let skill = if local_skill {
            project_skill
        } else {
            self.root.join("skills/local-projects-app/SKILL.md")
        };
        // Project-owned skills remain pinned. Always supplement them with the
        // current installation's real reference, without changing project files.
        let constraints = self.root.join(WINDOW_CONSTRAINTS_RESOURCE);
        let constraints = if check_no_symlinks(&constraints, false).is_ok() && constraints.is_file()
        {
            Some(constraints)
        } else {
            None
        };
        let constraints_line = constraints
            .as_ref()
            .map(|path| {
                format!(
                    "应用窗口约束（当前安装随附，项目自带 skill 与它不一致时以它为准）：{}",
                    path.display()
                )
            })
            .unwrap_or_else(|| {
                "当前安装缺少可核对的应用窗口约束资源；请按下面“遵守窗口约束”一条执行，不要引用不存在的路径或改写旧项目文件。".into()
            });
        let generated = root.join(".platform/handoff.md");
        let generated = if check_no_symlinks(&generated, false).is_ok() && generated.is_file() {
            Some(generated)
        } else {
            None
        };
        let generated_line = generated
            .as_ref()
            .map(|path| {
                format!(
                    "   此项目还有平台管理的接管记录，也请阅读：{}。其中可能记录复制、导入或迁移后的项目身份；不要继续编辑保留的旧目录。\n",
                    path.display()
                )
            })
            .unwrap_or_default();
        let installation_skill = self.root.join("skills/local-projects-app/SKILL.md");
        let cli = shell_quote(&self.cli_path.to_string_lossy());
        let project_id = shell_quote(id);
        use crate::ai_prompt_rules::{DATA_CONSENT, GIT_SAVE, SECRETS};
        let text = format!(
            r#"请先阅读并理解这个网页应用，暂时不要修改任何文件。

项目 ID：{id}
项目文件夹：{project_root}
平台 CLI：{cli_path}

请按顺序阅读：
1. 接管 skill（{skill_source}）：{skill_path}
2. {constraints_line}
{generated_line}3. 项目清单：{manifest_path}，以及其中 docs 声明的 README、数据库说明和平台集成说明。默认位置是 {readme_path}、{database_docs}、{platform_docs}；若清单声明不同，请读取实际声明的文件。
skill 和文档里如果出现旧路径或已退役的入口，以这里的实际路径和当前 CLI 帮助为准；不要改写旧项目自带的 skill、SDK 和文档。

读完后用通俗易懂的中文告诉我：
1. 这个应用是做什么的，主要有哪些功能
2. 代码和数据大致怎么组织：源码、发布目录、数据库主要表、附件
3. 你注意到的问题或风险
然后等我提出具体需求，再开始修改。

—— 之后修改时请遵守 ——
- 确认当前工作目录就是上面的项目文件夹。
- 只修改业务源码和项目文档；不修改平台源码、平台管理的 .platform/ 和 SDK（@local-projects/sdk 0.1.0），不手工覆盖发布目录，不另起后端或 Vite 开发服务。{SECRETS}
- 数据库只通过平台 CLI 的 db 命令和事务修改，不直接写入或替换 SQLite 文件。
- {GIT_SAVE}
- {DATA_CONSENT}不要为了修复构建或刷新页面而清空数据、附件或未保存草稿。
- 遵守窗口约束：每个应用只有一个窗口，重复打开只聚焦已有窗口；不弹新窗口，不用 window.open、target=_blank 或外部浏览器，不用 alert、confirm、prompt。
- 平台不提供源码或数据版本取回。代码版本由外部 AI 使用标准 Git 保存和回退；当前接入说明见平台随附 skill（{installation_skill}）。
- 改完后用平台构建，检查任务的最终结果，并在应用窗口里验证。CLI 连不上时，保留已改的代码，告诉我还差哪一步，不要绕过权限或直接改数据库文件。
- 完成后告诉我改了什么、数据库是否有变化、构建结果、实际验证了什么，并同步更新项目说明。

常用命令：
```sh
{cli} project info --project {project_id} --json
{cli} db schema --project {project_id} --json
{cli} db execute --project {project_id} --input request.json --json
{cli} project build --project {project_id} --json
# 使用返回的 taskId；若 waitTimedOut 为 true，继续等待，不代表失败或取消。
{cli} task wait --task TASK_ID --json
```
"#,
            project_root = root.display(),
            cli_path = self.cli_path.display(),
            skill_source = if local_skill {
                "项目自带"
            } else {
                "本机安装随附，旧项目文件保持原样"
            },
            skill_path = skill.display(),
            installation_skill = installation_skill.display(),
            readme_path = readme.display(),
            manifest_path = root.join("project.json").display(),
            database_docs = root.join("docs/database.md").display(),
            platform_docs = root.join("docs/platform.md").display(),
        );
        json!({"projectId":id,"projectRoot":root,"generatedHandoffPath":generated,"readmePath":readme,"skillPath":skill,"skillSource":if local_skill {"project"} else {"installation"},"runtimeConstraintsPath":constraints,"cliPath":self.cli_path,"text":text})
    }
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), ApiError> {
    check_no_symlinks(source, false)?;
    fs::create_dir_all(target)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let kind = item.file_type()?;
        if kind.is_symlink() {
            return Err(ApiError::new(
                "INVALID_TEMPLATE",
                "Template cannot contain symlinks",
            ));
        }
        let name = item.file_name();
        if ["node_modules", "dist", ".git"]
            .iter()
            .any(|ignored| name == *ignored)
        {
            continue;
        }
        let destination = target.join(name);
        if kind.is_dir() {
            copy_tree(&item.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(item.path(), destination)?;
        } else {
            return Err(ApiError::new(
                "INVALID_TEMPLATE",
                "Template contains unsupported file type",
            ));
        }
    }
    Ok(())
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateProjectInput, ProjectStore, RuntimePaths};
    use std::sync::Arc;
    fn installation(root: &Path) -> PathBuf {
        for relative in [
            "sdk/releases",
            "template/app/vendor",
            "template/app/src",
            "skills/local-projects-app/references",
        ] {
            fs::create_dir_all(root.join(relative)).unwrap();
        }
        for relative in [
            "sdk/releases/local-projects-sdk-0.1.0.tgz",
            "template/app/vendor/local-projects-sdk-0.1.0.tgz",
        ] {
            fs::write(root.join(relative), "sdk version one").unwrap();
        }
        for relative in [
            "template/template.json",
            "template/app/package.json",
            "template/app/package-lock.json",
        ] {
            fs::write(root.join(relative), "{}").unwrap();
        }
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
        crate::builds::write_test_starter(&root.join("template"));
        let cli = root.join("CLI with ' quote");
        fs::write(&cli, "fixture").unwrap();
        cli
    }
    #[test]
    fn handoff_explains_the_platform_with_current_paths_and_safe_cli_examples() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("App's installation resources");
        let cli = installation(&root);
        let constraints = root.join(WINDOW_CONSTRAINTS_RESOURCE);
        let installation_skill = root.join("skills/local-projects-app/SKILL.md");
        let resources = ProjectResources::new(root, cli.clone()).unwrap();
        let project_root = paths.workspace_root().join("业务 项目's directory");
        let skill = project_root.join(".agents/skills/local-projects-app/SKILL.md");
        let generated = project_root.join(".platform/handoff.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        let readme = b"Keep this project-owned README; private-fixture-value";
        fs::write(project_root.join("README.md"), readme).unwrap();
        fs::write(&skill, "Existing project skill").unwrap();
        fs::write(&generated, "Existing platform-managed handoff").unwrap();
        let old_reference = skill
            .parent()
            .unwrap()
            .join("references/app-window-runtime.md");
        fs::create_dir_all(old_reference.parent().unwrap()).unwrap();
        fs::write(&old_reference, "Older project-owned window instructions").unwrap();
        let old_sdk = project_root.join("app/vendor/pinned-sdk.tgz");
        fs::create_dir_all(old_sdk.parent().unwrap()).unwrap();
        fs::write(&old_sdk, "Pinned project SDK must stay unchanged").unwrap();

        let handoff = resources.handoff(&project_root, "prj_current");
        assert_eq!(handoff["projectId"], "prj_current");
        assert_eq!(handoff["skillSource"], "project");
        assert_eq!(handoff["skillPath"], skill.to_string_lossy().as_ref());
        assert_eq!(
            handoff["generatedHandoffPath"],
            generated.to_string_lossy().as_ref()
        );
        assert_eq!(handoff["cliPath"], cli.to_string_lossy().as_ref());
        assert_eq!(
            handoff["runtimeConstraintsPath"],
            constraints.to_string_lossy().as_ref()
        );
        assert!(constraints.is_file());
        let text = handoff["text"].as_str().unwrap();
        for expected in [
            "请先阅读并理解这个网页应用，暂时不要修改任何文件。",
            "项目 ID：prj_current",
            "接管 skill（项目自带）",
            "平台管理的接管记录",
            "若清单声明不同，请读取实际声明的文件",
            "等我提出具体需求，再开始修改",
            "不修改平台源码、平台管理的 .platform/ 和 SDK（@local-projects/sdk 0.1.0），不手工覆盖发布目录",
            "未保存草稿",
            "检查任务的最终结果",
            "不直接写入或替换 SQLite 文件",
            "当前安装随附，项目自带 skill 与它不一致时以它为准",
            "每个应用只有一个窗口，重复打开只聚焦已有窗口",
            "不弹新窗口",
            "在应用窗口里验证",
            "平台不提供源码或数据版本取回",
            "代码版本由外部 AI 使用标准 Git 保存和回退",
            crate::ai_prompt_rules::GIT_SAVE,
            crate::ai_prompt_rules::DATA_CONSENT,
            crate::ai_prompt_rules::SECRETS,
        ] {
            assert!(
                text.contains(expected),
                "Missing handoff instruction: {expected}"
            );
        }
        for path in [
            project_root.clone(),
            skill.clone(),
            generated.clone(),
            project_root.join("README.md"),
            project_root.join("project.json"),
            project_root.join("docs/database.md"),
            project_root.join("docs/platform.md"),
            constraints,
        ] {
            assert!(text.contains(path.to_string_lossy().as_ref()));
        }
        let quoted_cli = shell_quote(&cli.to_string_lossy());
        assert!(text.contains(&format!(
            "{quoted_cli} project build --project 'prj_current' --json"
        )));
        assert!(text.contains(&format!(
            "{quoted_cli} db schema --project 'prj_current' --json"
        )));
        assert!(text.contains(installation_skill.to_string_lossy().as_ref()));
        assert!(!text.contains("private-fixture-value"));
        for retired in [
            "Codex",
            "继续开发",
            "普通话",
            "source acquire",
            "env git doctor",
            "env git check",
            "configuration/credentials",
            "BACKUP_WORKFLOW_RETIRED",
            "data-restore-preview",
            "数据库与附件的 Git 取回",
        ] {
            assert!(
                !text.contains(retired),
                "Retired handoff wording: {retired}"
            );
        }
        assert_eq!(fs::read(project_root.join("README.md")).unwrap(), readme);
        assert_eq!(fs::read_to_string(skill).unwrap(), "Existing project skill");
        assert_eq!(
            fs::read_to_string(old_reference).unwrap(),
            "Older project-owned window instructions"
        );
        assert_eq!(
            fs::read_to_string(old_sdk).unwrap(),
            "Pinned project SDK must stay unchanged"
        );
        assert_eq!(
            fs::read_to_string(generated).unwrap(),
            "Existing platform-managed handoff"
        );
    }
    #[test]
    fn old_project_handoff_uses_installation_skill_without_creating_or_rewriting_files() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("test-install");
        let cli = installation(&root);
        let resources = ProjectResources::new(root.clone(), cli).unwrap();
        let project_root = paths.workspace_root().join("old project");
        fs::create_dir_all(&project_root).unwrap();
        let readme = b"Unchanged older project README with old installation paths";
        fs::write(project_root.join("README.md"), readme).unwrap();

        let handoff = resources.handoff(&project_root, "prj_older");
        let fallback = root.join("skills/local-projects-app/SKILL.md");
        assert_eq!(handoff["skillSource"], "installation");
        assert_eq!(handoff["skillPath"], fallback.to_string_lossy().as_ref());
        assert_eq!(
            handoff["runtimeConstraintsPath"],
            root.join(WINDOW_CONSTRAINTS_RESOURCE)
                .to_string_lossy()
                .as_ref()
        );
        assert!(handoff["generatedHandoffPath"].is_null());
        let text = handoff["text"].as_str().unwrap();
        assert!(text.contains("本机安装随附，旧项目文件保持原样"));
        assert!(text.contains(fallback.to_string_lossy().as_ref()));
        assert!(!text.contains("平台管理的接管记录"));
        assert_eq!(fs::read(project_root.join("README.md")).unwrap(), readme);
        assert_eq!(fs::read_dir(&project_root).unwrap().count(), 1);
    }
    #[test]
    fn missing_window_constraint_resource_is_not_reported_as_an_existing_handoff_path() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("test-install");
        let cli = installation(&root);
        let resources = ProjectResources::new(root.clone(), cli.clone()).unwrap();
        let constraint = root.join(WINDOW_CONSTRAINTS_RESOURCE);
        fs::remove_file(&constraint).unwrap();
        assert!(ProjectResources::new(root, cli).is_err());
        let handoff = resources.handoff(&paths.workspace_root().join("older project"), "prj_old");
        assert!(handoff["runtimeConstraintsPath"].is_null());
        let text = handoff["text"].as_str().unwrap();
        assert!(text.contains("当前安装缺少可核对的应用窗口约束资源"));
        assert!(!text.contains(constraint.to_string_lossy().as_ref()));
        assert!(text.contains("每个应用只有一个窗口"));
    }
    #[cfg(unix)]
    #[test]
    fn handoff_does_not_follow_a_replaced_installation_window_constraint_link() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("test-install");
        let cli = installation(&root);
        let resources = ProjectResources::new(root.clone(), cli.clone()).unwrap();
        let constraint = root.join(WINDOW_CONSTRAINTS_RESOURCE);
        let outside = paths.data_root().join("outside-window-instructions.txt");
        fs::write(
            &outside,
            "Outside file must not become current platform instructions",
        )
        .unwrap();
        fs::remove_file(&constraint).unwrap();
        std::os::unix::fs::symlink(&outside, &constraint).unwrap();
        assert!(ProjectResources::new(root, cli).is_err());
        let handoff = resources.handoff(&paths.workspace_root().join("older project"), "prj_old");
        assert!(handoff["runtimeConstraintsPath"].is_null());
        assert!(fs::symlink_metadata(constraint).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(outside).unwrap(),
            "Outside file must not become current platform instructions"
        );
    }
    #[cfg(unix)]
    #[test]
    fn handoff_does_not_follow_project_skill_or_generated_handoff_symlinks() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("test-install");
        let cli = installation(&root);
        let resources = ProjectResources::new(root.clone(), cli).unwrap();
        let project_root = paths.workspace_root().join("linked project");
        let skill = project_root.join(".agents/skills/local-projects-app/SKILL.md");
        let generated = project_root.join(".platform/handoff.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        let outside = paths.data_root().join("outside-project.txt");
        fs::write(&outside, "Do not follow this link").unwrap();
        std::os::unix::fs::symlink(&outside, &skill).unwrap();
        std::os::unix::fs::symlink(&outside, &generated).unwrap();

        let handoff = resources.handoff(&project_root, "prj_linked");
        assert_eq!(handoff["skillSource"], "installation");
        assert_eq!(
            handoff["skillPath"],
            root.join("skills/local-projects-app/SKILL.md")
                .to_string_lossy()
                .as_ref()
        );
        assert!(handoff["generatedHandoffPath"].is_null());
        assert!(fs::symlink_metadata(skill).unwrap().is_symlink());
        assert!(fs::symlink_metadata(generated).unwrap().is_symlink());
        assert_eq!(
            fs::read_to_string(outside).unwrap(),
            "Do not follow this link"
        );
    }
    #[tokio::test]
    async fn installed_template_is_ready_without_development_tools_and_reopening_never_overwrites_it(
    ) {
        let (_temporary, paths) = RuntimePaths::for_test();
        let root = paths.data_root().join("test-install");
        let cli = installation(&root);
        // A broken build environment cannot prevent copying/opening the bundled starter.
        fs::write(
            paths.data_root().join("toolchain.json"),
            r#"{"schemaVersion":1,"nodePath":"/missing/node"}"#,
        )
        .unwrap();
        assert!(
            !crate::environment::EnvironmentManager::new(paths.data_root().into())
                .doctor()
                .await
                .ready
        );
        fs::write(
            paths.data_root().join("git-toolchain.json"),
            r#"{"schemaVersion":1,"gitPath":"/missing/git"}"#,
        )
        .unwrap();
        assert!(
            !crate::git_environment::GitEnvironmentManager::new(
                paths.data_root().into(),
                Some(root.join("development-git"))
            )
            .doctor()
            .await
            .ready
        );
        let resources = Arc::new(ProjectResources::new(root.clone(), cli.clone()).unwrap());
        let store =
            ProjectStore::with_resources(paths.workspace_root().into(), Some(resources.clone()))
                .unwrap();
        let project = store
            .create(CreateProjectInput {
                name: "AI template".into(),
                description: None,
            })
            .await
            .unwrap();
        let project_root = &project.project_root;
        let readme = fs::read_to_string(project_root.join("README.md")).unwrap();
        assert!(readme.contains(&project.id));
        assert!(readme.contains(&shell_quote(&cli.to_string_lossy())));
        let skill_path = project_root.join(".agents/skills/local-projects-app/SKILL.md");
        assert_eq!(
            fs::read(&skill_path).unwrap(),
            fs::read(root.join("skills/local-projects-app/SKILL.md")).unwrap()
        );
        assert!(readme.contains(".agents/skills/local-projects-app/SKILL.md"));
        assert!(readme.contains("references/app-window-runtime.md"));
        assert!(readme.contains("single native application window"));
        assert!(readme.contains("only focuses the same window"));
        assert!(readme.contains("stops it when that application window closes"));
        assert!(readme.contains("not the standard application entry"));
        assert!(readme.contains("configuration/credentials/"));
        assert!(readme.contains("plaintext files protected by the current user's file permissions"));
        assert!(!readme.contains("project window and a normal browser"));
        assert_eq!(
            fs::read(
                project_root
                    .join(".agents")
                    .join(WINDOW_CONSTRAINTS_RESOURCE)
            )
            .unwrap(),
            fs::read(root.join(WINDOW_CONSTRAINTS_RESOURCE)).unwrap()
        );
        for entry in ["AGENTS.md", "CLAUDE.md"] {
            assert!(fs::read_to_string(project_root.join(entry))
                .unwrap()
                .contains(".agents/skills/local-projects-app/SKILL.md"));
        }
        let handoff = resources.handoff(project_root, &project.id);
        assert_eq!(handoff["skillSource"], "project");
        assert_eq!(handoff["skillPath"], skill_path.to_string_lossy().as_ref());
        // Older projects retain their files: the handoff can reference the bundled fallback.
        fs::remove_file(&skill_path).unwrap();
        assert_eq!(
            resources.handoff(project_root, &project.id)["skillSource"],
            "installation"
        );
        assert!(!skill_path.exists());
        assert_eq!(
            fs::read_to_string(project_root.join("README.md")).unwrap(),
            readme
        );
        assert!(!readme.contains("M2 才提供"));
        assert!(fs::read_to_string(project_root.join("docs/platform.md"))
            .unwrap()
            .contains("Use platform.db.execute"));
        assert!(fs::read_to_string(project_root.join("docs/platform.md"))
            .unwrap()
            .contains("Creation template: `test-template-1`"));
        assert_eq!(
            fs::read_to_string(project_root.join("app/src/App.tsx")).unwrap(),
            "template one"
        );
        assert_eq!(project.state, "ready");
        let handle = store.project(&project.id).unwrap();
        let build_info = crate::builds::build_info(&paths, &handle).unwrap();
        assert_eq!(build_info["sourceState"], "matched");
        assert_eq!(build_info["origin"], "bundledTemplate");
        assert!(build_info.get("provenance").is_none());
        assert!(build_info["starter"].get("taskId").is_none());
        let generation = crate::builds::artifact_generation(&handle)
            .unwrap()
            .unwrap();
        assert!(generation.build_id.starts_with("starter_"));
        assert_eq!(
            fs::read(generation.root.join("index.html")).unwrap(),
            fs::read(project_root.join("dist/index.html")).unwrap()
        );
        let connection = rusqlite::Connection::open(project_root.join("data/main.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(connection);
        fs::write(project_root.join("app/src/App.tsx"), "AI modified").unwrap();
        assert_eq!(
            crate::builds::build_info(&paths, &handle).unwrap()["sourceState"],
            "modified"
        );
        fs::write(
            root.join("template/app/src/App.tsx"),
            "new installation template",
        )
        .unwrap();
        store.shutdown().await;
        let reopened =
            ProjectStore::with_resources(paths.workspace_root().into(), Some(resources)).unwrap();
        assert_eq!(reopened.info(&project.id).await.unwrap().state, "ready");
        assert_eq!(
            fs::read_to_string(project_root.join("app/src/App.tsx")).unwrap(),
            "AI modified"
        );
        assert_eq!(
            fs::read_to_string(project_root.join("app/vendor/local-projects-sdk-0.1.0.tgz"))
                .unwrap(),
            "sdk version one"
        );
        reopened.shutdown().await;
    }
    #[tokio::test]
    async fn mismatched_packaged_source_or_artifact_never_publishes_a_project() {
        for relative in [
            "template/app/src/App.tsx",
            "template/starter/assets/starter.js",
        ] {
            let (_temporary, paths) = RuntimePaths::for_test();
            let root = paths.data_root().join("test-install");
            let cli = installation(&root);
            fs::write(root.join(relative), "changed after packaging").unwrap();
            let resources = Arc::new(ProjectResources::new(root, cli).unwrap());
            let store =
                ProjectStore::with_resources(paths.workspace_root().into(), Some(resources))
                    .unwrap();
            let error = store
                .create(CreateProjectInput {
                    name: "Invalid package".into(),
                    description: None,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code, "INVALID_TEMPLATE");
            assert!(store.list().await.unwrap().is_empty());
            let retained = fs::read_dir(paths.workspace_root().join("projects"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(retained.len(), 1);
            assert!(retained[0]
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(".creating-prj_"));
            assert_eq!(error.details["candidatePath"], json!(retained[0]));
            assert!(!Path::new(error.details["projectRoot"].as_str().unwrap()).exists());
            store.shutdown().await;
        }
    }
}
