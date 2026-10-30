//! Shared Git wire declarations. These do not inspect or adopt any repository.
use crate::{environment::ExecutableInfo, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[cfg(test)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitInitInput {
    pub expected_snapshot: String,
    pub initial_branch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitAuthor {
    pub name: String,
    pub email: String,
}
#[cfg(test)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSaveInput {
    #[serde(deserialize_with = "required_head")]
    pub expected_head: Option<String>,
    pub expected_snapshot: String,
    pub message: String,
    pub author: GitAuthor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitVersion {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: GitAuthor,
    pub authored_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitVersions {
    pub repository_root: PathBuf,
    pub head: GitHead,
    pub versions: Vec<GitVersion>,
    pub has_more: bool,
    pub limit: u32,
    pub git: ExecutableInfo,
    pub database: Value,
}
#[cfg(test)]
fn required_head<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHead {
    pub oid: Option<String>,
    pub branch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitMarker {
    pub path: PathBuf,
    pub kind: String,
    pub scope: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositoryState {
    pub state: String,
    pub root: Option<PathBuf>,
    pub markers: Vec<GitMarker>,
    pub head: Option<GitHead>,
    pub index_state: String,
    pub git: Option<ExecutableInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitExcludedPath {
    pub path: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceScope {
    pub policy: String,
    pub included_roots: Vec<String>,
    pub included_paths: Vec<String>,
    pub excluded: Vec<GitExcludedPath>,
    pub unclassified_paths: Vec<String>,
    pub protected_tracked_paths: Vec<String>,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    pub kind: String,
    pub staged: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitStatus {
    pub repository: GitRepositoryState,
    pub scope: GitSourceScope,
    pub changes: Vec<GitChange>,
    pub snapshot_digest: String,
    pub can_initialize: bool,
    pub can_save: bool,
    pub can_list_versions: bool,
    pub can_create_branch: bool,
    pub branch_creation_issue: Option<ApiError>,
    pub source_restore: crate::source_restore_guard::SourceRestoreState,
    pub database: Value,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSourceRestorePreviewInput {
    pub target_commit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSourceRestoreApplyInput {
    pub target_commit: String,
    pub expected_head: String,
    pub expected_branch: String,
    pub expected_snapshot: String,
    pub expected_plan_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSourceRestoreRecoverInput {
    pub restore_task_id: String,
    pub expected_plan_digest: String,
    pub expected_journal_digest: String,
    pub action: RestoreAction,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RestoreAction {
    Continue,
    Undo,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreDescriptor {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreChange {
    pub path: String,
    pub kind: String,
    pub before: Option<RestoreDescriptor>,
    pub after: Option<RestoreDescriptor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceRestorePreview {
    pub repository_root: PathBuf,
    pub head: GitHead,
    pub target_commit: String,
    pub snapshot_digest: String,
    pub plan_digest: String,
    pub changes: Vec<RestoreChange>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub can_apply: bool,
    pub issues: Vec<ApiError>,
    pub git: ExecutableInfo,
    pub database: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryChange {
    pub path: String,
    pub kind: String,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryOperation {
    pub restore_task_id: String,
    pub target_commit: String,
    pub phase: String,
    pub plan_digest: String,
    pub journal_digest: String,
    pub can_continue: bool,
    pub can_undo: bool,
    pub changes: Vec<RecoveryChange>,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSourceRestoreRecoveryStatus {
    pub state: String,
    pub operations: Vec<RecoveryOperation>,
    pub issues: Vec<ApiError>,
    pub database: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataRestorePreviewInput {
    pub target_commit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataVersionSummary {
    pub database_bytes: u64,
    pub attachment_count: usize,
    pub attachment_bytes: u64,
    pub directory_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataAttachmentChange {
    pub path: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDataRestorePreview {
    pub repository_root: PathBuf,
    pub head: GitHead,
    pub target_commit: String,
    pub document_epoch: String,
    pub snapshot_digest: String,
    pub live_data_digest: String,
    pub target_snapshot_digest: String,
    pub plan_digest: String,
    pub current: DataVersionSummary,
    pub target: DataVersionSummary,
    pub attachment_changes: Vec<DataAttachmentChange>,
    pub directory_changes: Vec<DataAttachmentChange>,
    pub database_changed: bool,
    pub can_apply: bool,
    pub apply_implemented: bool,
    pub reason: String,
    pub issues: Vec<ApiError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataRestoreApplyInput {
    pub target_commit: String,
    pub expected_head: String,
    pub expected_branch: String,
    pub expected_snapshot: String,
    pub expected_document_epoch: String,
    pub expected_live_data_digest: String,
    pub expected_target_snapshot_digest: String,
    pub expected_plan_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataRestoreRecoverInput {
    pub restore_task_id: String,
    pub expected_plan_digest: String,
    pub expected_journal_digest: String,
    pub action: RestoreAction,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreSlotState {
    pub kind: String,
    pub path: String,
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRestoreRecoveryOperation {
    pub restore_task_id: String,
    pub target_commit: String,
    pub phase: String,
    pub plan_digest: String,
    pub journal_digest: String,
    pub operation_outcome: String,
    pub data_state: String,
    pub document_epoch: Option<String>,
    pub can_continue: bool,
    pub can_undo: bool,
    pub completion_published: bool,
    pub original_retained: bool,
    pub retained_path: PathBuf,
    pub slots: Vec<DataRestoreSlotState>,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDataRestoreRecoveryStatus {
    pub state: String,
    pub business_blocked: bool,
    pub operations: Vec<DataRestoreRecoveryOperation>,
    pub issues: Vec<ApiError>,
}

#[cfg(test)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataSaveInput {
    #[serde(deserialize_with = "required_head")]
    pub expected_head: Option<String>,
    pub expected_snapshot: String,
    pub message: String,
    pub author: GitAuthor,
}
#[cfg(test)]
impl From<GitDataSaveInput> for GitSaveInput {
    fn from(input: GitDataSaveInput) -> Self {
        Self {
            expected_head: input.expected_head,
            expected_snapshot: input.expected_snapshot,
            message: input.message,
            author: input.author,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataSaveFinalizeInput {
    pub save_task_id: String,
    pub expected_journal_digest: String,
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn recovery_inputs_keep_action_spelling_and_reject_unknown_fields() {
        let value = serde_json::json!({"restoreTaskId":"format-only", "expectedPlanDigest":"format-only", "expectedJournalDigest":"format-only", "action":"continue"});
        let input: GitSourceRestoreRecoverInput = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(input.action, RestoreAction::Continue);
        assert_eq!(serde_json::to_value(input).unwrap(), value);
        let mut extra = value.clone();
        extra["unrecognized"] = serde_json::json!(true);
        assert!(serde_json::from_value::<GitSourceRestoreRecoverInput>(extra).is_err());
        let mut wrong_case = value;
        wrong_case["action"] = serde_json::json!("Continue");
        assert!(serde_json::from_value::<GitSourceRestoreRecoverInput>(wrong_case).is_err());
    }

    #[test]
    fn explicit_null_head_and_an_omitted_head_keep_the_existing_difference() {
        let mut value = serde_json::json!({"expectedSnapshot":"format-only", "message":"format-only", "author":{"name":"Disposable", "email":"format-only"}});
        assert!(serde_json::from_value::<GitSaveInput>(value.clone()).is_err());
        assert!(serde_json::from_value::<GitDataSaveInput>(value.clone()).is_err());
        value["expectedHead"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<GitSaveInput>(value.clone())
            .unwrap()
            .expected_head
            .is_none());
        let data: GitDataSaveInput = serde_json::from_value(value).unwrap();
        let source: GitSaveInput = data.into();
        assert!(source.expected_head.is_none());
    }
}
