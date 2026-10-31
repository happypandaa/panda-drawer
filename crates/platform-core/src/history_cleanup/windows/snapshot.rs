//! Native counterparts of the original cleanup inventory. Only full file IDs,
//! actual types, logical sizes and Windows read-only permission bits are used;
//! existing object owners/DACLs and change timestamps are not new admission.
use super::*;
use platform_native::windows::{files as native, removal as native_removal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Identity {
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) windows_file_id: [u8; 16],
}
impl Identity {
    pub(super) fn of(file: &File) -> Result<Self, ApiError> {
        let observed = native::identity(file)?;
        Ok(Self {
            device: observed.volume_serial,
            inode: u64::from_le_bytes(observed.file_id[..8].try_into().unwrap()),
            windows_file_id: observed.file_id,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) kind: String,
    pub(super) identity: Identity,
    pub(super) bytes: u64,
    pub(super) digest: Option<String>,
    pub(super) target: Option<PathBuf>,
    pub(super) read_only: bool,
}

pub(super) fn directory_identity(path: &Path) -> Result<Identity, ApiError> {
    let checked = native::open_no_reparse(path)?;
    if !checked.identity.directory {
        return Err(error("Expected an ordinary directory"));
    }
    Identity::of(&checked.file)
}
pub(super) fn parent_identities(path: &Path) -> Result<Vec<(PathBuf, Identity)>, ApiError> {
    if !normalized(path) {
        return Err(error("Unnormalized cleanup path"));
    }
    path.ancestors()
        .skip(1)
        .map(|path| Ok((path.to_owned(), directory_identity(path)?)))
        .collect()
}
pub(super) fn directory_names(directory: &File) -> Result<Vec<PathBuf>, ApiError> {
    let (actual, _scope) = native::directory_scope(directory)?;
    let mut result = Vec::new();
    for entry in fs::read_dir(actual)? {
        result.push(PathBuf::from(entry?.file_name()));
    }
    Ok(result)
}
fn child_path(parent: &File, name: &Path) -> Result<(PathBuf, native::PinnedDirectory), ApiError> {
    let mut parts = name.components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(error("Unsafe inventory child name"));
    }
    let (actual, scope) = native::directory_scope(parent)?;
    Ok((actual.join(name), scope))
}
pub(super) fn open_directory_at(parent: &File, name: &Path) -> Result<File, ApiError> {
    let (path, _scope) = child_path(parent, name)?;
    native::directory_file(&path).map_err(Into::into)
}
pub(super) fn open_file_at(parent: &File, name: &Path) -> Result<File, ApiError> {
    let (path, _scope) = child_path(parent, name)?;
    native::open_read_file_allowing_hard_links(&path).map_err(Into::into)
}
pub(super) fn read_link_at(parent: &File, name: &Path) -> Result<PathBuf, ApiError> {
    let (path, _scope) = child_path(parent, name)?;
    fs::read_link(path).map_err(Into::into)
}
pub(super) fn hash_reader(file: &mut File) -> Result<String, ApiError> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn observed_at(parent: &File, name: &Path, relative: PathBuf) -> Result<Entry, ApiError> {
    let (path, _scope) = child_path(parent, name)?;
    let observed = native_removal::inspect(&path)?;
    let metadata = observed.file.metadata()?;
    let reparse = platform_native::filesystem::is_reparse(&metadata);
    let kind = if reparse {
        "symlink"
    } else if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        return Err(error("Unsupported special file in cleanup tree"));
    };
    Ok(Entry {
        path: relative,
        kind: kind.into(),
        identity: Identity::of(&observed.file)?,
        bytes: if kind == "file" { metadata.len() } else { 0 },
        digest: None,
        target: if reparse {
            Some(read_link_at(parent, name)?)
        } else {
            None
        },
        read_only: metadata.permissions().readonly(),
    })
}
pub(super) fn matches_at(parent: &File, name: &Path, entry: &Entry) -> Result<(), ApiError> {
    let current = observed_at(parent, name, entry.path.clone())?;
    if current.identity != entry.identity
        || current.kind != entry.kind
        || current.bytes != entry.bytes
        || current.read_only != entry.read_only
    {
        return Err(error(
            "Cleanup entry identity, type, size or permissions changed",
        ));
    }
    Ok(())
}
pub(super) fn scan(path: &Path) -> Result<Vec<Entry>, ApiError> {
    scan_with_content(path, true)
}
pub(super) fn scan_with_content(path: &Path, contents: bool) -> Result<Vec<Entry>, ApiError> {
    let identity = directory_identity(path)?;
    let root = native::directory_file(path)?;
    if Identity::of(&root)? != identity {
        return Err(error("Cleanup root changed during scan"));
    }
    fn walk(
        directory: &File,
        relative: &Path,
        device: u64,
        out: &mut Vec<Entry>,
        contents: bool,
    ) -> Result<(), ApiError> {
        if relative.components().count() > 256 || out.len() > 500_000 {
            return Err(error("Cleanup inventory exceeds its depth or entry bound"));
        }
        let mut children = directory_names(directory)?;
        children.sort();
        for name in children {
            if out.len() >= 500_000 {
                return Err(error("Cleanup inventory exceeds its entry bound"));
            }
            let relative = relative.join(&name);
            if relative.to_str().is_none() {
                return Err(error("Non-UTF8 cleanup filename"));
            }
            let mut entry = observed_at(directory, &name, relative.clone())?;
            if entry.identity.device != device {
                return Err(error("Nested mount or changed filesystem"));
            }
            if entry.kind == "file" && contents {
                let mut file = open_file_at(directory, &name)?;
                if Identity::of(&file)? != entry.identity {
                    return Err(error("File changed while scanning"));
                }
                entry.digest = Some(hash_reader(&mut file)?);
            }
            matches_at(directory, &name, &entry)?;
            let child = if entry.kind == "directory" {
                Some(open_directory_at(directory, &name)?)
            } else {
                None
            };
            if let Some(child) = &child {
                if Identity::of(child)? != entry.identity {
                    return Err(error("Directory changed during scan"));
                }
            }
            out.push(entry.clone());
            if let Some(child) = child {
                walk(&child, &relative, device, out, contents)?;
            }
            matches_at(directory, &name, &entry)?;
        }
        Ok(())
    }
    let mut entries = Vec::new();
    walk(
        &root,
        Path::new(""),
        identity.device,
        &mut entries,
        contents,
    )?;
    if directory_identity(path)? != identity {
        return Err(error("Cleanup root changed during scan"));
    }
    Ok(entries)
}
