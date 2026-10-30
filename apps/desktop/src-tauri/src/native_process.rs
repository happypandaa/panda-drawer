//! Short native dispatch/verification children. Timeouts stop and reap the child
//! and join output reading before returning to the enclosing native I/O lease.
use std::{
    io,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

async fn finish(child: &mut tokio::process::Child, duration: Duration) -> io::Result<ExitStatus> {
    match tokio::time::timeout(duration, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        outcome => {
            // kill_on_drop is only a fallback. Awaiting wait after kill keeps
            // the operation's owned lifetime through actual child reaping.
            let _ = child.start_kill();
            child.wait().await?;
            match outcome {
                Ok(Err(error)) => Err(error),
                _ => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Native dispatch timed out",
                )),
            }
        }
    }
}
pub(super) async fn status(mut command: Command, duration: Duration) -> io::Result<ExitStatus> {
    let mut child = command.kill_on_drop(true).spawn()?;
    finish(&mut child, duration).await
}
pub(super) async fn output(
    mut command: Command,
    duration: Duration,
) -> io::Result<std::process::Output> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped output");
    let reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.take(65_537).read_to_end(&mut bytes).await?;
        if bytes.len() > 65_536 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Native output exceeded limit",
            ));
        }
        Ok::<_, io::Error>(bytes)
    });
    let status = finish(&mut child, duration).await;
    // Even on timeout/wait failure, drain the owned reader before returning.
    let stdout = reader.await.map_err(io::Error::other)?;
    Ok(std::process::Output {
        status: status?,
        stdout: stdout?,
        stderr: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn timeout_kills_and_reaps_the_exact_artificial_child() {
        #[cfg(unix)]
        let mut command = Command::new("/bin/sleep");
        #[cfg(unix)]
        command.arg("30");
        #[cfg(windows)]
        let temporary = tempfile::Builder::new()
            .prefix("disposable-native-dispatch-timeout-")
            .tempdir()
            .unwrap();
        #[cfg(windows)]
        let ready = temporary.path().join("child-ready");
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "native_process::tests::windows_artificial_wait_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("PANDA_NATIVE_DISPATCH_TEST_READY", &ready)
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            command
        };
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        #[cfg(windows)]
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                assert!(child.try_wait().unwrap().is_none(), "Fixture exited early");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("The exact child fixture must enter its real wait before testing timeout");
        assert_eq!(
            finish(&mut child, Duration::from_millis(10))
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(child.id().is_none());
        assert!(child.try_wait().unwrap().is_some());
        // The child object, not a second PID lookup, remains the ownership proof.
        assert!(pid > 0);
    }
    #[tokio::test]
    async fn bounded_artificial_output_finishes_before_returning() {
        #[cfg(unix)]
        let mut command = Command::new("/usr/bin/printf");
        #[cfg(unix)]
        command.arg("artificial output");
        #[cfg(windows)]
        let command = {
            let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
            let mut command = Command::new(
                std::path::PathBuf::from(system_root)
                    .join("System32")
                    .join("cmd.exe"),
            );
            // Fixed input only: no user text is interpolated into this shell command.
            command
                .args(["/d", "/c", "<nul set /p=artificial output&exit /b 0"])
                .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            command
        };
        let output = output(command, Duration::from_secs(2)).await.unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"artificial output");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Child fixture invoked by the exact timeout test"]
    fn windows_artificial_wait_fixture() {
        let Some(ready) = std::env::var_os("PANDA_NATIVE_DISPATCH_TEST_READY") else {
            return;
        };
        std::fs::write(ready, b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }
}
