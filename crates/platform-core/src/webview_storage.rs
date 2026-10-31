//! Native browser ownership outlives project files until the OS confirms removal.
//! The manager's store contains only global preferences and has no project owner.
use crate::{project_cleanup::ProjectKind, projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::io::Read;
use std::{fs, path::PathBuf};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{BoundProfile, ProfileSession};

type Result<T> = std::result::Result<T, ApiError>;
fn invalid() -> ApiError {
    ApiError::new(
        "WEBVIEW_STORE_IDENTITY_INVALID",
        "浏览器存储归属无法核验，未继续清理。",
    )
}
fn validate(kind: ProjectKind, id: &str) -> Result<()> {
    let mut notice =
        crate::project_cleanup::DeletedProjectNotice::new(kind, id.into(), vec![], vec![], vec![]);
    notice.normalize()
}
fn identifier(domain: &str, id: &str) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(crate::APP_IDENTIFIER.as_bytes());
    hash.update(b"/webkit-store/v1/");
    hash.update(domain.as_bytes());
    hash.update(b"/");
    hash.update(id.as_bytes());
    let bytes = hash.finalize();
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    uuid[6] = (uuid[6] & 0x0f) | 0x80;
    uuid[8] = (uuid[8] & 0x3f) | 0x80;
    uuid
}
pub fn manager_identifier() -> [u8; 16] {
    identifier("platform", "manager")
}
pub fn project_identifier(kind: ProjectKind, id: &str) -> Result<[u8; 16]> {
    validate(kind, id)?;
    Ok(identifier(kind.as_str(), id))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreRecord {
    version: u32,
    pub kind: ProjectKind,
    pub id: String,
    pub identifier: [u8; 16],
    directory_identity: crate::file_identity::PersistedIdentity,
}
#[derive(Clone)]
pub struct StoreLedger {
    data: PathBuf,
}
impl StoreLedger {
    pub fn new(data: PathBuf) -> Self {
        Self { data }
    }
    pub fn directory(&self) -> PathBuf {
        self.data.join("webview-stores")
    }
    pub fn path(&self, kind: ProjectKind, id: &str) -> Result<PathBuf> {
        Ok(self.project_directory(kind, id)?.join("store.json"))
    }
    pub fn project_directory(&self, kind: ProjectKind, id: &str) -> Result<PathBuf> {
        validate(kind, id)?;
        Ok(self.directory().join(format!("{}-{id}", kind.as_str())))
    }
    fn confirmation_path(&self, kind: ProjectKind, id: &str) -> Result<PathBuf> {
        validate(kind, id)?;
        Ok(self
            .directory()
            .join(format!("clear-{}-{id}.json", kind.as_str())))
    }
    #[cfg(unix)]
    fn decode(
        &self,
        kind: ProjectKind,
        id: &str,
        path: &std::path::Path,
        confirmed: bool,
    ) -> Result<Option<StoreRecord>> {
        check_no_symlinks(path, true)?;
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() || file.metadata()?.len() > 4096 {
            return Err(invalid());
        }
        let mut bytes = vec![];
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
        let directory = self.project_directory(kind, id)?;
        check_no_symlinks(&directory, true)?;
        match fs::symlink_metadata(&directory) {
            Ok(_)
                if record
                    .directory_identity
                    .matches_path(&directory)
                    .unwrap_or(false) => {}
            Err(error) if confirmed && error.kind() == std::io::ErrorKind::NotFound => {
                if !record
                    .directory_identity
                    .absent_on_expected_volume(&directory)
                    .map_err(|_| invalid())?
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        Ok(Some(record))
    }
    #[cfg(windows)]
    fn decode(
        &self,
        kind: ProjectKind,
        id: &str,
        path: &std::path::Path,
        confirmed: bool,
    ) -> Result<Option<StoreRecord>> {
        windows::ledger::decode(self, kind, id, path, confirmed)
    }
    #[cfg(unix)]
    fn read(&self, kind: ProjectKind, id: &str) -> Result<Option<StoreRecord>> {
        if let Some(record) = self.decode(kind, id, &self.confirmation_path(kind, id)?, true)? {
            return Ok(Some(record));
        }
        let record = self.decode(kind, id, &self.path(kind, id)?, false)?;
        if record.is_none() && self.project_directory(kind, id)?.exists() {
            return Err(invalid());
        }
        Ok(record)
    }
    #[cfg(windows)]
    fn read(&self, kind: ProjectKind, id: &str) -> Result<Option<StoreRecord>> {
        windows::ledger::read(self, kind, id)
    }
    /// A prior trusted native call observed browser exit before publishing
    /// this receipt. Its presence never claims that UDF deletion is complete.
    #[cfg(windows)]
    pub fn confirmed_cleanup_pending(&self, kind: ProjectKind, id: &str) -> Result<bool> {
        windows::ledger::confirmed_cleanup_pending(self, kind, id)
    }
    pub fn contains(&self, kind: ProjectKind, id: &str) -> Result<bool> {
        self.read(kind, id).map(|record| record.is_some())
    }
    /// Must complete before passing this identifier to a WebView builder.
    pub fn record(&self, kind: ProjectKind, id: &str) -> Result<[u8; 16]> {
        let identifier = project_identifier(kind, id)?;
        if self
            .decode(kind, id, &self.confirmation_path(kind, id)?, true)?
            .is_some()
        {
            return Err(ApiError::new(
                "WEBVIEW_STORE_REMOVAL_PENDING",
                "浏览器存储正在删除，不能重新创建窗口。",
            ));
        }
        if self.read(kind, id)?.is_none() {
            check_no_symlinks(&self.data, false)?;
            let container = self.directory();
            check_no_symlinks(&container, true)?;
            if !container.exists() {
                platform_native::filesystem::create_private_directory(&container)?;
            }
            let directory = self.project_directory(kind, id)?;
            platform_native::filesystem::create_private_directory(&directory)?;
            let directory_identity = crate::file_identity::PersistedIdentity::capture(&directory)
                .map_err(|_| invalid())?;
            crate::builds::atomic_json(
                &self.path(kind, id)?,
                &StoreRecord {
                    version: 1,
                    kind,
                    id: id.into(),
                    identifier,
                    directory_identity,
                },
            )?;
        }
        Ok(identifier)
    }
    /// Only call after the native API acknowledged removal and enumeration
    /// confirmed absence. A failed file removal retains the retry authority.
    #[cfg(unix)]
    fn persist_confirmation(&self, record: &StoreRecord) -> Result<()> {
        use std::io::Write;
        let path = self.confirmation_path(record.kind, &record.id)?;
        if self.decode(record.kind, &record.id, &path, true)?.is_some() {
            return Ok(());
        }
        let directory = self.project_directory(record.kind, &record.id)?;
        // Preparing files remain inside this project's already-bound directory.
        // The outside receipt is published before any child can be removed.
        let temporary = directory.join(format!(".clear-{}", uuid::Uuid::new_v4()));
        let mut file = platform_native::filesystem::create_new_private_file(&temporary, true)?;
        file.write_all(&serde_json::to_vec(record)?)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary, &path)?;
        fs::File::open(self.directory())?.sync_all()?;
        fs::remove_file(&temporary)?;
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
    #[cfg(windows)]
    fn persist_confirmation(&self, record: &StoreRecord) -> Result<()> {
        windows::ledger::persist_confirmation(self, record)
    }
    #[cfg(unix)]
    pub fn forget_confirmed(&self, kind: ProjectKind, id: &str) -> Result<()> {
        if let Some(record) = self.read(kind, id)? {
            self.persist_confirmation(&record)?;
            let directory = self.project_directory(kind, id)?;
            if directory.exists() {
                crate::history_cleanup::remove_owned_directory(
                    &directory,
                    &record.directory_identity,
                )?;
            }
            fs::File::open(self.directory())?.sync_all()?;
            fs::remove_file(self.confirmation_path(kind, id)?)?;
        }
        self.sync_absence(kind, id)
    }
    #[cfg(windows)]
    pub fn forget_confirmed(&self, kind: ProjectKind, id: &str) -> Result<()> {
        windows::ledger::forget_confirmed(self, kind, id)
    }
    #[cfg(unix)]
    pub(crate) fn sync_absence(&self, kind: ProjectKind, id: &str) -> Result<()> {
        if self.read(kind, id)?.is_some() {
            return Err(ApiError::new(
                "WEBVIEW_STORE_CLEANUP_UNCONFIRMED",
                "浏览器存储清理记录尚未结束。",
            ));
        }
        check_no_symlinks(&self.directory(), true)?;
        match fs::File::open(self.directory()) {
            Ok(directory) => directory.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
    #[cfg(windows)]
    pub(crate) fn sync_absence(&self, kind: ProjectKind, id: &str) -> Result<()> {
        windows::ledger::sync_absence(self, kind, id)
    }
    pub fn records(&self) -> Result<Vec<StoreRecord>> {
        let directory = self.directory();
        check_no_symlinks(&directory, true)?;
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut records = std::collections::BTreeMap::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| invalid())?;
            let name = if let Some(name) = name.strip_prefix("clear-") {
                name.strip_suffix(".json").ok_or_else(invalid)?
            } else {
                name.as_str()
            };
            let (kind, id) = if let Some(id) = name.strip_prefix("web-") {
                (ProjectKind::Web, id)
            } else if let Some(id) = name.strip_prefix("node-") {
                (ProjectKind::Node, id)
            } else {
                return Err(invalid());
            };
            records.insert(
                format!("{}-{id}", kind.as_str()),
                self.read(kind, id)?.ok_or_else(invalid)?,
            );
        }
        Ok(records.into_values().collect())
    }
    pub(crate) fn require_all_clear(&self) -> Result<()> {
        if !self.records()?.is_empty() {
            return Err(ApiError::new(
                "WEBVIEW_STORE_CLEANUP_REQUIRED",
                "项目浏览器存储尚未收到系统清理确认，请在桌面平台完成清理后重试。",
            )
            .outcome("not_started"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmed_directory_removal_tails_reopen_without_losing_identity_or_retaining_receipts() {
        for tail in ["receipt", "store-unlinked", "directory-unlinked"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().canonicalize().unwrap();
            let ledger = StoreLedger::new(data.clone());
            let a = "prj_disposable_clear_tail";
            let b = "prj_disposable_keep_tail";
            ledger.record(ProjectKind::Web, a).unwrap();
            ledger.record(ProjectKind::Web, b).unwrap();
            let record = ledger.read(ProjectKind::Web, a).unwrap().unwrap();
            ledger.persist_confirmation(&record).unwrap();
            if tail != "receipt" {
                fs::remove_file(ledger.path(ProjectKind::Web, a).unwrap()).unwrap();
            }
            if tail == "directory-unlinked" {
                fs::remove_dir(ledger.project_directory(ProjectKind::Web, a).unwrap()).unwrap();
            }
            let reopened = StoreLedger::new(data);
            assert!(reopened.contains(ProjectKind::Web, a).unwrap(), "{tail}");
            assert_eq!(reopened.records().unwrap().len(), 2, "{tail}");
            assert!(reopened.record(ProjectKind::Web, a).is_err(), "{tail}");
            reopened.forget_confirmed(ProjectKind::Web, a).unwrap();
            reopened.forget_confirmed(ProjectKind::Web, a).unwrap();
            assert!(!reopened.contains(ProjectKind::Web, a).unwrap(), "{tail}");
            assert!(reopened.contains(ProjectKind::Web, b).unwrap());
            assert!(!reopened
                .confirmation_path(ProjectKind::Web, a)
                .unwrap()
                .exists());
        }
    }
    #[test]
    fn a_confirmed_receipt_cannot_adopt_a_replaced_directory_or_an_unbound_initial_directory() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let ledger = StoreLedger::new(data.clone());
        let id = "prj_disposable_clear_replaced";
        ledger.record(ProjectKind::Web, id).unwrap();
        let record = ledger.read(ProjectKind::Web, id).unwrap().unwrap();
        ledger.persist_confirmation(&record).unwrap();
        let path = ledger.project_directory(ProjectKind::Web, id).unwrap();
        let held = data.join("disposable-held-original");
        fs::rename(&path, &held).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("foreign"), b"preserve").unwrap();
        assert!(ledger.forget_confirmed(ProjectKind::Web, id).is_err());
        assert_eq!(fs::read(path.join("foreign")).unwrap(), b"preserve");
        let unbound = "prj_disposable_unbound";
        fs::create_dir(ledger.project_directory(ProjectKind::Web, unbound).unwrap()).unwrap();
        assert!(ledger.contains(ProjectKind::Web, unbound).is_err());
        assert!(ledger.forget_confirmed(ProjectKind::Web, unbound).is_err());
    }
    #[tokio::test]
    async fn native_host_and_persistent_clear_receipt_are_required_before_finalization() {
        use crate::project_cleanup::{
            DeletedProjectNotice, NativeCleanupStage, ProjectCleanupCoordinator,
        };
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let cleanup = ProjectCleanupCoordinator::new(&data);
        let ledger = cleanup.stores();
        let a = "prj_disposable_native_receipt";
        let b = "prj_disposable_native_keep";
        ledger.record(ProjectKind::Web, a).unwrap();
        ledger.record(ProjectKind::Web, b).unwrap();
        let notice = DeletedProjectNotice::new(ProjectKind::Web, a.into(), vec![], vec![], vec![]);
        assert_eq!(
            cleanup.prepare(notice.clone()).await.unwrap_err().code,
            "WEBVIEW_NATIVE_HOST_REQUIRED"
        );
        cleanup.set_native(std::sync::Arc::new(|_, _| Box::pin(async { Ok(()) })));
        let notice = cleanup.prepare(notice).await.unwrap();
        assert_eq!(
            cleanup.purge(&notice).await.unwrap_err().code,
            "WEBVIEW_STORE_CLEANUP_UNCONFIRMED"
        );
        assert!(ledger.contains(ProjectKind::Web, a).unwrap());
        let callback_ledger = ledger.clone();
        cleanup.set_native(std::sync::Arc::new(move |stage, notice| {
            let ledger = callback_ledger.clone();
            Box::pin(async move {
                if stage == NativeCleanupStage::Purge {
                    ledger.forget_confirmed(notice.kind, &notice.id)?;
                }
                Ok(())
            })
        }));
        cleanup.purge(&notice).await.unwrap();
        cleanup.committed(&notice).await;
        assert!(!ledger.contains(ProjectKind::Web, a).unwrap());
        assert!(ledger.contains(ProjectKind::Web, b).unwrap());
    }
    #[test]
    fn stable_typed_identifiers_and_markers_are_independent_of_workspace_and_removed_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().canonicalize().unwrap();
        let ledger = StoreLedger::new(data.clone());
        let a = "prj_disposable_store_a";
        let b = "prj_disposable_store_b";
        let node = uuid::Uuid::new_v4().to_string();
        assert!(!ledger.directory().exists());
        assert!(!ledger.contains(ProjectKind::Web, a).unwrap());
        assert!(!ledger.directory().exists());
        let aid = ledger.record(ProjectKind::Web, a).unwrap();
        assert_eq!(
            aid,
            StoreLedger::new(data.clone())
                .record(ProjectKind::Web, a)
                .unwrap()
        );
        assert_ne!(aid, ledger.record(ProjectKind::Web, b).unwrap());
        assert_ne!(aid, ledger.record(ProjectKind::Node, &node).unwrap());
        assert_ne!(aid, manager_identifier());
        assert!(ledger.require_all_clear().is_err());
        ledger.forget_confirmed(ProjectKind::Web, a).unwrap();
        assert!(!ledger.contains(ProjectKind::Web, a).unwrap());
        assert_eq!(ledger.records().unwrap().len(), 2);
        ledger.forget_confirmed(ProjectKind::Web, b).unwrap();
        ledger.forget_confirmed(ProjectKind::Node, &node).unwrap();
        ledger.require_all_clear().unwrap();
        assert!(fs::read_dir(ledger.directory()).unwrap().next().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn corrupt_foreign_and_symlink_records_cannot_authorize_native_removal() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let ledger = StoreLedger::new(temp.path().canonicalize().unwrap());
        let id = "prj_disposable_store";
        ledger.record(ProjectKind::Web, id).unwrap();
        assert_eq!(
            fs::metadata(ledger.directory())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let path = ledger.path(ProjectKind::Web, id).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut record = ledger.read(ProjectKind::Web, id).unwrap().unwrap();
        record.identifier = manager_identifier();
        crate::builds::atomic_json(&path, &record).unwrap();
        assert!(ledger.records().is_err());
        assert!(ledger.forget_confirmed(ProjectKind::Web, id).is_err());
        fs::remove_file(&path).unwrap();
        let outside = temp.path().join("external-original");
        fs::write(&outside, b"preserve").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(ledger.contains(ProjectKind::Web, id).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"preserve");
    }
}
