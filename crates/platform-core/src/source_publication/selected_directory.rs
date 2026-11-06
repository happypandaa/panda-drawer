//! A selected folder stays user-owned. The wire revision binds its identity;
//! private per-entry receipts authorize publication and partial cleanup only.
use crate::{file_identity::PersistedIdentity, ApiError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedDirectory {
    pub path: PathBuf,
    pub revision: String,
}

pub fn inspect_selected_directory(path: &Path) -> Result<SelectedDirectory, ApiError> {
    #[cfg(target_os = "macos")]
    {
        macos::inspect(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err(unavailable())
    }
}

pub(crate) fn validate_selected_directory(
    selection: &SelectedDirectory,
) -> Result<PersistedIdentity, ApiError> {
    #[cfg(target_os = "macos")]
    {
        macos::validate(selection)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = selection;
        Err(unavailable())
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "当前平台尚不支持为推荐 agent 应用选择安装文件夹。",
    )
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Binding {
    pub source_id: String,
    pub app_id: String,
    pub lease_id: String,
    pub digest: String,
}

#[cfg(target_os = "macos")]
#[path = "selected_directory/macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{cleanup, prepare, publish, reconcile};
