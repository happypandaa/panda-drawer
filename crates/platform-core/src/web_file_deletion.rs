//! Native web file deletion boundary; unfinished Windows operations fail before I/O.
#[cfg(unix)]
#[path = "web_file_deletion/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "web_file_deletion/windows.rs"]
mod windows;
#[cfg(all(test, unix))]
pub(crate) use unix::{fixture_remove_after_capture, FileDeletionState};
#[cfg(unix)]
pub(crate) use unix::{owned_scopes, remove_owned_file, FileDeletionPlan};
#[cfg(all(test, windows))]
pub(crate) use windows::{fixture_remove_after_capture, FileDeletionState};
#[cfg(windows)]
pub(crate) use windows::{owned_scopes, remove_owned_file, FileDeletionPlan};
