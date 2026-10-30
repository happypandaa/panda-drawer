//! Windows implementation of the existing handle-relative project file API.
use super::{info, FileInfo};
use crate::ApiError;
use platform_native::windows::files as native;
use std::{fs::File, io, path::Path};

pub(crate) fn error(error: io::Error) -> ApiError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => "FILE_NOT_FOUND",
        io::ErrorKind::AlreadyExists => "FILE_EXISTS",
        io::ErrorKind::PermissionDenied => "FILE_ACCESS_DENIED",
        io::ErrorKind::InvalidInput => "INVALID_PATH",
        _ => "FILE_IO_ERROR",
    };
    ApiError::new(code, error.to_string())
}
fn invalid() -> ApiError {
    ApiError::new("INVALID_PATH", "Path must be a relative, non-empty Windows file path without dot segments or reparse points")
}

pub(crate) fn root(path: &Path) -> Result<File, ApiError> {
    native::directory_file(path).map_err(error)
}

pub(crate) fn parts(path: &str) -> Result<Vec<&str>, ApiError> {
    if path.is_empty()
        || path.len() > 2048
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.len() > 255
            || part.ends_with(['.', ' '])
            || part.contains(['<', '>', ':', '"', '|', '?', '*'])
            || reserved
        {
            return Err(invalid());
        }
    }
    Ok(parts)
}

pub(crate) fn parent(root: &File, path: &str, create: bool) -> Result<(File, String), ApiError> {
    let parts = parts(path)?;
    let (_, _scope) = native::directory_scope(root).map_err(error)?;
    let mut current = root.try_clone().map_err(error)?;
    // Each descriptor pins the previous directory until its child has opened.
    for part in &parts[..parts.len() - 1] {
        // Resolve the actual held parent again instead of trusting a pathname
        // saved before a different directory could have been renamed.
        let (directory, _parent_scope) = native::directory_scope(&current).map_err(error)?;
        let directory = directory.join(part);
        if create {
            match native::create_private_directory(&directory) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(error(e)),
            }
        }
        current = native::directory_file(&directory).map_err(error)?;
    }
    Ok((current, parts.last().unwrap().to_string()))
}

pub(crate) fn read(root: &File, path: &str) -> Result<File, ApiError> {
    let (directory, name) = parent(root, path, false)?;
    let (path, _scope) = native::directory_scope(&directory).map_err(error)?;
    native::open_read_file(&path.join(name)).map_err(error)
}

pub(crate) fn create(root: &File, path: &str) -> Result<File, ApiError> {
    let (directory, name) = parent(root, path, true)?;
    let (path, _scope) = native::directory_scope(&directory).map_err(error)?;
    native::create_private_file(&path.join(name)).map_err(error)
}

pub(crate) fn create_internal(root: &File, path: &str) -> Result<File, ApiError> {
    let tail = path.strip_prefix(".platform/").ok_or_else(invalid)?;
    parts(tail)?;
    // This prefix is caller-owned internal data, not a general path escape.
    create(root, path)
}

pub(crate) fn discard_temporary(root: &File, path: &str) {
    if !path.starts_with(".upload-") || parts(path).is_err() || path.contains('/') {
        return;
    }
    let _ = remove(root, path);
}

pub(crate) fn remove(root: &File, path: &str) -> Result<(), ApiError> {
    let (directory, name) = parent(root, path, false)?;
    let (path, _scope) = native::directory_scope(&directory).map_err(error)?;
    let target = path.join(name);
    let file = native::open_read_file(&target).map_err(error)?;
    let expected = native::identity(&file).map_err(error)?;
    native::remove_file(&target, &expected).map_err(error)
}

