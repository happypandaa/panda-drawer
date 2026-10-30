//! Workspace filesystem policy at the native boundary. No Windows migration
//! is admitted until volume checks, copies and recovery have been implemented.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
    pub(crate) digest: Option<String>,
    pub(crate) target: Option<PathBuf>,
    pub(crate) mode: u32,
}

#[cfg(unix)]
#[path = "workspace_file_io/unix.rs"]
mod unix;

pub(crate) fn filesystem(path: &Path) -> Result<Value> {
    #[cfg(unix)]
    {
        unix::filesystem(path)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Err(unavailable().into())
    }
}
pub(crate) fn require_unsynced(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        unix::require_unsynced(path)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Err(unavailable().into())
    }
}
pub(crate) fn require_supported_filesystem(info: &Value) -> Result<()> {
    #[cfg(unix)]
    {
        unix::require_supported_filesystem(info)
    }
    #[cfg(windows)]
    {
        let _ = info;
        Err(unavailable().into())
    }
}
pub(crate) fn collect(root: &Path, excluded: &BTreeSet<PathBuf>) -> Result<Vec<Entry>> {
    #[cfg(unix)]
    {
        unix::collect(root, excluded)
    }
    #[cfg(windows)]
    {
        let _ = (root, excluded);
        Err(unavailable().into())
    }
}
pub(crate) fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    #[cfg(unix)]
    {
        unix::copy_entries(source, target, entries)
    }
    #[cfg(windows)]
    {
        let _ = (source, target, entries);
        Err(unavailable().into())
    }
}
pub(crate) fn append_phase_log(path: &Path, phase: &str) -> Result<(), crate::ApiError> {
    #[cfg(unix)]
    {
        unix::append_phase_log(path, phase)
    }
    #[cfg(windows)]
    {
        let _ = (path, phase);
        Err(unavailable())
    }
}
#[cfg(windows)]
fn unavailable() -> crate::ApiError {
    crate::ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：工作区原生卷核对、迁移复制与恢复尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "workspace-native-migration")
    .outcome("not_started")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unfinished_workspace_io_does_not_copy_create_or_append_to_disposable_material() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-closed-workspace-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("retained.txt"), b"retained disposable source").unwrap();
        let before = crate::file_identity::PersistedIdentity::capture(&source).unwrap();
        let target = root.join("candidate");
        for error in [
            filesystem(&source).unwrap_err(),
            require_unsynced(&source).unwrap_err(),
            require_supported_filesystem(&Value::Null).unwrap_err(),
            collect(&source, &BTreeSet::new()).unwrap_err(),
            copy_entries(&source, &target, &[]).unwrap_err(),
        ] {
            let error = error.downcast_ref::<crate::ApiError>().unwrap();
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
        }
        assert!(!target.exists());
        assert!(before.matches_path(&source).unwrap());
        assert_eq!(
            fs::read(source.join("retained.txt")).unwrap(),
            b"retained disposable source"
        );
        let log = root.join("log.txt");
        fs::write(&log, b"retained disposable phase log").unwrap();
        assert!(append_phase_log(&log, "must not append").is_err());
        assert_eq!(fs::read(&log).unwrap(), b"retained disposable phase log");
        let missing = root.join("missing-log.txt");
        assert!(append_phase_log(&missing, "must not create").is_err());
        assert!(!missing.exists());
    }
}
