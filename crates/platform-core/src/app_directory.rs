//! Checked local-folder admission into the single application store.
use crate::{panda_manifest::PandaManifest, ApiError, RuntimeHandle};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;
type Result<T> = std::result::Result<T, ApiError>;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::DirectoryInitializeInput;
#[cfg(windows)]
pub use windows::DirectoryInitializeInput;
struct AppDirectories {
    runtime: RuntimeHandle,
    data_root: PathBuf,
    directory_changes: Mutex<()>,
}
struct PromptPaths {
    skill: Option<PathBuf>,
    cli: Option<PathBuf>,
}
fn fail(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("APP_DIRECTORY_INVALID", message.to_string())
}
fn ordinary_path(path: &Path, directory: bool) -> Result<()> {
    crate::projects::check_no_symlinks(path, false)?;
    let metadata = fs::metadata(path)?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(fail("文件夹路径无效。"));
    }
    Ok(())
}
fn directory_review_prompt(
    source: &Path,
    state: &str,
    issues: &[String],
    paths: &PromptPaths,
) -> String {
    let folder = source.display().to_string();
    let report = json!({"state":state,"issues":issues}).to_string();
    let unavailable = "当前平台缺少安装资源，暂时无法提供";
    let skill = paths
        .skill
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| unavailable.into());
    let cli = paths
        .cli
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| unavailable.into());
    crate::ai_prompt_rules::render(
        crate::ai_prompt_rules::DIRECTORY_PROMPT,
        &[
            ("folder", &folder),
            ("report", &report),
            ("skillPath", &skill),
            ("cliPath", &cli),
            ("summary", crate::ai_prompt_rules::PLATFORM_AI_SUMMARY),
        ],
    )
}
impl AppDirectories {
    fn new(runtime: RuntimeHandle) -> Self {
        Self {
            data_root: runtime.info().data_root,
            runtime,
            directory_changes: Mutex::new(()),
        }
    }
    fn prompt_paths(&self) -> PromptPaths {
        PromptPaths {
            skill: self.runtime.platform_skill_path(),
            cli: self.runtime.platform_cli_path(),
        }
    }
    fn starter_files(&self, name: &str) -> Result<(Vec<PathBuf>, Vec<(PathBuf, Vec<u8>)>)> {
        let temporary = tempfile::Builder::new()
            .prefix("panda-starter-")
            .tempdir_in(&self.data_root)?;
        let app = crate::panda_manifest::AppDecl {
            name: name.into(),
            description: None,
            version: None,
            icon: None,
        };
        if let Some(resources) = self.runtime.application_resources().as_ref() {
            resources.install(temporary.path(), &app)?;
        } else {
            fs::write(
                temporary.path().join("panda.json"),
                serde_json::to_vec_pretty(
                    &json!({"manifestVersion":2,"app":app,"web":{"root":".","entry":"index.html"},"platform":{"sdkApiVersion":1}}),
                )?,
            )?;
            fs::write(temporary.path().join("index.html"),b"<!doctype html><html><body><main>Panda app</main><script src=\"/_platform/sdk.js\"></script></body></html>")?;
            fs::write(
                temporary.path().join(".gitignore"),
                b"data/\nnode_modules/\n.env*\n",
            )?;
        }
        fn collect(
            root: &Path,
            relative: &Path,
            dirs: &mut Vec<PathBuf>,
            files: &mut Vec<(PathBuf, Vec<u8>)>,
        ) -> Result<()> {
            let mut entries =
                fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = relative.join(entry.file_name());
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.is_dir() {
                    dirs.push(path.clone());
                    collect(root, &path, dirs, files)?;
                } else if metadata.is_file() {
                    files.push((path, fs::read(entry.path())?));
                } else {
                    return Err(fail("起始项目包含不支持的文件。"));
                }
            }
            Ok(())
        }
        let mut dirs = vec![];
        let mut files = vec![];
        collect(temporary.path(), Path::new(""), &mut dirs, &mut files)?;
        Ok((dirs, files))
    }
}
impl RuntimeHandle {
    pub async fn register_directory(
        &self,
        path: String,
        expected_revision: String,
    ) -> Result<Value> {
        AppDirectories::new(self.clone())
            .register_directory(PathBuf::from(path), expected_revision)
            .await
    }
    pub async fn initialize_directory(&self, path: String, input: Value) -> Result<Value> {
        let directories = AppDirectories::new(self.clone());
        let input = serde_json::from_value(input)?;
        let source = path.clone();
        let generated = tokio::task::spawn_blocking(move || {
            directories.initialize_directory(&PathBuf::from(path), input)
        })
        .await
        .map_err(|_| fail("目录初始化中断，请重新检查。"))??;
        let revision = generated["expectedRevision"]
            .as_str()
            .ok_or_else(|| fail("生成后的目录检查结果缺失。"))?
            .to_owned();
        let app = self.register_directory(source, revision).await?;
        let id = app["id"]
            .as_str()
            .ok_or_else(|| fail("应用登记结果缺少身份。"))?
            .to_owned();
        if app["state"] == "needsBuild" && self.application_resources().is_some() {
            let task = self.build_project(&id).await?;
            let task_id = task["taskId"]
                .as_str()
                .ok_or_else(|| fail("起始项目构建任务缺失。"))?;
            loop {
                let result = self.task(task_id).await?;
                if matches!(
                    result["status"].as_str(),
                    Some("succeeded" | "failed" | "interrupted")
                ) {
                    if result["status"] != "succeeded" {
                        return Err(fail(
                            "起始文件已生成并登记，但构建未完成。请保留文件并让 AI 查看构建结果。",
                        )
                        .detail("appId", json!(id))
                        .detail("task", result)
                        .detail("initializationCompleted", true));
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        self.project(&id).await
    }
    pub async fn reserve_app(&self, name: Option<String>) -> Result<Value> {
        self.store().reserve_app(name.as_deref())
    }
    pub(super) async fn inspect_app_directory(&self, path: String) -> Result<Value> {
        let directories = AppDirectories::new(self.clone());
        tokio::task::spawn_blocking(move || directories.inspect_directory(&PathBuf::from(path)))
            .await
            .map_err(|_| fail("目录检查中断，请重新检查。"))?
    }
}
