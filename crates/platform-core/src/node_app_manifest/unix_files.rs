//! Original manifest observation and path policy.
use super::*;
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

pub(super) fn read_bytes(app_dir: &Path) -> Result<Vec<u8>> {
    if !app_dir.is_absolute() {
        return Err(invalid("应用目录必须为绝对路径。"));
    }
    let path = checked_child(app_dir, "local-app.json", false)?;
    let before = fs::symlink_metadata(&path).map_err(|_| invalid("无法读取应用声明。"))?;
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
        .map_err(|_| invalid("无法读取应用声明。"))?;
    let opened = file.metadata().map_err(|_| invalid("无法读取应用声明。"))?;
    if !same_manifest_file(&before, &opened) {
        return Err(invalid("应用声明必须是普通文件。"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("无法读取应用声明。"))?;
    let after = fs::symlink_metadata(&path).map_err(|_| invalid("应用声明在读取期间已变化。"))?;
    let held = file
        .metadata()
        .map_err(|_| invalid("应用声明在读取期间已变化。"))?;
    if !same_manifest_file(&opened, &after) || !same_manifest_file(&opened, &held) {
        return Err(invalid("应用声明在读取期间已变化，请重试。"));
    }
    Ok(bytes)
}

pub(super) fn checked_child(root: &Path, relative_path: &str, directory: bool) -> Result<PathBuf> {
    if !relative(relative_path) || !root.is_absolute() {
        return Err(invalid("应用路径超出允许范围。"));
    }
    let root_info = fs::symlink_metadata(root).map_err(|_| invalid("应用目录不可读取。"))?;
    if !root_info.is_dir() || root_info.file_type().is_symlink() {
        return Err(invalid("应用目录必须是普通目录。"));
    }
    let mut path = root
        .canonicalize()
        .map_err(|_| invalid("应用目录不可解析。"))?;
    let parts: Vec<_> = relative_path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| invalid("声明文件或目录不存在。"))?;
        if metadata.file_type().is_symlink()
            || if index + 1 == parts.len() {
                if directory {
                    !metadata.is_dir()
                } else {
                    !metadata.is_file()
                }
            } else {
                !metadata.is_dir()
            }
        {
            return Err(invalid("应用入口不能经过链接或特殊文件。"));
        }
    }
    Ok(path)
}
