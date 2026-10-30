//! Filesystem boundary for lifecycle operations. No workspace or delivery is
//! adopted by a directory type check; persistent identity remains separate.
use crate::{file_identity::PersistedIdentity, path_checks::check_no_symlinks, ApiError};
use std::path::Path;

#[cfg(unix)]
#[path = "lifecycle_file_io/unix.rs"]
mod unix;

pub(crate) fn require_directory(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Expected an ordinary project directory",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportPublicationPoint {
    BeforeRename,
    Renamed,
}

pub(crate) fn publish_export_file_with(
    stage: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
    checkpoint: impl FnMut(ExportPublicationPoint) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        unix::publish_export_file_with(stage, destination, expected, checkpoint)
    }
    #[cfg(windows)]
    {
        let _ = (stage, destination, expected, checkpoint);
        Err(ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 暂不支持此功能：外部导出的独占发布与中断恢复尚未完成。",
        )
        .detail("platform", "windows")
        .detail("capability", "external-export-publication")
        .outcome("not_started"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unfinished_publication_never_moves_or_overwrites_disposable_delivery_files() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-closed-export-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let stage = root.join("stage.zip");
        let destination = root.join("existing.zip");
        fs::write(&stage, b"disposable stand-in; not a ZIP").unwrap();
        fs::write(&destination, b"retained disposable destination").unwrap();
        let expected = PersistedIdentity::capture(&stage).unwrap();
        let error = publish_export_file_with(&stage, &destination, &expected, |_| {
            panic!("closed publication must not invoke callbacks")
        })
        .unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["operationOutcome"], "not_started");
        assert!(expected.matches_path(&stage).unwrap());
        assert_eq!(fs::read(stage).unwrap(), b"disposable stand-in; not a ZIP");
        assert_eq!(
            fs::read(destination).unwrap(),
            b"retained disposable destination"
        );
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
    }
}
