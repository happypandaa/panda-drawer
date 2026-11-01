//! Original manifest observation and path policy.
use super::*;
use std::fs;
use std::{fs::OpenOptions, io::Read};
fn same_manifest_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    if !left.is_file()
        || !right.is_file()
        || left.len() != right.len()
        || left.modified().ok() != right.modified().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.nlink() == 1
            && right.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(super) fn read_bytes(app_dir: &Path) -> ManifestResult<Vec<u8>> {
    if !app_dir.is_absolute() {
        return Err(invalid(MANIFEST_FILE, "应用目录必须为绝对路径。"));
    }
    let path = checked_child(app_dir, MANIFEST_FILE, false)?;
    let before =
        fs::symlink_metadata(&path).map_err(|_| invalid(MANIFEST_FILE, "无法读取应用声明。"))?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A declaration replaced by a FIFO must fail inspection instead of
        // blocking before its opened file type can be checked.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|_| invalid(MANIFEST_FILE, "无法读取应用声明。"))?;
    let opened = file
        .metadata()
        .map_err(|_| invalid(MANIFEST_FILE, "无法读取应用声明。"))?;
    if !same_manifest_file(&before, &opened) {
        return Err(invalid(MANIFEST_FILE, "应用声明必须是普通文件。"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid(MANIFEST_FILE, "无法读取应用声明。"))?;
    let after = fs::symlink_metadata(&path)
        .map_err(|_| invalid(MANIFEST_FILE, "应用声明在读取期间已变化。"))?;
    let held = file
        .metadata()
        .map_err(|_| invalid(MANIFEST_FILE, "应用声明在读取期间已变化。"))?;
    if !same_manifest_file(&opened, &after) || !same_manifest_file(&opened, &held) {
        return Err(invalid(MANIFEST_FILE, "应用声明在读取期间已变化，请重试。"));
    }
    Ok(bytes)
}

pub(super) fn checked_child(
    root: &Path,
    relative_path: &str,
    directory: bool,
) -> ManifestResult<PathBuf> {
    super::paths::resolve(root, relative_path, directory, false)?
        .ok_or_else(|| invalid(MANIFEST_FILE, "声明路径不存在"))
}
