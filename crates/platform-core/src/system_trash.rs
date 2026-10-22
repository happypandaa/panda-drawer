//! The operating system owns trashed files. Never fall back to permanent
//! deletion, a hand-made ~/.Trash move, or a platform-owned recovery directory.
use crate::ApiError;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

pub(crate) fn ensure_supported() -> Result<(), ApiError> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(ApiError::new(
            "SYSTEM_TRASH_UNSUPPORTED",
            "当前系统尚未接入系统废纸篓，应用和文件已保留。",
        ))
    }
}

/// Validate only the exact location returned by the OS. Opening each ancestor
/// for reading would ask to enumerate the protected system Trash and can fail
/// under TCC even though this application's own moved directory is accessible.
/// Source authorization still uses the stricter component-by-component capture.
pub(crate) fn moved_directory_matches(
    expected: &crate::file_identity::PersistedIdentity,
    path: &Path,
) -> Result<bool, ApiError> {
    use std::{
        fs::OpenOptions,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    crate::projects::check_no_symlinks(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)?;
    let observed = crate::file_identity::PersistedIdentity::from_file(&file)
        .map_err(|error| ApiError::new("SYSTEM_TRASH_IDENTITY_ERROR", error.to_string()))?;
    crate::projects::check_no_symlinks(path, false)?;
    let current = std::fs::symlink_metadata(path)?;
    Ok(current.is_dir()
        && !current.file_type().is_symlink()
        && current.dev() == observed.device
        && current.ino() == observed.inode
        && expected.matches_captured(&observed))
}

/// Call only with a platform-owned, exclusively held application directory.
/// Callers persist their intent first, because a crash can occur between the
/// OS move and recording the returned location. A missing source is never by
/// itself evidence of a successful move.
#[cfg(test)]
fn move_directory(path: &Path) -> Result<std::path::PathBuf, ApiError> {
    move_directory_inner(path, None)
}

pub(crate) fn move_directory_with_identity(
    path: &Path,
    expected: &crate::file_identity::PersistedIdentity,
) -> Result<std::path::PathBuf, ApiError> {
    move_directory_inner(path, Some(expected))
}

fn move_directory_inner(
    path: &Path,
    expected: Option<&crate::file_identity::PersistedIdentity>,
) -> Result<std::path::PathBuf, ApiError> {
    ensure_supported()?;
    if !path.is_absolute() || path.parent().is_none() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "只能将完整的应用目录移入系统废纸篓。",
        ));
    }
    crate::projects::check_no_symlinks(path, false)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "应用目录不是普通文件夹，已保留现场。",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let observed = crate::file_identity::PersistedIdentity::capture(path)
            .map_err(|error| ApiError::new("UNSAFE_PATH", error.to_string()))?;
        if expected.is_some_and(|expected| !expected.matches_captured(&observed)) {
            return Err(ApiError::new(
                "SYSTEM_TRASH_IDENTITY_CHANGED",
                "应用目录在确认后已被替换，本次没有移入废纸篓。请重新核对。",
            )
            .outcome("not_started"));
        }
        let destination = native_move(path)?;
        let destination_identity = moved_directory_matches(&observed, &destination);
        let source_missing = !path.try_exists().unwrap_or(true);
        let confirmed =
            source_missing && destination_identity.as_ref().is_ok_and(|matches| *matches);
        if !confirmed {
            return Err(ApiError::new(
                "SYSTEM_TRASH_RESULT_UNCONFIRMED",
                "系统已返回废纸篓位置，但文件身份或原目录状态尚未确认。请核对保留记录，不要重复删除。",
            )
            .detail("originalPath", serde_json::json!(path))
            .detail("systemTrashPath", serde_json::json!(destination))
            .detail("expectedIdentity", serde_json::json!(observed))
            .detail("destinationIdentityMatches", serde_json::json!(destination_identity.as_ref().ok()))
            .detail("identityError", destination_identity.err().map(|error|error.to_string()))
            .detail("sourceMissing", source_missing)
            .outcome("unknown"));
        }
        Ok(destination)
    }
    #[cfg(not(target_os = "macos"))]
    unreachable!("ensure_supported rejects this platform")
}

