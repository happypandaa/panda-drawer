//! Initial/reselected workspace policy, separate from migration-target sync
//! admission. The existing Mac startup policy adds no volume restriction here.
use anyhow::Result;
use std::path::Path;

pub(super) fn require_volume(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use anyhow::Context;
        platform_native::windows::workspace::require_workspace_volume(path)
            .with_context(|| format!("Workspace volume is not admitted: {}", path.display()))?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}
