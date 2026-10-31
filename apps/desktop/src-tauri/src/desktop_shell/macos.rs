use platform_core::ApiError;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

pub(crate) fn reveal(location: PathBuf, readme: bool) -> Result<(), ApiError> {
    let mut command = Command::new("/usr/bin/open");
    if readme {
        command.arg("-R");
    }
    command.arg(location);
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(ApiError::new("REVEAL_FAILED", "无法打开项目位置"));
    }
    Ok(())
}

pub(crate) fn external_link(url: &tauri::Url) {
    let _ = Command::new("/usr/bin/open")
        .arg(url.as_str())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) fn node_help(url: String) -> Result<(), ApiError> {
    Command::new("/usr/bin/open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|_| {
            ApiError::new(
                "BROWSER_OPEN_FAILED",
                "无法打开浏览器，请检查系统默认浏览器设置。",
            )
        })
}
