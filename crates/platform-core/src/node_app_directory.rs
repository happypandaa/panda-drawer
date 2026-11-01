//! Native local-directory onboarding with the same preview and ownership policy.
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
#[path = "node_app_directory/windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::DirectoryInitializeInput;
#[cfg(windows)]
pub(super) fn stale() -> crate::ApiError {
    windows::stale()
}

#[cfg(test)]
#[path = "node_app_directory/web_tests.rs"]
mod web_tests;
