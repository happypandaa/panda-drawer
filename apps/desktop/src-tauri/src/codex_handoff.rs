//! Native installed-editor dispatch; an unfinished integration never launches a fallback.
#[cfg(target_os = "macos")]
include!("codex_handoff/macos.rs");
#[cfg(not(target_os = "macos"))]
mod unavailable;
#[cfg(not(target_os = "macos"))]
pub use unavailable::{installed_app, open};
