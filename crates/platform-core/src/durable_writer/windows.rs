//! Closed until durable markers are connected to actual Windows Job/I/O
//! ownership. A guessed boot UUID or elapsed delay cannot prove writer closure.
use crate::ApiError;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：持久写入进程归属核对尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "durable-process-writer")
    .outcome("not_started")
}

#[derive(Clone)]
pub(crate) enum WriterOwner {}
pub(crate) enum WriterGuard {}
impl WriterOwner {
    pub(crate) fn node(_: &Path, _: &str) -> Result<Self, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn web(_: &Path, _: &str) -> Result<Self, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn platform(_: &Path) -> Result<Self, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn begin(&self, _: &str) -> Result<WriterGuard, ApiError> {
        Err(unavailable())
    }
    #[cfg(test)]
    pub(crate) fn fixture_mark_unconfirmed(&self) -> Result<PathBuf, ApiError> {
        Err(unavailable())
    }
}
impl WriterGuard {
    pub(crate) fn finish(&mut self) -> Result<(), ApiError> {
        match *self {}
    }
}
pub(crate) fn require_owner_clear(_: &WriterOwner) -> Result<(), ApiError> {
    Err(unavailable())
}
pub(crate) fn require_all_clear(_: &Path) -> Result<(), ApiError> {
    Err(unavailable())
}
