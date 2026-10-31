//! The three selected members are bound independently; their common parent is
//! only a display location, never authority to count or protect its whole tree.
use crate::ApiError;
use platform_native::windows::files as native;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stamp {
    pub path: PathBuf,
    pub volume_serial: u64,
    pub file_id: [u8; 16],
    pub directory: bool,
    pub bytes: u64,
    pub created_100ns: u64,
    pub modified_100ns: u64,
    pub attributes: u32,
    pub links: u32,
}
impl Stamp {
    pub(crate) fn capture(path: &Path) -> Result<Self, ApiError> {
        let opened = native::open_no_reparse(path)?;
        let metadata = opened.file.metadata()?;
        let path = fs::canonicalize(path)?;
        let named = native::open_no_reparse(&path)?;
        if !opened.identity.same_object(&named.identity) {
            return Err(ApiError::new(
                "INSTALLATION_SCOPE_CHANGED",
                "The selected installation member changed while binding its path",
            ));
        }
        Ok(Self {
            path,
            volume_serial: opened.identity.volume_serial,
            file_id: opened.identity.file_id,
            directory: opened.identity.directory,
            bytes: metadata.len(),
            created_100ns: metadata.creation_time(),
            modified_100ns: metadata.last_write_time(),
            attributes: metadata.file_attributes(),
            links: opened.identity.links,
        })
    }
    pub(crate) fn same_object(&self, other: &Self) -> bool {
        self.volume_serial == other.volume_serial
            && self.file_id == other.file_id
            && self.directory == other.directory
    }
}

pub(crate) struct Member {
    pub kind: &'static str,
    pub path: PathBuf,
    pub directory: bool,
    pub original: Option<Stamp>,
    pub binding_error: Option<String>,
}
impl Member {
    fn capture(kind: &'static str, path: &Path, directory: bool) -> Self {
        let captured = Stamp::capture(path).and_then(|stamp| {
            if stamp.directory != directory {
                return Err(ApiError::new(
                    "INSTALLATION_MEMBER_TYPE",
                    "Selected installation member has the wrong object type",
                ));
            }
            Ok(stamp)
        });
        let (original, binding_error) = match captured {
            Ok(stamp) => (Some(stamp), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            kind,
            path: original
                .as_ref()
                .map(|stamp| stamp.path.clone())
                .unwrap_or_else(|| {
                    fs::canonicalize(path)
                        .ok()
                        .or_else(|| {
                            Some(
                                fs::canonicalize(path.parent()?)
                                    .ok()?
                                    .join(path.file_name()?),
                            )
                        })
                        .unwrap_or_else(|| path.to_path_buf())
                }),
            directory,
            original,
            binding_error,
        }
    }

    pub(crate) fn observe(&self) -> Result<Stamp, ApiError> {
        let original = self.original.as_ref().ok_or_else(|| {
            ApiError::new(
                "INSTALLATION_MEMBER_UNBOUND",
                self.binding_error
                    .as_deref()
                    .unwrap_or("Member was not bound"),
            )
        })?;
        let current = Stamp::capture(&self.path)?;
        if !original.same_object(&current) || (!self.directory && original != &current) {
            return Err(ApiError::new(
                "INSTALLATION_SCOPE_CHANGED",
                "The original installation member identity or executable stamp changed",
            ));
        }
        Ok(current)
    }
}

pub(super) struct Scope {
    pub members: [Member; 3],
}
impl Scope {
    pub fn capture(manager: &Path, cli: &Path, resources: &Path) -> Self {
        // Capture errors affect accounting, not runtime admission. No owner,
        // private-DACL, executable signature or single-link rule is added.
        Self {
            members: [
                Member::capture("managerExecutable", manager, false),
                Member::capture("cliExecutable", cli, false),
                Member::capture("resources", resources, true),
            ],
        }
    }

    pub fn require_destination(&self, destination: &Path) -> Result<(), ApiError> {
        let parent = destination
            .parent()
            .ok_or_else(|| ApiError::new("INVALID_PATH", "Delivery destination has no parent"))?;
        let parent = fs::canonicalize(parent)?;
        let target = parent.join(destination.file_name().ok_or_else(|| {
            ApiError::new("INVALID_PATH", "Delivery destination has no filename")
        })?);
        for member in &self.members {
            let lexical = if member.directory {
                target.starts_with(&member.path)
            } else {
                target == member.path
            };
            let bound = member.original.as_ref().is_some_and(|original| {
                if member.directory {
                    parent.ancestors().any(|ancestor| {
                        Stamp::capture(ancestor).is_ok_and(|current| original.same_object(&current))
                    })
                } else {
                    Stamp::capture(&target).is_ok_and(|current| original.same_object(&current))
                }
            });
            if lexical || bound {
                return Err(ApiError::new(
                    "INSTALLATION_DESTINATION_PROTECTED",
                    "Choose a delivery location outside the selected platform executables and resources",
                )
                .detail("member", member.kind)
                .detail("path", json!(target)));
            }
        }
        Ok(())
    }

    pub fn add_platform_info(&self, info: &mut Value) {
        info["installationPath"] = json!(self.members[0].path.parent());
        info["installationScope"] = json!({
            "kind":"selectedWindowsMembers", "wholeDirectory":false,
            "members":self.members.iter().map(|member| json!({
                "kind":member.kind,"path":member.path,"directory":member.directory,
                "bound":member.original.is_some(),"bindingIssue":member.binding_error,
            })).collect::<Vec<_>>()
        });
    }
}

#[cfg(test)]
mod tests;
