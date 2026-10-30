//! Shared Git API with native transaction implementations.
#[path = "project_git/types.rs"]
mod types;
pub use types::*;

// Include at this module level to preserve native test/fixture paths and the
// existing private parent-child visibility of the Unix transaction modules.
#[cfg(unix)]
include!("project_git/unix.rs");
#[cfg(windows)]
include!("project_git/windows.rs");
