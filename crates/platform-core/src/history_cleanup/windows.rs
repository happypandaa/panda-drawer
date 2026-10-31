//! Explicit, scoped historical cleanup. Windows preserves the original plan,
//! task and confirmation policy; native snapshots and unlink own OS semantics.
use crate::{
    builds::BuildManager,
    file_identity::PersistedIdentity,
    projects::{check_no_symlinks, valid_id},
    ApiError, ProjectStore, RuntimePaths, TaskRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{atomic::Ordering, Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[path = "windows/coordinator.rs"]
mod coordinator;
#[path = "windows/manager.rs"]
mod manager;
#[path = "windows/plan_io.rs"]
mod plan_io;
#[path = "windows/removal.rs"]
mod removal;
#[path = "windows/snapshot.rs"]
mod snapshot;
#[path = "windows/types.rs"]
mod types;
use coordinator::Coordinator;
use plan_io::{read_json, remove_tree};
use snapshot::{directory_identity, parent_identities, scan, Entry, Identity};
use types::*;

const CONFIRMATION: &str = "永久清理所选内容";
const PLAN_TTL: u64 = 10 * 60 * 1000;

struct WorkspaceGuard(Arc<ProjectStore>);
impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        self.0.resume_workspace();
    }
}

pub(crate) struct CleanupManager {
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    instance_id: String,
    admission: Mutex<()>,
    plans: Mutex<BTreeMap<String, Plan>>,
    coordinator: Coordinator,
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn error(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("WORKSPACE_CLEANUP_BLOCKED", message.to_string())
}
fn protected(message: &str) -> ApiError {
    ApiError::new("CLEANUP_ITEM_PROTECTED", message)
}
fn normalized(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}
fn wire_u64(value: Option<u64>) -> Value {
    match value {
        None => Value::Null,
        Some(value) if value <= 9_007_199_254_740_991 => json!(value),
        Some(value) => json!({"$int64":value.to_string()}),
    }
}

pub(crate) fn remove_owned_directory(
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<Value, ApiError> {
    removal::remove(path, expected)
}
pub(crate) fn remove_owned_directory_checked(
    path: &Path,
    expected: &PersistedIdentity,
    verify: impl FnOnce(&Value) -> Result<(), ApiError>,
) -> Result<Value, ApiError> {
    removal::remove_checked(path, expected, verify)
}
pub(crate) fn retired_cleanup_status(
    paths: &RuntimePaths,
    id: &str,
) -> Result<Option<Value>, ApiError> {
    plan_io::retired_cleanup_status_impl(paths, id)
}

#[cfg(test)]
#[path = "windows/manager_tests.rs"]
mod manager_tests;
#[cfg(test)]
#[path = "windows/plan_tests.rs"]
mod plan_tests;
#[cfg(test)]
#[path = "windows/status_tests.rs"]
mod status_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_verification_retains_disposable_objects_without_a_cleanup_receipt() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-checked-cleanup-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let candidate = root.join("candidate");
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join("payload"), b"disposable retained bytes").unwrap();
        let expected = PersistedIdentity::capture(&candidate).unwrap();
        let mut called = false;
        let error = remove_owned_directory_checked(&candidate, &expected, |_| {
            called = true;
            Err(ApiError::new(
                "DISPOSABLE_PROOF_REJECTED",
                "keep the original test material",
            ))
        })
        .unwrap_err();
        assert!(called);
        assert_eq!(error.code, "DISPOSABLE_PROOF_REJECTED");
        assert!(expected.matches_path(&candidate).unwrap());
        assert_eq!(
            fs::read(candidate.join("payload")).unwrap(),
            b"disposable retained bytes"
        );
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }
}
