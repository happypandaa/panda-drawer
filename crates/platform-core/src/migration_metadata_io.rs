//! Bounded read-only migration evidence. This never grants deletion authority.
use crate::{path_checks::check_no_symlinks, ApiError};
use platform_native::tool_files::Facts;
use serde_json::Value;
use std::{io::Read, path::Path};

const MAX_METADATA: u64 = 32_000_000;

fn blocked(message: &str) -> ApiError {
    ApiError::new("WORKSPACE_SOURCE_CLEANUP_PENDING", message)
}

pub(crate) fn read_metadata(path: &Path) -> Result<Value, ApiError> {
    check_no_symlinks(path, false)?;
    let mut file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksCloseOnExec,
    )?;
    let before = Facts::from_file(&file)?;
    if !before.is_file() || before.links() != 1 || before.len() > MAX_METADATA {
        return Err(blocked("迁移证明不是独立普通文件或超过大小限制。"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)?;
    let after = Facts::from_file(&file)?;
    if bytes.len() as u64 > MAX_METADATA
        || !before.same_modified_and_changed(&after)
        || after.links() != 1
    {
        return Err(blocked("读取迁移证明时文件发生变化。"));
    }
    serde_json::from_slice(&bytes).map_err(|_| blocked("迁移证明不是有效 JSON。"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn evidence_read_preserves_bytes_and_rejects_missing_oversized_or_linked_inputs() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-migration-evidence-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("copy-inventory.json");
        assert!(read_metadata(&path).is_err());
        assert!(!path.exists());
        fs::write(&path, b"{\"disposable\":true}").unwrap();
        let before = Facts::capture(&path).unwrap();
        assert_eq!(
            read_metadata(&path).unwrap(),
            serde_json::json!({"disposable":true})
        );
        assert!(before.unchanged(&Facts::capture(&path).unwrap()));
        fs::hard_link(&path, root.join("retained-link")).unwrap();
        assert_eq!(
            read_metadata(&path).unwrap_err().code,
            "WORKSPACE_SOURCE_CLEANUP_PENDING"
        );
        assert_eq!(fs::read(&path).unwrap(), b"{\"disposable\":true}");
        let oversized = root.join("oversized.json");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_METADATA + 1).unwrap();
        drop(file);
        assert_eq!(
            read_metadata(&oversized).unwrap_err().code,
            "WORKSPACE_SOURCE_CLEANUP_PENDING"
        );
        assert_eq!(fs::metadata(&oversized).unwrap().len(), MAX_METADATA + 1);
    }
}
