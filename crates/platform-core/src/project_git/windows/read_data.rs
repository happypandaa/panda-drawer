//! Windows readonly data-version views use the unchanged schema and snapshot
//! reader. Mutating or recovering previous Git data transactions stays closed.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DataPublication {
    pub manifest_digest: String,
    pub previous_manifest_digest: Option<String>,
    pub previous_index: Option<String>,
    pub prepared_index: String,
    pub lock_identity: (u64, u64),
    pub repository_identity: String,
    pub operation_identity: String,
    pub files: BTreeMap<String, SourceFile>,
}

pub(super) fn is_data_path(path: &str) -> bool {
    path == version_data::DIRECTORY || path.starts_with("version-data/")
}
pub(super) fn inspect_current_data(
    project: &ProjectHandle,
    context: &Context,
) -> Result<Option<version_data::PreparedDataSnapshot>, ApiError> {
    version_data::validate_layout(&project.manifest)?;
    let root = project.root.join(version_data::DIRECTORY);
    check_no_symlinks(&root, true)?;
    if !root.try_exists()? {
        return Ok(None);
    }
    version_data::validate_snapshot(
        &root,
        &project.manifest,
        version_data::SnapshotLimits::default(),
        &context.stop,
        &context.cancel,
    )
    .map(Some)
}
pub(super) fn data_status(
    snapshot: Option<&version_data::PreparedDataSnapshot>,
    scope: &str,
) -> Value {
    json!({"state":if scope=="project-root"{"supported"}else if scope=="source-only"{"source-only"}else{"not-supported"},"canSave":false,"currentSavedSnapshot":snapshot.map(|data|json!({"manifestDigest":data.manifest_digest,"databaseBytes":data.manifest.database.bytes,"attachmentCount":data.manifest.attachments.len()})),"reason":if scope=="project-root"{"平台可以核对和取回已有的一致性数据版本，不再提供 Git 保存入口。普通源码提交不会自动保存或恢复业务数据库与附件。"}else if scope=="source-only"{"现有Git仅覆盖源码目录，不能纳入项目数据库；未移动Git根或创建第二个仓库。"}else{"数据版本取回需要项目根的标准Git仓库；Node自有数据库不在本步范围。"}})
}
pub(super) async fn check_data_tracking(
    snapshot: Option<&version_data::PreparedDataSnapshot>,
    head: &BTreeMap<String, (String, String)>,
    index: &BTreeMap<String, (String, String)>,
    issues: &mut Vec<ApiError>,
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
) -> Result<(), ApiError> {
    let paths = head
        .keys()
        .chain(index.keys())
        .filter(|path| is_data_path(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    let Some(snapshot) = snapshot else {
        if !paths.is_empty() {
            issues.push(
                recovery().detail("reason", "已跟踪的数据快照缺失；没有在保存源码时删除它。"),
            );
        }
        return Ok(());
    };
    let files = version_data::tracked_files(snapshot);
    if paths != files.keys().cloned().collect::<BTreeSet<_>>() {
        issues.push(recovery().detail(
            "reason",
            "version-data目录与已有Git跟踪范围不符；不采用同名目录。",
        ));
        return Ok(());
    }
    let hashes = hash_paths(
        tools,
        context,
        Some(project),
        &project.root,
        files.keys(),
        false,
    )
    .await?;
    for path in files.keys() {
        let expected = (
            "100644".to_owned(),
            hashes.get(path).ok_or_else(output_invalid)?.clone(),
        );
        if head.get(path) != Some(&expected) || index.get(path) != Some(&expected) {
            issues.push(recovery().detail(
                "reason",
                "数据快照存在未保存或外部改动；源码保存没有替换数据树。",
            ));
            break;
        }
    }
    check_attributes(project, tools, context, files.keys(), issues).await
}

pub(crate) fn check_data_operations_available(workspace: &Path, id: &str) -> Result<(), ApiError> {
    super::data_inventory::check_project(workspace, id)
}
pub(crate) fn check_data_workspace_available(workspace: &Path) -> Result<(), ApiError> {
    super::data_inventory::check_workspace(workspace)
}
pub(crate) fn check_data_recovery_record(
    workspace: &Path,
    id: &str,
    input: &GitDataSaveFinalizeInput,
) -> Result<(), ApiError> {
    let _ = (workspace, id, input);
    Err(super::unavailable())
}

#[cfg(test)]
#[path = "read_data_tests.rs"]
mod tests;
