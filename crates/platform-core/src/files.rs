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

use serde::Serialize;
use std::fs::File;

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
