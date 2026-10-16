//! Immutable installation resources are copied only when creating a project.
use crate::{projects::check_no_symlinks, ApiError, ProjectManifest};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

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

First read the project-owned skill at `.agents/skills/local-projects-app/SKILL.md`, then this README, `project.json`, `docs/database.md`, and `docs/platform.md`. The skill can be read directly by any AI with file access; automatic discovery is not required. `AGENTS.md` and `CLAUDE.md` point to the same instructions. Implement ordinary usable webpage features: the user must be able to continue using the application when AI is offline.

This project starts with a ready-to-open welcome application, its matching React source and an empty independent database. Creating and opening it requires neither Node nor dependency downloads. The bundled template is recorded separately from user build tasks; after changing source, request a platform build to publish the new version. A failed first build preserves the initial application, but does not roll back any separately committed database changes.

The native platform runs the local service and owns SQLite and attachments. Node is needed only for builds (minimum 22.12, recommended 24 LTS, supported branches 22/24). Do not start a separate backend or Vite development server. The platform displays published static builds in its project window and a normal browser.

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

Open the returned fresh `launchUrl` in a browser for debugging. It is short lived; never save it or any credentials in source or documentation. After a platform restart, obtain a new URL. `task wait` timing out does not cancel a build. A failed build keeps the previous webpage and saved data. Check task status and logs before claiming success.

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
- Browser validation steps and result: pending.
- Desktop validation steps and result: pending.
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
        let generated = root.join(".platform/handoff.md");
        let generated = if check_no_symlinks(&generated, false).is_ok() && generated.is_file() {
            Some(generated)
        } else {
            None
        };
        json!({"projectId":id,"generatedHandoffPath":generated,"readmePath":readme,"skillPath":skill,"skillSource":if local_skill {"project"} else {"installation"},"cliPath":self.cli_path,"text":format!("请在这个业务项目目录中工作：{}\n先读取接管 skill：{}\n再完整阅读项目 README.md、project.json、docs/database.md 和 docs/platform.md。skill 中的相对路径均以业务项目目录为根。根据我的需求修改项目源码，使用下列当前平台 CLI 操作数据库和构建（若 README 中旧路径失效，以下列路径为准）；检查任务结果，再获取新的浏览器入口验证。不要修改平台源码，不要删除用户数据。\n项目 ID：{}\n平台 CLI：{}\n请把功能、schema 变化、验证和已知问题写回项目说明。",root.display(),skill.display(),id,self.cli_path.display())})
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
            "skills/local-projects-app",
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
    #[tokio::test]
    async fn installed_template_is_ready_without_node_and_reopening_never_overwrites_it() {
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
            assert_eq!(
                fs::read_dir(paths.workspace_root().join("projects"))
                    .unwrap()
                    .count(),
                0
            );
            store.shutdown().await;
        }
    }
}
