//! Filesystem policy used by the project registry. No workspace is discovered here.
use crate::{path_checks::check_no_symlinks, ApiError};
#[cfg(any(unix, test))]
use std::fs;
use std::{fs::File, path::Path};

/// Use the caller's existing Unix observation at the same policy checkpoint.
/// Windows obtains the real link count from a no-reparse file handle.
pub(crate) fn observed_link_count(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        Ok(metadata.nlink())
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        link_count(path)
    }
}

pub(crate) fn link_count(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(fs::metadata(path)?.nlink())
    }
    #[cfg(windows)]
    {
        Ok(u64::from(
            platform_native::windows::files::open_no_reparse(path)?
                .identity
                .links,
        ))
    }
}

#[cfg(windows)]
pub(crate) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        platform_native::windows::files::create_private_directory(path)
    }
}

pub(crate) fn ensure_directory(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        let mut missing = Vec::new();
        let mut current = path;
        while !current.try_exists()? {
            missing.push(current);
            current = current.parent().ok_or_else(|| {
                ApiError::new("UNSAFE_PATH", "Project directory has no existing parent")
            })?;
        }
        for directory in missing.into_iter().rev() {
            create_private_directory(directory)?;
        }
        // Only the final admitted directory is changed when it already exists.
        // Existing system/workspace ancestors are never modified here.
        platform_native::windows::files::make_directory_private(path)?;
    }
    Ok(())
}

pub(crate) fn create_new_file(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }
    #[cfg(windows)]
    {
        platform_native::windows::files::create_private_file(path)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn registry_files_and_new_directory_levels_are_private_without_changing_the_parent() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-project-registry-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let root_file = platform_native::windows::files::open_no_reparse(&root).unwrap();
        assert!(platform_native::windows::files::verify_private(&root_file.file).is_err());
        let directory = root.join("new").join("nested");
        ensure_directory(&directory).unwrap();
        ensure_directory(&directory).unwrap();
        for path in [&root.join("new"), &directory] {
            let opened = platform_native::windows::files::open_no_reparse(path).unwrap();
            platform_native::windows::files::verify_private(&opened.file).unwrap();
        }
        assert!(platform_native::windows::files::verify_private(&root_file.file).is_err());
        let path = directory.join("registration.json");
        let mut file = create_new_file(&path).unwrap();
        file.write_all(b"disposable registry bytes").unwrap();
        file.sync_all().unwrap();
        platform_native::windows::files::verify_private(&file).unwrap();
        assert!(create_new_file(&path).is_err());
        assert_eq!(link_count(&path).unwrap(), 1);
        assert_eq!(
            observed_link_count(&path, &fs::symlink_metadata(&path).unwrap()).unwrap(),
            1
        );
        fs::hard_link(&path, directory.join("disposable-link")).unwrap();
        assert_eq!(link_count(&path).unwrap(), 2);
        assert_eq!(
            observed_link_count(&path, &fs::symlink_metadata(&path).unwrap()).unwrap(),
            2
        );
        assert_eq!(fs::read(&path).unwrap(), b"disposable registry bytes");
    }
}
