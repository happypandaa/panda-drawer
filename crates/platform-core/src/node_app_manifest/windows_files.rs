//! Full Windows identity with the original manifest observation fields.
use super::*;
use platform_native::{tool_files::Facts, windows::files as native};
use std::io::Read;

fn same(left: &Facts, right: &Facts) -> bool {
    left.is_file()
        && right.is_file()
        && left.links() == 1
        && right.links() == 1
        && left.same_object(right)
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

pub(super) fn read_bytes(app_dir: &Path) -> Result<Vec<u8>> {
    if !app_dir.is_absolute() {
        return Err(invalid("应用目录必须为绝对路径。"));
    }
    let path = checked_child(app_dir, "local-app.json", false)?;
    let before = native::open_no_reparse(&path).map_err(|_| invalid("无法读取应用声明。"))?;
    let before_facts = Facts::from_file(&before.file).map_err(|_| invalid("无法读取应用声明。"))?;
    let file = native::open_read_file(&path).map_err(|_| invalid("无法读取应用声明。"))?;
    let opened = Facts::from_file(&file).map_err(|_| invalid("无法读取应用声明。"))?;
    if !same(&before_facts, &opened) {
        return Err(invalid("应用声明必须是普通文件。"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("无法读取应用声明。"))?;
    let after =
        native::open_no_reparse(&path).map_err(|_| invalid("应用声明在读取期间已变化。"))?;
    let after_facts =
        Facts::from_file(&after.file).map_err(|_| invalid("应用声明在读取期间已变化。"))?;
    let held = Facts::from_file(&file).map_err(|_| invalid("应用声明在读取期间已变化。"))?;
    if !same(&opened, &after_facts) || !same(&opened, &held) {
        return Err(invalid("应用声明在读取期间已变化，请重试。"));
    }
    Ok(bytes)
}

pub(super) fn checked_child(root: &Path, relative_path: &str, directory: bool) -> Result<PathBuf> {
    if !relative(relative_path) || !root.is_absolute() {
        return Err(invalid("应用路径超出允许范围。"));
    }
    let root_info = fs::symlink_metadata(root).map_err(|_| invalid("应用目录不可读取。"))?;
    if !root_info.is_dir() || platform_native::filesystem::is_reparse(&root_info) {
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
        if platform_native::filesystem::is_reparse(&metadata)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_plain_manifest_reads_keep_original_budget_and_single_link_policy() {
        let root = tempfile::Builder::new()
            .prefix("panda-disposable-node-manifest-中文 space-")
            .tempdir()
            .unwrap();
        let path = root.path().join("local-app.json");
        fs::write(&path, b"{}").unwrap();
        assert_eq!(read_bytes(root.path()).unwrap(), b"{}");
        let other = root.path().join("original-manifest-hardlink");
        fs::hard_link(&path, &other).unwrap();
        assert!(read_bytes(root.path()).is_err());
        assert_eq!(fs::read(&other).unwrap(), b"{}");
    }
    #[test]
    fn actual_manifest_junction_and_child_junction_are_not_followed_or_modified() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-node-manifest-junction-")
            .tempdir()
            .unwrap();
        let root = temporary.path().join("app");
        let target = temporary.path().join("external-original");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("local-app.json"), b"external original").unwrap();
        let link = root.join("linked");
        let output = std::process::Command::new(
            platform_native::windows::system_paths::SystemPaths::current()
                .unwrap()
                .system
                .join("cmd.exe"),
        )
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
        assert!(output.status.success());
        assert!(read_bytes(&link).is_err());
        assert!(checked_child(&root, "linked/local-app.json", false).is_err());
        assert_eq!(
            fs::read(target.join("local-app.json")).unwrap(),
            b"external original"
        );
    }
}
