//! Source admission is identity checked and never grants ownership from a path.
use crate::ApiError;
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};
static ADMISSION: OnceLock<Mutex<()>> = OnceLock::new();
type Result<T> = std::result::Result<T, ApiError>;
pub(crate) fn with_source_ownership_admission<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = ADMISSION
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| ApiError::new("APP_SOURCE_BUSY", "源码登记锁不可用。"))?;
    work()
}
pub(crate) fn require_new_web_project_location(data: &Path, source: &Path) -> Result<()> {
    crate::projects::check_no_symlinks(source, true)?;
    crate::project_tool_boundary::require_source_excludes_global_tools(data, source)?;
    crate::web_project_ownership::WebProjectOwnership::require_source_unoccupied(data, source)
}
pub(crate) fn require_delivery_location_unclaimed(data: &Path, source: &Path) -> Result<()> {
    // A delivery parent may contain an unrelated application's sibling. Only
    // an actual owned ancestor excludes this location; reverse overlap is a
    // source-admission rule and would incorrectly reject common ancestors.
    crate::web_project_ownership::WebProjectOwnership::new(data.into(), Default::default())
        .require_global_tool_unowned(source)
}

#[cfg(test)]
pub(crate) fn claim_source_after_verified_migration(
    data: &Path,
    workspace: &Path,
    id: &str,
    source: &Path,
) -> Result<()> {
    with_source_ownership_admission(|| {
        require_new_web_project_location(data, source)?;
        let owner =
            crate::web_project_ownership::WebProjectOwnership::new(data.into(), workspace.into());
        owner.initialize(id)?;
        owner.record_user_root(id, source)
    })
}
