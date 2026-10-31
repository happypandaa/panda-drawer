//! Original artifact rename/swap and directory fsync behavior.
use crate::{builds::ensure_dir, projects::check_no_symlinks, ApiError};
use std::{
    fs::{self, File},
    path::Path,
};

pub(super) fn replace_directory(staged: &Path, current: &Path) -> Result<(), ApiError> {
    check_no_symlinks(staged, false)?;
    check_no_symlinks(current, true)?;
    if !current.exists() {
        ensure_dir(current.parent().unwrap())?;
        fs::rename(staged, current)?;
    } else {
        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let a = CString::new(staged.as_os_str().as_bytes())
                .map_err(|_| ApiError::new("INVALID_PATH", "NUL in artifact path"))?;
            let b = CString::new(current.as_os_str().as_bytes())
                .map_err(|_| ApiError::new("INVALID_PATH", "NUL in artifact path"))?;
            let code = unsafe {
                libc::renameatx_np(
                    libc::AT_FDCWD,
                    a.as_ptr(),
                    libc::AT_FDCWD,
                    b.as_ptr(),
                    libc::RENAME_SWAP,
                )
            };
            if code != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(ApiError::new(
                "UNSUPPORTED_PLATFORM",
                "Atomic artifact exchange currently targets macOS",
            ));
        }
    }
    File::open(current.parent().unwrap())?.sync_all()?;
    File::open(staged.parent().unwrap())?.sync_all()?;
    Ok(())
}
