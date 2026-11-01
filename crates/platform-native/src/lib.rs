//! Native operating-system primitives used by the platform adaptation layer.
//! This crate never discovers or initializes an application data directory.

pub mod app_listener;
pub mod directory;
pub mod filesystem;
pub mod process_status;
pub mod runtime_logs;
pub mod tool_files;

#[cfg(windows)]
pub mod windows;
