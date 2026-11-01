//! Resolve author paths without allowing internal links to escape their boundary.
use super::*;
use std::fs;

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        platform_native::filesystem::is_reparse(metadata)
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn resolve(
    root: &Path,
    relative_path: &str,
    directory: bool,
    allow_missing: bool,
) -> ManifestResult<Option<PathBuf>> {
    if !relative_root(relative_path) || !root.is_absolute() {
        return Err(invalid(MANIFEST_FILE, "路径须位于已选择的项目目录内"));
    }
    let root_info =
        fs::symlink_metadata(root).map_err(|_| invalid(MANIFEST_FILE, "项目目录不可读取"))?;
    if !root_info.is_dir() || is_link(&root_info) {
        return Err(invalid(MANIFEST_FILE, "项目根须为普通目录"));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| invalid(MANIFEST_FILE, "项目目录无法解析"))?;
    if relative_path == "." {
        return if directory {
            Ok(Some(canonical_root))
        } else {
            Err(invalid(MANIFEST_FILE, "入口必须为文件"))
        };
    }
    let parts: Vec<_> = relative_path.split('/').collect();
    let mut path = canonical_root.clone();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            Err(_) => return Err(invalid(MANIFEST_FILE, "声明文件或目录不存在或不可读取")),
            Ok(_) => {}
        }
        // Resolve a link only after observing its own directory entry. Dangling
        // links are errors, rather than a missing build output.
        let resolved = path
            .canonicalize()
            .map_err(|_| invalid(MANIFEST_FILE, "声明路径的链接无法解析"))?;
        if !resolved.starts_with(&canonical_root) {
            return Err(invalid(MANIFEST_FILE, "声明路径或链接指向目录之外"));
        }
        let facts = fs::symlink_metadata(&resolved)
            .map_err(|_| invalid(MANIFEST_FILE, "声明路径在检查期间变化"))?;
        if is_link(&facts) {
            return Err(invalid(MANIFEST_FILE, "声明路径在检查期间变化，请重新检查"));
        }
        let require_directory = index + 1 < parts.len() || directory;
        if (require_directory && !facts.is_dir()) || (!require_directory && !facts.is_file()) {
            return Err(invalid(MANIFEST_FILE, "声明路径不是所需的普通文件或目录"));
        }
        path = resolved;
    }
    Ok(Some(path))
}
