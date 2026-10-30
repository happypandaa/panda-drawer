//! Windows installed-editor verification/dispatch remains a temporary boundary.
use platform_core::ApiError;
use std::path::{Path, PathBuf};

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：编辑器安装验证与自动打开尚未完成。请手动打开编辑器、选择项目目录，再粘贴接手说明。",
    )
    .detail("capability", "codex-desktop-handoff")
    .outcome("not_started")
}

pub async fn installed_app() -> Result<PathBuf, ApiError> {
    Err(unavailable())
}

pub async fn open(_: &Path, _: &Path) -> Result<(), ApiError> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_installation_never_supplies_an_editor_path() {
        let error = installed_app().await.unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
    }

    #[tokio::test]
    async fn direct_dispatch_preserves_a_disposable_project_without_creating_fallback_material() {
        let root = tempfile::tempdir().unwrap();
        let sentinel = root.path().join("source.txt");
        std::fs::write(&sentinel, b"retain external author source").unwrap();
        let missing_app = root.path().join("not-an-installed-editor.exe");
        let error = open(&missing_app, root.path()).await.unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
        assert!(!missing_app.exists());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(sentinel).unwrap(),
            b"retain external author source"
        );
    }
}
