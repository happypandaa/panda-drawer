//! Bounded independent-file reads and private create-new used by runtime controls.
use crate::{path_checks::check_no_symlinks, ApiError};
#[cfg(test)]
use std::fs::File;
use std::{io::Read, path::Path};

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("INVALID_METADATA", message)
}

#[cfg(test)]
pub(crate) fn create_new(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
    #[cfg(windows)]
    {
        platform_native::windows::files::create_private_file(path)
    }
}

pub(crate) fn read_file(path: &Path, max: u64) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false)?;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
    };
    #[cfg(windows)]
    let file = platform_native::windows::files::open_read_file(path)?;
    let metadata = platform_native::tool_files::Facts::from_file(&file)?;
    if !metadata.is_file() || metadata.links() != 1 {
        return Err(invalid("Only independent regular files are accepted"));
    }
    if metadata.len() > max {
        return Err(invalid(format!(
            "File exceeds internal size limit of {max} bytes"
        )));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(invalid("File grew beyond the size limit while reading"));
    }
    Ok(bytes)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bounded_metadata_reads_reject_hardlinks_and_directory_junctions() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-metadata-read-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("selected.png");
        fs::write(&path, b"disposable content").unwrap();
        assert_eq!(read_file(&path, 18).unwrap(), b"disposable content");
        assert_eq!(read_file(&path, 4).unwrap_err().code, "INVALID_METADATA");
        fs::hard_link(&path, root.join("hard.png")).unwrap();
        assert!(read_file(&path, 18).is_err());
        fs::remove_file(root.join("hard.png")).unwrap();
        let junction = root.join("junction");
        assert!(std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(
            read_file(&junction.join("selected.png"), 18)
                .unwrap_err()
                .code,
            "UNSAFE_PATH"
        );
        fs::remove_dir(junction).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"disposable content");
    }
}
