//! The original bounded proof reader, using retained native parent handles and
//! real FullId/length/link-count/mtime/ChangeTime/read-only observations.
use super::FileProof;
use crate::{file_identity::PersistedIdentity as FileIdentity, files, ApiError};
use platform_native::{tool_files::Facts, windows::files as native};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

struct MetadataStamp {
    facts: Facts,
    read_only: bool,
}
impl MetadataStamp {
    fn from_file(file: &File) -> Result<Self, ApiError> {
        Ok(Self {
            facts: Facts::from_file(file)?,
            read_only: file.metadata()?.permissions().readonly(),
        })
    }
    fn matches(&self, other: &Self) -> bool {
        // Facts uses actual LastWriteTime and ChangeTime. Windows creation time
        // is not Unix ctime. Do not add unrelated Windows attribute bits to the
        // original mode policy: the native permission counterpart is readOnly.
        self.facts.same_modified_and_changed(&other.facts)
            && self.facts.links() == other.facts.links()
            && self.facts.is_file() == other.facts.is_file()
            && self.facts.is_dir() == other.facts.is_dir()
            && self.facts.is_reparse() == other.facts.is_reparse()
            && self.read_only == other.read_only
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    AfterOpen,
    AfterRead,
}
pub(super) fn read_proof(
    path: &Path,
    maximum: u64,
    keep: bool,
    invalid: fn() -> ApiError,
    changed: fn() -> ApiError,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    read_proof_with(path, maximum, keep, invalid, changed, |_, _| Ok(()))
}
fn read_proof_with(
    path: &Path,
    maximum: u64,
    keep: bool,
    invalid: fn() -> ApiError,
    changed: fn() -> ApiError,
    mut observe: impl FnMut(Checkpoint, &File) -> Result<(), ApiError>,
) -> Result<(FileProof, Vec<u8>), ApiError> {
    let parent = files::root(path.parent().ok_or_else(invalid)?)?;
    // File alone pins this parent. Retain its actual ancestor scope throughout
    // the read and the final named identity check, like descriptor traversal.
    let (_parent_path, _parent_scope) = native::directory_scope(&parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let mut file = files::read(&parent, name)?;
    let before = MetadataStamp::from_file(&file)?;
    if before.facts.len() > maximum {
        return Err(invalid());
    }
    let identity = FileIdentity::from_file(&file).map_err(|_| invalid())?;
    observe(Checkpoint::AfterOpen, &file)?;
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
    observe(Checkpoint::AfterRead, &file)?;
    let after = MetadataStamp::from_file(&file)?;
    if total != before.facts.len()
        || !before.matches(&after)
        || !identity.matches_path(path).map_err(|_| changed())?
    {
        return Err(changed());
    }
    Ok((
        FileProof {
            identity,
            sha256: format!("{:x}", hash.finalize()),
            bytes: total,
            read_only: before.read_only,
        },
        bytes,
    ))
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
