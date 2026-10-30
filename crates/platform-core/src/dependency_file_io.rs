//! Native file policy for dependency preparation; no npm process is started here.
use crate::{path_checks::check_no_symlinks, ApiError};
use platform_native::tool_files::Facts;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path},
};

type Result<T> = std::result::Result<T, ApiError>;

fn invalid(message: &str) -> ApiError {
    ApiError::new("NODE_INSTALL_INVALID", message)
}

fn changed() -> ApiError {
    ApiError::new("NODE_DEPENDENCY_CHANGED", "依赖准备时项目文件发生变化。")
}

pub(crate) fn ordinary_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid("安装路径必须为规范的绝对路径。"));
    }
    check_no_symlinks(path, false)?;
    if !fs::metadata(path)?.is_dir() {
        return Err(invalid("安装路径不是普通目录。"));
    }
    Ok(())
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    check_no_symlinks(path, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path)?;
        ordinary_directory(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        crate::project_file_io::ensure_directory(path)?;
        ordinary_directory(path)?;
    }
    Ok(())
}

pub(crate) fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    check_no_symlinks(path, false)?;
    let before = Facts::capture(path)?;
    if !before.is_file() || before.links() != 1 || before.len() > limit {
        return Err(invalid("安装文件必须是预算内的独立普通文件。"));
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinks,
    )?;
    let opened = Facts::from_file(&file)?;
    if !before.same_object(&opened) {
        return Err(changed());
    }
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes)?;
    let after = Facts::capture(path)?;
    if bytes.len() as u64 > limit || !before.same_modified_permissions(&after) {
        return Err(changed());
    }
    Ok(bytes)
}

pub(crate) fn write_new(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    check_no_symlinks(path, true)?;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
    };
    #[cfg(windows)]
    let mut file = {
        // Windows execution is checked by the actual process owner, not by a
        // fabricated Unix permission bit. Both files use the same private DACL.
        let _ = executable;
        platform_native::windows::files::create_private_file(path)?
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use platform_native::windows::files::{open_no_reparse, verify_private};

    #[test]
    fn bounded_inputs_are_read_without_creating_missing_files_or_admitting_hard_links() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-npm-input-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("package.json");
        assert!(read_file(&path, 1024).is_err());
        assert!(!path.exists());
        fs::write(&path, b"{\"private\":true}").unwrap();
        let before = Facts::capture(&path).unwrap();
        assert_eq!(read_file(&path, 1024).unwrap(), b"{\"private\":true}");
        assert_eq!(
            read_file(&path, 2).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
        assert!(before.same_object(&Facts::capture(&path).unwrap()));
        fs::hard_link(&path, root.join("retained-input-link")).unwrap();
        assert_eq!(
            read_file(&path, 1024).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
        assert_eq!(fs::read(&path).unwrap(), b"{\"private\":true}");
    }

    #[test]
    fn private_npm_files_are_exclusive_and_do_not_change_existing_parent_access() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-npm-private-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let parent = open_no_reparse(&root).unwrap();
        assert!(verify_private(&parent.file).is_err());
        let home = root.join("operation/npm-home");
        private_dir(&home).unwrap();
        for directory in [&root.join("operation"), &home] {
            verify_private(&open_no_reparse(directory).unwrap().file).unwrap();
        }
        for executable in [false, true] {
            let path = home.join(if executable {
                "input.exe"
            } else {
                "user.npmrc"
            });
            write_new(&path, b"disposable bytes; never executed", executable).unwrap();
            let before = open_no_reparse(&path).unwrap();
            verify_private(&before.file).unwrap();
            assert!(write_new(&path, b"must not overwrite", executable).is_err());
            assert_eq!(
                fs::read(&path).unwrap(),
                b"disposable bytes; never executed"
            );
            assert!(before
                .identity
                .same_object(&open_no_reparse(&path).unwrap().identity));
        }
        assert!(verify_private(&parent.file).is_err());
    }
}
