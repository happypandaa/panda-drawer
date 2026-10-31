//! Live closure only. This Windows marker has no invented boot/session proof;
//! startup inventory never treats it as authority to retire a crashed writer.
use super::{
    io::{self, blocked, Directory, Result, MAX_MARKER},
    Kind, WriterOwner,
};
use crate::file_identity::PersistedIdentity;
use platform_native::windows::files::{self as native, FileIdentity};
use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Marker<'a> {
    version: u32,
    platform: &'static str,
    kind: &'static str,
    owner_id: &'a str,
    token: String,
    component: &'a str,
    file_identity: PersistedIdentity,
}

/// No deleting Drop. Only the caller's actual no-child or joined Job/I/O
/// closure branch may call finish; a task status is never a closure receipt.
pub(crate) struct WriterGuard {
    path: PathBuf,
    identity: FileIdentity,
    bytes: Vec<u8>,
    file: Option<File>,
    directory: Directory,
    _owner: Directory,
    retiring: bool,
    closed: bool,
}

pub(super) fn begin(owner: &WriterOwner, component: &str) -> Result<WriterGuard> {
    begin_with(owner, component, |_, _| Ok(()))
}

fn begin_with(
    owner: &WriterOwner,
    component: &str,
    before_publish: impl FnOnce(&std::path::Path, &std::path::Path) -> Result<()>,
) -> Result<WriterGuard> {
    if component.is_empty() || component.len() > 256 || component.chars().any(char::is_control) {
        return Err(super::invalid_owner());
    }
    let selected = io::admitted_owner(owner)?;
    let directory = Directory::private(&selected.path.join("writers"), true)?;
    let token = Uuid::new_v4().to_string();
    let path = directory.path.join(format!("{token}.json"));
    let preparing = directory
        .path
        .join(format!(".preparing-{}", Uuid::new_v4()));
    let mut file = native::create_private_file_for_publication(&preparing)
        .map_err(|error| blocked(&preparing, error))?;
    let identity = native::identity(&file).map_err(|error| blocked(&preparing, error))?;
    let marker = Marker {
        version: 1,
        platform: "windows",
        kind: match owner._kind {
            Kind::Node => "node",
            Kind::Web => "web",
            Kind::Platform => "platform",
        },
        owner_id: &owner._id,
        token,
        component,
        file_identity: PersistedIdentity::from_file(&file)
            .map_err(|error| blocked(&preparing, error))?,
    };
    let bytes = serde_json::to_vec(&marker).map_err(|error| blocked(&preparing, error))?;
    if bytes.len() as u64 > MAX_MARKER {
        return Err(blocked(&preparing, "Marker exceeds budget"));
    }
    file.write_all(&bytes)
        .map_err(|error| blocked(&preparing, error))?;
    file.sync_all()
        .map_err(|error| blocked(&preparing, error))?;
    before_publish(&preparing, &path)?;
    // This exact newly created HANDLE publishes only into an absent name. Any
    // failure preserves both preparation and caller/external material.
    native::rename_private_file_handle(&file, &path, false).map_err(|error| {
        blocked(&preparing, error).detail("publishedPath", path.display().to_string())
    })?;
    file.sync_all().map_err(|error| blocked(&path, error))?;
    let mut observed = native::open_read_file(&path).map_err(|error| blocked(&path, error))?;
    native::verify_private(&observed).map_err(|error| blocked(&path, error))?;
    if !identity.same_object(&native::identity(&observed).map_err(|error| blocked(&path, error))?) {
        return Err(blocked(&path, "Published marker identity changed"));
    }
    let mut actual = Vec::new();
    (&mut observed)
        .take(MAX_MARKER + 1)
        .read_to_end(&mut actual)
        .map_err(|error| blocked(&path, error))?;
    if actual != bytes {
        return Err(blocked(&path, "Published marker bytes changed"));
    }
    directory.verify(true)?;
    Ok(WriterGuard {
        path,
        identity,
        bytes,
        file: Some(file),
        directory,
        _owner: selected,
        retiring: false,
        closed: false,
    })
}

impl WriterGuard {
    pub(crate) fn finish(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.directory.verify(true)?;
        if !self.retiring {
            // Reopen with write sharing disabled. Closing this original writer
            // handle grants no deletion permission: ID and original bytes must
            // still match on the subsequently locked ordinary private handle.
            drop(self.file.take());
            let (file, bytes, identity) = io::locked_marker(&self.path)?;
            if !self.identity.same_object(&identity) || bytes != self.bytes {
                return Err(blocked(
                    &self.path,
                    "Writer marker identity or bytes changed",
                ));
            }
            native::remove_file(&self.path, &identity)
                .map_err(|error| blocked(&self.path, error))?;
            self.retiring = true;
            drop(file);
        }
        // Another open reader can delay Windows deletion. Keep the real guard
        // pending until this fixed name is actually absent, without re-deleting
        // any replacement or treating an access/sharing error as absence.
        io::missing(&self.path)?;
        self.closed = true;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn retained_path(&self) -> PathBuf {
        self.path.clone()
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
