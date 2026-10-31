//! Bind an existing persistent store record to the exact Windows directory
//! before WebView2 receives it. Binding neither creates nor clears a profile.
use super::{invalid, project_identifier, validate, ProjectKind, Result, StoreLedger, StoreRecord};
use crate::{file_identity::PersistedIdentity, ApiError};
use platform_native::windows::files as native;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// The directory identity comes from the persisted store record. Keeping this
/// value alive pins both that directory and its actual ancestors against rename.
/// Its fields cannot be constructed from an arbitrary caller-selected path.
pub struct BoundProfile {
    directory: PathBuf,
    identity: PersistedIdentity,
    scope: native::PinnedDirectory,
    kind: ProjectKind,
    id: String,
    identifier: [u8; 16],
}
impl BoundProfile {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn kind(&self) -> ProjectKind {
        self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn identifier(&self) -> [u8; 16] {
        self.identifier
    }

    pub fn verify(&self) -> Result<()> {
        let actual = native::open_no_reparse(&self.directory).map_err(|_| invalid())?;
        if !actual.identity.directory
            || !self
                .identity
                .matches_open_runtime(&actual.file)
                .map_err(|_| invalid())?
            || !self
                .identity
                .matches_open_runtime(&self.scope.file)
                .map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// Check the actual Environment7.UserDataFolder returned by WebView2,
    /// preserving the complete Windows file identity rather than path spelling.
    pub fn verify_actual_directory(&self, directory: &Path) -> Result<()> {
        self.verify()?;
        let actual = native::open_no_reparse(directory).map_err(|_| {
            ApiError::new(
                "WEBVIEW_PROFILE_ACTUAL_UNKNOWN",
                "无法核实 WebView2 实际项目存储目录，项目页面尚未加载。",
            )
        })?;
        if !actual.identity.directory
            || !self
                .identity
                .matches_open_runtime(&actual.file)
                .map_err(|_| invalid())?
        {
            return Err(ApiError::new("WEBVIEW_PROFILE_ACTUAL_MISMATCH",
                "WebView2 实际项目存储目录与本项目持久凭据不一致。项目页面尚未加载，实际目录已保留。"));
        }
        Ok(())
    }
}

fn read_record(path: &Path, kind: ProjectKind, id: &str) -> Result<Option<StoreRecord>> {
    // The Mac contract admits hard links. Reading a record does not acquire
    // authority over its other names or add a Windows single-link requirement.
    let file = match native::open_read_file_allowing_hard_links(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    if file.metadata()?.len() > 4096 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    let record: StoreRecord = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if record.version != 1
        || record.kind != kind
        || record.id != id
        || record.identifier != project_identifier(kind, id)?
    {
        return Err(invalid());
    }
    Ok(Some(record))
}

impl StoreLedger {
    /// Only a prior durable `record` grants a project profile. Missing,
    /// unknown, replaced and removal-pending roots are never adopted here.
    pub fn bind_profile(&self, kind: ProjectKind, id: &str) -> Result<BoundProfile> {
        validate(kind, id)?;
        let _data = native::pin_directory(&self.data).map_err(|_| invalid())?;
        let _container = native::pin_directory(&self.directory()).map_err(|_| invalid())?;
        if read_record(&self.confirmation_path(kind, id)?, kind, id)?.is_some() {
            return Err(ApiError::new(
                "WEBVIEW_STORE_REMOVAL_PENDING",
                "浏览器存储正在删除，不能重新创建窗口。",
            ));
        }
        let directory = self.project_directory(kind, id)?;
        let scope = native::pin_directory(&directory).map_err(|_| invalid())?;
        let record = read_record(&self.path(kind, id)?, kind, id)?.ok_or_else(invalid)?;
        if !record
            .directory_identity
            .matches_open_runtime(&scope.file)
            .map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        // Protect only this registered profile, never system or user ancestors.
        native::verify_private(&scope.file).map_err(|_| invalid())?;
        let profile = BoundProfile {
            directory,
            scope,
            identity: record.directory_identity,
            kind,
            id: id.into(),
            identifier: record.identifier,
        };
        profile.verify()?;
        Ok(profile)
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
