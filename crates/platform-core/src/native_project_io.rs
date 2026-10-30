//! Opaque native writers hold the original document identity and a real project
//! read lease. Dropping a writer never invents its I/O completion receipt.
use crate::{
    durable_writer::{WriterGuard, WriterOwner},
    file_identity::PersistedIdentity,
    ApiError, ProjectHandle, ProjectStore,
};
use std::path::{Path, PathBuf};
use tokio::sync::OwnedRwLockReadGuard;

pub struct NativeProjectIoGuard {
    project: ProjectHandle,
    _lease: OwnedRwLockReadGuard<()>,
    writer: WriterGuard,
    owner: PathBuf,
    owner_identity: PersistedIdentity,
}
impl NativeProjectIoGuard {
    pub fn project_id(&self) -> &str {
        &self.project.manifest.id
    }
    pub(crate) fn try_begin(
        store: &ProjectStore,
        data: &Path,
        id: &str,
        epoch: &str,
        component: &str,
    ) -> Result<Self, ApiError> {
        let project = store.project(id)?;
        let lease = project
            .operation_gate
            .clone()
            .try_read_owned()
            .map_err(|_| ApiError::new("PROJECT_BUSY", "项目正在处理其他操作，暂不能保存文件。"))?;
        let owner = data.join("web-project-ownership").join(id);
        store
            .with_registered_epoch(id, epoch, || {
                project.validate_epoch(Some(epoch))?;
                let owner_identity = PersistedIdentity::capture(&owner)
                    .map_err(|error| ApiError::new("NATIVE_IO_OWNER_CHANGED", error.to_string()))?;
                let writer = WriterOwner::web(data, id)?.begin(component)?;
                Ok(Self {
                    project,
                    _lease: lease,
                    writer,
                    owner,
                    owner_identity,
                })
            })?
            .ok_or_else(|| {
                ApiError::new(
                    "PROJECT_RELOAD_REQUIRED",
                    "项目身份已变化，请重新打开项目后保存。",
                )
            })
    }
    pub fn validate(&self) -> Result<(), ApiError> {
        self.project
            .validate_epoch(Some(&self.project.document_epoch))?;
        if !self
            .owner_identity
            .matches_path(&self.owner)
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                "NATIVE_IO_OWNER_CHANGED",
                "项目控制目录已变化。",
            ));
        }
        Ok(())
    }
    /// A caller may create only a private child here; it must never recreate the
    /// owner itself. The project deletion owns this complete fixed subtree.
    pub fn stage_parent(&self) -> Result<PathBuf, ApiError> {
        self.validate()?;
        Ok(self.owner.join("native-downloads"))
    }
    /// Reuse this admission's read lease; taking a second read lease could wait
    /// behind a queued deletion writer while that writer waits for this guard.
    pub fn attachment_file(&self, path: &str) -> Result<std::fs::File, ApiError> {
        self.validate()?;
        let root = crate::files::root(
            &self
                .project
                .root
                .join(&self.project.manifest.attachments_dir),
        )?;
        crate::files::read(&root, path)
    }
    /// Call only after actual engine/copy I/O, handles and stage cleanup ended.
    /// Failure preserves its durable marker so deletion cannot report success.
    pub fn finish(mut self) -> Result<(), ApiError> {
        self.writer.finish()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{CreateProjectInput, RuntimePaths};
    use std::{fs, os::unix::fs::PermissionsExt};

    async fn fixture() -> (
        tempfile::TempDir,
        RuntimePaths,
        ProjectStore,
        crate::ProjectInfo,
    ) {
        let (temp, paths) = RuntimePaths::for_test();
        let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
        store.set_node_source_data_root(paths.data_root().into());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable native I/O identity".into(),
                description: None,
            })
            .await
            .unwrap();
        (temp, paths, store, project)
    }
    #[tokio::test]
    async fn native_writer_holds_the_original_epoch_read_gate_and_private_durable_receipt() {
        let (_temp, paths, store, info) = fixture().await;
        let project = store.project(&info.id).unwrap();
        let writer = NativeProjectIoGuard::try_begin(
            &store,
            paths.data_root(),
            &info.id,
            &info.document_epoch,
            "disposable-engine",
        )
        .unwrap();
        let owner = paths
            .data_root()
            .join("web-project-ownership")
            .join(&info.id);
        assert_eq!(
            writer.stage_parent().unwrap(),
            owner.join("native-downloads")
        );
        assert!(project.operation_gate.try_write().is_err());
        let markers = fs::read_dir(owner.join("writers"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(markers.len(), 1);
        assert_eq!(
            fs::metadata(&markers[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        writer.finish().unwrap();
        assert!(!markers[0].exists());
        assert!(project.operation_gate.try_write().is_ok());
        store.shutdown().await;
    }
    #[tokio::test]
    async fn stale_or_busy_native_admission_never_creates_a_writer() {
        let (_temp, paths, store, info) = fixture().await;
        let owner = paths
            .data_root()
            .join("web-project-ownership")
            .join(&info.id);
        assert!(NativeProjectIoGuard::try_begin(
            &store,
            paths.data_root(),
            &info.id,
            &uuid::Uuid::new_v4().to_string(),
            "disposable-stale"
        )
        .is_err());
        assert!(!owner.join("writers").exists());
        let project = store.project(&info.id).unwrap();
        let lease = project.operation_gate.write().await;
        assert_eq!(
            NativeProjectIoGuard::try_begin(
                &store,
                paths.data_root(),
                &info.id,
                &info.document_epoch,
                "disposable-busy"
            )
            .err()
            .unwrap()
            .code,
            "PROJECT_BUSY"
        );
        assert!(!owner.join("writers").exists());
        drop(lease);
        store.shutdown().await;
    }
    #[tokio::test]
    async fn dropping_unknown_native_io_preserves_evidence_and_missing_owner_is_never_recreated() {
        let (_temp, paths, store, info) = fixture().await;
        let writer = NativeProjectIoGuard::try_begin(
            &store,
            paths.data_root(),
            &info.id,
            &info.document_epoch,
            "disposable-missing-finished",
        )
        .unwrap();
        let owner = paths
            .data_root()
            .join("web-project-ownership")
            .join(&info.id);
        drop(writer);
        assert!(crate::durable_writer::require_owner_clear(
            &WriterOwner::web(paths.data_root(), &info.id).unwrap()
        )
        .is_err());
        fs::remove_dir_all(&owner).unwrap();
        assert!(NativeProjectIoGuard::try_begin(
            &store,
            paths.data_root(),
            &info.id,
            &info.document_epoch,
            "disposable-late"
        )
        .is_err());
        assert!(!owner.exists());
        store.shutdown().await;
    }
    #[tokio::test]
    async fn attachment_open_reuses_its_read_lease_even_with_a_queued_writer() {
        use std::io::Read;
        let (_temp, paths, store, info) = fixture().await;
        fs::write(
            info.project_root.join("data/attachments/disposable.txt"),
            b"private attachment bytes",
        )
        .unwrap();
        let writer = NativeProjectIoGuard::try_begin(
            &store,
            paths.data_root(),
            &info.id,
            &info.document_epoch,
            "disposable-attachment",
        )
        .unwrap();
        let gate = store.project(&info.id).unwrap().operation_gate;
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let queued = tokio::spawn(async move {
            let _ = entered.send(());
            gate.write_owned().await
        });
        entered_rx.await.unwrap();
        tokio::task::yield_now().await;
        let mut file = writer.attachment_file("disposable.txt").unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        drop(file);
        assert_eq!(bytes, b"private attachment bytes");
        assert!(!queued.is_finished());
        writer.finish().unwrap();
        drop(queued.await.unwrap());
        store.shutdown().await;
    }
}
