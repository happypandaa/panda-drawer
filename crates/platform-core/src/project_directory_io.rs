//! Native project-directory publication; this adapter never selects a workspace.
use crate::{path_checks::check_no_symlinks, ApiError};
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::path::Path;

#[cfg(windows)]
#[path = "project_directory_io/windows_claims.rs"]
mod windows_claims;

/// Lexical allocation claims only. This never opens, resolves or grants access
/// to the retained absolute directory; actual project bindings stay separate.
pub(crate) fn retained_directory_leaf(root: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        windows_claims::retained_directory_leaf(root)
    }
    #[cfg(not(windows))]
    {
        let text = root.to_str()?;
        if !root.is_absolute()
            || text.contains('\\')
            || text.chars().any(char::is_control)
            || text[1..]
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || root
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some("projects")
        {
            return None;
        }
        root.file_name()?.to_str().map(str::to_owned)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    fn mapping_error() -> ApiError {
        ApiError::new(
            "PROJECT_DIRECTORY_MAPPING_INVALID",
            "Directory mapping is invalid",
        )
    }

    #[test]
    fn existing_empty_directory_is_retained_and_publication_succeeds_only_after_it_is_absent() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-directory-publish-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let candidate = root.join("candidate");
        let destination = root.join("existing");
        fs::create_dir(&candidate).unwrap();
        fs::write(
            candidate.join("payload.txt"),
            b"disposable publication bytes",
        )
        .unwrap();
        fs::create_dir(&destination).unwrap();
        let expected_candidate = platform_native::windows::files::open_no_reparse(&candidate)
            .unwrap()
            .identity;
        let expected_destination = platform_native::windows::files::open_no_reparse(&destination)
            .unwrap()
            .identity;
        let error =
            publish_directory_noreplace(&candidate, &destination, mapping_error).unwrap_err();
        assert_eq!(error.code, "PROJECT_DIRECTORY_PUBLISH_FAILED");
        assert!(expected_destination.same_object(
            &platform_native::windows::files::open_no_reparse(&destination)
                .unwrap()
                .identity
        ));
        assert!(expected_candidate.same_object(
            &platform_native::windows::files::open_no_reparse(&candidate)
                .unwrap()
                .identity
        ));
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
        assert_eq!(
            fs::read(candidate.join("payload.txt")).unwrap(),
            b"disposable publication bytes"
        );
        fs::remove_dir(&destination).unwrap();
        publish_directory_noreplace(&candidate, &destination, mapping_error).unwrap();
        assert!(!candidate.exists());
        assert!(expected_candidate.same_object(
            &platform_native::windows::files::open_no_reparse(&destination)
                .unwrap()
                .identity
        ));
        assert_eq!(
            fs::read(destination.join("payload.txt")).unwrap(),
            b"disposable publication bytes"
        );
    }

    #[test]
    fn junction_candidate_and_occupied_directory_are_retained_on_failure() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-directory-boundary-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let candidate = root.join("candidate");
        let destination = root.join("destination");
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join("payload"), b"retained disposable bytes").unwrap();
        let held = platform_native::windows::files::directory_file(&candidate).unwrap();
        assert!(publish_directory_noreplace(&candidate, &destination, mapping_error).is_err());
        assert_eq!(
            fs::read(candidate.join("payload")).unwrap(),
            b"retained disposable bytes"
        );
        assert!(!destination.exists());
        drop(held);
        let link = root.join("disposable-junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&candidate)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            publish_directory_noreplace(&link, &destination, mapping_error)
                .unwrap_err()
                .code,
            "UNSAFE_PATH"
        );
        assert_eq!(
            fs::read(candidate.join("payload")).unwrap(),
            b"retained disposable bytes"
        );
        fs::remove_dir(link).unwrap();
        publish_directory_noreplace(&candidate, &destination, mapping_error).unwrap();
        assert_eq!(
            fs::read(destination.join("payload")).unwrap(),
            b"retained disposable bytes"
        );
    }
}

#[cfg(windows)]
pub(crate) fn publish_directory_noreplace(
    from: &Path,
    to: &Path,
    directory_mapping_error: fn() -> ApiError,
) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    if from.parent().is_none()
        || from.file_name().is_none()
        || to.parent().is_none()
        || to.file_name().is_none()
    {
        return Err(directory_mapping_error());
    }
    platform_native::windows::files::rename_directory_noreplace(from, to).map_err(|error| {
        let unknown = error.kind() == std::io::ErrorKind::InvalidData;
        let result = ApiError::new("PROJECT_DIRECTORY_PUBLISH_FAILED", if unknown {
            "The directory publication identity is unconfirmed; retained paths require inspection"
        } else {
            "The project candidate was not published; existing directories and the candidate were retained"
        }).detail("reason", error.to_string()).detail("rawOsError", error.raw_os_error());
        if unknown { result.outcome("unknown") } else { result }
    })
}

#[cfg(unix)]
pub(crate) fn publish_directory_noreplace(
    from: &Path,
    to: &Path,
    directory_mapping_error: fn() -> ApiError,
) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::unix::fs::OpenOptionsExt,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    let parent = |path: &Path| -> Result<File, ApiError> {
        let path = path.parent().ok_or_else(directory_mapping_error)?;
        check_no_symlinks(path, false)?;
        Ok(OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?)
    };
    let from_parent = parent(from)?;
    let to_parent = parent(to)?;
    let name = |path: &Path| -> Result<CString, ApiError> {
        CString::new(
            path.file_name()
                .ok_or_else(directory_mapping_error)?
                .as_bytes(),
        )
        .map_err(|_| directory_mapping_error())
    };
    let from_name = name(from)?;
    let to_name = name(to)?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            from_parent.as_raw_fd(),
            from_name.as_ptr(),
            to_parent.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            from_parent.as_raw_fd(),
            from_name.as_ptr(),
            to_parent.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err(ApiError::new(
        "PLATFORM_STAGE_UNAVAILABLE",
        "Atomic project directory publication is unavailable on this platform",
    ));
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Err(ApiError::new("PROJECT_DIRECTORY_PUBLISH_FAILED", "The project candidate was not published; existing directories and the candidate were retained")
            .detail("reason", error.to_string()).detail("rawOsError", error.raw_os_error()));
    }
    from_parent.sync_all()?;
    to_parent.sync_all()?;
    Ok(())
}
