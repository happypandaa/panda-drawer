//! Metadata-only installation budgets; links are counted but never followed.
use crate::ApiError;
#[cfg(windows)]
use std::fs;
use std::path::PathBuf;
#[cfg(unix)]
#[path = "unix_space.rs"]
mod unix;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MIN_FREE: u64 = 64 * 1024 * 1024;
fn limit() -> ApiError {
    ApiError::new(
        "BUILD_SPACE_LIMIT",
        "构建的依赖、缓存或临时材料超过文件大小、数量或总占用预算。",
    )
}
pub(super) fn check(roots: &[PathBuf]) -> Result<(), ApiError> {
    #[cfg(windows)]
    fn visit(
        path: &std::path::Path,
        depth: usize,
        usage: &mut (usize, u64),
    ) -> Result<(), ApiError> {
        if depth > 128 {
            return Err(limit());
        }
        crate::projects::check_no_symlinks(path, true)?;
        #[cfg(windows)]
        let _pin = match platform_native::windows::files::pin_directory(path) {
            Ok(pin) => Some(pin),
            Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(issue) => return Err(issue.into()),
        };
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(issue) => return Err(issue.into()),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => continue,
                Err(issue) => return Err(issue.into()),
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => continue,
                Err(issue) => return Err(issue.into()),
            };
            usage.0 = usage.0.checked_add(1).ok_or_else(limit)?;
            if usage.0 > MAX_ENTRIES {
                return Err(limit());
            }
            if metadata.is_dir() && !platform_native::filesystem::is_reparse(&metadata) {
                visit(&entry.path(), depth + 1, usage)?;
            } else {
                if metadata.is_file() && metadata.len() > MAX_FILE {
                    return Err(limit());
                }
                usage.1 = usage.1.checked_add(metadata.len()).ok_or_else(limit)?;
                if usage.1 > MAX_TOTAL {
                    return Err(limit());
                }
            }
        }
        Ok(())
    }
    let mut usage = (0, 0);
    for root in roots {
        #[cfg(windows)]
        visit(root, 0, &mut usage)?;
        #[cfg(unix)]
        unix::visit(root, &mut usage)?;
        let mut existing = root.as_path();
        while !existing.try_exists()? {
            existing = existing.parent().ok_or_else(limit)?;
        }
        if fs2::available_space(existing)? < MIN_FREE {
            return Err(ApiError::new(
                "BUILD_SPACE_INSUFFICIENT",
                "磁盘可用空间不足，构建已停止。",
            ));
        }
    }
    Ok(())
}
