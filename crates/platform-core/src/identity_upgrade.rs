//! Platform boundary for the existing offline Mac volume-identity repair.
//! Windows requires complete native identities and does not adopt old records.
#[cfg(unix)]
#[path = "identity_upgrade/unix.rs"]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{apply, legacy_identity_matches, preview};

#[cfg(windows)]
fn unavailable() -> anyhow::Error {
    crate::ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 不兼容旧实验数据，不能通过 Mac 卷身份修复采用旧记录。",
    )
    .detail("platform", "windows")
    .detail("capability", "legacy-volume-identity-repair")
    .outcome("not_started")
    .into()
}
#[cfg(windows)]
pub(crate) fn legacy_identity_matches(_: &std::path::Path, _: u64, _: u64) -> anyhow::Result<bool> {
    Err(unavailable())
}
#[cfg(windows)]
pub(crate) fn preview(
    _: &crate::RuntimePaths,
    _: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    Err(unavailable())
}
#[cfg(windows)]
pub(crate) fn apply(
    _: &crate::RuntimePaths,
    _: Option<&str>,
    _: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    Err(unavailable())
}
