//! Native proof capture boundary; a closed Windows path never fabricates mode.
use crate::{projects::data_restore_registration::FileProof, ApiError};
use std::path::Path;
#[cfg(unix)]
#[path = "data_restore_proof_io/unix.rs"]
mod unix;

pub(crate) fn read_proof(
    path: &Path,
    maximum: u64,
    keep: bool,
    invalid: fn() -> ApiError,
    changed: fn() -> ApiError,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    #[cfg(unix)]
    {
        unix::read_proof(path, maximum, keep, invalid, changed)
    }
    #[cfg(windows)]
    {
        let _ = (path, maximum, keep, invalid, changed);
        Err(ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 暂不支持此功能：数据恢复登记的原生不可变证明尚未完成。",
        )
        .detail("platform", "windows")
        .detail("capability", "data-restore-registration-proof")
        .outcome("not_started"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn must_not_call() -> ApiError {
        panic!("closed proof capture must not invoke native validation callbacks")
    }

    #[test]
    fn unfinished_proof_capture_keeps_disposable_evidence_without_creating_missing_inputs() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-closed-data-proof-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("retained.json");
        std::fs::write(&path, b"disposable evidence; not an authorized proof").unwrap();
        let before = crate::file_identity::PersistedIdentity::capture(&path).unwrap();
        for keep in [false, true] {
            let error = read_proof(&path, 1024, keep, must_not_call, must_not_call).unwrap_err();
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
        }
        let missing = root.join("missing.json");
        assert!(read_proof(&missing, 1024, false, must_not_call, must_not_call).is_err());
        assert!(!missing.exists());
        assert!(before.matches_path(&path).unwrap());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"disposable evidence; not an authorized proof"
        );
    }
}
