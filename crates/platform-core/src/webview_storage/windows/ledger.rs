//! Browser exit is supplied by the trusted desktop caller. The durable clear
//! receipt retains that fact while the exact UDF and then the receipt retire.
//! A receipt is not a successful-deletion result, nor authority over a new root.
use super::super::{
    invalid, project_identifier, validate, ProjectKind, Result, StoreLedger, StoreRecord,
};
use crate::ApiError;
use platform_native::windows::{files as native, removal};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

const MAX_RECORD: u64 = 4096;
const SHARE_READ_DELETE: u32 = 0x0000_0001 | 0x0000_0004;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

fn pending(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "WEBVIEW_STORE_CLEANUP_UNCONFIRMED",
        "浏览器存储清理尚未完成，已保留原清理凭据，请解除占用后重试。",
    )
    .detail("remainingPath", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
}
fn retryable(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

struct RecordFile {
    record: StoreRecord,
    bytes: Vec<u8>,
    identity: native::FileIdentity,
}
fn read_file(mut file: &File, kind: ProjectKind, id: &str) -> Result<RecordFile> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || metadata.len() > MAX_RECORD
    {
        return Err(invalid());
    }
    let identity = native::identity(file)?;
    if identity.directory {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    (&mut file).take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD as usize {
        return Err(invalid());
    }
    let record: StoreRecord = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if record.version != 1
        || record.kind != kind
        || record.id != id
        || record.identifier != project_identifier(kind, id)?
    {
        return Err(invalid());
    }
    Ok(RecordFile {
        record,
        bytes,
        identity,
    })
}
fn open_record(path: &Path, kind: ProjectKind, id: &str) -> Result<Option<RecordFile>> {
    // The original ledger permits ordinary hard-linked records. No ACL repair,
    // owner audit or single-link policy is added to its read boundary.
    let file = match native::open_read_file_allowing_hard_links(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let selected = read_file(&file, kind, id)?;
    let named = native::open_no_reparse(path)?;
    if !selected.identity.same_object(&named.identity) {
        return Err(invalid());
    }
    Ok(Some(selected))
}
fn verify_root(ledger: &StoreLedger, record: &StoreRecord, confirmed: bool) -> Result<()> {
    let directory = ledger.project_directory(record.kind, &record.id)?;
    match native::open_no_reparse(&directory) {
        Ok(opened)
            if opened.identity.directory
                && record
                    .directory_identity
                    .matches_open_runtime(&opened.file)
                    .map_err(|_| invalid())? =>
        {
            Ok(())
        }
        Err(error) if confirmed && error.kind() == std::io::ErrorKind::NotFound => {
            if record
                .directory_identity
                .absent_on_expected_volume(&directory)
                .map_err(|_| invalid())?
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        _ => Err(invalid()),
    }
}
pub(in crate::webview_storage) fn decode(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
    path: &Path,
    confirmed: bool,
) -> Result<Option<StoreRecord>> {
    validate(kind, id)?;
    let Some(selected) = open_record(path, kind, id)? else {
        return Ok(None);
    };
    verify_root(ledger, &selected.record, confirmed)?;
    Ok(Some(selected.record))
}
pub(in crate::webview_storage) fn read(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
) -> Result<Option<StoreRecord>> {
    if let Some(record) = decode(ledger, kind, id, &ledger.confirmation_path(kind, id)?, true)? {
        return Ok(Some(record));
    }
    let record = decode(ledger, kind, id, &ledger.path(kind, id)?, false)?;
    if record.is_none() {
        match fs::symlink_metadata(ledger.project_directory(kind, id)?) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(invalid()),
        }
    }
    Ok(record)
}
pub(in crate::webview_storage) fn confirmed_cleanup_pending(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
) -> Result<bool> {
    decode(ledger, kind, id, &ledger.confirmation_path(kind, id)?, true)
        .map(|record| record.is_some())
}
pub(in crate::webview_storage) fn persist_confirmation(
    ledger: &StoreLedger,
    record: &StoreRecord,
) -> Result<()> {
    let path = ledger.confirmation_path(record.kind, &record.id)?;
    if let Some(existing) = decode(ledger, record.kind, &record.id, &path, true)? {
        if !record
            .directory_identity
            .matches_captured(&existing.directory_identity)
        {
            return Err(invalid());
        }
        return Ok(());
    }
    let directory = ledger.project_directory(record.kind, &record.id)?;
    let root = native::pin_directory(&directory)?;
    if !record
        .directory_identity
        .matches_open_runtime(&root.file)
        .map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    let temporary = directory.join(format!(".clear-{}", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_RECORD as usize {
        return Err(invalid());
    }
    let mut file = native::create_private_file_for_publication(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    // Native rename success is the publication point. There is no fallible
    // post-rename observation which could authorize deleting the receipt.
    native::rename_private_file_handle(&file, &path, false)?;
    drop(file);
    Ok(())
}

fn actual_absence(path: &Path, expected: &native::FileIdentity) -> Result<()> {
    for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if retryable(&error) => (),
            Err(error) => return Err(pending(path, error)),
            Ok(_) => match removal::inspect(path) {
                Ok(observed) if !expected.same_object(&observed.identity) => {
                    return Err(pending(path, "A replacement occupies the retired name"));
                }
                Ok(_) => (),
                Err(error) if retryable(&error) => (),
                Err(error) => return Err(pending(path, error)),
            },
        }
    }
    Err(pending(
        path,
        "The original name is still present or delete-pending",
    ))
}
fn retire_receipt(path: &Path, expected: &RecordFile) -> Result<()> {
    // Prevent writes while checking the original bytes and issuing the exact
    // deletion. Existing hard links remain admitted and their other names stay.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ_DELETE)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| pending(path, error))?;
    let actual = read_file(&file, expected.record.kind, &expected.record.id)?;
    if !expected.identity.same_object(&actual.identity) || expected.bytes != actual.bytes {
        return Err(pending(
            path,
            "Cleanup receipt identity or original bytes changed",
        ));
    }
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, &expected.identity) {
            Ok(()) => break,
            Err(error) if retryable(&error) => {
                let Some(delay) = waits.next() else {
                    return Err(pending(path, error));
                };
                std::thread::sleep(Duration::from_millis(delay));
            }
            Err(error) => return Err(pending(path, error)),
        }
    }
    // Our own read HANDLE must close before observing the delete-pending name.
    drop(file);
    actual_absence(path, &expected.identity)
}
pub(in crate::webview_storage) fn forget_confirmed(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
) -> Result<()> {
    forget_confirmed_with(ledger, kind, id, |_| Ok(()))
}
fn forget_confirmed_with(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
    before_receipt_retirement: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let Some(record) = read(ledger, kind, id)? else {
        return sync_absence(ledger, kind, id);
    };
    // Only the trusted desktop browser-exit path invokes this operation. The
    // receipt preserves its closure fact across later occupancy failures.
    persist_confirmation(ledger, &record)?;
    let receipt_path = ledger.confirmation_path(kind, id)?;
    let receipt = open_record(&receipt_path, kind, id)?.ok_or_else(invalid)?;
    if !record
        .directory_identity
        .matches_captured(&receipt.record.directory_identity)
    {
        return Err(invalid());
    }
    let parent = native::pin_directory(&ledger.directory())?;
    let directory = ledger.project_directory(kind, id)?;
    match native::open_no_reparse(&directory) {
        Ok(root) => {
            if !root.identity.directory
                || !record
                    .directory_identity
                    .matches_open_runtime(&root.file)
                    .map_err(|_| invalid())?
            {
                return Err(invalid());
            }
            let identity = root.identity.clone();
            drop(root);
            // Detect a retained BoundProfile/root reader before touching any
            // child. This is an access probe, not a deletion or closure proof.
            removal::verify_removal_access(&directory, &identity)
                .map_err(|error| pending(&directory, error))?;
            crate::history_cleanup::remove_owned_directory(&directory, &record.directory_identity)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(pending(&directory, error)),
    }
    if !record
        .directory_identity
        .absent_on_expected_volume(&directory)
        .map_err(|error| pending(&directory, error))?
    {
        return Err(pending(
            &directory,
            "The original UDF has not become absent",
        ));
    }
    before_receipt_retirement(&receipt_path)?;
    retire_receipt(&receipt_path, &receipt)?;
    native::sync_directory(&parent.file)?;
    sync_absence(ledger, kind, id)
}
pub(in crate::webview_storage) fn sync_absence(
    ledger: &StoreLedger,
    kind: ProjectKind,
    id: &str,
) -> Result<()> {
    if read(ledger, kind, id)?.is_some() {
        return Err(pending(
            &ledger.project_directory(kind, id)?,
            "Cleanup receipt remains",
        ));
    }
    match native::open_no_reparse(&ledger.directory()) {
        Ok(container) if container.identity.directory => native::sync_directory(&container.file)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(invalid()),
    }
    Ok(())
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
