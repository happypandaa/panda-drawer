//! Exact data-slot evidence and exclusive, descriptor-relative moves. No helper
//! deletes a file, replaces an occupied destination, or follows a symlink.
use super::*;
use crate::projects::data_restore_registration::{read_proof, FileProof};
use crate::{file_identity::PersistedIdentity, files};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectoryProof {
    #[serde(
        deserialize_with = "crate::projects::data_restore_registration::strict_identity::deserialize"
    )]
    pub identity: PersistedIdentity,
    pub mode: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TreeProof {
    pub root: DirectoryProof,
    pub directories: BTreeMap<String, DirectoryProof>,
    pub files: BTreeMap<String, FileProof>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EntryProof {
    pub file: Option<FileProof>,
    pub tree: Option<TreeProof>,
}
impl DirectoryProof {
    pub fn capture(path: &Path) -> Result<Self, ApiError> {
        let directory = files::root(path)?;
        let metadata = directory.metadata()?;
        let identity = PersistedIdentity::capture(path).map_err(|_| changed())?;
        if identity.is_legacy()
            || identity.device != metadata.dev()
            || identity.inode != metadata.ino()
        {
            return Err(changed());
        }
        Ok(Self {
            identity,
            mode: metadata.mode(),
        })
    }
    pub fn matches(&self, other: &Self) -> bool {
        self.identity.matches_captured(&other.identity) && self.mode == other.mode
    }
}
impl EntryProof {
    pub fn capture(
        path: &Path,
        directory: bool,
        check: &mut impl FnMut() -> Result<(), ApiError>,
    ) -> Result<Option<Self>, ApiError> {
        check()?;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                check_no_symlinks(path, true)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        if !directory {
            return Ok(Some(Self {
                file: Some(read_proof(path, 256_000_000, false)?.0),
                tree: None,
            }));
        }
        let root = DirectoryProof::capture(path)?;
        let mut directories = BTreeMap::new();
        let mut found = BTreeMap::new();
        let mut pending = vec![String::new()];
        let mut total = 0u64;
        while let Some(relative) = pending.pop() {
            check()?;
            let parent = path.join(&relative);
            let before = DirectoryProof::capture(&parent)?;
            for entry in fs::read_dir(&parent)? {
                check()?;
                let entry = entry?;
                let name = entry.file_name().into_string().map_err(|_| changed())?;
                let key = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                files::parts(&key)?;
                if directories.len() + found.len() >= 10_000 {
                    return Err(limit());
                }
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.is_dir() {
                    directories.insert(key.clone(), DirectoryProof::capture(&entry.path())?);
                    pending.push(key);
                } else if metadata.is_file() {
                    let proof = read_proof(&entry.path(), 256_000_000, false)?.0;
                    total = total.checked_add(proof.bytes).ok_or_else(limit)?;
                    if total > version_data::SnapshotLimits::default().max_total_bytes {
                        return Err(limit());
                    }
                    found.insert(key, proof);
                } else {
                    return Err(changed());
                }
            }
            if !before.matches(&DirectoryProof::capture(&parent)?) {
                return Err(changed());
            }
        }
        let tree = TreeProof {
            root,
            directories,
            files: found,
        };
        if !tree.root.matches(&DirectoryProof::capture(path)?) {
            return Err(changed());
        }
        Ok(Some(Self {
            file: None,
            tree: Some(tree),
        }))
    }
    pub fn matches(&self, other: &Self) -> bool {
        match (&self.file, &other.file, &self.tree, &other.tree) {
            (Some(a), Some(b), None, None) => a.matches(b),
            (None, None, Some(a), Some(b)) => {
                a.root.matches(&b.root)
                    && a.directories.len() == b.directories.len()
                    && a.files.len() == b.files.len()
                    && a.directories
                        .iter()
                        .all(|(p, v)| b.directories.get(p).is_some_and(|o| v.matches(o)))
                    && a.files
                        .iter()
                        .all(|(p, v)| b.files.get(p).is_some_and(|o| v.matches(o)))
            }
            _ => false,
        }
    }
    pub fn validate_shape(&self) -> Result<(), ApiError> {
        match (&self.file, &self.tree) {
            (Some(file), None)
                if file.bytes <= 256_000_000
                    && !file.identity.is_legacy()
                    && valid_sha(&file.sha256) =>
            {
                Ok(())
            }
            (None, Some(tree))
                if !tree.root.identity.is_legacy()
                    && tree.directories.len() + tree.files.len() <= 10_000 =>
            {
                for (path, dir) in &tree.directories {
                    files::parts(path)?;
                    if dir.identity.is_legacy() {
                        return Err(invalid());
                    }
                }
                let mut total = 0u64;
                for (path, file) in &tree.files {
                    files::parts(path)?;
                    if file.identity.is_legacy()
                        || !valid_sha(&file.sha256)
                        || file.bytes > 256_000_000
                    {
                        return Err(invalid());
                    }
                    total = total.checked_add(file.bytes).ok_or_else(limit)?;
                }
                if total > version_data::SnapshotLimits::default().max_total_bytes {
                    return Err(limit());
                }
                Ok(())
            }
            _ => Err(invalid()),
        }
    }
}

pub(crate) fn same_optional(a: &Option<EntryProof>, b: &Option<EntryProof>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.matches(b),
        _ => false,
    }
}
pub(crate) fn capture_parents(
    root: &Path,
    paths: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, DirectoryProof>, ApiError> {
    let mut result = BTreeMap::new();
    result.insert(String::new(), DirectoryProof::capture(root)?);
    for path in paths {
        let mut parent = Path::new(&path).parent();
        while let Some(value) = parent.filter(|value| !value.as_os_str().is_empty()) {
            let key = value.to_str().ok_or_else(invalid)?.to_string();
            result
                .entry(key.clone())
                .or_insert(DirectoryProof::capture(&root.join(&key))?);
            parent = value.parent();
        }
    }
    Ok(result)
}
pub(crate) fn verify_parents(
    root: &Path,
    parents: &BTreeMap<String, DirectoryProof>,
) -> Result<(), ApiError> {
    for (path, proof) in parents {
        if !proof.matches(&DirectoryProof::capture(&root.join(path))?) {
            return Err(changed());
        }
    }
    Ok(())
}
pub(crate) fn copy_target(
    snapshot: &version_data::PreparedDataSnapshot,
    staging: &Path,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    for kind in ["database", "wal", "shm", "journal", "attachments"] {
        private_dir(&staging.join("slots").join(kind))?;
    }
    private_dir(&staging.join("metadata-attempts"))?;
    private_dir(&staging.join("metadata-reserve"))?;
    let reserves = files::root(&staging.join("metadata-reserve"))?;
    for name in ["completion", "terminal"] {
        files::create(&reserves, name)?.sync_all()?;
    }
    reserves.sync_all()?;
    let destination = staging.join("slots/attachments/after");
    private_dir(&destination)?;
    let output = files::root(&destination)?;
    for directory in &snapshot.manifest.directories {
        check()?;
        files::parent(&output, &format!("{directory}/.probe"), true)?;
    }
    let source = files::root(&snapshot.root.join("attachments"))?;
    for path in snapshot.manifest.attachments.keys() {
        copy_file(
            files::read(&source, path)?,
            files::create(&output, path)?,
            check,
        )?;
    }
    let source = files::root(&snapshot.root)?;
    let output = files::root(&staging.join("slots/database"))?;
    copy_file(
        files::read(&source, "database.sqlite")?,
        files::create(&output, "after")?,
        check,
    )?;
    sync_ordinary(&staging.join("slots"))?;
    Ok(())
}
fn copy_file(
    mut input: File,
    mut output: File,
    check: &mut impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        check()?;
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        total += size as u64;
        if total > 256_000_000 {
            return Err(limit());
        }
        output.write_all(&buffer[..size])?;
    }
    output.sync_all()?;
    Ok(())
}
pub(crate) fn sync_ordinary(root: &Path) -> Result<(), ApiError> {
    let mut pending = vec![root.to_path_buf()];
    let mut directories = vec![];
    let mut count = 0usize;
    while let Some(path) = pending.pop() {
        let directory = files::root(&path)?;
        directories.push(directory);
        for entry in fs::read_dir(&path)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            count += 1;
            if count > 10_020 {
                return Err(limit());
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() && metadata.nlink() == 1 {
                File::open(path)?.sync_all()?;
            } else {
                return Err(changed());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        directory.sync_all()?;
    }
    Ok(())
}

pub(crate) fn move_exclusive_with(
    from: &Path,
    to: &Path,
    after_rename: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };
        let left = files::root(from.parent().ok_or_else(invalid)?)?;
        let right = files::root(to.parent().ok_or_else(invalid)?)?;
        if left.metadata()?.dev() != right.metadata()?.dev() {
            return Err(invalid());
        }
        let from = CString::new(from.file_name().ok_or_else(invalid)?.as_bytes())
            .map_err(|_| invalid())?;
        let to =
            CString::new(to.file_name().ok_or_else(invalid)?.as_bytes()).map_err(|_| invalid())?;
        if unsafe {
            libc::renameatx_np(
                left.as_raw_fd(),
                from.as_ptr(),
                right.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(changed());
        }
        after_rename()?;
        left.sync_all()?;
        right.sync_all()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to, after_rename);
        Err(invalid())
    }
}
