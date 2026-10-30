//! Platform boundary for identity-bound observation of owned Node processes.
//! macOS retains process-group/nonce recovery. Windows retains full native
//! creation times and observes old markers without acquiring numeric-PID kills.
#[cfg(not(windows))]
#[path = "node_process_identity/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "node_process_identity/windows.rs"]
mod windows;

#[cfg(all(test, not(windows)))]
pub(crate) use unix::capture;
#[cfg(not(windows))]
pub(crate) use unix::{
    boot_time, capture_owned, observe, stop, Observation, RuntimeProcessIdentity, RUN_IDENTITY_ENV,
};
#[cfg(windows)]
pub(crate) use windows::{
    boot_time, capture_owned, observe, stop, Observation, RuntimeProcessIdentity, RUN_IDENTITY_ENV,
};
