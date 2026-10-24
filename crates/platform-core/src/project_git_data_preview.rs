//! Preview of a project-v1 data version. This slice reads local raw Git objects
//! and a fresh consistent live snapshot. It has no live-data replacement API.
use super::*;
use crate::{files, projects::ProjectTaskLease};
use unicode_normalization::UnicodeNormalization;

#[path = "project_git_data_objects.rs"]
pub(super) mod objects;

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

impl ProjectGitManager {
    pub async fn data_restore_preview(
        &self,
        id: &str,
        input: GitDataRestorePreviewInput,
        stop: watch::Receiver<bool>,
    ) -> Result<GitDataRestorePreview, ApiError> {
        if input.target_commit.len() != 40 || !valid_oid(&input.target_commit) {
            return Err(input_error());
        }
        if !cfg!(target_os = "macos") {
            return Err(preview_error(
                "UNSUPPORTED_PLATFORM",
                "数据版本预览目前仅验证 macOS。",
            ));
        }
        self.available()?;
        let operation = self
            .store
            .reserve_transient_operation(id, "data-restore-preview")?;
        self.store.check_development_available(id)?;
        let project = self.store.project(id)?;
        project.control.check(false, None, false)?;
        version_data::validate_layout(&project.manifest)?;
        crate::projects::data_restore_registration::validate_data_restore_layout(
            &project.manifest,
        )?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        let gate = data_versions::read_gate_for_data(&project, &mut context).await?;
        let tools = context.resolve(&self.git).await?;
        let initial =
            Box::pin(self.preview_data_git_status(&project, &tools, &mut context)).await?;
        let tree = Box::pin(objects::target_tree(
            &project,
            &input.target_commit,
            &tools,
            &mut context,
        ))
        .await?;
        let (target_temporary, target) =
            Box::pin(target_snapshot(self, &project, &tools, &mut context, tree)).await?;
        drop(gate);
        // The blocking worker owns both the freeze/gate and operation lease. If
        // this async request is dropped, deletion/migration cannot race its I/O.
        let (_operation, _live_temporary, current) =
            current_snapshot(self, project.clone(), operation, &mut context).await?;
        let _gate = data_versions::read_gate_for_data(&project, &mut context).await?;
        let fresh = Box::pin(self.preview_data_git_status(&project, &tools, &mut context)).await?;
        if initial.view.snapshot_digest != fresh.view.snapshot_digest {
            return Err(preview_error(
                "PROJECT_DATA_RESTORE_STALE",
                "预览期间源码或 Git 状态变化；请重新预览当前数据版本。",
            ));
        }
        project.check()?;
        let plan_digest = plan_digest(
            &project,
            &fresh,
            &input.target_commit,
            &current.manifest_digest,
            &target.manifest_digest,
        )?;
        let result = GitDataRestorePreview {
            repository_root: project.root.clone(),
            head: fresh.view.repository.head.ok_or_else(output_invalid)?,
            target_commit: input.target_commit,
            document_epoch: project.document_epoch.clone(),
            snapshot_digest: fresh.view.snapshot_digest,
            live_data_digest: current.manifest_digest.clone(),
            target_snapshot_digest: target.manifest_digest.clone(),
            plan_digest,
            current: summary(&current.manifest),
            target: summary(&target.manifest),
            attachment_changes: attachment_changes(&current.manifest, &target.manifest),
            directory_changes: directory_changes(&current.manifest, &target.manifest),
            database_changed: current.manifest.database != target.manifest.database,
            can_apply: cfg!(target_os = "macos"),
            apply_implemented: cfg!(target_os = "macos"),
            reason: if cfg!(target_os = "macos") {
                "已核对目标数据版本和本次当前数据摘要；确认取回前仍会重新核对，完成后需重新打开业务页面。"
            } else {
                "已核对目标数据版本和当前数据摘要；数据取回目前仅验证 macOS。"
            }.into(),
            issues: vec![],
        };
        // These are private temporary candidates, not retained data history.
        // Their TempDirs remain alive until every blocking reader has completed.
        drop(target_temporary);
        Ok(result)
    }

