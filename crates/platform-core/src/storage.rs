//! Native storage boundary; unfinished Windows operations fail before I/O.
#[cfg(unix)]
#[path = "storage/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "storage/windows.rs"]
mod windows;
#[cfg(unix)]
pub(crate) use unix::StorageManager;
#[cfg(windows)]
pub(crate) use windows::StorageManager;

pub(crate) fn configure_installation_scope(
    manager: &StorageManager,
    scope: Option<std::sync::Arc<crate::installation_scope::InstallationScope>>,
) {
    #[cfg(windows)]
    manager.set_installation_scope(scope);
    #[cfg(not(windows))]
    let _ = (manager, scope);
}
