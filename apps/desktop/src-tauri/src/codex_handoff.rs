//! Native installed-editor dispatch; an unfinished integration never launches a fallback.
#[cfg(target_os = "macos")]
include!("codex_handoff/macos.rs");
#[cfg(windows)]
#[path = "codex_handoff/windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::{installed_app, open};
#[cfg(not(any(target_os = "macos", windows)))]
mod unavailable;
#[cfg(not(any(target_os = "macos", windows)))]
pub use unavailable::{installed_app, open};