    pub(super) async fn preview_data_git_status(
        &self,
        project: &ProjectHandle,
        tools: &GitToolchain,
        context: &mut Context,
    ) -> Result<Snapshot, ApiError> {
        let mut snapshot = inspect(project, Some(tools), context, None).await?;
        self.add_recovery_issue(&project.manifest.id, &mut snapshot.view)?;
        snapshot.view.source_restore =
            crate::source_restore_guard::inspect(self.paths.workspace_root(), &project.manifest.id);
        finalize_capabilities(&mut snapshot.view);
        if !snapshot.view.can_save || head_oid(&snapshot.view).is_none() {
            return Err(preview_error("PROJECT_DATA_RESTORE_UNAVAILABLE", "数据版本预览需要健康项目、项目根标准 Git、已提交版本和匹配的索引；请先处理当前状态。")
                .detail("repositoryState", snapshot.view.repository.state)
                .detail("issues", json!(snapshot.view.issues)));
        }
        Ok(snapshot)
    }

    /// The request may be cancelled or dropped while a blocking SQLite read is
    /// still finishing. Its runner remains joinable by manager shutdown, and
    /// admission shares the shutdown mutex so no worker appears after draining.
    pub(super) fn preview_worker<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<T, ApiError>>, ApiError> {
        let _submission = self.submission.lock().unwrap();
        self.available()?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(work);
        let runner = tokio::spawn(async move {
            let result = worker
                .await
                .map_err(|_| preview_worker_failed())
                .and_then(|result| result);
            let _ = sender.send(result);
        });
        let mut runners = self.runners.lock().unwrap();
        // Preserve completed outcomes for the shutdown join; panic is not success.
        runners.push(runner);
        Ok(receiver)
    }
}

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

