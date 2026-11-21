//! Read-only Windows handle/identity verification shared by the compiler and
//! the production runtime. A runtime guard retains every verified tool file.
use super::*;
use platform_native::{filesystem as native_fs, windows::files as native};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions},
    io::Read,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

pub struct VerifiedResources {
    _files: Vec<File>,
    _directories: Vec<native::PinnedDirectory>,
}

fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    a.file_size() == b.file_size()
        && a.creation_time() == b.creation_time()
        && a.last_write_time() == b.last_write_time()
        && a.file_attributes() == b.file_attributes()
}

fn absolute(path: &Path) -> Result<(), String> {
    if !native_fs::is_explicit_non_root_absolute_path(path) {
        return Err("Expected an explicit non-root Windows resource path".into());
    }
    Ok(())
}

fn locked_file(path: &Path, max: u64) -> Result<File, String> {
    absolute(path)?;
    let checked = native::open_no_reparse(path).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)
        .map_err(|e| e.to_string())?; // FILE_SHARE_READ / OPEN_REPARSE_POINT
    let id = native::identity(&file).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if id.directory
        || id.links != 1
        || !id.same_object(&checked.identity)
        || native_fs::is_reparse(&meta)
        || meta.len() > max
    {
        return Err("Expected an independent ordinary Windows resource file".into());
    }
    Ok(file)
}

pub fn read_manifest(path: &Path) -> Result<Vec<u8>, String> {
    absolute(path)?;
    let _scope = native::pin_directory(path.parent().ok_or("Manifest parent missing")?)
        .map_err(|e| e.to_string())?;
    let mut file = locked_file(path, MAX_MANIFEST_BYTES as u64)?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != before.len()
        || !unchanged(&before, &file.metadata().map_err(|e| e.to_string())?)
    {
        return Err("Windows resource manifest changed during its bounded read".into());
    }
    Ok(bytes)
}

