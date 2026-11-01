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
    #[serde(skip)]
    source_scope: Option<Arc<DeletionSourceScope>>,
}
#[derive(Debug)]
struct DeletionSourceScope {
    gate: Arc<tokio::sync::RwLock<()>>,
    _lease: tokio::sync::OwnedRwLockReadGuard<()>,
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
            source_scope: None,
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

type WebClose = Arc<dyn Fn(String) -> BoxFuture<'static, Result<()>> + Send + Sync>;

pub(crate) struct ProjectCleanupCoordinator {
    stores: crate::webview_storage::StoreLedger,
    windows: Arc<WindowSizeStore>,
    storage: Mutex<Vec<Weak<crate::storage::StorageManager>>>,
    native: RwLock<Option<NativeProjectCleanup>>,
    web_close: RwLock<Option<WebClose>>,
    migration_sources: Arc<tokio::sync::RwLock<()>>,
}
impl ProjectCleanupCoordinator {
    pub(crate) fn new(data: &std::path::Path) -> Arc<Self> {
        Arc::new(Self {
            stores: crate::webview_storage::StoreLedger::new(data.into()),
            windows: Arc::new(WindowSizeStore::new(data)),
            storage: Mutex::new(vec![]),
            native: RwLock::new(None),
            web_close: RwLock::new(None),
            migration_sources: Arc::new(tokio::sync::RwLock::new(())),
        })
    }
    pub(crate) fn windows(&self) -> Arc<WindowSizeStore> {
        self.windows.clone()
    }
    pub(crate) fn stores(&self) -> crate::webview_storage::StoreLedger {
        self.stores.clone()
    }
    pub(crate) fn set_native(&self, hook: NativeProjectCleanup) {
        *self.native.write().unwrap_or_else(|p| p.into_inner()) = Some(hook);
    }
    pub(crate) fn set_web_close(&self, close: WebClose) {
        *self.web_close.write().unwrap_or_else(|e| e.into_inner()) = Some(close);
    }
    pub(crate) fn register_storage(&self, storage: &Arc<crate::storage::StorageManager>) {
        self.storage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::downgrade(storage));
    }
    fn storages(&self) -> Vec<Arc<crate::storage::StorageManager>> {
        let mut list = self.storage.lock().unwrap_or_else(|p| p.into_inner());
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
        } else if stage != NativeCleanupStage::Committed
            && self.stores.contains(notice.kind, &notice.id)?
        {
            return Err(ApiError::new(
                "WEBVIEW_NATIVE_HOST_REQUIRED",
                "此项目的浏览器存储需要桌面平台收到系统清理确认后才能删除，请启动桌面平台重试。",
            ));
        }
        Ok(())
    }
    pub(crate) fn source_registration_scope(
        &self,
    ) -> Result<tokio::sync::OwnedRwLockReadGuard<()>> {
        self.migration_sources
            .clone()
            .try_read_owned()
            .map_err(|_| {
                ApiError::new(
                    "WORKSPACE_SOURCE_CLEANUP_RUNNING",
                    "迁移原目录正在收尾，请结束后重试这次文件夹操作。",
                )
            })
    }
    pub(crate) async fn prepare(
        &self,
        mut notice: DeletedProjectNotice,
    ) -> Result<DeletedProjectNotice> {
        notice.normalize()?;
        if notice
            .source_scope
            .as_ref()
            .is_some_and(|scope| !Arc::ptr_eq(&scope.gate, &self.migration_sources))
        {
            return Err(ApiError::new(
                "PROJECT_CLEANUP_SCOPE_CHANGED",
                "项目材料清理的工作区归属已变化。",
            ));
        }
        if notice.source_scope.is_none() {
            let gate = self.migration_sources.clone();
            let lease = gate.clone().read_owned().await;
            notice.source_scope = Some(Arc::new(DeletionSourceScope {
                gate,
                _lease: lease,
            }));
        }
        self.windows
            .forget_project(notice.kind.as_str(), &notice.id)?;
        self.native(NativeCleanupStage::Prepare, &notice).await?;
        if notice.kind == ProjectKind::Web {
            let close = self
                .web_close
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(close) = close {
                close(notice.id.clone()).await?;
            }
        }
        for storage in self.storages() {
            notice
                .task_ids
                .extend(storage.forget_deleted_project(&notice.id).await?);
        }
        notice.normalize()?;
        Ok(notice)
    }
    /// Source cleanup and explicit project deletion can both remove retained
    /// migration materials. Serialize those mutations without pausing normal
    /// business reads/writes in the new workspace.
    pub(crate) fn try_migration_source_cleanup(
        &self,
    ) -> Result<tokio::sync::OwnedRwLockWriteGuard<()>> {
        self.migration_sources
            .clone()
            .try_write_owned()
            .map_err(|_| {
                ApiError::new(
                    "WORKSPACE_SOURCE_CLEANUP_BUSY",
                    "项目删除仍在处理迁移材料，请在它结束后重试原目录收尾。",
                )
            })
    }
    pub(crate) async fn purge(&self, notice: &DeletedProjectNotice) -> Result<()> {
        self.native(NativeCleanupStage::Purge, notice).await?;
        if self.stores.contains(notice.kind, &notice.id)? {
            return Err(ApiError::new(
                "WEBVIEW_STORE_CLEANUP_UNCONFIRMED",
                "项目浏览器存储仍未确认清理，已保留删除记录，请重试。",
            ));
        }
        self.stores.sync_absence(notice.kind, &notice.id)?;
        Ok(())
    }
    pub(crate) async fn committed(&self, notice: &DeletedProjectNotice) {
        if let Err(error) = self.windows.finish_forget(notice.kind.as_str(), &notice.id) {
            eprintln!("Window cleanup completion: {}", error.code);
        }
        for storage in self.storages() {
            storage.finish_deleted_project(&notice.id);
        }
        if let Err(error) = self.native(NativeCleanupStage::Committed, notice).await {
            eprintln!("Project deletion notification: {}", error.code);
        }
    }
}
