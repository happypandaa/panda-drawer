//! Native editable-source publication. An unfinished implementation cannot
//! publish a candidate, retain ownership or fabricate a committed receipt.
#[cfg(unix)]
use super::*;
#[cfg(unix)]
#[path = "source_publication/unix.rs"]
mod unix;

#[cfg(windows)]
impl super::SourceAcquisitionManager {
    pub(crate) fn publish_author_source(
        &self,
        _: &super::SourceCandidate,
        _: &str,
        _: &str,
    ) -> Result<std::path::PathBuf, crate::ApiError> {
        Err(crate::ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 暂不支持此功能：源码目录发布与中断恢复尚未完成。",
        )
        .detail("platform", "windows")
        .detail("capability", "editable-source-publication")
        .outcome("not_started"))
    }
}
