//! Closed until Windows disk/file snapshots and non-executing Git acquisition
//! are connected. Shared request data is not a ready-source or lease receipt.
use super::{AcquireSourceInput, SourceCandidate};
use crate::{
    builds::BuildManager, git_environment::GitEnvironmentManager, ApiError, ProjectStore,
    RuntimePaths,
};
use std::sync::Arc;
use tokio::sync::watch;

#[path = "../source_publication.rs"]
mod publication;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：源码获取的文件核对与受控复制尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "non-executing-source-acquisition")
    .outcome("not_started")
}

pub enum SourceAcquisitionManager {}
impl SourceAcquisitionManager {
    pub fn new(
        _: RuntimePaths,
        _: Arc<ProjectStore>,
        _: Arc<BuildManager>,
        _: GitEnvironmentManager,
    ) -> Result<Arc<Self>, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn verify_ready_leased(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<SourceCandidate, ApiError> {
        Err(unavailable())
    }
    pub(crate) fn owner_app_id(&self, _: &str) -> Result<String, ApiError> {
        Err(unavailable())
    }
    pub(crate) async fn acquire_for_import(
        &self,
        _: AcquireSourceInput,
        _: &str,
        _: &str,
        _: &str,
        _: watch::Receiver<bool>,
        _: watch::Receiver<bool>,
    ) -> Result<SourceCandidate, ApiError> {
        Err(unavailable())
    }
    #[cfg(test)]
    pub(crate) fn pause_import_writer(
        &self,
        _: tokio::sync::oneshot::Sender<()>,
        _: std::sync::mpsc::Receiver<()>,
    ) {
        match *self {}
    }
}
