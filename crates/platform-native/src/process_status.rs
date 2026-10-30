//! Exit observations do not grant process ownership or stopping permission.
use std::process::ExitStatus;

pub fn signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    #[test]
    fn a_real_windows_exit_is_an_exit_code_without_a_unix_signal() {
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "exit", "42"])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(42));
        assert_eq!(signal(&status), None);
    }
}
