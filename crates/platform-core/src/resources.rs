//! Immutable installation resources are copied only when creating a project.
use crate::{projects::check_no_symlinks, ApiError, ProjectManifest};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const WINDOW_CONSTRAINTS_RESOURCE: &str =
    "skills/local-projects-app/references/app-window-runtime.md";

#[derive(Clone)]
pub struct ProjectResources {
    root: PathBuf,
    pub cli_path: PathBuf,
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
        Ok(Self { root, cli_path })
    }
    /// Development tools are optional for running published apps. A missing or
    /// damaged bundle is reported by the build environment, not startup.
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
        fs::write(staging.join("docs/database.md"), "# Database contract\n\nDatabase: `data/main.sqlite`, independent SQLite database. Initially empty; no platform business tables.\n\n## Tables\n\nFor every table document its purpose, columns/types, nullability, primary/foreign keys, indexes, defaults, units, enums and timestamp convention. Keep this document synchronized with every schema change.\n\n## Attachments\n\nStore file logical paths, MIME types and lifecycle rules here. Files belong to `data/attachments/`, never the web build.\n\n## Optional template media sample\n\nThe media panel creates no schema when opening or building. Only its explicit Save action creates `media_samples`: `id TEXT PRIMARY KEY`, `attachment_path TEXT NOT NULL`, `mime_type TEXT NOT NULL`, `extension TEXT NOT NULL`, `size_bytes INTEGER NOT NULL`, `duration_ms INTEGER NULL` (measured milliseconds), `original_name TEXT NOT NULL`, `created_at TEXT NOT NULL` (ISO-8601 UTC). There are no additional indexes, defaults or foreign keys. IDs are UUIDs; attachment paths are project-relative logical references, never blob URLs. Deleting a row or clearing the preview does not remove its attachment. Define explicit retention/deletion and orphan recovery before adapting this optional example to business data. See app/BROWSER_TOOLS.md.\n\n## Changes and compatibility\n\nRecord every schema change, the applied SQL, data compatibility and validation. The platform enables foreign keys. Use CLI managed transactions; do not replace a registered database file or manipulate its WAL/SHM while running. Before incompatible schema changes use maintenance begin, wait for a successful baseline and readyForChanges, pass --maintenance ID to SQL/build, and explicitly end maintenance after validation. The independent development environment still uses disposable test data.\n")?;
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
- This is a development project. Use disposable test data in this isolated development environment. Full ZIP backup, maintenance and recovery are available via the CLI.

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
{cli} task logs --task TASK_ID --json
{cli} project url --project {id} --json
```

Verify the result in the platform's native application window. The returned fresh `launchUrl` is only for necessary HTTP/browser diagnostics, not the standard application entry or a substitute for native-window acceptance. It is short lived; never save it or any credentials in source or documentation. After a platform restart, obtain a new URL if diagnostics are needed. `task wait` timing out does not cancel a build. A failed build keeps the previous webpage and saved data. Check task status and logs before claiming success.

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

Use `project metadata get --project {id} --json` to read the current `metadataRevision`, then `project metadata set --project {id} --input metadata.json --json` with that value as `expectedRevision`. Change only name, description or icon; resolve a revision conflict by reading the current values again. Pass the current maintenance ID when applicable. Metadata updates do not require a build. Custom PNG icons live in persistent `meta/`, never in generated `dist/`; keep them in full project backups.

Use `storage info/scan/files/cleanup --project {id}` and `workspace storage` for platform accounting. `scan` and `cleanup` return tasks; wait for the final result. `files` and `cleanup` accept `--input` JSON according to CLI help. Measurements are logical file sizes with timestamps, completeness and stale indicators; a missing or unreadable category is unknown, not zero. Cleanup accepts only the platform's explicitly supported regenerable cache categories. Do not use it to remove databases, attachments, meta, backups, recycle-bin contents or retained operation evidence. Inspect the candidate and result before reporting reclaimed bytes.

## Browser capabilities and media

Read `app/BROWSER_TOOLS.md` and the independent `app/src/lib/browser-tools.js` helper before implementing recording, camera or file controls. It is copied only into new projects; SDK 0.1.0 remains fixed. Default runtimeRequirements are empty and no device is requested on load. Declare specific camera/microphone purposes and precise public HTTPS origins separately under network.connect/images/media. No credentials, remote script permission, localhost/LAN proxy or secret storage is provided. Document data sent and offline behavior. A declaration change needs explicit reload after handling drafts; declarations do not instantly revoke arbitrary old-page behavior.

The helper caps foreground capture at 60 seconds and media at min(10 MiB, context.limits.fileUploadMaxBytes), uses actual MIME detection, and releases devices on hidden/leave/maintenance/disconnect/reload states. These are internal test settings, not final product limits. File picking is the fallback. Media saving first writes the attachment and then the business association; only both successful acknowledgements mean saved. Do not replay unknown outcomes. If the optional media demo is used, its table is created only by the explicit Save button; record the media_samples schema and attachment retention policy in docs/database.md. Preview blob URLs are not persistent paths. Real signed-app/device/browser validation must be reported separately from mocks and builds.

## User flows

Document entry points, forms, validation, empty/loading/error states and saved data behavior. Handle errors visibly; do not report a failed save as success. Writes are not automatically retried. Reopen or explicitly refresh when the SDK reports that a new document is required; never automatically discard an unsaved form.

## Schema and data changes

Use the CLI to inspect existing schema and execute parameterized SQL or managed transactions. Keep `docs/database.md` complete. Use `backup create/list/info/restore` and `maintenance status/begin/end`. Wait for each task to finish. Enter maintenance before incompatible changes, wait for readyForChanges and baselineBackupId, then pass --maintenance ID to modifications and builds. Restoring keeps maintenance active; end explicitly after checks. Never invent a successful backup or replace a live SQLite file.

## Project lifecycle and workspace

The CLI supports `project export/import/copy/delete`, `trash list/info/restore/purge` and `workspace info/migrate`. These are management operations; project webpages do not have their permissions. Follow the documented JSON inputs and wait for each task. Import accepts complete platform ZIP archives and never runs project scripts; copies receive a new platform ID while business primary keys stay unchanged. Deletion first moves the whole project into the recycle bin. Permanent purge is a separate, explicit operation and retains historical backups.

Maintenance exports, copies and deletion require the current maintenance ID. A copied or imported maintenance snapshot keeps its pending checks and receives its own baseline; do not treat a successful copy as permission to end maintenance. Workspace migration uses a separate empty local APFS directory, retains the source, changes runtime credentials and requires fresh launch sessions. Obtain the current project path and AI handoff from the platform after moving; never continue editing a retained old copy.

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
        let constraints_note = constraints
            .as_ref()
            .map(|path| {
                format!(
                    "还必须阅读当前安装随附的最新应用窗口约束（项目自带 skill 保持原样）：{}\n若旧 skill 仍推荐默认另开浏览器，按这份当前平台约束补充执行；不要覆盖旧项目 skill、SDK 或文档。",
                    path.display()
                )
            })
            .unwrap_or_else(|| {
                "当前安装缺少可核对的应用窗口约束资源；请修复平台安装资源，不要引用不存在的路径或改写旧项目文件。下述窗口约束仍须遵守。".into()
            });
        let generated = root.join(".platform/handoff.md");
        let generated = if check_no_symlinks(&generated, false).is_ok() && generated.is_file() {
            Some(generated)
        } else {
            None
        };
        let generated_note = generated
            .as_ref()
            .map(|path| {
                format!(
                    "\n此项目还有平台管理的接管记录，请先阅读其当前路径：{}\n其中可能记录复制、导入或迁移后的项目身份；不要继续编辑保留的旧目录。\n",
                    path.display()
                )
            })
            .unwrap_or_default();
        let cli = shell_quote(&self.cli_path.to_string_lossy());
        let project_id = shell_quote(id);
        let text = format!(
            r#"请接手以下业务应用，并根据我接下来提供的需求继续开发。

熊猫应用小抽屉（开发版）是 macOS 上的本地业务应用平台。它管理各个项目的源码、独立 SQLite 数据库、附件、运行入口、构建发布、备份与维护。你要开发的是下面的业务项目；平台会把发布后的网页显示在唯一原生应用窗口中，用户应当能在 AI 离线时继续使用。

业务项目 ID：{id}
业务项目根目录：{project_root}
当前平台 CLI：{cli_path}

先读取当前接管 skill（{skill_source}）：{skill_path}
{constraints_note}
{generated_note}
然后读取项目清单：{manifest_path}
根据清单中的 docs 声明完整阅读 README、数据库和平台集成说明，了解业务目的、现有功能、数据结构和平台约束。以下是默认模板的文档路径；若清单声明不同，请读取实际声明的文件：
- 默认 README：{readme_path}
- 默认数据库说明：{database_docs}
- 默认平台集成说明：{platform_docs}
业务文档和 skill 的 PROJECT_ROOT 指上述业务项目根目录；skill 的 references/ 等资源链接相对该 skill 所在目录解析。当前目录、安装约束和 CLI 以上述实际路径为准；不要沿用旧 README 中已经失效的安装或迁移前路径。

Codex 接手时，请确认当前工作目录正是上述业务项目根目录。平台“在 Codex 打开”只准备项目目录，不自动发送需求。若 CLI 报本机连接受限或服务不可达，先核对平台是否运行及本会话对本机服务的访问权限；不要据此删除连接记录、重建项目、直接打开 SQLite 或绕过工具权限。不能访问时保留已完成的源码修改，给出待执行的公开 CLI 请求与未验事项，由用户在允许访问的平台会话中继续。

平台应用窗口约束：每个应用实例最多一个原生窗口，重复打开只聚焦已有窗口，不重载或丢弃草稿；不得弹出新窗口、调用 window.open/target=_blank、另建 WebView 或转外部浏览器作为运行方式。业务确认与表单留在当前窗口内，不用脚本 alert/confirm/prompt。网页类型不启动自己的 Node；Node 类型由平台在打开应用时后台启动唯一后端，关闭应用窗口就停止，启动中的重复打开复用同一次任务。保存配置不启动或重启，明确重启或关窗后再打开才使用新配置。此交接仍针对已有 project-v1，不把独立 Node 源码槽的能力套入其安装、数据或迁移生命周期。

当前 Node 秘密保存按用户最新选择：每个应用自己的本机受管槽 configuration/credentials/ 私有文件，目录0700、文件0600，是当前用户文件权限保护的明文，不是 macOS Keychain 或系统加密库。该槽与作者源码、分享包和 Git 分开；AI 不读取或复制秘密文件，不读删旧 Keychain 条目，不把秘密写进交接、日志或源码。配置入口负责保存并仅向目标 Node 创建环境注入；本条不为当前静态 project-v1 增加秘密接口。

Git 工具可通过当前 CLI 的 env git doctor --json 诊断，Mac 内部包默认配套私有 Git；已有显式选择优先，失效不偷偷换来源。env git set --path /绝对路径/git 与 env git clear 仅修改本平台选择，后者明确恢复默认。系统 SSH 只检查工具版本，认证仍未验证；当前未提供提交／分支／取回版本接口，应用日常运行不依赖 Git／SSH。不要为工具诊断读取私钥、修改全局 Git 或初始化项目仓库。

需要检查指定公开仓库时，明确使用 env git check --url https://github.com/owner/repository.git --json，也支持标准 Gitee HTTPS 仓库地址。它在20秒执行预算内匿名读取远端HEAD，不读取账号凭据、不克隆或下载仓库、不推送或修改项目。结果只适用于该地址和本次检查时间；未返回HEAD不表示空仓库，也不改变doctor的认证未验证状态。不要把该检查加入普通离线工具诊断，不把它当作SSH／私有仓库认证或完整获取流程。

实际获取公开源码须明确使用 source acquire --url 标准HTTPS仓库地址 --json，再用 task wait 核对最终任务，source list/get 查看来源版本和保留目录。source inspect --path /绝对路径 --json 只检查本机声明；source cancel --task TASK_ID --json 请求取消后仍须等待最终状态。获取不会安装或执行代码，失败/中断现场保留、重试另建目录；不要将源码候选当已登记应用或用其来源ID调用项目数据库。当前完整安装和私有仓库认证尚未开放。

源码、发布产物、数据库、附件和文档的位置分别以 project.json 中的 sourceDir、outputDir、database、attachmentsDir 和 docs 声明为准；app/、dist/、data/main.sqlite 和 data/attachments/ 仅为默认模板示例。先理解现有实现，再按我的需求修改业务源码，并同步项目说明和清单声明的数据库文档。使用项目固定的 @local-projects/sdk 0.1.0；不要改写或替换 SDK，不要修改平台源码、平台管理的 .platform/ 或手工覆盖 outputDir 声明的发布目录，也不要另起后端或 Vite 开发服务。

使用当前平台 CLI 检查环境、项目和数据库，再通过平台管理的 SQL/事务与构建流程完成修改，例如：

```sh
{cli} service status --json
{cli} project info --project {project_id} --json
{cli} db schema --project {project_id} --json
{cli} db execute --project {project_id} --input request.json --json
{cli} db transaction --project {project_id} --input transaction.json --json
{cli} project build --project {project_id} --json
# 使用返回的 taskId；若 waitTimedOut 为 true，继续等待，不代表失败或取消。
{cli} task wait --task TASK_ID --json
{cli} task logs --task TASK_ID --json
{cli} project url --project {project_id} --json
```

SQL 输入格式和维护规则以 skill、项目文档及 CLI 帮助为准；不要直接写入或替换运行中的 SQLite 文件或 WAL/SHM。涉及不兼容数据变更时，先进入维护并等待基线备份成功和 readyForChanges，再携带维护 ID 操作；验证后明确结束维护。

保留现有用户数据、附件、备份和未保存草稿；不要为了修复构建或刷新页面清空数据。检查异步任务的最终状态与日志，在平台原生应用窗口验证发布结果、重复打开聚焦、关闭重开与数据保留；短期 launchUrl 的 HTTP/浏览器诊断只作补充，不能代替原生窗口验收。平台重启或工作区迁移后重新获取项目路径与入口，不要保存端口、令牌、票据、Cookie 或其他密钥。未知写入结果不要自动重放，刷新或重载前先处理草稿。

完成后报告实际修改、数据库/schema 变化、构建任务结果、实际验证和已知限制；没有执行的验证请明确说明，并把这些信息写回项目说明。
"#,
            project_root = root.display(),
            cli_path = self.cli_path.display(),
            skill_source = if local_skill {
                "项目自带"
            } else {
                "本机安装随附，旧项目文件保持原样"
            },
            skill_path = skill.display(),
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
            "熊猫应用小抽屉（开发版）是 macOS 上的本地业务应用平台",
            "独立 SQLite 数据库",
            "业务项目 ID：prj_current",
            "先读取当前接管 skill（项目自带）",
            "平台管理的接管记录",
            "sourceDir、outputDir、database、attachmentsDir 和 docs 声明为准",
            "仅为默认模板示例",
            "若清单声明不同，请读取实际声明的文件",
            "不要修改平台源码、平台管理的 .platform/ 或手工覆盖 outputDir 声明的发布目录",
            "@local-projects/sdk 0.1.0",
            "未保存草稿",
            "readyForChanges",
            "检查异步任务的最终状态与日志",
            "不要直接写入或替换运行中的 SQLite 文件",
            "还必须阅读当前安装随附的最新应用窗口约束",
            "每个应用实例最多一个原生窗口",
            "重复打开只聚焦已有窗口",
            "打开应用时后台启动唯一后端，关闭应用窗口就停止",
            "不得弹出新窗口",
            "不能代替原生窗口验收",
            "configuration/credentials/ 私有文件，目录0700、文件0600",
            "当前用户文件权限保护的明文",
            "不是 macOS Keychain 或系统加密库",
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
            "{quoted_cli} project url --project 'prj_current' --json"
        )));
        assert!(!text.contains("private-fixture-value"));
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
        assert!(text.contains("每个应用实例最多一个原生窗口"));
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