pub(crate) fn publish(
    staging: &File,
    temporary: &str,
    root: &File,
    path: &str,
    overwrite: bool,
) -> Result<(), ApiError> {
    // The staging entry is a single checked name in its own held directory.
    if parts(temporary)?.len() != 1 {
        return Err(invalid());
    }
    let (directory, name) = parent(root, path, true)?;
    match read(root, path) {
        Ok(_) if !overwrite => {
            return Err(ApiError::new(
                "FILE_EXISTS",
                "An attachment already exists at this path",
            ))
        }
        Ok(_) => (),
        Err(error) if error.code == "FILE_NOT_FOUND" => (),
        Err(error) => return Err(error),
    }
    let (stage, _stage_scope) = native::directory_scope(staging).map_err(error)?;
    let (destination, _destination_scope) = native::directory_scope(&directory).map_err(error)?;
    native::rename_file(&stage.join(temporary), &destination.join(name), overwrite).map_err(|e| {
        if e.kind() == io::ErrorKind::InvalidData {
            // The native rename occurred, but a subsequent identity check could
            // not confirm publication. Never report this as a clean rollback.
            error(e).outcome("unknown")
        } else {
            error(e)
        }
    })
}

pub(crate) fn list(root_path: &Path, prefix: &str) -> Result<Vec<FileInfo>, ApiError> {
    if !prefix.is_empty() {
        parts(prefix.trim_end_matches('/'))?;
    }
    let root = root(root_path)?;
    let mut queue = vec![(root_path.to_path_buf(), String::new())];
    let mut output = Vec::new();
    let mut count = 0;
    while let Some((directory, relative)) = queue.pop() {
        let _directory = native::pin_directory(&directory).map_err(error)?;
        for entry in std::fs::read_dir(directory).map_err(error)? {
            let entry = entry.map_err(error)?;
            count += 1;
            if count > 10000 {
                return Err(ApiError::new(
                    "FILE_LIST_LIMIT_EXCEEDED",
                    "Attachment listing exceeds 10,000 entries; use narrower folders",
                ));
            }
            let name = entry.file_name().into_string().map_err(|_| invalid())?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let metadata = entry.metadata().map_err(error)?;
            if metadata.is_dir() {
                let (directory, leaf) = parent(&root, &path, false)?;
                let (native_path, _scope) = native::directory_scope(&directory).map_err(error)?;
                let _checked = native::directory_file(&native_path.join(leaf)).map_err(error)?;
                if prefix.starts_with(&format!("{path}/")) || path.starts_with(prefix) {
                    queue.push((entry.path(), path));
                }
            } else if path.starts_with(prefix) {
                output.push(info(&path, &read(&root, &path)?)?);
            }
        }
    }
    output.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        os::windows::fs::OpenOptionsExt,
    };

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, File) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-project-files-中文 space-")
            .tempdir()
            .unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let directory = root(&path).unwrap();
        (temporary, path, directory)
    }

    #[test]
    fn real_attachment_create_read_publish_list_and_remove_keep_the_existing_contract() {
        let (_temporary, path, directory) = fixture();
        create(&directory, "a/original.txt")
            .unwrap()
            .write_all(b"old")
            .unwrap();
        create(&directory, ".upload-disposable")
            .unwrap()
            .write_all(b"new")
            .unwrap();
        assert!(publish(
            &directory,
            ".upload-disposable",
            &directory,
            "a/original.txt",
            false
        )
        .is_err());
        assert_eq!(fs::read(path.join("a/original.txt")).unwrap(), b"old");
        assert!(path.join(".upload-disposable").exists());
        publish(
            &directory,
            ".upload-disposable",
            &directory,
            "a/original.txt",
            true,
        )
        .unwrap();
        assert!(!path.join(".upload-disposable").exists());
        let mut text = String::new();
        read(&directory, "a/original.txt")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "new");
        let listing = list(&path, "a/").unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].path, "a/original.txt");
        assert_eq!(listing[0].size, 3);
        assert_eq!(listing[0].media_type, "text/plain; charset=utf-8");
        create_internal(&directory, ".platform/disposable.json")
            .unwrap()
            .write_all(b"{}")
            .unwrap();
        assert!(create_internal(&directory, "outside.json").is_err());
        create(&directory, ".upload-discarded").unwrap();
        discard_temporary(&directory, ".upload-discarded");
        assert!(!path.join(".upload-discarded").exists());
        discard_temporary(&directory, "a/original.txt");
        assert!(path.join("a/original.txt").exists());
        remove(&directory, "a/original.txt").unwrap();
        assert!(!path.join("a/original.txt").exists());
    }

    #[test]
    fn windows_names_case_collisions_and_long_unicode_paths_use_real_files() {
        let (_temporary, path, directory) = fixture();
        for invalid in [
            "../escape",
            "a//b",
            "a/./b",
            "a\\b",
            "a/b:",
            "CON",
            "nul.txt",
            "com1.log",
            "LPT9.txt",
            "a/trailing.",
            "a/trailing ",
            "a/question?",
        ] {
            assert!(create(&directory, invalid).is_err(), "{invalid}");
        }
        create(&directory, "Case.txt").unwrap();
        assert!(create(&directory, "case.TXT").is_err());
        let relative = (0..12)
            .map(|n| format!("中文 long directory {n}"))
            .chain(["附件.txt".into()])
            .collect::<Vec<_>>()
            .join("/");
        assert!(path.join(&relative).as_os_str().len() > 260);
        create(&directory, &relative)
            .unwrap()
            .write_all(b"long-path-bytes")
            .unwrap();
        let mut bytes = Vec::new();
        read(&directory, &relative)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"long-path-bytes");
        remove(&directory, &relative).unwrap();
        assert!(!path.join(relative).exists());
    }

    #[test]
    fn hardlinks_junctions_and_occupied_files_preserve_the_disposable_foreign_bytes() {
        let (_temporary, path, directory) = fixture();
        let foreign = path.join("disposable-foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("original.txt"), b"retained foreign bytes").unwrap();
        fs::hard_link(foreign.join("original.txt"), path.join("hardlink.txt")).unwrap();
        assert!(read(&directory, "hardlink.txt").is_err());
        assert!(remove(&directory, "hardlink.txt").is_err());
        let junction = path.join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&foreign)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(create(&directory, "junction/new.txt").is_err());
        assert!(read(&directory, "junction/original.txt").is_err());
        fs::remove_dir(&junction).unwrap();
        create(&directory, "occupied.txt")
            .unwrap()
            .write_all(b"occupied retained bytes")
            .unwrap();
        let blocker = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path.join("occupied.txt"))
            .unwrap();
        assert!(remove(&directory, "occupied.txt").is_err());
        drop(blocker);
        assert_eq!(
            fs::read(path.join("occupied.txt")).unwrap(),
            b"occupied retained bytes"
        );
        remove(&directory, "occupied.txt").unwrap();
        assert_eq!(
            fs::read(foreign.join("original.txt")).unwrap(),
            b"retained foreign bytes"
        );
        assert!(!foreign.join("new.txt").exists());
    }

    #[test]
    fn a_stale_identity_cannot_delete_its_replacement() {
        let (_temporary, path, directory) = fixture();
        create(&directory, "selected.txt")
            .unwrap()
            .write_all(b"selected old file")
            .unwrap();
        let selected = read(&directory, "selected.txt").unwrap();
        let expected = native::identity(&selected).unwrap();
        fs::rename(path.join("selected.txt"), path.join("retained-old.txt")).unwrap();
        create(&directory, "selected.txt")
            .unwrap()
            .write_all(b"foreign replacement")
            .unwrap();
        assert!(native::remove_file(&path.join("selected.txt"), &expected).is_err());
        assert_eq!(
            fs::read(path.join("selected.txt")).unwrap(),
            b"foreign replacement"
        );
        assert_eq!(
            fs::read(path.join("retained-old.txt")).unwrap(),
            b"selected old file"
        );
    }
}
