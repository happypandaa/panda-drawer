//! Native evidence and exclusive slot moves; original Unix policy is unchanged.
use super::*;
#[cfg(unix)]
#[path = "project_git_data_restore/files_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "project_git_data_restore/files_windows.rs"]
mod platform;
pub(super) use platform::*;
