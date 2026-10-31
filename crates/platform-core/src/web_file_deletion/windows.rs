//! Windows counterpart of the identity-bound, same-volume file deletion plan.
use crate::{
    file_identity::PersistedIdentity, web_project_ownership::WebProjectOwnership, ApiError,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;
type Result<T> = std::result::Result<T, ApiError>;
const PREFIX: &str = ".panda-delete-file-";
const PAYLOAD: &str = "payload";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FileDeletionPlan {
    pub(crate) original: PathBuf,
    pub(crate) expected: PersistedIdentity,
    pub(crate) parent_identity: PersistedIdentity,
    pub(crate) staging_path: PathBuf,
    pub(crate) staging_identity: Option<PersistedIdentity>,
    pub(crate) state: FileDeletionState,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FileDeletionState {
    Preparing,
    RenameIntent,
    Staged,
    Removing,
    Removed,
}

impl FileDeletionPlan {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.original.is_absolute()
            || self.original.file_name().is_none()
            || self
                .original
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
            || self.staging_path.parent() != self.original.parent()
        {
            return Err(changed("文件删除计划路径无效。"));
        }
        let name = self
            .staging_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(PREFIX))
            .ok_or_else(|| changed("文件删除暂存路径无效。"))?;
        if Uuid::parse_str(name)
            .ok()
            .is_none_or(|id| id.is_nil() || id.to_string() != name)
        {
            return Err(changed("文件删除暂存标识无效。"));
        }
        if self.staging_identity.is_none() && self.state != FileDeletionState::Preparing {
            return Err(changed("文件删除暂存身份缺失。"));
        }
        for identity in std::iter::once(&self.expected)
            .chain(std::iter::once(&self.parent_identity))
            .chain(self.staging_identity.iter())
        {
            require_windows_identity(identity)?;
        }
        Ok(())
    }

    /// Owner updates may advance progress, never widen a recorded path/identity.
    pub(crate) fn same_authorization(&self, previous: &Self) -> bool {
        self.original == previous.original
            && self.expected.matches_captured(&previous.expected)
            && self
                .parent_identity
                .matches_captured(&previous.parent_identity)
            && self.staging_path == previous.staging_path
            && match (&previous.staging_identity, &self.staging_identity) {
                (None, _) => true,
                (Some(a), Some(b)) => a.matches_captured(b),
                (Some(_), None) => false,
            }
    }

    pub(crate) fn staged_payload(&self) -> PathBuf {
        self.staging_path.join(PAYLOAD)
    }
}

fn changed(message: impl Into<String>) -> ApiError {
    ApiError::new("WEB_PROJECT_MATERIAL_CHANGED", message)
}
fn unavailable(error: impl std::fmt::Display) -> ApiError {
    ApiError::new("WEB_PROJECT_MATERIAL_UNAVAILABLE", error.to_string())
}
fn require_windows_identity(identity: &PersistedIdentity) -> Result<()> {
    if identity.windows_file_id.is_none() || identity.volume_uuid.is_some() {
        return Err(changed("文件删除计划缺少完整 Windows 文件身份。"));
    }
    Ok(())
}

#[path = "windows/backend.rs"]
mod backend;
#[path = "windows/io.rs"]
mod io;
#[cfg(test)]
pub(crate) use backend::fixture_remove_after_capture;
pub(crate) use backend::{owned_scopes, remove_owned_file};
