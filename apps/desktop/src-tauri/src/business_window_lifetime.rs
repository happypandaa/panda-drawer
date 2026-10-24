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
    destroyed: AtomicBool,
    creation_failed: AtomicBool,
    changed: Notify,
}
impl Receipt {
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
#[derive(Default)]
pub(super) struct WindowLifetimes {
    entries: Mutex<BTreeMap<String, Arc<Receipt>>>,
    next: AtomicU64,
    closing: AtomicBool,
}
impl WindowLifetimes {
    pub fn seal(&self) {
        let _entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        self.closing.store(true, Ordering::Release);
    }
    pub fn reserve(&self, label: &str) -> Result<Arc<Receipt>, Issue> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if self.closing.load(Ordering::Acquire) {
            return Err(Issue::new("DESKTOP_STOPPING", false, false));
        }
        if entries.get(label).is_some_and(|r| !r.destroyed()) {
            return Err(Issue::new("WINDOW_GENERATION_STILL_OWNED", false, true));
        }
        let receipt = Arc::new(Receipt {
            label: label.into(),
            generation: self.next.fetch_add(1, Ordering::AcqRel) + 1,
            destroyed: AtomicBool::new(false),
            creation_failed: AtomicBool::new(false),
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
    pub fn destroyed(&self, receipt: &Arc<Receipt>, on_current: impl FnOnce()) {
        let entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if entries
            .get(&receipt.label)
            .is_some_and(|r| Arc::ptr_eq(r, receipt))
        {
            on_current();
        }
        receipt.destroyed.store(true, Ordering::Release);
        receipt.changed.notify_waiters();
    }
}

#[cfg(test)]
#[path = "business_window_lifetime_tests.rs"]
mod tests;