pub(super) async fn target_snapshot(
    manager: &ProjectGitManager,
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    tree: objects::DataTree,
) -> Result<(TempDir, version_data::PreparedDataSnapshot), ApiError> {
    let temporary = tempfile::Builder::new()
        .prefix("local-projects-data-target-")
        .tempdir()?;
    let root = temporary.path().canonicalize()?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let key = "version-data/snapshot.json".to_string();
    let manifest_blob = BTreeMap::from([(key.clone(), tree.files[&key].clone())]);
    objects::materialize(project, tools, context, &root, &manifest_blob).await?;
    let bytes = read_file(&root.join(version_data::MANIFEST), MAX_METADATA)?;
    let manifest: version_data::DataSnapshotManifest =
        serde_json::from_slice(&bytes).map_err(|_| target_error("目标数据快照清单格式无效。"))?;
    if manifest.format_version != 1
        || manifest.project_id != project.manifest.id
        || manifest.sdk_api_version != project.manifest.sdk_api_version
        || manifest.database_path != project.manifest.database
        || manifest.attachments_dir != project.manifest.attachments_dir
    {
        return Err(target_error(
            "目标数据快照不属于当前项目或其数据库/附件路径已经变化。",
        ));
    }
    let provisional = version_data::PreparedDataSnapshot {
        root: root.clone(),
        manifest,
        manifest_digest: sha256(&bytes),
        manifest_bytes: bytes,
    };
    let limits = version_data::SnapshotLimits::default();
    validate_manifest_entries(&provisional.manifest, &project.manifest, limits, || {
        context.checkpoint()
    })?;
    let expected = version_data::tracked_files(&provisional);
    if expected.len() != tree.files.len()
        || expected.iter().any(|(path, file)| {
            tree.files
                .get(path)
                .is_none_or(|blob| blob.bytes != file.bytes)
        })
    {
        return Err(target_error(
            "目标 Git 数据树与清单列出的完整文件或长度不一致。",
        ));
    }
    // Git does not store empty attachment directories; these are represented by
    // the manifest. Any tree Git *does* store must be a parent of a listed file.
    let mut expected_directories = BTreeSet::new();
    for path in expected.keys() {
        context.checkpoint()?;
        let mut parent = Path::new(path).parent();
        while let Some(directory) = parent.filter(|directory| !directory.as_os_str().is_empty()) {
            expected_directories.insert(directory.to_str().ok_or_else(unsafe_path)?.to_string());
            parent = directory.parent();
        }
    }
    if tree.directories != expected_directories {
        return Err(target_error("目标 Git 数据树含额外或缺失的目录对象。"));
    }
    let root_fd = files::root(&root)?;
    let _ = files::parent(&root_fd, "attachments/.directory-placeholder", true)?;
    for directory in &provisional.manifest.directories {
        context.checkpoint()?;
        let _ = files::parent(
            &root_fd,
            &format!("attachments/{directory}/.directory-placeholder"),
            true,
        )?;
    }
    let remaining = tree
        .files
        .iter()
        .filter(|(path, _)| **path != key)
        .map(|(path, blob)| (path.clone(), blob.clone()))
        .collect();
    objects::materialize(project, tools, context, &root, &remaining).await?;
    // Re-hash through raw hash-object --no-filters without -w. This verifies that
    // the streamed bytes really have the immutable OIDs advertised by ls-tree.
    let relative = tree
        .files
        .iter()
        .map(|(path, blob)| {
            (
                path.strip_prefix("version-data/").unwrap().to_string(),
                blob.oid.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if hash_paths(tools, context, None, &root, relative.keys(), false).await? != relative {
        return Err(target_error("目标 Git 对象的原始内容与对象标识不一致。"));
    }
    let expected_manifest = project.manifest.clone();
    let stop = context.stop.clone();
    let cancel = context.cancel.clone();
    let mut worker = manager.preview_worker(move || {
        let result = version_data::validate_snapshot(
            &root,
            &expected_manifest,
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )?;
        Ok::<_, ApiError>((temporary, result))
    })?;
    await_worker(&mut worker, context).await
}

/// Validate untrusted manifest names before mkdir/create. Requiring every parent
/// to be explicitly listed makes the entry budget a bound on the actual tree,
/// including empty directories, rather than just a count of deeply nested tips.
fn validate_manifest_entries(
    manifest: &version_data::DataSnapshotManifest,
    project: &ProjectManifest,
    limits: version_data::SnapshotLimits,
    mut checkpoint: impl FnMut() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if manifest
        .attachments
        .len()
        .checked_add(manifest.directories.len())
        .is_none_or(|count| count > limits.max_entries)
        || manifest.database.bytes > limits.max_file_bytes
        || manifest
            .directories
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(preview_limit());
    }
    let directories = manifest
        .directories
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut normalized = BTreeSet::new();
    for path in manifest
        .directories
        .iter()
        .chain(manifest.attachments.keys())
    {
        checkpoint()?;
        let parts = files::parts(path)?;
        if protected(&format!("version-data/attachments/{path}"), project).is_some()
            || parts.iter().any(|part| {
                [".git", ".gitattributes", ".gitignore", ".gitmodules"]
                    .iter()
                    .any(|reserved| part.eq_ignore_ascii_case(reserved))
            })
            || !normalized.insert(path.nfd().flat_map(char::to_lowercase).collect::<String>())
        {
            return Err(target_error(
                "目标附件路径包含受保护名称或大小写、Unicode、文件与目录碰撞。",
            ));
        }
        let mut ancestor = path.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            checkpoint()?;
            if !directories.contains(parent) {
                return Err(target_error(
                    "目标数据清单缺少附件目录的完整父链；未创建隐含目录。",
                ));
            }
            ancestor = parent;
        }
    }
    let mut total = manifest.database.bytes;
    for file in std::iter::once(&manifest.database).chain(manifest.attachments.values()) {
        checkpoint()?;
        if file.bytes > limits.max_file_bytes
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(target_error("目标数据清单含无效长度或 SHA-256 摘要。"));
        }
    }
    for file in manifest.attachments.values() {
        total = total.checked_add(file.bytes).ok_or_else(preview_limit)?;
    }
    if total > limits.max_total_bytes {
        return Err(preview_limit());
    }
    Ok(())
}

async fn current_snapshot(
    manager: &ProjectGitManager,
    project: ProjectHandle,
    operation: ProjectTaskLease,
    context: &mut Context,
) -> Result<
    (
        ProjectTaskLease,
        TempDir,
        version_data::PreparedDataSnapshot,
    ),
    ApiError,
