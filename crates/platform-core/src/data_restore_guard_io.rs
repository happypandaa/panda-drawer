//! Prepared restore-directory publication and native directory persistence.
use super::{changed, invalid};
use crate::{file_identity::PersistedIdentity, ApiError};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{fs::File, path::Path};

pub(super) fn sync_prepared_journal(
    file: File,
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        let _ = (path, expected);
        file.sync_all().map_err(|_| invalid())?;
    }
    #[cfg(windows)]
    {
        use platform_native::windows::files as native;
        use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
        let _parent = native::pin_directory(path.parent().ok_or_else(invalid)?)?;
        if !expected
            .matches_open_runtime(&file)
            .map_err(|_| invalid())?
        {
            return Err(changed());
        }
        let writable = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(7)
            .custom_flags(0x0020_0000)
            .open(path)?;
        let metadata = writable.metadata()?;
        if !metadata.is_file()
            || platform_native::filesystem::is_reparse(&metadata)
            || native::identity(&writable)?.links != 1
            || !expected
                .matches_open_runtime(&writable)
                .map_err(|_| invalid())?
        {
            return Err(changed());
        }
        // The application-created control is flushed through actual write
        // access. The ordinary payload/read-only admission is unchanged.
        writable.sync_all().map_err(|_| invalid())?;
    }
    Ok(())
}

pub(super) fn prepare_staging(staging_fd: File) -> Result<Option<File>, ApiError> {
    #[cfg(unix)]
    {
        staging_fd.sync_all().map_err(|_| invalid())?;
        // Keep the original Unix descriptor alive until the caller's scope ends.
        Ok(Some(staging_fd))
    }
    #[cfg(windows)]
    {
        platform_native::windows::files::sync_directory(&staging_fd).map_err(|_| invalid())?;
        // Our own no-DELETE directory pin must close before the exact rename.
        // The immutable header retains the original source FullId.
        drop(staging_fd);
        Ok(None)
    }
}

pub(super) fn sync_parent(path: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    crate::files::root(path)?.sync_all()?;
    #[cfg(windows)]
    platform_native::windows::files::sync_directory(&crate::files::root(path)?)?;
    Ok(())
}

pub(super) fn move_prepared_directory(
    from: &Path,
    to: &Path,
    expected: &PersistedIdentity,
) -> Result<(), ApiError> {
    #[cfg(not(windows))]
    {
        let _ = expected;
        move_directory_exclusive(from, to)
    }
    #[cfg(windows)]
    {
        let file_id = expected.windows_file_id.ok_or_else(invalid)?;
        if expected.volume_uuid.is_some() {
            return Err(invalid());
        }
        let identity = platform_native::windows::files::FileIdentity {
            volume_serial: expected.device,
            file_id,
            links: 0,
            directory: true,
        };
        platform_native::windows::files::rename_directory_noreplace_expected(from, to, &identity)?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn move_directory_exclusive(from: &Path, to: &Path) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    let left = crate::files::root(from.parent().ok_or_else(invalid)?)?;
    let right = crate::files::root(to.parent().ok_or_else(invalid)?)?;
    if left.metadata().map_err(|_| invalid())?.dev()
        != right.metadata().map_err(|_| invalid())?.dev()
    {
        return Err(invalid());
    }
    let a =
        CString::new(from.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    let b = CString::new(to.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
    if unsafe {
        libc::renameatx_np(
            left.as_raw_fd(),
            a.as_ptr(),
            right.as_raw_fd(),
            b.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(changed());
    }
    left.sync_all().map_err(|_| invalid())?;
    right.sync_all().map_err(|_| invalid())?;
    Ok(())
}
#[cfg(not(any(target_os = "macos", windows)))]
fn move_directory_exclusive(_from: &Path, _to: &Path) -> Result<(), ApiError> {
    Err(ApiError::new(
        "UNSUPPORTED_PLATFORM",
        "数据取回现场发布目前仅验证macOS。",
    ))
}
