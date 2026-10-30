//! Native local-directory onboarding. Windows cannot inspect or initialize a
//! directory until its actual descriptor/handle guarantees are implemented.
#[cfg(unix)]
use super::*;
#[cfg(unix)]
#[path = "node_app_directory/unix.rs"]
mod unix;
#[cfg(unix)]
pub use unix::DirectoryInitializeInput;

#[cfg(unix)]
pub(super) fn stale() -> crate::ApiError {
    unix::stale()
}
#[cfg(windows)]
pub(super) fn stale() -> crate::ApiError {
    crate::ApiError::new(
        "NODE_DIRECTORY_CHANGED",
        "目录或应用声明已变化，请重新检查并确认后再继续。",
    )
    .outcome("not_started")
}
#[cfg(windows)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryInitializeInput {
    pub expected_revision: String,
    pub name: String,
}
#[cfg(windows)]
fn unavailable() -> crate::ApiError {
    crate::ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：本机目录检查、初始化与登记尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "local-directory-onboarding")
    .outcome("not_started")
}
#[cfg(windows)]
impl super::NodeApps {
    pub fn inspect_directory(
        &self,
        _: &std::path::Path,
    ) -> Result<serde_json::Value, crate::ApiError> {
        Err(unavailable())
    }
    pub fn initialize_directory(
        &self,
        _: &std::path::Path,
        _: DirectoryInitializeInput,
    ) -> Result<serde_json::Value, crate::ApiError> {
        Err(unavailable())
    }
    pub async fn initialize_and_register_directory(
        &self,
        _: std::path::PathBuf,
        _: DirectoryInitializeInput,
    ) -> Result<serde_json::Value, crate::ApiError> {
        Err(unavailable())
    }
    pub async fn register_directory(
        &self,
        _: std::path::PathBuf,
        _: String,
    ) -> Result<serde_json::Value, crate::ApiError> {
        Err(unavailable())
    }
}
