//! Remove an explicitly bound cleanup file through its native parent scope.
use crate::{file_identity::PersistedIdentity, ApiError};
use std::path::{Path, PathBuf};

fn changed() -> ApiError {
    ApiError::new(
        "LOCAL_CLEANUP_CHANGED",
        "清理范围或目录身份已变化，请重新核对两次确认；未继续删除。",
    )
    .outcome("not_started")
}

pub(crate) fn remove_file(
    path: &Path,
    identity: &PersistedIdentity,
    parents: &[(PathBuf, PersistedIdentity)],
) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            fs::OpenOptions,
            os::{
                fd::AsRawFd,
                unix::{ffi::OsStrExt, fs::OpenOptionsExt},
            },
        };
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path.parent().ok_or_else(changed)?)?;
        let parent_identity = PersistedIdentity::from_file(&parent).map_err(|_| changed())?;
        if !parents
            .first()
            .is_some_and(|(_, expected)| expected.same_runtime_object(&parent_identity))
        {
            return Err(changed());
        }
        let name = CString::new(path.file_name().ok_or_else(changed)?.as_bytes())
            .map_err(|_| changed())?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_dev as u64 != identity.device
            || stat.st_ino != identity.inode
        {
            return Err(changed());
        }
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        Ok(())
    }
    #[cfg(windows)]
    {
        use platform_native::windows::files;
        let parent = files::pin_directory(path.parent().ok_or_else(changed)?)?;
        let parent_identity = PersistedIdentity::from_file(&parent.file).map_err(|_| changed())?;
        if !parents
            .first()
            .is_some_and(|(_, expected)| expected.same_runtime_object(&parent_identity))
        {
            return Err(changed());
        }
        let opened = files::open_no_reparse(path)?;
        if opened.identity.directory
            || !identity
                .matches_open_runtime(&opened.file)
                .map_err(|_| changed())?
        {
            return Err(changed());
        }
        files::remove_file_allowing_hard_links(path, &opened.identity)?;
        files::sync_directory(&parent.file)?;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_bound_name_can_be_removed_without_deleting_its_other_hard_link() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-bound-cleanup-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("owned.json");
        let retained = root.join("retained-link.json");
        fs::write(&path, b"disposable cleanup evidence").unwrap();
        fs::hard_link(&path, &retained).unwrap();
        let expected = PersistedIdentity::capture(&path).unwrap();
        let parents = vec![(root.clone(), PersistedIdentity::capture(&root).unwrap())];
        remove_file(&path, &expected, &parents).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&retained).unwrap(), b"disposable cleanup evidence");
        assert!(expected.matches_path(&retained).unwrap());
    }

    #[test]
    fn stale_leaf_and_parent_identities_cannot_remove_replacement_material() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-bound-cleanup-replacement-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("owned.json");
        let retained = root.join("retained-original.json");
        fs::write(&path, b"original disposable evidence").unwrap();
        let expected = PersistedIdentity::capture(&path).unwrap();
        let parents = vec![(root.clone(), PersistedIdentity::capture(&root).unwrap())];
        fs::rename(&path, &retained).unwrap();
        fs::write(&path, b"replacement disposable bytes").unwrap();
        assert_eq!(
            remove_file(&path, &expected, &parents).unwrap_err().code,
            "LOCAL_CLEANUP_CHANGED"
        );
        let replacement = PersistedIdentity::capture(&path).unwrap();
        let other = root.join("other-parent");
        fs::create_dir(&other).unwrap();
        let wrong_parent = vec![(root.clone(), PersistedIdentity::capture(&other).unwrap())];
        assert_eq!(
            remove_file(&path, &replacement, &wrong_parent)
                .unwrap_err()
                .code,
            "LOCAL_CLEANUP_CHANGED"
        );
        assert_eq!(fs::read(&path).unwrap(), b"replacement disposable bytes");
        assert_eq!(
            fs::read(&retained).unwrap(),
            b"original disposable evidence"
        );
        assert!(replacement.matches_path(&path).unwrap());
    }
}