#[cfg(target_os = "macos")]
fn native_move(path: &Path) -> Result<PathBuf, ApiError> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSFileManager, NSURL};
    use std::{
        ffi::{CStr, CString, OsStr},
        os::unix::ffi::OsStrExt,
        ptr::NonNull,
    };

    let representation = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ApiError::new("UNSAFE_PATH", "应用目录包含无效字符。"))?;
    autoreleasepool(|_| {
        // NSURL copies the nul-terminated file-system representation. The
        // CString stays alive until the synchronous Foundation call completes.
        let url = unsafe {
            NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                NonNull::new(representation.as_ptr().cast_mut()).unwrap(),
                true,
                None,
            )
        };
        let mut moved_url = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut moved_url))
            .map_err(|error| {
                ApiError::new(
                    "SYSTEM_TRASH_FAILED",
                    format!(
                        "移入系统废纸篓失败：{}。未执行永久删除。",
                        error.localizedDescription()
                    ),
                )
                .detail("originalPath", serde_json::json!(path))
            })?;
        let moved_url = moved_url.ok_or_else(|| {
            ApiError::new(
                "SYSTEM_TRASH_RESULT_UNCONFIRMED",
                "系统没有返回废纸篓位置，请核对原目录与系统废纸篓，不要重复删除。",
            )
            .detail("originalPath", serde_json::json!(path))
            .outcome("unknown")
        })?;
        // The representation belongs to moved_url and is copied before it is
        // released. No lossy UTF-8 conversion of user file names is needed.
        let bytes = unsafe { CStr::from_ptr(moved_url.fileSystemRepresentation().as_ptr()) };
        Ok(PathBuf::from(OsStr::from_bytes(bytes.to_bytes())))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_moved_directory_check_rejects_replaced_or_linked_paths() {
        use std::{fs, os::unix::fs::symlink};
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-source");
        let moved = root.join("disposable-moved");
        fs::create_dir(&source).unwrap();
        let expected = crate::file_identity::PersistedIdentity::capture(&source).unwrap();
        fs::rename(&source, &moved).unwrap();
        assert!(moved_directory_matches(&expected, &moved).unwrap());
        let retained = root.join("retained-original");
        fs::rename(&moved, &retained).unwrap();
        fs::create_dir(&moved).unwrap();
        assert!(!moved_directory_matches(&expected, &moved).unwrap());
        fs::rename(&moved, root.join("retained-replacement")).unwrap();
        symlink(&retained, &moved).unwrap();
        assert!(moved_directory_matches(&expected, &moved).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn refuses_non_application_roots_and_links_without_moving_files() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let app = root.join("disposable-app");
        std::fs::create_dir(&app).unwrap();
        let link = root.join("link");
        symlink(&app, &link).unwrap();
        assert!(move_directory(Path::new("/")).is_err());
        assert!(move_directory(Path::new("relative")).is_err());
        assert!(move_directory(&link).is_err());
        assert!(move_directory(&root.join("absent")).is_err());
        let file = root.join("file");
        std::fs::write(&file, "keep").unwrap();
        assert!(move_directory(&file).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
        assert!(app.is_dir());
    }

    /// Explicit only: creates one disposable folder, moves it using Foundation,
    /// and leaves its verified payload in the system Trash as local evidence.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "moves a newly created disposable fixture into the real macOS Trash"]
    fn native_disposable_system_trash_smoke() {
        let temporary = tempfile::tempdir().unwrap();
        let app = temporary.path().canonicalize().unwrap().join(format!(
            "Panda disposable 删除验证 {}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(app.join("data")).unwrap();
        std::fs::create_dir_all(app.join(".git")).unwrap();
        std::fs::write(app.join("data/fixture.txt"), "disposable-data").unwrap();
        std::fs::write(app.join(".git/HEAD"), "disposable-git-fixture").unwrap();
        let destination = move_directory(&app).unwrap();
        assert!(!app.exists());
        assert_eq!(
            std::fs::read_to_string(destination.join("data/fixture.txt")).unwrap(),
            "disposable-data"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join(".git/HEAD")).unwrap(),
            "disposable-git-fixture"
        );
        eprintln!(
            "disposable native system Trash evidence: {}",
            destination.display()
        );
    }
}
