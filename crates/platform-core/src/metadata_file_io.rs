//! Native file boundary for metadata; does not open SQLite or run project code.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::{fs::File, io::Read, path::Path};

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("INVALID_METADATA", message)
}

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

pub(crate) fn publish_file(
    source: &Path,
    destination: &Path,
    committed: &mut bool,
) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        std::fs::rename(source, destination)?;
        *committed = true;
        Ok(())
    }
    #[cfg(windows)]
    {
        match platform_native::windows::files::rename_file(source, destination, true) {
            Ok(()) => {
                *committed = true;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                // The move may have committed. Retain related material instead
                // of deleting it under the pre-commit rollback path.
                *committed = true;
                Err(ApiError::from(error).outcome("unknown"))
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::windows::fs::OpenOptionsExt};

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

    #[test]
    fn metadata_publication_preserves_old_bytes_on_occupancy_and_keeps_revision_identity() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-metadata-publish-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let destination = root.join("project.json");
        let stage = root.join(".metadata-disposable");
        fs::write(&destination, b"old").unwrap();
        let before = platform_native::tool_files::Facts::capture(&destination).unwrap();
        let occupied = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&destination)
            .unwrap();
        let mut file = create_new(&stage).unwrap();
        file.write_all(b"new metadata").unwrap();
        file.sync_all().unwrap();
        platform_native::windows::files::verify_private(&file).unwrap();
        let mut committed = false;
        assert!(publish_file(&stage, &destination, &mut committed).is_err());
        assert!(!committed);
        assert!(stage.exists());
        drop(occupied);
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        publish_file(&stage, &destination, &mut committed).unwrap();
        assert!(committed);
        let after = platform_native::tool_files::Facts::capture(&destination).unwrap();
        assert_ne!(
            before.identity_revision_bytes(),
            after.identity_revision_bytes()
        );
        assert_ne!(
            before.content_revision_bytes(),
            after.content_revision_bytes()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"new metadata");
        assert!(!stage.exists());
    }
}
