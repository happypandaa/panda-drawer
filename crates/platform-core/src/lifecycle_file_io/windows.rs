use super::*;
use platform_native::windows::files as native;
use serde_json::json;

fn changed(stage: &Path, destination: &Path) -> ApiError {
    ApiError::new(
        "EXPORT_STAGE_CHANGED",
        "导出发布时文件或目录被替换，现有文件已保留，未确认交付。",
    )
    .detail("stagePath", json!(stage))
    .detail("destinationPath", json!(destination))
}
pub(super) fn publish_export_file_with(
    stage: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
    mut checkpoint: impl FnMut(ExportPublicationPoint) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if !stage.is_absolute()
        || !destination.is_absolute()
        || stage == destination
        || stage.parent().is_none()
        || stage.parent() != destination.parent()
    {
        return Err(ApiError::new(
            "EXPORT_FAILED",
            "导出发布必须在同一目录原子完成。",
        ));
    }
    let parent_path = stage.parent().unwrap();
    let parent = native::pin_directory(parent_path)?;
    let parent_identity = native::identity(&parent.file)?;
    let source = native::open_read_file_allowing_hard_links(stage)
        .map_err(|_| changed(stage, destination))?;
    if !expected.matches_open_runtime(&source)? {
        return Err(changed(stage, destination));
    }
    let source_identity = native::identity(&source)?;
    checkpoint(ExportPublicationPoint::BeforeRename)?;
    if !parent_identity.same_object(&native::open_no_reparse(parent_path)?.identity) {
        return Err(changed(stage, destination));
    }
    native::rename_file_noreplace_expected_allowing_hard_links(
        stage,
        destination,
        &source_identity,
    )
    .map_err(|error| {
        if native::publication_outcome_unknown(&error) {
            changed(stage, destination).outcome("unknown")
        } else if !expected.matches_path(stage).unwrap_or(false) {
            changed(stage, destination)
        } else {
            ApiError::new(
                "EXPORT_DESTINATION_EXISTS",
                "导出目标已存在或无法独占发布，未覆盖。",
            )
        }
    })?;
    native::sync_directory(&parent.file)?;
    checkpoint(ExportPublicationPoint::Renamed)?;
    if !expected.matches_path(destination).unwrap_or(false)
        || !parent_identity.same_object(&native::open_no_reparse(parent_path)?.identity)
    {
        // Only restore our exact captured object into a free original name.
        // A foreign replacement at either name never acquires deletion rights.
        let _ = native::rename_file_noreplace_expected_allowing_hard_links(
            destination,
            stage,
            &source_identity,
        );
        return Err(changed(stage, destination).outcome("unknown"));
    }
    Ok(())
}
