//! Native operating-system primitives used by the platform adaptation layer.
//! This crate never discovers or initializes an application data directory.

#[cfg(windows)]
pub mod windows;
