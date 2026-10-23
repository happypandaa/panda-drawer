//! Dispatch validated local locations to Finder without a shell or URL input.
use platform_core::ApiError;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

fn finder_command(location: &Path, select_file: bool) -> Result<Command, ApiError> {
    if !location.is_absolute() {
        return Err(ApiError::new("REVEAL_FAILED", "本地目录位置无效"));
    }
    let mut command = Command::new("/usr/bin/open");
    if select_file {
        command.arg("-R");
    } else {
        // A directory must always open in Finder, even if it has an app-like
        // extension. Paths never choose an executable or become shell input.
        command.args(["-a", "Finder"]);
    }
    command
        .arg(location)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

pub(super) async fn open(location: &Path, select_file: bool) -> Result<(), ApiError> {
    #[cfg(not(target_os = "macos"))]
    return Err(ApiError::new(
        "REVEAL_UNSUPPORTED",
        "此系统暂不支持打开本地目录",
    ));
    #[cfg(target_os = "macos")]
    match tokio::time::timeout(
        Duration::from_secs(5),
        finder_command(location, select_file)?.status(),
    )
    .await
    {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => Err(ApiError::new(
            "REVEAL_FAILED",
            "无法打开本地目录，请检查目录是否仍可访问",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_is_one_literal_finder_argument() {
        let location = Path::new("/tmp/可丢弃 $(echo test) '#?&.app");
        let command = finder_command(location, false).unwrap();
        let command = command.as_std();
        assert_eq!(command.get_program(), "/usr/bin/open");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-a", "Finder", location.to_str().unwrap()]
        );
        let command = finder_command(location, true).unwrap();
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["-R", location.to_str().unwrap()]
        );
        assert!(finder_command(Path::new("relative"), false).is_err());
    }
}