fn directory_names(path: &Path) -> Result<Vec<OsString>, String> {
    let mut names = fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

fn verify_payload_pinned(root: &Path, payload: &Payload) -> Result<VerifiedResources, String> {
    absolute(root)?;
    let expected: BTreeMap<_, _> = payload
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    if expected.len() != payload.entries.len() || expected.is_empty() || expected.len() > MAX_FILES
    {
        return Err("Invalid compiled Windows resource member set".into());
    }
    let mut directories = BTreeSet::new();
    for entry in &payload.entries {
        if !relative(&entry.path) {
            return Err("Unsafe compiled Windows resource member".into());
        }
        let mut parent = Path::new(&entry.path).parent();
        while let Some(path) = parent.filter(|p| !p.as_os_str().is_empty()) {
            directories.insert(path.to_string_lossy().replace('\\', "/"));
            parent = path.parent();
        }
    }
    struct Scan {
        files: Vec<File>,
        directories: Vec<native::PinnedDirectory>,
        snapshots: Vec<(PathBuf, Metadata, Vec<OsString>)>,
        seen: BTreeSet<String>,
    }
    fn visit(
        root: &Path,
        directory: &Path,
        expected: &BTreeMap<&str, &Entry>,
        allowed_dirs: &BTreeSet<String>,
        scan: &mut Scan,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("Windows resource directory depth exceeded".into());
        }
        let pin = native::pin_directory(directory).map_err(|e| e.to_string())?;
        let before = pin.file.metadata().map_err(|e| e.to_string())?;
        let names = directory_names(directory)?;
        if names.is_empty() {
            return Err("Unexpected empty Windows tool directory".into());
        }
        for name in &names {
            let path = directory.join(name);
            let relative_name = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .components()
                .map(|part| {
                    part.as_os_str()
                        .to_str()
                        .map(str::to_owned)
                        .ok_or("Non UTF-8 Windows tool member")
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            if !relative(&relative_name) {
                return Err("Unsafe Windows tool member".into());
            }
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if native_fs::is_reparse(&meta) {
                return Err("Windows tool reparse point is forbidden".into());
            }
            if meta.is_dir() {
                if !allowed_dirs.contains(&relative_name) {
                    return Err("Unexpected Windows tool directory".into());
                }
                visit(root, &path, expected, allowed_dirs, scan, depth + 1)?;
            } else {
                let entry = expected
                    .get(relative_name.as_str())
                    .ok_or("Unexpected Windows tool file")?;
                let mut file = locked_file(&path, MAX_FILE_BYTES)?;
                let id = native::identity(&file).map_err(|e| e.to_string())?;
                let before = file.metadata().map_err(|e| e.to_string())?;
                if before.len() != entry.size {
                    return Err("Windows tool file size differs".into());
                }
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 65536];
                let mut total = 0u64;
                loop {
                    let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
                    if count == 0 {
                        break;
                    }
                    total = total
                        .checked_add(count as u64)
                        .ok_or("Windows tool file size overflow")?;
                    if total > entry.size {
                        return Err("Windows tool file grew during verification".into());
                    }
                    hash.update(&buffer[..count]);
                }
                let after = native::identity(&file).map_err(|e| e.to_string())?;
                if total != entry.size
                    || format!("{:x}", hash.finalize()) != entry.sha256
                    || !id.same_object(&after)
                    || after.links != 1
                    || !unchanged(&before, &file.metadata().map_err(|e| e.to_string())?)
                {
                    return Err("Windows tool file bytes or identity changed".into());
                }
                if !scan.seen.insert(relative_name) {
                    return Err("Duplicate Windows tool file".into());
                }
                scan.files.push(file);
            }
        }
        scan.snapshots
            .push((directory.to_path_buf(), before, names));
        scan.directories.push(pin);
        Ok(())
    }
    let mut scan = Scan {
        files: Vec::new(),
        directories: Vec::new(),
        snapshots: Vec::new(),
        seen: BTreeSet::new(),
    };
    visit(root, root, &expected, &directories, &mut scan, 0)?;
    if scan.seen.len() != expected.len() {
        return Err("Required Windows tool members are missing".into());
    }
    for (path, before, names) in &scan.snapshots {
        let checked = native::pin_directory(path).map_err(|e| e.to_string())?;
        if !unchanged(before, &checked.file.metadata().map_err(|e| e.to_string())?)
            || *names != directory_names(path)?
        {
            return Err("Windows tool directory changed during verification".into());
        }
    }
    Ok(VerifiedResources {
        _files: scan.files,
        _directories: scan.directories,
    })
}

pub fn verify_payload(root: &Path, payload: &Payload) -> Result<(), String> {
    verify_payload_pinned(root, payload).map(drop)
}

pub fn pin_verified_resources(
    root: &Path,
    manifest: &Manifest,
) -> Result<VerifiedResources, String> {
    absolute(root)?;
    let pin = native::pin_directory(root).map_err(|e| e.to_string())?;
    let mut guard = VerifiedResources {
        _files: Vec::new(),
        _directories: vec![pin],
    };
    for kind in [
        PayloadKind::NodeRuntime,
        PayloadKind::DevelopmentNode,
        PayloadKind::DevelopmentGit,
    ] {
        let mut payload_guard =
            verify_payload_pinned(&root.join(kind.root()), payload(manifest, kind))?;
        guard._files.append(&mut payload_guard._files);
        guard._directories.append(&mut payload_guard._directories);
    }
    Ok(guard)
}

pub fn verify_resources(root: &Path, manifest: &Manifest) -> Result<(), String> {
    pin_verified_resources(root, manifest).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> (tempfile::TempDir, PathBuf, Payload) {
        let directory = tempfile::Builder::new()
            .prefix("disposable-windows-production-tools-")
            .tempdir()
            .unwrap();
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("node-runtime");
        native::create_private_directory(&root).unwrap();
        fs::write(root.join("member.txt"), b"fixed bytes").unwrap();
        let entries = vec![Entry {
            path: "member.txt".into(),
            size: 11,
            sha256: digest(b"fixed bytes"),
        }];
        let (file_count, total_bytes, sha256) = tree(entries.iter()).unwrap();
        (
            directory,
            root,
            Payload {
                root: "node-runtime".into(),
                tool_version: "synthetic".into(),
                npm_version: None,
                file_count,
                total_bytes,
                sha256,
                entries,
            },
        )
    }
    #[test]
    fn actual_windows_resource_guard_rejects_changes_extras_and_hardlinks() {
        let (directory, root, payload) = sample();
        let guard = verify_payload_pinned(&root, &payload).unwrap();
        assert!(fs::write(root.join("member.txt"), b"replacement").is_err());
        assert!(fs::rename(root.join("member.txt"), root.join("renamed.txt")).is_err());
        drop(guard);
        fs::write(root.join("extra.txt"), b"unknown").unwrap();
        assert!(verify_payload(&root, &payload).is_err());
        fs::remove_file(root.join("extra.txt")).unwrap();
        fs::write(root.join("member.txt"), b"other bytes").unwrap();
        assert!(verify_payload(&root, &payload).is_err());
        fs::write(root.join("member.txt"), b"fixed bytes").unwrap();
        fs::hard_link(root.join("member.txt"), directory.path().join("alias.txt")).unwrap();
        assert!(verify_payload(&root, &payload).is_err());
    }
}
