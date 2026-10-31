//! Cleanup of the exact source of a newly accepted workspace migration.
//!
//! This module does not decide that a runtime has reopened. Its caller must
//! supply that receipt only after the new runtime is usable and all old native
//! and HTTP file access has actually drained. It never pauses the new store.
use crate::migration_metadata_io::{entries, read_metadata, CopyEntry};
use crate::{file_identity::PersistedIdentity, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

const SELECTION: &str = ".platform/workspace-selection.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SourceCleanupPhase {
    AwaitingReopen,
    Pending,
    Removed,
    Absent,
}

/// Kept inside the fixed migration journal, not inside the old workspace.
/// Older journals have no value of this type and gain no automatic deletion
/// permission. Successful cleanup needs no per-file progress or payload copy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceCleanup {
    phase: SourceCleanupPhase,
    #[serde(default)]
    started: bool,
    #[serde(default)]
    excluded_paths: BTreeSet<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ApiError>,
}

impl SourceCleanup {
    pub(crate) fn awaiting_reopen() -> Self {
        Self {
            phase: SourceCleanupPhase::AwaitingReopen,
            started: false,
            excluded_paths: BTreeSet::new(),
            error: None,
        }
    }

    /// Capture the same exclusions used by the verified copy. Recomputing them
    /// from a partly removed source would lose its project manifest bindings.
    pub(crate) fn record_copy_exclusions(
        &mut self,
        paths: BTreeSet<PathBuf>,
    ) -> Result<(), ApiError> {
        if self.phase != SourceCleanupPhase::AwaitingReopen || self.started {
            return Err(blocked("复制排除项不能在工作区重开后改变。"));
        }
        validate_exclusions(&paths)?;
        self.excluded_paths = paths;
        Ok(())
    }

    /// Called by the runtime coordinator, never by startup metadata recovery or
    /// by a request containing a user-supplied path.
    pub(crate) fn acknowledge_reopened(&mut self) {
        if self.phase == SourceCleanupPhase::AwaitingReopen {
            self.phase = SourceCleanupPhase::Pending;
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.phase == SourceCleanupPhase::Pending
    }
    pub(crate) fn needs_reopen(&self) -> bool {
        self.phase == SourceCleanupPhase::AwaitingReopen
    }
    pub(crate) fn needs_attention(&self) -> bool {
        self.pending() || (self.needs_reopen() && self.error.is_some())
    }
    pub(crate) fn retain_reopen_failure(&mut self, source: &Path, failure: &ApiError) {
        self.error = Some(
            blocked("新工作区可继续使用；原目录的旧写入者或访问收尾尚未核对。")
                .detail("sourcePath", json!(source))
                .detail("causeCode", json!(failure.code))
                .detail("cause", json!(failure.message)),
        );
    }

    pub(crate) fn completed(&self) -> bool {
        matches!(
            self.phase,
            SourceCleanupPhase::Removed | SourceCleanupPhase::Absent
        )
    }

    pub(crate) fn status(&self, source: &Path) -> Value {
        json!({
            "phase":self.phase,
            "cleanupPending":self.needs_attention(),
            "completed":self.completed(),
            "sourcePath":source,
            "error":self.error,
        })
    }
}

/// The immutable copy/commit facts from one registered migration. This is an
/// internal input, deliberately unrelated to the retry HTTP request shape.
pub(crate) struct CleanupRequest<'a> {
    pub(crate) data_root: &'a Path,
    pub(crate) migration_id: &'a str,
    pub(crate) source: &'a Path,
    pub(crate) source_identity: &'a PersistedIdentity,
    pub(crate) destination: &'a Path,
    pub(crate) destination_identity: &'a PersistedIdentity,
    pub(crate) published: bool,
    pub(crate) committed: bool,
    pub(crate) epochs_applied: bool,
}

/// Returned only after settings, target identity and both saved copy inventories
/// agree. The target's current business contents are intentionally not read:
/// the reopened runtime may already have accepted legitimate writes there.
pub(crate) struct VerifiedSourceCleanup {
    manifest: BTreeMap<PathBuf, CopyEntry>,
    excluded_paths: BTreeSet<PathBuf>,
}

