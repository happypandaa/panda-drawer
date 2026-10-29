//! Native Destroyed receipts are generation-bound. Registry absence, a destroy
//! request's Ok, about:blank and a late older event are never destruction proof.
use super::desktop_shutdown::Issue;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Notify;

pub(super) struct Receipt {
    pub label: String,
    pub generation: u64,
    project: Option<ProjectKey>,
    destroyed: AtomicBool,
    creation_failed: AtomicBool,
    close_requested: AtomicBool,
    resources: Mutex<Vec<Box<dyn Send + Sync>>>,
    changed: Notify,
}
impl Receipt {
    /// Native creation transfers runtime resources here before asking the OS to
    /// build a window. Unknown creation outcomes retain them with this receipt.
    pub fn hold_resource(&self, resource: impl Send + Sync + 'static) -> Result<(), Issue> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        if self.destroyed() {
            return Err(Issue::new("WINDOW_ALREADY_DESTROYED", false, false));
        }
        resources.push(Box::new(resource));
        Ok(())
    }
    pub fn begin_close(&self) -> bool {
        !self.destroyed()
            && self
                .close_requested
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    pub fn reset_close(&self) {
        self.close_requested.store(false, Ordering::Release);
    }
    pub fn failed(&self) {
        self.creation_failed.store(true, Ordering::Release);
    }
    pub fn creation_failed(&self) -> bool {
        self.creation_failed.load(Ordering::Acquire)
    }
    pub fn destroyed(&self) -> bool {
        self.destroyed.load(Ordering::Acquire)
    }
    pub async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            if self.destroyed() {
                return;
            }
            changed.await;
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProjectKey {
    kind: String,
    id: String,
}
impl ProjectKey {
    fn new(kind: &str, id: &str) -> Result<Self, Issue> {
        if !matches!(kind, "web" | "node")
            || id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Issue::new("PROJECT_WINDOW_ID_INVALID", false, false));
        }
        Ok(Self {
            kind: kind.into(),
            id: id.into(),
        })
    }
}
struct ProjectState {
    revoked: Arc<AtomicBool>,
    operation: Arc<tokio::sync::Mutex<()>>,
    sealed: bool,
}
/// Captured before queueing native work. Removing the project map cannot make
/// an older queued closure valid again: it retains the revoked generation.
#[derive(Clone)]
pub(super) struct ProjectAdmission {
    key: ProjectKey,
    revoked: Arc<AtomicBool>,
    operation: Arc<tokio::sync::Mutex<()>>,
    closing: Arc<AtomicBool>,
}
impl ProjectAdmission {
    pub async fn operation(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, Issue> {
        self.check()?;
        let operation = self.operation.clone().lock_owned().await;
        self.check()?;
        Ok(operation)
    }
    pub fn check(&self) -> Result<(), Issue> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Issue::new("DESKTOP_STOPPING", false, false));
        }
        if self.revoked.load(Ordering::Acquire) {
            return Err(Issue::new("PROJECT_WINDOWS_SEALED", false, true));
        }
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct WindowLifetimes {
    entries: Mutex<BTreeMap<String, Arc<Receipt>>>,
    next: AtomicU64,
    closing: Arc<AtomicBool>,
    projects: Mutex<BTreeMap<ProjectKey, ProjectState>>,
}
impl WindowLifetimes {
    pub fn seal(&self) {
        let _entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        self.closing.store(true, Ordering::Release);
    }
    pub fn admit_project(&self, kind: &str, id: &str) -> Result<ProjectAdmission, Issue> {
        let key = ProjectKey::new(kind, id)?;
        let mut projects = self.projects.lock().unwrap_or_else(|p| p.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return Err(Issue::new("DESKTOP_STOPPING", false, false));
        }
        let project = projects.entry(key.clone()).or_insert_with(|| ProjectState {
            revoked: Arc::new(AtomicBool::new(false)),
            operation: Arc::new(tokio::sync::Mutex::new(())),
            sealed: false,
        });
        if project.sealed {
            return Err(Issue::new("PROJECT_WINDOWS_SEALED", false, true));
        }
        Ok(ProjectAdmission {
            key,
            revoked: project.revoked.clone(),
            operation: project.operation.clone(),
            closing: self.closing.clone(),
        })
    }
    pub fn seal_project(&self, kind: &str, id: &str) -> Result<Arc<tokio::sync::Mutex<()>>, Issue> {
        let key = ProjectKey::new(kind, id)?;
        let mut projects = self.projects.lock().unwrap_or_else(|p| p.into_inner());
        let project = projects.entry(key).or_insert_with(|| ProjectState {
            revoked: Arc::new(AtomicBool::new(false)),
            operation: Arc::new(tokio::sync::Mutex::new(())),
            sealed: false,
        });
        project.sealed = true;
        project.revoked.store(true, Ordering::Release);
        Ok(project.operation.clone())
    }
    pub fn reserve_project(
        &self,
        label: &str,
        admission: &ProjectAdmission,
    ) -> Result<Arc<Receipt>, Issue> {
        let projects = self.projects.lock().unwrap_or_else(|p| p.into_inner());
        admission.check()?;
        if !projects
            .get(&admission.key)
            .is_some_and(|state| !state.sealed && Arc::ptr_eq(&state.revoked, &admission.revoked))
        {
            return Err(Issue::new("PROJECT_WINDOW_GENERATION_CHANGED", false, true));
        }
        self.reserve_inner(label, Some(admission.key.clone()))
    }
    pub fn project_receipt(&self, kind: &str, id: &str) -> Result<Option<Arc<Receipt>>, Issue> {
        let key = ProjectKey::new(kind, id)?;
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .find(|r| r.project.as_ref() == Some(&key))
            .cloned())
    }
    /// Called only after native destruction and a main-queue barrier. Unknown
    /// native generations retain their receipt and continue to block deletion.
    pub fn forget_project(&self, kind: &str, id: &str) -> Result<(), Issue> {
        let key = ProjectKey::new(kind, id)?;
        let projects = self.projects.lock().unwrap_or_else(|p| p.into_inner());
        if !projects.get(&key).is_some_and(|state| state.sealed) {
            return Err(Issue::new("PROJECT_WINDOWS_NOT_SEALED", false, false));
        }
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if entries
            .values()
            .any(|r| r.project.as_ref() == Some(&key) && !r.destroyed())
        {
            return Err(Issue::new(
                "PROJECT_WINDOW_DESTROY_UNCONFIRMED",
                false,
                true,
            ));
        }
        entries.retain(|_, r| r.project.as_ref() != Some(&key));
        Ok(())
    }
    pub fn finish_project(&self, kind: &str, id: &str) -> Result<(), Issue> {
        let key = ProjectKey::new(kind, id)?;
        let mut projects = self.projects.lock().unwrap_or_else(|p| p.into_inner());
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if entries.values().any(|r| r.project.as_ref() == Some(&key)) {
            return Err(Issue::new("PROJECT_WINDOW_RECEIPT_RETAINED", false, true));
        }
        if projects.get(&key).is_some_and(|state| !state.sealed) {
            return Err(Issue::new("PROJECT_WINDOWS_NOT_SEALED", false, false));
        }
        projects.remove(&key);
        Ok(())
    }
    #[cfg(test)]
    pub fn reserve(&self, label: &str) -> Result<Arc<Receipt>, Issue> {
        self.reserve_inner(label, None)
    }
    fn reserve_inner(
        &self,
        label: &str,
        project: Option<ProjectKey>,
    ) -> Result<Arc<Receipt>, Issue> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return Err(Issue::new("DESKTOP_STOPPING", false, false));
        }
        if entries.get(label).is_some_and(|r| !r.destroyed())
            || project.as_ref().is_some_and(|key| {
                entries
                    .values()
                    .any(|r| r.project.as_ref() == Some(key) && !r.destroyed())
            })
        {
            return Err(Issue::new("WINDOW_GENERATION_STILL_OWNED", false, true));
        }
        let receipt = Arc::new(Receipt {
            label: label.into(),
            generation: self.next.fetch_add(1, Ordering::AcqRel) + 1,
            project,
            destroyed: AtomicBool::new(false),
            creation_failed: AtomicBool::new(false),
            close_requested: AtomicBool::new(false),
            resources: Mutex::new(Vec::new()),
            changed: Notify::new(),
        });
        entries.insert(label.into(), receipt.clone());
        Ok(receipt)
    }
    pub fn active(&self) -> Vec<Arc<Receipt>> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|r| !r.destroyed())
            .cloned()
            .collect()
    }
    pub fn receipt(&self, label: &str) -> Option<Arc<Receipt>> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(label)
            .cloned()
    }
    pub fn is_current(&self, receipt: &Arc<Receipt>) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&receipt.label)
            .is_some_and(|current| Arc::ptr_eq(current, receipt))
    }
    pub fn destroyed(&self, receipt: &Arc<Receipt>, on_current: impl FnOnce()) {
        let resources = {
            let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            if entries
                .get(&receipt.label)
                .is_some_and(|r| Arc::ptr_eq(r, receipt))
            {
                on_current();
            }
            let mut resources = receipt.resources.lock().unwrap_or_else(|p| p.into_inner());
            receipt.destroyed.store(true, Ordering::Release);
            std::mem::take(&mut *resources)
        };
        // Releasing a build pin can queue core cleanup. It must never run while
        // the native receipt registry is locked or touch a replacement receipt.
        drop(resources);
        receipt.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "business_window_lifetime_tests.rs"]
mod tests;
