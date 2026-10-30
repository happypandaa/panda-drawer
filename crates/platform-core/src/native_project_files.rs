//! Platform boundary for native delivery. Each implementation owns its native
//! publication/recovery guarantees; unsupported capabilities fail before I/O.
#[cfg(unix)]
#[path = "native_project_files/unix.rs"]
mod unix;
#[cfg(unix)]
pub(crate) use unix::publish_file;

#[cfg(windows)]
pub(crate) fn publish_file(
    data: &std::path::Path,
    workspace: &std::path::Path,
    project: &crate::ProjectHandle,
    source: std::fs::File,
    destination: &std::path::Path,
    validate: impl Fn(&std::path::Path) -> Result<(), crate::ApiError>,
) -> Result<(), crate::ApiError> {
    let _ = (data, workspace, project, source, destination, validate);
    Err(crate::ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：原生文件交付的发布与中断恢复尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "native-file-delivery")
    .outcome("not_started"))
}
