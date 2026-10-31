//! Admission for unfinished native import work. Existing request/status records
//! remain readable; these checks never recover, accept or mutate material.
use crate::ApiError;

pub(super) fn require_start<T>(sources: &Result<T, ApiError>) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        sources.as_ref().map(|_| ()).map_err(Clone::clone)
    }
    #[cfg(not(windows))]
    {
        // Preserve the original Mac sequence, including its deferred source
        // error after task acceptance. A source error is not a new Mac policy.
        let _ = sources;
        Ok(())
    }
}

pub(super) fn require_recovery() -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        Err(ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 暂不支持此功能：Git 添加的中断恢复与清理尚未完成，现有记录和材料已保留。",
        )
        .detail("platform", "windows")
        .detail("capability", "node-git-import-recovery")
        .detail("accepted", false)
        .outcome("not_started"))
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn new_import_preserves_the_actual_source_failure_without_accepting_a_task() {
        let mut issue = ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "retained source capability failure",
        )
        .detail("platform", "windows")
        .detail("capability", "non-executing-source-acquisition")
        .detail("retainedMaterial", "owned source evidence")
        .outcome("not_started");
        issue.retryable = true;
        let expected = serde_json::to_value(&issue).unwrap();
        let sources = Err::<(), _>(issue);
        assert_eq!(
            serde_json::to_value(require_start(&sources).unwrap_err()).unwrap(),
            expected
        );
        let Err(retained) = sources else {
            panic!("The original source failure must remain retained");
        };
        assert_eq!(serde_json::to_value(retained).unwrap(), expected);
    }

    #[test]
    fn recovery_is_explicitly_unavailable_without_claiming_an_empty_recovered_list() {
        let issue = require_recovery().unwrap_err();
        assert_eq!(issue.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(issue.details["platform"], "windows");
        assert_eq!(issue.details["capability"], "node-git-import-recovery");
        assert_eq!(issue.details["operationOutcome"], "not_started");
        assert_eq!(issue.details["accepted"], false);
        assert!(issue.message.contains("现有记录和材料已保留"));
        assert!(issue.details.get("recovered").is_none());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn mac_source_errors_remain_deferred_and_recovery_keeps_its_original_entry() {
        let sources = Err::<(), _>(ApiError::new("SOURCE_RETAINED", "retained source evidence"));
        require_start(&sources).unwrap();
        require_recovery().unwrap();
    }
}
