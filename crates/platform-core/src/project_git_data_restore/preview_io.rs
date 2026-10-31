//! Native preview identity and privacy; the original Mac digest stays exact.
use super::*;
pub(super) fn implemented() -> bool {
    cfg!(any(target_os = "macos", windows))
}
pub(super) fn require_supported() -> Result<(), ApiError> {
    if implemented() {
        Ok(())
    } else {
        Err(preview_error(
            "UNSUPPORTED_PLATFORM",
            "数据版本预览目前仅验证 macOS。",
        ))
    }
}
pub(super) fn make_temporary_private(root: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    #[cfg(windows)]
    platform_native::windows::files::make_directory_private(root)?;
    Ok(())
}
#[cfg(unix)]
pub(super) fn plan_digest(
    project: &ProjectHandle,
    snapshot: &Snapshot,
    target: &str,
    live: &str,
    target_digest: &str,
) -> Result<String, ApiError> {
    let metadata = fs::metadata(&project.root)?;
    Ok(sha256(&serde_json::to_vec(
        &json!({"policy":"project-data-restore-preview-v1","projectId":project.manifest.id,"projectRoot":project.root,"projectRootIdentity":[metadata.dev(),metadata.ino()],"documentEpoch":project.document_epoch,"snapshotDigest":snapshot.view.snapshot_digest,"targetCommit":target,"targetSnapshotDigest":target_digest,"liveDataDigest":live}),
    )?))
}

#[cfg(windows)]
pub(super) fn plan_digest(
    project: &ProjectHandle,
    snapshot: &Snapshot,
    target: &str,
    live: &str,
    target_digest: &str,
) -> Result<String, ApiError> {
    let identity = crate::file_identity::PersistedIdentity::capture(&project.root)
        .map_err(|_| output_invalid())?;
    Ok(sha256(&serde_json::to_vec(
        &json!({"policy":"project-data-restore-preview-v1","projectId":project.manifest.id,"projectRoot":project.root,"projectRootIdentity":identity.deletion_revision_value(),"documentEpoch":project.document_epoch,"snapshotDigest":snapshot.view.snapshot_digest,"targetCommit":target,"targetSnapshotDigest":target_digest,"liveDataDigest":live}),
    )?))
}