impl VerifiedSourceCleanup {
    /// Validate the content-hashed snapshot that the shared descriptor-relative
    /// deletion engine will subsequently remove. Extra identity fields from
    /// that engine are accepted; every persistent path and digest is checked.
    pub(crate) fn verify_snapshot(&self, snapshot: &Value) -> Result<(), ApiError> {
        let actual = entries(snapshot.clone())?;
        let persistent = actual
            .into_iter()
            .filter(|(path, _)| {
                !self
                    .excluded_paths
                    .iter()
                    .any(|excluded| path == excluded || path.starts_with(excluded))
            })
            .collect::<BTreeMap<_, _>>();
        // Missing entries are already absent and cannot contain an uncopied
        // write. This also handles an explicit project deletion while cleanup
        // was pending. Every remaining entry must still match the verified
        // copy; no newly created material may be adopted by a retry.
        for (path, entry) in persistent {
            if self.manifest.get(&path) != Some(&entry) {
                return Err(source_modified());
            }
        }
        Ok(())
    }
}

fn blocked(message: impl Into<String>) -> ApiError {
    ApiError::new("WORKSPACE_SOURCE_CLEANUP_PENDING", message.into())
}

fn source_modified() -> ApiError {
    ApiError::new(
        "OLD_WORKSPACE_MODIFIED",
        "迁移原目录的内容与已核验副本不一致；原目录保留，请核对改动。",
    )
}

fn clean_absolute(path: &Path) -> Result<(), ApiError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(blocked("迁移目录不是可核对的绝对路径。"));
    }
    Ok(())
}

fn relative(path: &Path) -> Result<(), ApiError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(blocked("迁移文件清单含有越界或空路径。"));
    }
    Ok(())
}

fn validate_exclusions(paths: &BTreeSet<PathBuf>) -> Result<(), ApiError> {
    for path in paths {
        relative(path)?;
        let fixed = matches!(
            path.to_str(),
            Some(
                ".platform/cache/storage"
                    | ".platform/workspace-selection.json"
                    | ".platform/workspace-retired.json"
                    | ".platform/identity-binding.json"
            )
        );
        let dependency = path.starts_with("projects")
            && path.components().count() >= 3
            && path.file_name().is_some_and(|name| name == "node_modules");
        if !fixed && !dependency {
            return Err(blocked("迁移复制排除项不属于已知缓存或开发依赖。"));
        }
    }
    Ok(())
}

fn authorize(
    request: &CleanupRequest<'_>,
    state: &SourceCleanup,
) -> Result<VerifiedSourceCleanup, ApiError> {
    if state.phase != SourceCleanupPhase::Pending {
        return Err(blocked("新位置尚未完成重开和旧访问收尾，不能删除原目录。"));
    }
    if !request.published || !request.committed || !request.epochs_applied {
        return Err(blocked("迁移尚未完成复制、核验、提交和登记重绑。"));
    }
    if !crate::projects::valid_id(request.migration_id) {
        return Err(blocked("迁移任务身份无效。"));
    }
    for path in [request.data_root, request.source, request.destination] {
        clean_absolute(path)?;
    }
    if request.source == request.destination
        || request.source.starts_with(request.destination)
        || request.destination.starts_with(request.source)
        || request.data_root.starts_with(request.source)
        || request.data_root.starts_with(request.destination)
    {
        return Err(blocked("迁移源、目标或固定控制目录存在不安全重叠。"));
    }
    validate_exclusions(&state.excluded_paths)?;
    let settings = read_metadata(&request.data_root.join("settings.json"))?;
    if settings["migrationId"] != request.migration_id
        || settings["workspaceRoot"] != json!(request.destination)
        || !request
            .destination_identity
            .matches_path(request.destination)
            .map_err(|error| blocked(error.to_string()))?
    {
        return Err(blocked("当前工作区不是这次迁移已提交并重开的目标。"));
    }
    let selection = read_metadata(&request.destination.join(SELECTION))?;
    if selection["schemaVersion"] != 1
        || selection["taskId"] != request.migration_id
        || selection["identifier"] != crate::APP_IDENTIFIER
        || selection["environment"] != crate::ENVIRONMENT
    {
        return Err(blocked("当前工作区的迁移选择标记不一致。"));
    }
    let directory = request
        .data_root
        .join("migrations")
        .join(request.migration_id);
    let manifest = entries(read_metadata(&directory.join("manifest.json"))?)?;
    let target = entries(read_metadata(&directory.join("target-manifest.json"))?)?;
    if manifest != target {
        return Err(blocked("迁移源和已核验副本的持久清单不一致。"));
    }
    Ok(VerifiedSourceCleanup {
        manifest,
        excluded_paths: state.excluded_paths.clone(),
    })
}

