//! Original Unix data-restore proof capture. Callers retain their error policy.
use crate::{
    file_identity::PersistedIdentity as FileIdentity, files,
    projects::data_restore_registration::FileProof, ApiError,
};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::unix::fs::MetadataExt, path::Path};

pub(super) fn read_proof(
    path: &Path,
    maximum: u64,
    keep: bool,
    invalid: fn() -> ApiError,
    changed: fn() -> ApiError,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    let parent = files::root(path.parent().ok_or_else(invalid)?)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let mut file = files::read(&parent, name)?;
    let before = file.metadata()?;
    if before.len() > maximum {
        return Err(invalid());
    }
    let identity = FileIdentity::from_file(&file).map_err(|_| invalid())?;
    let mut bytes = Vec::new();
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or_else(invalid)?;
        if total > maximum {
            return Err(invalid());
        }
        hash.update(&buffer[..n]);
        if keep {
            bytes.extend_from_slice(&buffer[..n]);
        }
    }
    let after = file.metadata()?;
    if total != before.len()
        || metadata_stamp(&before) != metadata_stamp(&after)
        || !identity.matches_path(path).map_err(|_| changed())?
    {
        return Err(changed());
    }
    Ok((
        FileProof {
            identity,
            sha256: format!("{:x}", hash.finalize()),
            bytes: total,
            mode: before.mode(),
        },
        bytes,
    ))
}
fn metadata_stamp(value: &fs::Metadata) -> (u64, u64, u32, u64, u64, i64, i64, i64, i64) {
    (
        value.dev(),
        value.ino(),
        value.mode(),
        value.nlink(),
        value.len(),
        value.mtime(),
        value.mtime_nsec(),
        value.ctime(),
        value.ctime_nsec(),
    )
}
