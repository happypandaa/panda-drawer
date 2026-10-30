//! Native operating-system primitives used by the platform adaptation layer.
//! This crate never discovers or initializes an application data directory.

pub mod directory;
pub mod filesystem;
pub mod runtime_logs;

#[cfg(windows)]
pub mod windows;
