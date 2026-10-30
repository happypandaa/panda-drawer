//! Keep an actual operation directory alive with its phase owner.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::{fs::File, path::Path};

pub(crate) fn open(path: &Path) -> Result<File, ApiError> {
    check_no_symlinks(path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let before = std::fs::symlink_metadata(path)?;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        let after = directory.metadata()?;
        if !before.is_dir() || before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(changed());
        }
        Ok(directory)
    }
    #[cfg(windows)]
    {
        let before = platform_native::windows::files::open_no_reparse(path)?;
        let directory = platform_native::windows::files::directory_file(path)?;
        let after = platform_native::windows::files::identity(&directory)?;
        if !before.identity.directory || !before.identity.same_object(&after) {
            return Err(changed());
        }
        Ok(directory)
    }
}

fn changed() -> ApiError {
    ApiError::new(
        "NODE_PROCESS_OPERATION_CHANGED",
        "候选操作目录身份已变化，现场保留。",
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn operation_handle_keeps_the_real_directory_and_rejects_a_later_junction() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-operation-owner-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let operation = root.join("operation");
        let retained = root.join("retained");
        fs::create_dir(&operation).unwrap();
        fs::write(operation.join("payload"), b"disposable evidence").unwrap();
        let file = open(&operation).unwrap();
        assert!(fs::rename(&operation, &retained).is_err());
        assert_eq!(
            fs::read(operation.join("payload")).unwrap(),
            b"disposable evidence"
        );
        drop(file);
        fs::rename(&operation, &retained).unwrap();
        assert!(std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&operation)
            .arg(&retained)
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(open(&operation).unwrap_err().code, "UNSAFE_PATH");
        fs::remove_dir(operation).unwrap();
        assert_eq!(
            fs::read(retained.join("payload")).unwrap(),
            b"disposable evidence"
        );
    }
}
