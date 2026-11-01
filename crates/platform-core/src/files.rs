//! Platform boundary for handle-bound project file operations.
use crate::ApiError;
#[cfg(unix)]
#[path = "files/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "files/windows.rs"]
mod windows;
#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(windows)]
pub(crate) use windows::*;

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileInfo {
    pub path: String,
    pub size: u64,
    pub media_type: String,
    pub modified_at: u64,
}
pub(crate) fn info(path: &str, file: &File) -> Result<FileInfo, ApiError> {
    let meta = file.metadata().map_err(error)?;
    Ok(FileInfo {
        path: path.into(),
        size: meta.len(),
        media_type: mime(path).into(),
        modified_at: meta
            .modified()
            .ok()
            .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|v| v.as_millis() as u64)
            .unwrap_or(0),
    })
}
pub(crate) fn mime(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "webm" => "audio/webm",
        "mp4" => "video/mp4",
        _ => mime_guess::from_path(path)
            .first_raw()
            .unwrap_or("application/octet-stream"),
    }
}

/// A destination-volume temporary is SDK data. Its exact deletion authority is
/// recorded privately before bytes are copied, never inferred by a name scan.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachmentStage {
    version: u32,
    parent: PathBuf,
    parent_identity: crate::file_identity::PersistedIdentity,
    name: String,
    identity: Option<crate::file_identity::PersistedIdentity>,
}
pub(crate) fn publish_tracked(
    staging: &File,
    temporary: &str,
    root: &File,
    path: &str,
    overwrite: bool,
    attachments: &Path,
    record_path: &Path,
) -> Result<(), ApiError> {
    use std::io::{Seek, SeekFrom};
    let (parent, _) = parent(root, path, true)?;
    let relative = Path::new(path).parent().unwrap_or(Path::new(""));
    let parent_path = attachments.join(relative);
    let parent_identity = crate::file_identity::PersistedIdentity::from_file(&parent)
        .map_err(|e| ApiError::new("FILE_IO_ERROR", e.to_string()))?;
    if !parent_identity.matches_path(&parent_path).unwrap_or(false) {
        return Err(ApiError::new(
            "FILE_IDENTITY_CHANGED",
            "附件目标目录已变化。",
        ));
    }
    let name = format!(".upload-{}", uuid::Uuid::new_v4().simple());
    let mut record = AttachmentStage {
        version: 1,
        parent: parent_path,
        parent_identity,
        name: name.clone(),
        identity: None,
    };
    crate::builds::atomic_json(record_path, &record)?;
    let mut output = create(&parent, &name)?;
    record.identity = Some(
        crate::file_identity::PersistedIdentity::from_file(&output)
            .map_err(|e| ApiError::new("FILE_IO_ERROR", e.to_string()))?,
    );
    crate::builds::atomic_json(record_path, &record)?;
    let result = (|| {
        let mut input = read(staging, temporary)?;
        input.seek(SeekFrom::Start(0)).map_err(error)?;
        std::io::copy(&mut input, &mut output).map_err(error)?;
        output.sync_all().map_err(error)?;
        drop(output);
        publish(&parent, &name, root, path, overwrite)
    })();
    let cleanup = cleanup_attachment_stage(record_path);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), Err(e)) => Err(e.outcome("committed")),
        (Err(e), Err(c)) => Err(e.detail("stageCleanupError", serde_json::json!(c))),
    }
}
fn cleanup_attachment_stage(record_path: &Path) -> Result<(), ApiError> {
    crate::projects::check_no_symlinks(record_path, false)?;
    let record: AttachmentStage = serde_json::from_slice(&fs::read(record_path)?)?;
    if record.version != 1
        || !record.name.starts_with(".upload-")
        || record.name.contains('/')
        || record.name.contains('\\')
    {
        return Err(ApiError::new("UPLOAD_STAGE_INVALID", "附件暂存记录无效。"));
    }
    if !record.parent.try_exists()?
        && record
            .parent_identity
            .absent_on_expected_volume(&record.parent)
            .map_err(|e| ApiError::new("UPLOAD_STAGE_UNAVAILABLE", e.to_string()))?
    {
        fs::remove_file(record_path)?;
        return Ok(());
    }
    if !record
        .parent_identity
        .matches_path(&record.parent)
        .unwrap_or(false)
    {
        return Err(ApiError::new(
            "UPLOAD_STAGE_CHANGED",
            "附件暂存的父目录已变化。",
        ));
    }
    let parent = root(&record.parent)?;
    let path = record.parent.join(&record.name);
    if path.try_exists()? {
        let identity = record.identity.ok_or_else(|| {
            ApiError::new(
                "UPLOAD_STAGE_UNCONFIRMED",
                "附件暂存创建中断，无法核对文件身份。",
            )
        })?;
        let file = read(&parent, &record.name)?;
        if !identity.matches_captured(
            &crate::file_identity::PersistedIdentity::from_file(&file)
                .map_err(|e| ApiError::new("FILE_IO_ERROR", e.to_string()))?,
        ) {
            return Err(ApiError::new(
                "UPLOAD_STAGE_CHANGED",
                "附件暂存已被替换，未删除。",
            ));
        }
        remove(&parent, &record.name)?;
    }
    fs::remove_file(record_path)?;
    Ok(())
}
pub(crate) fn recover_attachment_stages(private_uploads: &Path) -> Result<(), ApiError> {
    crate::projects::check_no_symlinks(private_uploads, true)?;
    if !private_uploads.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(private_uploads)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.starts_with("publication-") && n.ends_with(".json"))
        {
            cleanup_attachment_stage(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn publication_copies_across_the_control_boundary_then_publishes_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let private = path.join("private");
        let attachments = path.join("user-data/attachments");
        fs::create_dir_all(&private).unwrap();
        fs::create_dir_all(&attachments).unwrap();
        fs::write(private.join(".upload-source"), b"disposable attachment").unwrap();
        let staging = root(&private).unwrap();
        let target = root(&attachments).unwrap();
        let record = private.join("publication-disposable.json");
        publish_tracked(
            &staging,
            ".upload-source",
            &target,
            "nested/result.bin",
            false,
            &attachments,
            &record,
        )
        .unwrap();
        assert_eq!(
            fs::read(attachments.join("nested/result.bin")).unwrap(),
            b"disposable attachment"
        );
        assert_eq!(
            fs::read(private.join(".upload-source")).unwrap(),
            b"disposable attachment"
        );
        assert!(!record.exists());
        assert_eq!(fs::read_dir(attachments.join("nested")).unwrap().count(), 1);
        let error = publish_tracked(
            &staging,
            ".upload-source",
            &target,
            "nested/result.bin",
            false,
            &attachments,
            &record,
        )
        .unwrap_err();
        assert_eq!(error.code, "FILE_EXISTS");
        assert!(!record.exists());
        assert_eq!(fs::read_dir(attachments.join("nested")).unwrap().count(), 1);
    }
    #[test]
    fn recovery_refuses_a_replaced_destination_temporary_and_keeps_its_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let private = path.join("private");
        let destination = path.join("attachments");
        fs::create_dir(&private).unwrap();
        fs::create_dir(&destination).unwrap();
        let stage = destination.join(".upload-disposable");
        fs::write(&stage, b"old").unwrap();
        let record = AttachmentStage {
            version: 1,
            parent: destination.clone(),
            parent_identity: crate::file_identity::PersistedIdentity::capture(&destination)
                .unwrap(),
            name: ".upload-disposable".into(),
            identity: Some(crate::file_identity::PersistedIdentity::capture(&stage).unwrap()),
        };
        let retained = destination.join("retained-original");
        fs::rename(&stage, &retained).unwrap();
        fs::write(&stage, b"replacement").unwrap();
        let record_path = private.join("publication-disposable.json");
        crate::builds::atomic_json(&record_path, &record).unwrap();
        assert_eq!(
            recover_attachment_stages(&private).unwrap_err().code,
            "UPLOAD_STAGE_CHANGED"
        );
        assert_eq!(fs::read(stage).unwrap(), b"replacement");
        assert!(record_path.exists());
    }
}
