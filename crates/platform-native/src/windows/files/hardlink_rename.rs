//! Move one selected ordinary file name. Other hard-link names never become
//! owned, and neither their attributes nor their bytes are changed.
use super::*;

pub fn rename_file_noreplace_expected_allowing_hard_links(
    source: &Path,
    destination: &Path,
    expected: &FileIdentity,
) -> io::Result<()> {
    rename_with(source, destination, expected, || {}, || {})
}

fn rename_with(
    source: &Path,
    destination: &Path,
    expected: &FileIdentity,
    before_move: impl FnOnce(),
    after_move: impl FnOnce(),
) -> io::Result<()> {
    if expected.directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an ordinary file identity",
        ));
    }
    let _source_parent = pin_parent(source)?;
    let _destination_parent = pin_parent(destination)?;
    let from = native_path(source)?;
    let raw = unsafe {
        CreateFileW(
            from.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let captured = identity(&file)?;
    if captured.directory || !expected.same_object(&captured) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Owned file changed before publication",
        ));
    }
    before_move();
    rename_opened_handle(&file, destination, false)?;
    after_move();
    confirm_publication(destination, expected)
}

#[cfg(test)]
#[path = "hardlink_rename_tests.rs"]
mod tests;
