//! Manager-only, read-only AI context and explicit editor dispatch.
use super::{codex_handoff, desktop_path_io, require_manager, ApiError, AppState};
use platform_core::{BootstrapInfo, RuntimeHandle};
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Manager;

#[derive(Clone)]
struct ReviewRuntime {
    instance: String,
    data: PathBuf,
    workspace: PathBuf,
}
impl ReviewRuntime {
    fn new(info: &BootstrapInfo) -> Self {
        Self {
            instance: info.instance_id.clone(),
            data: info.data_root.clone(),
            workspace: info.workspace_root.clone(),
        }
    }
    fn validate(&self, info: &BootstrapInfo) -> Result<(), ApiError> {
        if self.instance != info.instance_id
            || self.data != info.data_root
            || self.workspace != info.workspace_root
        {
            return Err(changed());
        }
        Ok(())
    }
}

struct ReviewRoot {
    selected: PathBuf,
    root: PathBuf,
    identity: desktop_path_io::DirectoryIdentity,
}
impl ReviewRoot {
    fn new(selected: PathBuf, runtime: &ReviewRuntime) -> Result<Self, ApiError> {
        let root = platform_core::project_review::review_root(
            &selected,
            &runtime.data,
            &runtime.workspace,
        )?;
        let identity = desktop_path_io::directory_identity(&root)?.ok_or_else(changed)?;
        Ok(Self {
            selected,
            root,
            identity,
        })
    }
    fn validate(&self, runtime: &ReviewRuntime) -> Result<(), ApiError> {
        let current = Self::new(self.selected.clone(), runtime)?;
        if current.root != self.root || current.identity != self.identity {
            return Err(changed());
        }
        Ok(())
    }
}

fn changed() -> ApiError {
    ApiError::new(
        "PROJECT_REVIEW_CHANGED",
        "项目文件夹或平台位置已变化，请重新选择后准备说明。",
    )
}
fn worker_failed() -> ApiError {
    ApiError::new(
        "PROJECT_REVIEW_UNAVAILABLE",
        "项目材料检查未完成，请重新选择项目文件夹。",
    )
}
fn validate_runtime(handle: &RuntimeHandle, runtime: &ReviewRuntime) -> Result<(), ApiError> {
    handle.io_lifetime().check_open()?;
    runtime.validate(&handle.info())
}
async fn checked_root(
    handle: &RuntimeHandle,
    source_path: String,
) -> Result<(ReviewRoot, ReviewRuntime), ApiError> {
    handle.io_lifetime().check_open()?;
    let runtime = ReviewRuntime::new(&handle.info());
    let worker_runtime = runtime.clone();
    let root = tokio::task::spawn_blocking(move || {
        ReviewRoot::new(PathBuf::from(source_path), &worker_runtime)
    })
    .await
    .map_err(|_| worker_failed())??;
    validate_runtime(handle, &runtime)?;
    Ok((root, runtime))
}

#[tauri::command]
pub(super) async fn source_review_capabilities(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    io_lease
        .run(async move {
            let available = codex_handoff::installed_app().await.is_ok();
            handle.io_lifetime().check_open()?;
            Ok(json!({"codexAvailable":available}))
        })
        .await?
}

#[tauri::command]
pub(super) async fn source_review_context(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let handle = state.handle.clone();
    io_lease
        .run(async move {
            let (root, runtime) = checked_root(&handle, source_path).await?;
            let worker_runtime = runtime.clone();
            let value = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
                root.validate(&worker_runtime)?;
                let value = platform_core::project_review::context(
                    &root.root,
                    &worker_runtime.data,
                    &worker_runtime.workspace,
                )?;
                root.validate(&worker_runtime)?;
                Ok(value)
            })
            .await
            .map_err(|_| worker_failed())??;
            validate_runtime(&handle, &runtime)?;
            Ok(value)
        })
        .await?
}

