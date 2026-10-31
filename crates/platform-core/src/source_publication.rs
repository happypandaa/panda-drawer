//! Native editable-source publication and identity-bound interruption recovery.
#[cfg(any(unix, windows))]
use super::*;
#[cfg(unix)]
#[path = "source_publication/unix.rs"]
mod unix;

#[cfg(windows)]
#[path = "source_publication/windows.rs"]
mod windows;
