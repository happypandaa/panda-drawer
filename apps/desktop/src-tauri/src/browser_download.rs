//! Platform-specific browser download admission, staging, and publication.
//! Windows stays closed until native writer ownership and delivery are ready.

// Keep the original Unix implementation and tests at their existing module level.
#[cfg(unix)]
include!("browser_download/unix.rs");

#[cfg(windows)]
#[path = "browser_download/windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::handler;
