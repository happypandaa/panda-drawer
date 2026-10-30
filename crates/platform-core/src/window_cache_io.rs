//! Native private window preference I/O. No application roots are discovered.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::{fs::File, path::Path};

#[cfg(target_os = "macos")]
#[path = "window_cache_io/macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{publish_cache, retire_file};
#[cfg(windows)]
#[path = "window_cache_io/windows.rs"]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{publish_cache, recover, retire_file};

#[cfg(not(windows))]
pub(crate) fn recover(_: &Path) -> Result<(), ApiError> {
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub(crate) fn publish_cache(
    _: &Path,
    _: Option<&crate::app_windows::Snapshot>,
    _: &[u8],
    _: &mut impl FnMut(),
) -> Result<crate::app_windows::Snapshot, ApiError> {
    Err(ApiError::new(
        "WINDOW_CACHE_UNSUPPORTED",
        "当前系统尚未启用原子窗口尺寸缓存发布。",
    ))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub(crate) fn retire_file(
    _: &Path,
    _: &std::ffi::OsStr,
    _: &crate::file_identity::PersistedIdentity,
    _: Option<&[u8]>,
) -> Result<(), ApiError> {
    Err(ApiError::new(
        "WINDOW_CACHE_UNSUPPORTED",
        "当前系统尚未启用窗口尺寸缓存清理。",
    ))
}

fn invalid(message: &str) -> ApiError {
    ApiError::new("INVALID_WINDOW_CACHE", message)
}

pub(crate) fn private_directory(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    #[cfg(unix)]
    let private = {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o700
    };
    #[cfg(windows)]
    let private = {
        let opened = platform_native::windows::files::open_no_reparse(path)?;
        opened.identity.directory && private_access(&opened.file)?
    };
    if !private {
        return Err(invalid("窗口尺寸目录必须是本项目私有目录。"));
    }
    Ok(())
}

pub(crate) fn open_private(path: &Path) -> Result<File, ApiError> {
    check_no_symlinks(path, false)?;
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblockingCloseOnExec,
    )?;
    #[cfg(unix)]
    let private = {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o7777 == 0o600
    };
    #[cfg(windows)]
    let private = {
        let facts = platform_native::windows::files::identity(&file)?;
        !facts.directory && facts.links == 1 && private_access(&file)?
    };
    if !private {
        return Err(invalid("窗口尺寸文件必须是本项目私有普通文件。"));
    }
    Ok(file)
}

#[cfg(windows)]
fn private_access(file: &File) -> std::io::Result<bool> {
    match platform_native::windows::files::verify_private(file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
    };
    #[test]
    fn actual_private_access_preserves_identity_bytes_and_absence() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-window-cache-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert_eq!(
            private_directory(&root).unwrap_err().code,
            "INVALID_WINDOW_CACHE"
        );
        let private = root.join("private-project-owner");
        platform_native::windows::files::create_private_directory(&private).unwrap();
        private_directory(&private).unwrap();
        let path = private.join("window-size.json");
        let mut file = platform_native::windows::files::create_private_file(&path).unwrap();
        file.write_all(b"{\"disposable\":true}").unwrap();
        file.sync_all().unwrap();
        let before = platform_native::windows::files::identity(&file).unwrap();
        let mut opened = open_private(&path).unwrap();
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"{\"disposable\":true}");
        assert!(before.same_object(&platform_native::windows::files::identity(&opened).unwrap()));
        assert!(open_private(&private.join("missing.json")).is_err());
        assert!(!private.join("missing.json").exists());
        assert_eq!(
            private_directory(&root).unwrap_err().code,
            "INVALID_WINDOW_CACHE"
        );
    }

    #[test]
    fn inherited_files_hardlinks_and_junctions_do_not_acquire_private_read_authority() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-window-cache-boundary-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let inherited = root.join("inherited.json");
        fs::write(&inherited, b"retained disposable inherited bytes").unwrap();
        assert_eq!(
            open_private(&inherited).unwrap_err().code,
            "INVALID_WINDOW_CACHE"
        );
        let private = root.join("private");
        platform_native::windows::files::create_private_directory(&private).unwrap();
        let path = private.join("window-size.json");
        let mut file = platform_native::windows::files::create_private_file(&path).unwrap();
        file.write_all(b"retained disposable private bytes")
            .unwrap();
        file.sync_all().unwrap();
        fs::hard_link(&path, private.join("second-name")).unwrap();
        assert_eq!(
            open_private(&path).unwrap_err().code,
            "INVALID_WINDOW_CACHE"
        );
        let junction = root.join("disposable-junction");
        assert!(std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&private)
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(
            private_directory(&junction).unwrap_err().code,
            "UNSAFE_PATH"
        );
        assert_eq!(
            open_private(&junction.join("window-size.json"))
                .unwrap_err()
                .code,
            "UNSAFE_PATH"
        );
        fs::remove_dir(junction).unwrap();
        assert_eq!(
            fs::read(path).unwrap(),
            b"retained disposable private bytes"
        );
        assert_eq!(
            fs::read(inherited).unwrap(),
            b"retained disposable inherited bytes"
        );
    }
}
