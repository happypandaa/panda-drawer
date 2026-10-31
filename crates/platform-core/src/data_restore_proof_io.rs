//! Native proof capture boundary; each host persists its actual permission facts.
use crate::ApiError;
use std::path::Path;
#[cfg(unix)]
#[path = "data_restore_proof_io/unix_proof.rs"]
mod proof;
#[cfg(windows)]
#[path = "data_restore_proof_io/windows_proof.rs"]
mod proof;
#[cfg(unix)]
#[path = "data_restore_proof_io/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "data_restore_proof_io/windows.rs"]
mod windows;
pub(crate) use proof::FileProof;

pub(crate) fn read_proof(
    path: &Path,
    maximum: u64,
    keep: bool,
    invalid: fn() -> ApiError,
    changed: fn() -> ApiError,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    #[cfg(unix)]
    {
        unix::read_proof(path, maximum, keep, invalid, changed)
    }
    #[cfg(windows)]
    {
        windows::read_proof(path, maximum, keep, invalid, changed)
    }
}
