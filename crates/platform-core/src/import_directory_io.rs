//! Directory admission for a Git import's private operation material.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::{fs, path::Path};

pub(crate) fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    #[cfg(unix)]
    {
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
    }
    #[cfg(windows)]
    {
        if !path.exists() {
            crate::project_file_io::ensure_directory(path)?;
        }
    }
    check_no_symlinks(path, false)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(ApiError::new(
            "NODE_GIT_IMPORT_INVALID",
            "导入位置不是普通目录。",
        ));
    }
    platform_native::filesystem::make_directory_private(path)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use platform_native::windows::files::{open_no_reparse, verify_private};

    #[test]
    fn import_material_is_private_and_foreign_files_or_junction_targets_are_preserved() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-import-directory-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let parent = open_no_reparse(&root).unwrap();
        assert!(verify_private(&parent.file).is_err());
        let operation = root.join("new/operation");
        private_dir(&operation).unwrap();
        private_dir(&operation).unwrap();
        verify_private(&open_no_reparse(&operation).unwrap().file).unwrap();
        assert!(verify_private(&parent.file).is_err());
        let file = root.join("retained-file");
        fs::write(&file, b"retained disposable bytes").unwrap();
        assert_eq!(
            private_dir(&file).unwrap_err().code,
            "NODE_GIT_IMPORT_INVALID"
        );
        assert_eq!(fs::read(&file).unwrap(), b"retained disposable bytes");
        let outside = root.join("retained-directory");
        fs::create_dir(&outside).unwrap();
        let before = open_no_reparse(&outside).unwrap();
        let junction = root.join("junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(private_dir(&junction).is_err());
        let after = open_no_reparse(&outside).unwrap();
        assert!(before.identity.same_object(&after.identity));
        assert!(verify_private(&after.file).is_err());
    }
}
