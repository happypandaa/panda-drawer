//! Native durable writer boundary; unfinished Windows operations fail before I/O.
#[cfg(unix)]
#[path = "durable_writer/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "durable_writer/windows.rs"]
mod windows;
#[cfg(unix)]
pub(crate) use unix::{require_all_clear, require_owner_clear, WriterGuard, WriterOwner};
#[cfg(windows)]
pub(crate) use windows::{require_all_clear, require_owner_clear, WriterGuard, WriterOwner};
