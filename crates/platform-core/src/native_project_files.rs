//! Platform boundary for native delivery. Each implementation owns its native
//! publication/recovery guarantees; unsupported capabilities fail before I/O.
#[cfg(unix)]
#[path = "native_project_files/unix.rs"]
mod unix;
#[cfg(unix)]
pub(crate) use unix::publish_file;

#[cfg(windows)]
#[path = "native_project_files/windows.rs"]
mod windows;
#[cfg(windows)]
pub(crate) use windows::publish_file;
