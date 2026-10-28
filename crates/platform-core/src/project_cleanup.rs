//! Shared deletion callbacks survive workspace service replacement. Native
//! preparation runs before a project write gate, and notification follows the
//! actual removal of its files, identity and task records.
use crate::{app_windows::WindowSizeStore, ApiError};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, Weak},
};

type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    Web,
    Node,
}
impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Node => "node",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeletedProjectNotice {
    pub version: u32,
    pub kind: ProjectKind,
    pub id: String,
    pub task_ids: Vec<String>,
    pub source_ids: Vec<String>,
    pub source_roots: Vec<PathBuf>,
}
impl DeletedProjectNotice {
    pub(crate) fn new(
        kind: ProjectKind,
        id: String,
        task_ids: Vec<String>,
        source_ids: Vec<String>,
        source_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            version: 1,
            kind,
            id,
            task_ids,
            source_ids,
            source_roots,
        }
    }
    pub(crate) fn normalize(&mut self) -> Result<()> {
        let valid_id = match self.kind {
            ProjectKind::Web => self.id.starts_with("prj_") && crate::projects::valid_id(&self.id),
            ProjectKind::Node => {
                uuid::Uuid::parse_str(&self.id).is_ok_and(|id| id.to_string() == self.id)
            }
        };
        if self.version != 1
            || !valid_id
            || self
                .task_ids
                .iter()
                .any(|id| !id.starts_with("task_") || !crate::projects::valid_id(id))
            || self
                .source_ids
                .iter()
                .any(|id| !id.starts_with("src_") || !crate::projects::valid_id(id))
            || self.source_roots.iter().any(|path| {
                !path.is_absolute()
                    || path.components().any(|part| {
                        matches!(
                            part,
                            std::path::Component::CurDir | std::path::Component::ParentDir
                        )
                    })
            })
        {
            return Err(ApiError::new(
                "PROJECT_CLEANUP_IDENTITY_INVALID",
                "项目清理身份无法核验。",
            ));
        }
        self.task_ids.sort();
        self.task_ids.dedup();
        self.source_ids.sort();
        self.source_ids.dedup();
        self.source_roots.sort();
        self.source_roots.dedup();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCleanupStage {
    Prepare,
    Purge,
    Committed,
}
pub type NativeProjectCleanup = Arc<
    dyn Fn(NativeCleanupStage, DeletedProjectNotice) -> BoxFuture<'static, Result<()>>
        + Send
        + Sync,
>;

pub(crate) struct ProjectCleanupCoordinator {
    windows: Arc<WindowSizeStore>,
    storage: Mutex<Vec<Weak<crate::storage::StorageManager>>>,
    history: Mutex<Vec<Weak<crate::history_cleanup::CleanupManager>>>,
    native: RwLock<Option<NativeProjectCleanup>>,
}
impl ProjectCleanupCoordinator {
    pub(crate) fn new(data: &std::path::Path) -> Arc<Self> {
        Arc::new(Self {
            windows: Arc::new(WindowSizeStore::new(data)),
            storage: Mutex::new(vec![]),
            history: Mutex::new(vec![]),
            native: RwLock::new(None),
        })
    }
    pub(crate) fn windows(&self) -> Arc<WindowSizeStore> {
        self.windows.clone()
    }
    pub(crate) fn set_native(&self, hook: NativeProjectCleanup) {
        *self.native.write().unwrap_or_else(|p| p.into_inner()) = Some(hook);
    }
    pub(crate) fn register_storage(&self, storage: &Arc<crate::storage::StorageManager>) {
        self.storage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::downgrade(storage));
    }
    pub(crate) fn register_history(&self, history: &Arc<crate::history_cleanup::CleanupManager>) {
        self.history
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::downgrade(history));
    }
    fn storages(&self) -> Vec<Arc<crate::storage::StorageManager>> {
        let mut list = self.storage.lock().unwrap_or_else(|p| p.into_inner());
        let live = list.iter().filter_map(Weak::upgrade).collect();
        list.retain(|weak| weak.strong_count() > 0);
        live
    }
    fn histories(&self) -> Vec<Arc<crate::history_cleanup::CleanupManager>> {
        let mut list = self.history.lock().unwrap_or_else(|p| p.into_inner());
        let live = list.iter().filter_map(Weak::upgrade).collect();
        list.retain(|weak| weak.strong_count() > 0);
        live
    }
    async fn native(&self, stage: NativeCleanupStage, notice: &DeletedProjectNotice) -> Result<()> {
        let hook = {
            self.native
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        };
        if let Some(hook) = hook {
            hook(stage, notice.clone()).await?;
        }
        Ok(())
    }
    pub(crate) async fn prepare(
        &self,
        mut notice: DeletedProjectNotice,
    ) -> Result<DeletedProjectNotice> {
        notice.normalize()?;
        self.windows
            .forget_project(notice.kind.as_str(), &notice.id)?;
        self.native(NativeCleanupStage::Prepare, &notice).await?;
        for storage in self.storages() {
            notice
                .task_ids
                .extend(storage.forget_deleted_project(&notice.id).await?);
        }
        for history in self.histories() {
            notice.task_ids.extend(
                history
                    .forget_deleted_project(&notice.id, &notice.source_ids, &notice.source_roots)
                    .await?,
            );
        }
        notice.normalize()?;
        Ok(notice)
    }
    pub(crate) async fn purge(&self, notice: &DeletedProjectNotice) -> Result<()> {
        self.native(NativeCleanupStage::Purge, notice).await
    }
    pub(crate) async fn committed(&self, notice: &DeletedProjectNotice) {
        if let Err(error) = self.windows.finish_forget(notice.kind.as_str(), &notice.id) {
            eprintln!("Window cleanup completion: {}", error.code);
        }
        for storage in self.storages() {
            storage.finish_deleted_project(&notice.id);
        }
        for history in self.histories() {
            history.finish_deleted_project(&notice.id);
        }
        if let Err(error) = self.native(NativeCleanupStage::Committed, notice).await {
            eprintln!("Project deletion notification: {}", error.code);
        }
    }
}
