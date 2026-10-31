use super::{invalid_owner, lexical_path, Kind, WriterOwner};
use crate::ApiError;
use platform_native::windows::files::{self as native, FileIdentity, PinnedDirectory};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub(super) type Result<T> = std::result::Result<T, ApiError>;
pub(super) const MAX_MARKER: u64 = 4096;
const SHARE_READ: u32 = 1;
const SHARE_DELETE: u32 = 4;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

pub(super) fn blocked(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    invalid_owner()
        .detail("platform", "windows")
        .detail("capability", "durable-process-writer")
        .detail("retainedPath", path.to_string_lossy().into_owned())
        .detail("cause", cause.to_string())
}

pub(super) struct Directory {
    pub path: PathBuf,
    pub pin: PinnedDirectory,
    pub identity: FileIdentity,
}
impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let pin = native::pin_directory(path).map_err(|error| blocked(path, error))?;
        let (path, pin) =
            native::directory_scope(&pin.file).map_err(|error| blocked(path, error))?;
        let identity = native::identity(&pin.file).map_err(|error| blocked(&path, error))?;
        Ok(Self {
            path,
            pin,
            identity,
        })
    }
    pub fn private(path: &Path, create: bool) -> Result<Self> {
        if create {
            match native::create_private_directory(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(blocked(path, error)),
            }
        }
        let selected = Self::open(path)?;
        native::verify_private(&selected.pin.file).map_err(|error| blocked(path, error))?;
        Ok(selected)
    }
    pub fn verify(&self, private: bool) -> Result<()> {
        let current =
            native::open_no_reparse(&self.path).map_err(|error| blocked(&self.path, error))?;
        if !current.identity.directory || !self.identity.same_object(&current.identity) {
            return Err(blocked(&self.path, "Directory identity changed"));
        }
        if private {
            native::verify_private(&current.file).map_err(|error| blocked(&self.path, error))?;
        }
        Ok(())
    }
}

pub(super) fn owner_path(owner: &WriterOwner) -> PathBuf {
    let data = lexical_path(&owner._data);
    match owner._kind {
        Kind::Node => data.join("node-project-ownership").join(&owner._id),
        Kind::Web => data.join("web-project-ownership").join(&owner._id),
        Kind::Platform => data.join("platform-writers"),
    }
}

pub(super) fn admitted_owner(owner: &WriterOwner) -> Result<Directory> {
    let data = Directory::open(&lexical_path(&owner._data))?;
    let root = match owner._kind {
        Kind::Node => data.path.join("node-project-ownership").join(&owner._id),
        Kind::Web => data.path.join("web-project-ownership").join(&owner._id),
        Kind::Platform => data.path.join("platform-writers"),
    };
    let root = Directory::private(&root, owner._kind == Kind::Platform)?;
    if owner._kind != Kind::Platform {
        let path = root.path.join("ownership.json");
        let file = native::open_read_file(&path).map_err(|error| blocked(&path, error))?;
        native::verify_private(&file).map_err(|error| blocked(&path, error))?;
        if file
            .metadata()
            .map_err(|error| blocked(&path, error))?
            .len()
            == 0
        {
            return Err(blocked(&path, "Owner record is empty"));
        }
    }
    Ok(root)
}

/// Keep this handle through deletion: its sharing policy prevents another
/// writer from changing checked bytes while the DELETE handle is opened.
pub(super) fn locked_marker(path: &Path) -> Result<(File, Vec<u8>, FileIdentity)> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ | SHARE_DELETE)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| blocked(path, error))?;
    let metadata = file.metadata().map_err(|error| blocked(path, error))?;
    let identity = native::identity(&file).map_err(|error| blocked(path, error))?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || identity.directory
        || identity.links != 1
        || metadata.len() > MAX_MARKER
    {
        return Err(blocked(
            path,
            "Expected an independent bounded ordinary marker",
        ));
    }
    native::verify_private(&file).map_err(|error| blocked(path, error))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_MARKER + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| blocked(path, error))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(blocked(path, "Marker size changed"));
    }
    Ok((file, bytes, identity))
}

pub(super) fn missing(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(blocked(path, error)),
        Ok(_) => Err(blocked(path, "Retained writer material is present")),
    }
}