> {
    let lease =
        version_data::SnapshotLease::acquire(project, context.stop.clone(), context.cancel.clone())
            .await?;
    let temporary = tempfile::Builder::new()
        .prefix("local-projects-data-current-")
        .tempdir()?;
    let destination = temporary.path().canonicalize()?.join("current");
    let stop = context.stop.clone();
    let cancel = context.cancel.clone();
    let mut worker = manager.preview_worker(move || {
        let snapshot = version_data::prepare_snapshot(
            lease,
            &destination,
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )
        .map_err(|mut error| {
            // A preview owns only temporary candidates, never retained
            // operation evidence. Do not report deleted temp paths as saved.
            if let Some(details) = error.details.as_object_mut() {
                details.remove("candidatePath");
                details.remove("partialFilesRetained");
            }
            if error.code == "PROJECT_DATA_SNAPSHOT_CHANGED" {
                error.message =
                    "数据或附件在读取期间变化；本次预览未完成，当前数据未被替换。".into();
            }
            error = error
                .outcome("not_started")
                .detail("liveDataChanged", false);
            error
        })?;
        Ok::<_, ApiError>((operation, temporary, snapshot))
    })?;
    await_worker(&mut worker, context).await
}

pub(super) async fn await_worker<T: Send + 'static>(
    worker: &mut tokio::sync::oneshot::Receiver<Result<T, ApiError>>,
    context: &mut Context,
) -> Result<T, ApiError> {
    tokio::select! { biased;
        _ = watch_stop(&mut context.stop) => Err(interrupted()),
        _ = watch_stop(&mut context.cancel) => Err(interrupted()),
        _ = tokio::time::sleep_until(context.deadline) => Err(preview_timeout()),
        result = worker => result.map_err(|_| preview_worker_failed())?,
    }
}

fn summary(manifest: &version_data::DataSnapshotManifest) -> DataVersionSummary {
    DataVersionSummary {
        database_bytes: manifest.database.bytes,
        attachment_count: manifest.attachments.len(),
        attachment_bytes: manifest.attachments.values().map(|file| file.bytes).sum(),
        directory_count: manifest.directories.len(),
    }
}
fn attachment_changes(
    current: &version_data::DataSnapshotManifest,
    target: &version_data::DataSnapshotManifest,
) -> Vec<DataAttachmentChange> {
    let paths = current
        .attachments
        .keys()
        .chain(target.attachments.keys())
        .collect::<BTreeSet<_>>();
    paths
        .into_iter()
        .filter_map(|path| {
            let before = current.attachments.get(path);
            let after = target.attachments.get(path);
            if before == after {
                return None;
            }
            Some(DataAttachmentChange {
                path: path.clone(),
                kind: if before.is_none() {
                    "add"
                } else if after.is_none() {
                    "delete"
                } else {
                    "modify"
                }
                .into(),
            })
        })
        .collect()
}
fn directory_changes(
    current: &version_data::DataSnapshotManifest,
    target: &version_data::DataSnapshotManifest,
) -> Vec<DataAttachmentChange> {
    let before = current.directories.iter().collect::<BTreeSet<_>>();
    let after = target.directories.iter().collect::<BTreeSet<_>>();
    before
        .union(&after)
        .filter_map(|path| {
            if before.contains(path) == after.contains(path) {
                return None;
            }
            Some(DataAttachmentChange {
                path: (*path).clone(),
                kind: if before.contains(path) {
                    "delete"
                } else {
                    "add"
                }
                .into(),
            })
        })
        .collect()
}
fn preview_error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
        .outcome("not_started")
        .detail("liveDataChanged", false)
}
fn target_error(message: &str) -> ApiError {
    preview_error("PROJECT_DATA_RESTORE_TARGET_INVALID", message)
}
fn preview_limit() -> ApiError {
    preview_error(
        "PROJECT_DATA_RESTORE_LIMIT",
        "数据版本超过本次完整预览预算；没有截断内容后继续。",
    )
}
fn preview_timeout() -> ApiError {
    preview_error(
        "PROJECT_DATA_RESTORE_TIMEOUT",
        "数据预览超过内部时间限制；当前数据未被替换。",
    )
}
fn preview_worker_failed() -> ApiError {
    preview_error(
        "PROJECT_DATA_RESTORE_PREVIEW_FAILED",
        "数据预览工作未完成；当前数据未被替换。",
    )
}

#[cfg(test)]
#[path = "project_git_data_preview_tests.rs"]
mod tests;
