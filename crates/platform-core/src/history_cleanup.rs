//! Native cleanup boundary. Unfinished Windows removal fails before I/O.
#[cfg(unix)]
#[path = "history_cleanup/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "history_cleanup/windows.rs"]
mod windows;
#[cfg(unix)]
pub(crate) use unix::{
    remove_owned_directory, remove_owned_directory_checked, retired_cleanup_status, CleanupManager,
};
#[cfg(windows)]
pub(crate) use windows::{
    remove_owned_directory, remove_owned_directory_checked, retired_cleanup_status, CleanupManager,
};
