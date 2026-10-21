//! Open an installed Codex desktop application without relying on a shell/CLI.
//! The workspace URL follows OpenAI's codex-rs/cli/src/desktop_app/mac.rs.
use platform_core::ApiError;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const BUNDLE_ID: &str = "com.openai.codex";
const SIGNATURE: &str = "identifier \"com.openai.codex\" and anchor apple generic and certificate leaf[subject.OU] = \"2DC432GLL2\"";

fn workspace_url(root: &Path) -> Result<tauri::Url, ApiError> {
    let root = root
        .to_str()
        .filter(|_| root.is_absolute())
        .ok_or_else(|| ApiError::new("CODEX_PATH_INVALID", "项目目录必须是可读取的绝对路径"))?;
    let mut url = tauri::Url::parse("codex://threads/new").expect("constant URL");
    url.query_pairs_mut().append_pair("path", root);
    Ok(url)
}

pub async fn installed_app() -> Result<PathBuf, ApiError> {
    let mut directories = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(PathBuf::from(home).join("Applications"));
    }
    find_app(&directories).await
}

async fn find_app(directories: &[PathBuf]) -> Result<PathBuf, ApiError> {
    for directory in directories {
        for name in ["ChatGPT.app", "Codex.app"] {
            let candidate = directory.join(name);
            if !candidate.is_dir() {
                continue;
            }
            let mut command = Command::new("/usr/bin/plutil");
            command
                .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
                .arg(candidate.join("Contents/Info.plist"))
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let output = tokio::time::timeout(Duration::from_secs(3), command.output()).await;
            if !matches!(output, Ok(Ok(ref value)) if value.status.success() && String::from_utf8_lossy(&value.stdout).trim() == BUNDLE_ID)
            {
                continue;
            }
            verify_app(&candidate).await?;
            return Ok(candidate);
        }
    }
    Err(ApiError::new(
        "CODEX_NOT_INSTALLED",
        "未找到已安装的 Codex 桌面应用。可手动打开 Codex、选择下方项目目录，再粘贴接手说明。",
    ))
}

async fn verify_app(path: &Path) -> Result<(), ApiError> {
    let mut command = Command::new("/usr/bin/codesign");
    command
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R={SIGNATURE}"))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(20), command.status()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => Err(ApiError::new(
            "CODEX_INSTALLATION_INVALID",
            "无法验证 Codex 安装，请检查应用是否完整。接手说明已保留，可手动选择项目目录。",
        )),
    }
}

pub async fn open(app: &Path, root: &Path) -> Result<(), ApiError> {
    let url = workspace_url(root)?;
    let mut command = Command::new("/usr/bin/open");
    command
        .arg("-a")
        .arg(app)
        .arg(url.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(5), command.status()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => Err(ApiError::new(
            "CODEX_OPEN_FAILED",
            "未能确认 Codex 已打开，请手动选择下方项目目录并粘贴说明。",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_is_one_encoded_parameter_not_a_prompt_or_command() {
        let root = Path::new("/tmp/可丢弃 项目/#?&prompt=hello'$(echo test)");
        let url = workspace_url(root).unwrap();
        assert_eq!(url.scheme(), "codex");
        assert_eq!(url.host_str(), Some("threads"));
        assert_eq!(url.path(), "/new");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("path".into(), root.to_str().unwrap().into())]
        );
        assert!(workspace_url(Path::new("relative")).is_err());
    }
    #[tokio::test]
    async fn unavailable_app_is_reported_without_installing_or_opening_anything() {
        let error = find_app(&[]).await.unwrap_err();
        assert_eq!(error.code, "CODEX_NOT_INSTALLED");
    }
    #[tokio::test]
    async fn familiar_name_or_bundle_id_alone_does_not_authorize_launch() {
        let directory = tempfile::tempdir().unwrap();
        let contents = directory.path().join("ChatGPT.app/Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let plist = |id| {
            format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>")
        };
        std::fs::write(contents.join("Info.plist"), plist("com.openai.chat")).unwrap();
        assert_eq!(
            find_app(&[directory.path().to_path_buf()])
                .await
                .unwrap_err()
                .code,
            "CODEX_NOT_INSTALLED"
        );
        std::fs::write(contents.join("Info.plist"), plist(BUNDLE_ID)).unwrap();
        assert_eq!(
            find_app(&[directory.path().to_path_buf()])
                .await
                .unwrap_err()
                .code,
            "CODEX_INSTALLATION_INVALID"
        );
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "no install or fallback app created"
        );
    }
}
