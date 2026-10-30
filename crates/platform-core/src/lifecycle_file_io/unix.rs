//! Original Unix descriptor-bound external export publication.
use super::*;
use serde_json::json;
use std::fs::File;

pub(super) fn publish_export_file_with(
    stage: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
    mut checkpoint: impl FnMut(ExportPublicationPoint) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
    };
    if !stage.is_absolute()
        || !destination.is_absolute()
        || stage == destination
        || stage.parent().is_none()
        || stage.parent() != destination.parent()
    {
        return Err(ApiError::new(
            "EXPORT_FAILED",
            "导出发布必须在同一目录原子完成。",
        ));
    }
    let parent_path = stage.parent().unwrap();
    let parent = crate::files::root(parent_path)?;
    let parent_identity = PersistedIdentity::from_file(&parent)
        .map_err(|error| ApiError::new("EXPORT_FAILED", error.to_string()))?;
    let from = CString::new(
        stage
            .file_name()
            .ok_or_else(|| ApiError::new("INVALID_EXPORT_PATH", "导出临时文件名缺失。"))?
            .as_encoded_bytes(),
    )
    .map_err(|_| ApiError::new("INVALID_EXPORT_PATH", "导出文件名无效。"))?;
    let to = CString::new(
        destination
            .file_name()
            .ok_or_else(|| ApiError::new("INVALID_EXPORT_PATH", "导出目标文件名缺失。"))?
            .as_encoded_bytes(),
    )
    .map_err(|_| ApiError::new("INVALID_EXPORT_PATH", "导出文件名无效。"))?;

    let file_at = |name: &CString| -> Result<File, ApiError> {
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(ApiError::new(
                "EXPORT_STAGE_CHANGED",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(ApiError::new(
                "EXPORT_STAGE_CHANGED",
                "导出临时材料不是普通文件，已保留。",
            ));
        }
        Ok(file)
    };
    // Holding the source descriptor also prevents its inode from being reused
    // while a public temporary name is exchanged by another process.
    let source = file_at(&from)?;
    if !expected.matches_captured(
        &PersistedIdentity::from_file(&source)
            .map_err(|error| ApiError::new("EXPORT_STAGE_CHANGED", error.to_string()))?,
    ) {
        return Err(ApiError::new(
            "EXPORT_STAGE_CHANGED",
            "导出临时文件身份已变化，未交付。",
        ));
    }
    checkpoint(ExportPublicationPoint::BeforeRename)?;
    if !parent_identity.matches_path(parent_path).unwrap_or(false) {
        return Err(ApiError::new(
            "EXPORT_STAGE_CHANGED",
            "导出目录身份已变化，未交付。",
        ));
    }
    export_rename_exclusive(&parent, &from, &to)?;
    parent.sync_all()?;
    checkpoint(ExportPublicationPoint::Renamed)?;
    let matched = file_at(&to)
        .and_then(|file| {
            PersistedIdentity::from_file(&file)
                .map_err(|error| ApiError::new("EXPORT_STAGE_CHANGED", error.to_string()))
        })
        .is_ok_and(|identity| expected.matches_captured(&identity));
    if !matched || !parent_identity.matches_path(parent_path).unwrap_or(false) {
        // A substituted temporary object can be captured by the exclusive
        // rename. Return it only if its old name is still free; never replace a
        // newly created stage, or enroll the final user file in our ownership.
        let _ = export_rename_exclusive(&parent, &to, &from);
        parent.sync_all()?;
        return Err(ApiError::new(
            "EXPORT_STAGE_CHANGED",
            "导出发布时文件或目录被替换，现有文件已保留或归还，未确认交付。",
        )
        .detail("stagePath", json!(stage))
        .detail("destinationPath", json!(destination)));
    }
    Ok(())
}

fn export_rename_exclusive(
    parent: &File,
    from: &std::ffi::CString,
    to: &std::ffi::CString,
) -> Result<(), ApiError> {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = -1;
    if result != 0 {
        return Err(ApiError::new(
            "EXPORT_DESTINATION_EXISTS",
            "导出目标已存在或无法独占发布，未覆盖。",
        ));
    }
    Ok(())
}
