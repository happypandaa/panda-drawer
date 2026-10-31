use super::*;
use platform_native::windows::removal;
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    time::Duration,
};

pub(super) const MAX_CONTROL: u64 = 1024 * 1024;
pub(super) fn pin(path: &Path, expected: &PersistedIdentity) -> Result<native::PinnedDirectory> {
    let scope = native::pin_directory(path).map_err(cause)?;
    if !expected.matches_open_runtime(&scope.file).map_err(cause)? {
        return Err(failure("交付目录身份变化，已保留现场。").detail("path", json!(path)));
    }
    Ok(scope)
}
pub(super) fn file_identity(path: &Path) -> Result<Option<PersistedIdentity>> {
    let opened = match native::open_no_reparse(path) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(cause(error)),
    };
    if opened.identity.directory || !opened.file.metadata()?.is_file() {
        return Err(failure("交付名字不是普通文件，已保留。").detail("path", json!(path)));
    }
    PersistedIdentity::from_file(&opened.file)
        .map(Some)
        .map_err(cause)
}
pub(super) fn same(
    expected: Option<&PersistedIdentity>,
    actual: Option<&PersistedIdentity>,
) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(a), Some(b)) => a.matches_captured(b),
        _ => false,
    }
}
pub(super) fn native_id(
    value: &PersistedIdentity,
    directory: bool,
) -> Result<native::FileIdentity> {
    full_identity(value)?;
    Ok(native::FileIdentity {
        volume_serial: value.device,
        file_id: value.windows_file_id.unwrap(),
        links: 0,
        directory,
    })
}
pub(super) fn move_file(from: &Path, to: &Path, expected: &PersistedIdentity) -> Result<()> {
    native::rename_file_noreplace_expected_allowing_hard_links(
        from,
        to,
        &native_id(expected, false)?,
    )
    .map_err(|error| {
        cause(error)
            .detail("sourcePath", json!(from))
            .detail("destinationPath", json!(to))
    })
}
pub(super) fn move_control(from: &Path, to: &Path, expected: &PersistedIdentity) -> Result<()> {
    let mut identity = native_id(expected, false)?;
    identity.links = 1;
    native::rename_file_noreplace_expected(from, to, &identity).map_err(cause)
}
pub(super) struct Control {
    _file: File,
}
pub(super) fn read_control(
    path: &Path,
    id: &PersistedIdentity,
    hash: &str,
) -> Result<Option<Control>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .share_mode(4)
        .custom_flags(0x0020_0000)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(cause(error)),
    };
    let metadata = file.metadata()?;
    let actual = native::identity(&file)?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || actual.links != 1
        || metadata.len() > MAX_CONTROL
        || !id.matches_open_runtime(&file).map_err(cause)?
    {
        return Err(failure("交付控制材料身份或类型变化，已保留。").detail("path", json!(path)));
    }
    native::verify_private(&file).map_err(cause)?;
    let mut bytes = Vec::new();
    (&mut file).take(MAX_CONTROL + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONTROL || digest(&bytes) != hash {
        return Err(failure("交付控制材料字节变化，已保留。").detail("path", json!(path)));
    }
    Ok(Some(Control { _file: file }))
}
pub(super) fn optional_control(path: &Path, id: &PersistedIdentity, hash: &str) -> Result<bool> {
    Ok(read_control(path, id, hash)?.is_some())
}
fn busy(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}
pub(super) fn remove_name(path: &Path, id: &PersistedIdentity, directory: bool) -> Result<()> {
    let _parent = native::pin_directory(
        path.parent()
            .ok_or_else(|| failure("删除交付材料的父目录缺失。"))?,
    )?;
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match removal::remove(path, &native_id(id, directory)?) {
            Ok(()) => break,
            Err(error) if busy(&error) => {
                if let Some(ms) = waits.next() {
                    std::thread::sleep(Duration::from_millis(ms));
                } else {
                    return Err(cause(error).detail("remainingPath", json!(path)));
                }
            }
            Err(error) => return Err(cause(error)),
        }
    }
    for ms in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if ms > 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if id.absent_on_expected_volume(path).map_err(cause)? {
                    return Ok(());
                }
                return Err(failure("删除后交付材料名字出现新内容，已保留。"));
            }
            Err(error) if busy(&error) => (),
            Err(error) => return Err(cause(error)),
            Ok(_) => match removal::inspect(path) {
                Ok(entry) if !id.matches_open_runtime(&entry.file).map_err(cause)? => {
                    return Err(failure("交付材料名字被替换，已保留。"))
                }
                Ok(_) => (),
                Err(error) if busy(&error) || error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(cause(error)),
            },
        }
    }
    Err(failure("交付材料仍处于删除待定状态。")
        .detail("remainingPath", json!(path))
        .detail("reason", "file_in_use"))
}
