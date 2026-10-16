//! Project display metadata is read without opening SQLite or executing project code.
use crate::{
    maintenance::MaintenanceManager,
    projects::{check_no_symlinks, checked_relative, validate_manifest},
    ApiError, ProjectManifest, ProjectStore,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const MANIFEST_MAX: u64 = 2_000_000;
// Internal implementation limits, not the product's finalized general media quota.
const ICON_MAX: u64 = 8 * 1024 * 1024;
const ICON_PIXELS: u64 = 4 * 1024 * 1024;
const ICON_DIMENSION: u32 = 4096;
const DECODE_MAX: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct MetadataManager {
    store: Arc<ProjectStore>,
    instance_id: String,
}
struct Snapshot {
    root: PathBuf,
    manifest: Option<Value>,
    revision: String,
    editable: bool,
    response: Value,
}
impl MetadataManager {
    pub fn new(store: Arc<ProjectStore>, instance_id: String) -> Self {
        Self { store, instance_id }
    }

    pub async fn info(&self, id: &str) -> Result<Value, ApiError> {
        let gate = self.store.gate(id);
        let _read = gate.read_owned().await;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _read = _read;
            manager.snapshot(&id).map(|s| s.response)
        })
        .await
        .map_err(|error| ApiError::new("METADATA_READ_FAILED", error.to_string()))?
    }

    pub async fn update(&self, id: &str, input: Value) -> Result<Value, ApiError> {
        validate_patch(&input)?;
        self.check_writable(id, input["maintenanceId"].as_str())?;
        let _write = self.store.gate(id).write_owned().await;
        self.check_writable(id, input["maintenanceId"].as_str())?;
        let manager = self.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _write = _write;
            manager.commit_patch(&id, input, None)
        })
        .await
        .map_err(|error| ApiError::new("METADATA_WRITE_FAILED", error.to_string()))?
    }

    /// `source` must come from the trusted manager's native picker; HTTP callers
    /// must not receive an arbitrary local-file read primitive through this API.
    pub async fn import_icon(
        &self,
        id: &str,
        source: &Path,
        expected_revision: &str,
        maintenance_id: Option<&str>,
    ) -> Result<Value, ApiError> {
        if !source.is_absolute() {
            return Err(invalid("The selected icon needs an absolute native path"));
        }
        let mut input = json!({"expectedRevision":expected_revision});
        if let Some(maintenance) = maintenance_id {
            input["maintenanceId"] = json!(maintenance);
        }
        if expected_revision.is_empty() {
            return Err(invalid("expectedRevision is required"));
        }
        self.check_writable(id, maintenance_id)?;
        let _write = self.store.gate(id).write_owned().await;
        self.check_writable(id, maintenance_id)?;
        let manager = self.clone();
        let id = id.to_owned();
        let source = source.to_owned();
        tokio::task::spawn_blocking(move || {
            let _write = _write;
            let bytes = read_file(&source, ICON_MAX)?;
            decode_png(&bytes)?;
            manager.commit_patch(&id, input, Some(bytes))
        })
        .await
        .map_err(|error| ApiError::new("METADATA_WRITE_FAILED", error.to_string()))?
    }

    fn check_writable(&self, id: &str, maintenance: Option<&str>) -> Result<(), ApiError> {
        self.store.check_workspace_available()?;
        if let Some(task) = self.store.active_task(id) {
            return Err(ApiError::new(
                "PROJECT_BUSY",
                "Wait for the current project operation before editing its metadata",
            )
            .detail("taskId", task));
        }
        self.store.metadata_root(id)?;
        self.store.control(id)?.check(false, maintenance, false)
    }

    fn snapshot(&self, id: &str) -> Result<Snapshot, ApiError> {
        let (root, _) = self.store.metadata_root(id)?;
        let root_stat = fs::symlink_metadata(&root)?;
        let workspace_stat = fs::symlink_metadata(self.store.workspace_root())?;
        let epoch = self.store.registered_epoch(id)?;
        let registration_path = self
            .store
            .workspace_root()
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        check_no_symlinks(&registration_path, true)?;
        let registration_missing = !registration_path.exists();
        let registration_valid = registration_missing || epoch.is_some();
        let mut issues = Vec::new();
        if !registration_valid {
            issues.push(
                "Project root no longer matches its registration; metadata edits are disabled"
                    .to_owned(),
            );
        }
        let manifest_bytes = read_file(&root.join("project.json"), MANIFEST_MAX);
        let manifest = manifest_bytes
            .as_ref()
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        let typed = manifest
            .clone()
            .and_then(|raw| serde_json::from_value::<ProjectManifest>(raw).ok());
        let valid = typed
            .as_ref()
            .ok_or_else(|| invalid("Project manifest is unavailable or malformed"))
            .and_then(|manifest| validate_manifest(manifest, id, &root));
        if let Err(error) = &valid {
            issues.push(error.message.clone());
        }
        if let Err(error) = &manifest_bytes {
            issues.push(error.message.clone());
        }
        let mut digest = Sha256::new();
        digest.update(self.instance_id.as_bytes());
        digest.update(id.as_bytes());
        digest.update(root.to_string_lossy().as_bytes());
        digest.update(workspace_stat.dev().to_le_bytes());
        digest.update(workspace_stat.ino().to_le_bytes());
        digest.update(root_stat.dev().to_le_bytes());
        digest.update(root_stat.ino().to_le_bytes());
        digest.update(epoch.as_deref().unwrap_or("unregistered").as_bytes());
        match &manifest_bytes {
            Ok(bytes) => digest.update(bytes),
            Err(error) => digest.update(error.to_string().as_bytes()),
        }
        let icon = manifest
            .as_ref()
            .and_then(|m| m.get("icon"))
            .filter(|v| !v.is_null());
        let mut icon_status = json!({"state":"default","path":null});
        let mut data_url = None;
        if let Some(value) = icon {
            let checked = value
                .as_str()
                .ok_or_else(|| invalid("Icon must be a project-relative PNG path"))
                .and_then(|path| icon_path(&root, path))
                .and_then(|path| {
                    // Include bounded invalid-content fingerprints and file metadata as well
                    // as validity, so an externally repaired icon invalidates old editors.
                    if let Ok(meta) = fs::symlink_metadata(&path) {
                        digest.update(meta.len().to_le_bytes());
                        digest.update(meta.ino().to_le_bytes());
                        digest.update(meta.mtime().to_le_bytes());
                        digest.update(meta.mtime_nsec().to_le_bytes());
                    }
                    let bytes = read_file(&path, ICON_MAX)?;
                    digest.update(&bytes);
                    let (width, height) = decode_png(&bytes)?;
                    Ok((bytes, width, height))
                });
            match checked {
                Ok((bytes, width, height)) => {
                    icon_status = json!({"state":"valid","path":value,"width":width,"height":height,"sizeBytes":bytes.len()});
                    data_url = Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)));
                }
                Err(error) => {
                    icon_status = json!({"state":"invalid","path":value,"message":error.message});
                    issues.push(format!(
                        "Icon unavailable; using default: {}",
                        error.message
                    ));
                    digest.update(error.code.as_bytes());
                    digest.update(error.message.as_bytes());
                }
            }
        }
        let revision = format!("metadata_{:x}", digest.finalize());
        let editable = valid.is_ok() && registration_valid;
        let response = json!({
            "projectId":id,"projectRoot":root,"name":manifest.as_ref().and_then(|m|m["name"].as_str()).unwrap_or(id),
            "description":manifest.as_ref().and_then(|m|m["description"].as_str()).unwrap_or(""),
            "icon":icon,"metadataRevision":if editable{Some(&revision)}else{None},"iconStatus":icon_status,
            "iconDataUrl":data_url,"metadataIssues":issues,"manifest":manifest,
            "iconLimits":{"maxBytes":ICON_MAX,"maxDimension":ICON_DIMENSION,"maxPixels":ICON_PIXELS,"classification":"internalTest"}
        });
        Ok(Snapshot {
            root,
            manifest,
            revision,
            editable,
            response,
        })
    }

    fn commit_patch(
        &self,
        id: &str,
        input: Value,
        image: Option<Vec<u8>>,
    ) -> Result<Value, ApiError> {
        let before = self.snapshot(id)?;
        if !before.editable {
            return Err(ApiError::new("INVALID_PROJECT_MANIFEST", "The manifest or project registration needs repair; the existing file was preserved"));
        }
        if input["expectedRevision"].as_str() != Some(before.revision.as_str()) {
            return Err(changed());
        }
        let mut manifest = before.manifest.clone().expect("editable manifest");
        let object = manifest
            .as_object_mut()
            .ok_or_else(|| invalid("Manifest must be an object"))?;
        for field in ["name", "description", "icon"] {
            if let Some(value) = input.get(field) {
                if value.is_null() {
                    object.remove(field);
                } else if field == "name" {
                    object.insert(
                        field.into(),
                        json!(value.as_str().expect("validated patch").trim()),
                    );
                } else {
                    object.insert(field.into(), value.clone());
                }
            }
        }
        if image.is_none() {
            if let Some(path) = input
                .get("icon")
                .filter(|v| !v.is_null())
                .and_then(Value::as_str)
            {
                decode_png(&read_file(&icon_path(&before.root, path)?, ICON_MAX)?)?;
            }
        }
        let mut new_icon = None;
        if let Some(bytes) = image {
            let relative = format!("meta/icon-{}.png", Uuid::new_v4().simple());
            let path = checked_relative(&before.root, &relative, true)?;
            let directory = path.parent().expect("icon parent");
            fs::create_dir_all(directory)?;
            check_no_symlinks(directory, false)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?;
            let written = file
                .write_all(&bytes)
                .and_then(|()| file.sync_all())
                .and_then(|()| File::open(directory)?.sync_all());
            if let Err(error) = written {
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
            manifest["icon"] = json!(relative);
            new_icon = Some(path);
        }
        let mut committed = false;
        let result = (|| {
            let typed: ProjectManifest = serde_json::from_value(manifest.clone())?;
            validate_manifest(&typed, id, &before.root)?;
            // A second read includes manifest/icon changes made while preparing the PNG.
            if self.snapshot(id)?.revision != before.revision {
                return Err(changed());
            }
            self.check_writable(id, input["maintenanceId"].as_str())?;
            commit_manifest(&before.root.join("project.json"), &manifest, &mut committed)?;
            MaintenanceManager::invalidate_storage(self.store.workspace_root(), id)
                .map_err(|e| e.outcome("committed"))?;
            self.snapshot(id)
                .map(|s| s.response)
                .map_err(|e| e.outcome("committed"))
        })();
        if !committed {
            if let Some(path) = new_icon {
                let _ = fs::remove_file(path);
            }
        }
        result
    }
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("INVALID_METADATA", message)
}
fn changed() -> ApiError {
    ApiError::new("METADATA_CHANGED", "Project information changed. Read the latest values and merge your edits before saving again").outcome("not_started")
}
fn validate_patch(input: &Value) -> Result<(), ApiError> {
    let object = input
        .as_object()
        .ok_or_else(|| invalid("Metadata patch must be an object"))?;
    if !object
        .keys()
        .any(|key| matches!(key.as_str(), "name" | "description" | "icon"))
    {
        return Err(invalid("At least one editable field is required"));
    }
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "expectedRevision" | "name" | "description" | "icon" | "maintenanceId"
        )
    }) {
        return Err(invalid("Only name, description and icon can be edited"));
    }
    if !input["expectedRevision"]
        .as_str()
        .is_some_and(|v| !v.is_empty() && v.len() <= 256)
    {
        return Err(invalid("expectedRevision is required"));
    }
    if let Some(name) = input.get("name") {
        if !name
            .as_str()
            .is_some_and(|v| !v.trim().is_empty() && v.len() <= 512)
        {
            return Err(invalid(
                "Name must be nonempty plain text of at most 512 UTF-8 bytes",
            ));
        }
    }
    for field in ["description", "icon"] {
        if let Some(value) = input.get(field) {
            if !value.is_null()
                && !value
                    .as_str()
                    .is_some_and(|v| v.len() <= if field == "description" { 16_384 } else { 1024 })
            {
                return Err(invalid(format!("Invalid {field}")));
            }
        }
    }
    if let Some(value) = input.get("maintenanceId") {
        if !value.as_str().is_some_and(|v| !v.is_empty()) {
            return Err(invalid("maintenanceId must be a nonempty string"));
        }
    }
    Ok(())
}
fn icon_path(root: &Path, relative: &str) -> Result<PathBuf, ApiError> {
    let path = Path::new(relative);
    if !path.starts_with("meta")
        || path == Path::new("meta")
        || path
            .extension()
            .and_then(|v| v.to_str())
            .is_none_or(|v| !v.eq_ignore_ascii_case("png"))
    {
        return Err(invalid(
            "Icon must reference a static PNG inside this project's meta/ directory",
        ));
    }
    checked_relative(root, relative, false)
}
fn read_file(path: &Path, max: u64) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("Only independent regular files are accepted"));
    }
    if metadata.len() > max {
        return Err(invalid(format!(
            "File exceeds internal size limit of {max} bytes"
        )));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(invalid("File grew beyond the size limit while reading"));
    }
    Ok(bytes)
}
fn decode_png(bytes: &[u8]) -> Result<(u32, u32), ApiError> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(invalid("Icon must be a decodable PNG"));
    }
    let mut offset = 8usize;
    let mut ended = false;
    while offset + 12 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|v| v.checked_add(12))
            .ok_or_else(|| invalid("Invalid PNG chunk size"))?;
        if end > bytes.len() {
            return Err(invalid("Truncated PNG chunk"));
        }
        let kind = &bytes[offset + 4..offset + 8];
        if matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            return Err(invalid("Animated PNG icons are not supported"));
        }
        offset = end;
        if kind == b"IEND" {
            ended = true;
            break;
        }
    }
    if !ended || offset != bytes.len() {
        return Err(invalid(
            "PNG must have a complete IEND and no trailing content",
        ));
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: DECODE_MAX });
    let mut reader = decoder
        .read_info()
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0
        || height == 0
        || width > ICON_DIMENSION
        || height > ICON_DIMENSION
        || u64::from(width) * u64::from(height) > ICON_PIXELS
    {
        return Err(invalid(
            "PNG dimensions exceed the internal 4096-side / 4-megapixel limit",
        ));
    }
    let size = reader
        .output_buffer_size()
        .filter(|v| *v <= DECODE_MAX)
        .ok_or_else(|| invalid("PNG decoded image exceeds memory limit"))?;
    let mut output = vec![0; size];
    reader
        .next_frame(&mut output)
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    reader
        .finish()
        .map_err(|e| invalid(format!("PNG decode failed: {e}")))?;
    Ok((width, height))
}
fn commit_manifest(path: &Path, value: &Value, committed: &mut bool) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Manifest has no parent"))?;
    let temporary = parent.join(format!(".metadata-{}.tmp", Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        *committed = true;
        File::open(parent)?
            .sync_all()
            .map_err(|e| ApiError::from(e).outcome("committed"))?;
        Ok(())
    })();
    if !*committed {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