fn source_absent_on_original_volume(request: &CleanupRequest<'_>) -> Result<bool, ApiError> {
    match fs::symlink_metadata(request.source) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || !request
                    .source_identity
                    .matches_path(request.source)
                    .map_err(|error| blocked(error.to_string()))?
            {
                return Err(blocked("迁移原目录的身份已变化，未删除替代目录。"));
            }
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => request
            .source_identity
            .absent_on_expected_volume(request.source)
            .map_err(|error| blocked(error.to_string())),
        Err(error) => Err(error.into()),
    }
}

fn report(state: &SourceCleanup, source: &Path, already_absent: bool) -> Value {
    let mut result = state.status(source);
    result["alreadyAbsent"] = json!(already_absent);
    result["sourceRetained"] = json!(!state.completed());
    result
}

/// Run in an independently owned blocking operation. `persist` must atomically
/// save the fixed migration journal. `remove` prepares a content-hashed snapshot,
/// calls `verify_snapshot`, then removes exactly that snapshot with the existing
/// descriptor-relative engine. Losing an API waiter does not cancel this owner.
pub(crate) fn cleanup_source(
    request: &CleanupRequest<'_>,
    state: &mut SourceCleanup,
    mut persist: impl FnMut(&SourceCleanup) -> Result<(), ApiError>,
    remove: impl FnOnce(&VerifiedSourceCleanup) -> Result<Value, ApiError>,
) -> Result<Value, ApiError> {
    if state.completed() {
        return Ok(report(
            state,
            request.source,
            state.phase == SourceCleanupPhase::Absent,
        ));
    }
    if state.phase == SourceCleanupPhase::AwaitingReopen {
        return Err(blocked("新位置尚未完成重开和旧访问收尾，不能删除原目录。"));
    }
    let operation: Result<Value, ApiError> = (|| {
        let verified = authorize(request, state)?;
        if source_absent_on_original_volume(request)? {
            state.phase = SourceCleanupPhase::Absent;
            state.error = None;
            persist(state)?;
            return Ok(report(state, request.source, true));
        }
        // This durable intent precedes the first unlink. There is no per-file
        // deletion journal: the verified remaining snapshot is sufficient for
        // an interrupted attempt or a separately deleted project.
        state.started = true;
        state.error = None;
        persist(state)?;
        let removed = remove(&verified)?;
        if !source_absent_on_original_volume(request)? {
            return Err(blocked("原目录仍然存在，删除尚未完成。"));
        }
        state.phase = SourceCleanupPhase::Removed;
        state.error = None;
        persist(state)?;
        let mut result = report(state, request.source, false);
        result["removed"] = removed;
        Ok(result)
    })();
    match operation {
        Ok(result) => Ok(result),
        Err(error) => {
            // A cleanup error must not revoke the already reopened workspace.
            // Keep only a small retry explanation, not a copy of the old tree.
            if state.phase != SourceCleanupPhase::AwaitingReopen {
                state.phase = SourceCleanupPhase::Pending;
            }
            state.error = Some(
                blocked("新工作区可继续使用；迁移原目录的收尾需要重试。")
                    .detail("sourcePath", json!(request.source))
                    .detail("causeCode", json!(error.code))
                    .detail("cause", json!(error.message)),
            );
            persist(state)?;
            Ok(report(state, request.source, false))
        }
    }
}

#[cfg(all(test, unix))]
#[path = "workspace_migration_cleanup_tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "workspace_migration_cleanup/windows_tests.rs"]
mod windows_tests;