#[tauri::command]
pub(super) async fn source_review_open_codex(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<Value, ApiError> {
    require_manager(&window)?;
    let io_lease = state.handle.io_lifetime().admit()?;
    let io_app = window.app_handle().clone();
    io_lease
        .run(async move {
            let state = io_app.state::<AppState>();
            let (root, runtime) = checked_root(&state.handle, source_path).await?;
            // App verification can yield for filesystem/signature tools. A path
            // selected before that work cannot authorize a replacement folder.
            let app = codex_handoff::installed_app().await?;
            validate_runtime(&state.handle, &runtime)?;
            root.validate(&runtime)?;
            codex_handoff::open(&app, &root.root).await?;
            validate_runtime(&state.handle, &runtime)?;
            root.validate(&runtime)?;
            Ok(json!({"sourcePath":root.root,"requested":true}))
        })
        .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, ReviewRuntime, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let fixture_root = temp.path().canonicalize().unwrap();
        let data = fixture_root.join("disposable-platform");
        #[cfg(unix)]
        let project = fixture_root.join("disposable-project $(echo test) #?&.app");
        #[cfg(windows)]
        let project = fixture_root.join("disposable-project $(echo test) #&.app");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&project).unwrap();
        let runtime = ReviewRuntime {
            instance: "disposable-instance".into(),
            workspace: data.join("workspace"),
            data,
        };
        (temp, runtime, project)
    }
    fn info(runtime: &ReviewRuntime) -> BootstrapInfo {
        BootstrapInfo {
            display_name: "可丢弃·项目评估测试".into(),
            identifier: "disposable-test".into(),
            environment: "development".into(),
            api_version: 1,
            instance_id: runtime.instance.clone(),
            base_url: "http://127.0.0.1:1".into(),
            workspace_root: runtime.workspace.clone(),
            data_root: runtime.data.clone(),
            sqlite_version: "test".into(),
        }
    }

    #[test]
    fn a_selected_directory_is_literal_and_no_project_content_is_needed_to_dispatch() {
        let (_temp, runtime, project) = fixture();
        fs::write(project.join(".env"), "SYNTHETIC_SECRET=not-a-real-key").unwrap();
        let binding = ReviewRoot::new(project.clone(), &runtime).unwrap();
        assert_eq!(binding.root, project.canonicalize().unwrap());
        binding.validate(&runtime).unwrap();
        assert_eq!(fs::read_dir(project).unwrap().count(), 1);
    }

    #[test]
    fn a_replaced_directory_cannot_reuse_an_earlier_editor_confirmation() {
        let (_temp, runtime, project) = fixture();
        let binding = ReviewRoot::new(project.clone(), &runtime).unwrap();
        let preserved = project.with_extension("preserved");
        fs::rename(&project, &preserved).unwrap();
        fs::create_dir(&project).unwrap();
        assert_eq!(
            binding.validate(&runtime).unwrap_err().code,
            "PROJECT_REVIEW_CHANGED"
        );
        assert!(preserved.is_dir());
        assert!(project.is_dir());
    }

    #[test]
    fn links_and_platform_private_roots_are_not_editor_or_excerpt_targets() {
        let (_temp, runtime, _project) = fixture();
        #[cfg(unix)]
        {
            let linked = _temp.path().canonicalize().unwrap().join("disposable-link");
            std::os::unix::fs::symlink(&_project, &linked).unwrap();
            assert!(ReviewRoot::new(linked, &runtime).is_err());
        }
        assert!(ReviewRoot::new(runtime.data.clone(), &runtime).is_err());
        let private = runtime.data.join("configuration");
        fs::create_dir(&private).unwrap();
        assert!(ReviewRoot::new(private, &runtime).is_err());
    }

    #[test]
    fn a_runtime_or_workspace_change_revokes_the_prepared_location() {
        let (_temp, runtime, _project) = fixture();
        let original = info(&runtime);
        runtime.validate(&original).unwrap();
        let mut changed = original.clone();
        changed.instance_id.push_str("-changed");
        assert_eq!(
            runtime.validate(&changed).unwrap_err().code,
            "PROJECT_REVIEW_CHANGED"
        );
        changed = original.clone();
        changed.workspace_root = runtime.workspace.with_extension("changed");
        assert!(runtime.validate(&changed).is_err());
        changed = original;
        changed.data_root = runtime.data.with_extension("changed");
        assert!(runtime.validate(&changed).is_err());
    }
}
