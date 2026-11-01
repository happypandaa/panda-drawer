//! File-side Git selection checks. This module never executes a tool.
use crate::{path_checks::check_no_symlinks, ApiError};
use platform_native::tool_files::{self as native_tools, Facts as Metadata};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const MAX_SETTINGS_BYTES: u64 = 16 * 1024;

pub(super) fn settings_error(message: impl Into<String>) -> ApiError {
    ApiError::new("GIT_TOOLCHAIN_SETTINGS_INVALID", message)
}
pub(super) fn absolute_clean(path: &Path) -> bool {
    native_tools::absolute_path_valid(path)
}
fn unchanged(left: &Metadata, right: &Metadata) -> bool {
    left.unchanged(right)
}
pub(super) fn read_settings_bytes(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    if !absolute_clean(path) {
        return Err(settings_error("Git 工具选择文件路径无效。"));
    }
    check_no_symlinks(path, true)
        .map_err(|_| settings_error("Git 工具选择文件路径含链接或无法安全读取。"))?;
    let before = match Metadata::capture(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(settings_error(format!("无法读取 Git 工具选择：{error}"))),
    };
    if !before.is_file() || before.links() != 1 || before.len() > MAX_SETTINGS_BYTES {
        return Err(settings_error(
            "Git 工具选择需要无链接且大小受限的普通文件。",
        ));
    }
    let mut file = native_tools::open_read(path)
        .map_err(|error| settings_error(format!("无法读取 Git 工具选择：{error}")))?;
    if !unchanged(&before, &Metadata::from_file(&file)?) {
        return Err(settings_error("Git 工具选择文件在打开时发生变化。"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.len()
        || !unchanged(&before, &Metadata::from_file(&file)?)
        || !unchanged(&before, &Metadata::capture(path)?)
    {
        return Err(settings_error("Git 工具选择文件在读取时发生变化。"));
    }
    check_no_symlinks(path, false)
        .map_err(|_| settings_error("Git 工具选择文件路径在读取时发生变化。"))?;
    Ok(Some(bytes))
}

pub(super) fn ordinary_executable(candidate: &Path) -> Result<PathBuf, ApiError> {
    if !absolute_clean(candidate) {
        return Err(ApiError::new(
            "INVALID_GIT_PATH",
            "请选择不含越界或控制字符的 Git 绝对路径。",
        ));
    }
    // Explicit local selection may intentionally use a package-manager symlink.
    let path = fs::canonicalize(candidate).map_err(|_| {
        ApiError::new(
            "GIT_NOT_FOUND",
            format!("未找到所选工具：{}", candidate.display()),
        )
    })?;
    let info = Metadata::capture_following(&path)?;
    if !native_tools::executable_flags_allowed(&info) {
        return Err(ApiError::new(
            "GIT_NOT_FOUND",
            "所选工具不是可执行的普通文件。",
        ));
    }
    Ok(path)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("panda-disposable-git-selection-中文 space-")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn settings_read_is_bounded_and_missing_reads_do_not_create_files() {
        let temporary = root();
        let path = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("git-toolchain.json");
        assert_eq!(read_settings_bytes(&path).unwrap(), None);
        assert!(!path.exists());
        fs::write(&path, b"{\"schemaVersion\":1,\"gitPath\":null}").unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(read_settings_bytes(&path).unwrap().unwrap(), original);
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_SETTINGS_BYTES + 1)
            .unwrap();
        assert!(read_settings_bytes(&path).is_err());
        assert!(!absolute_clean(Path::new("relative/git.exe")));
    }

    #[test]
    fn hardlinked_settings_fail_without_modifying_either_disposable_name() {
        let temporary = root();
        let root = temporary.path().canonicalize().unwrap();
        let original = root.join("retained-original.json");
        let selected = root.join("git-toolchain.json");
        fs::write(&original, b"{\"disposable\":true}").unwrap();
        fs::hard_link(&original, &selected).unwrap();
        assert!(read_settings_bytes(&selected).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"{\"disposable\":true}");
        assert_eq!(fs::read(&selected).unwrap(), b"{\"disposable\":true}");
    }

    #[test]
    fn config_paths_reject_junctions_while_explicit_tool_selection_resolves_them() {
        let temporary = root();
        let root = temporary.path().canonicalize().unwrap();
        let target = root.join("disposable-tools");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("git-toolchain.json"), b"{\"disposable\":true}").unwrap();
        fs::write(
            target.join("synthetic-tool.dat"),
            b"synthetic, never executed",
        )
        .unwrap();
        let alias = root.join("disposable-package-manager-alias");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&alias)
            .arg(&target)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(read_settings_bytes(&alias.join("git-toolchain.json")).is_err());
        // Only the ordinary-file selection precheck is exercised. A real
        // owned version probe must still establish that this is usable Git.
        assert_eq!(
            ordinary_executable(&alias.join("synthetic-tool.dat")).unwrap(),
            target.join("synthetic-tool.dat").canonicalize().unwrap()
        );
        fs::remove_dir(alias).unwrap();
        assert_eq!(
            fs::read(target.join("git-toolchain.json")).unwrap(),
            b"{\"disposable\":true}"
        );
    }

    #[test]
    fn selection_precheck_rejects_directories_and_keeps_unicode_file_paths() {
        let temporary = root();
        let root = temporary.path().canonicalize().unwrap();
        assert!(ordinary_executable(&root).is_err());
        let file = root.join("可丢弃 工具.dat");
        fs::write(&file, b"synthetic, never executed").unwrap();
        assert_eq!(
            ordinary_executable(&file).unwrap(),
            file.canonicalize().unwrap()
        );
        assert!(ordinary_executable(Path::new("relative/tool")).is_err());
    }
}
