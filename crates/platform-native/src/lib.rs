//! Native operating-system primitives used by the platform adaptation layer.
//! This crate never discovers or initializes an application data directory.

pub mod filesystem;

#[cfg(windows)]
pub mod windows;
