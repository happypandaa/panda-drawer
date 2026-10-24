//! Data-save integration. Git owns history; retained directories own only this
//! operation's unfinished publication and displaced current snapshot evidence.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitDataSaveInput {
    #[serde(deserialize_with = "required_head")]
    pub expected_head: Option<String>,
    pub expected_snapshot: String,
    pub message: String,
    pub author: GitAuthor,
}
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

impl ProjectGitManager {
    pub async fn save_data(
        self: &Arc<Self>,
        id: &str,
        input: GitDataSaveInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        // Apply the exact existing validation without starting a source-only task.
        let candidate: GitSaveInput = input.clone().into();
        validate_digest(&candidate.expected_snapshot)?;
        if candidate
            .expected_head
            .as_deref()
            .is_some_and(|head| !valid_oid(head))
            || candidate.message.trim().is_empty()
            || candidate.message.len() > 4000
            || candidate
                .message
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            || candidate.author.name.trim().is_empty()
            || candidate.author.name.len() > 200
            || candidate
                .author
                .name
                .chars()
                .any(|c| c.is_control() || "<>".contains(c))
            || candidate.author.email.len() > 254
            || !candidate.author.email.contains('@')
            || candidate
                .author
                .email
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || "<>".contains(c))
        {
            return Err(input_error());
        }
        self.submit(id, Operation::SaveData(input), stop)
    }

    /// Deliberately bypass ordinary Git inspection so damaged publication remains
    /// inspectable. This never creates a lock, repairs an index, or adopts files.
    pub async fn data_save_status(
        &self,
        id: &str,
        stop: watch::Receiver<bool>,
    ) -> Result<Value, ApiError> {
        self.available()?;
        let _lease = self
            .store
            .reserve_transient_operation(id, "data-save-recovery-status")?;
        let project = self.store.project(id)?;
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(stop, cancel)?;
        let _gate = read_gate_for_data(&project, &mut context).await?;
        let root = self
            .journal_path(id, "task_unused")
            .parent()
            .unwrap()
            .to_path_buf();
        check_no_symlinks(&root, true)?;
        let mut operations = vec![];
        let mut issues = vec![];
        if root.try_exists()? {
            for entry in fs::read_dir(&root)? {
                context.checkpoint()?;
                let path = entry?.path();
                if path.extension().is_none_or(|ext| ext != "json") {
                    continue;
                }
                let bytes = read_file(&path, MAX_METADATA)?;
                let journal: Journal = match serde_json::from_slice(&bytes) {
                    Ok(value) => value,
                    Err(_) => {
                        issues.push(recovery());
                        continue;
                    }
                };
                if journal.operation != "save-data"
                    || matches!(journal.phase.as_str(), "completed" | "not-committed")
                {
                    continue;
                }
                let outcome = self.check_data_finalization(&project, &journal);
                operations.push(json!({"saveTaskId":journal.task_id,"phase":journal.phase,"journalDigest":sha256(&bytes),"targetCommit":journal.new_head,"canFinalize":outcome.is_ok(),"operationOutcome":outcome.as_ref().map(|value|if value.committed{"committed"}else{"not_committed"}).ok(),"issues":outcome.err().into_iter().collect::<Vec<_>>()}));
            }
        }
        Ok(
            json!({"state":if operations.is_empty() && issues.is_empty(){"clear"}else if !issues.is_empty() || operations.iter().any(|op| op["canFinalize"]!=true){"blocked"}else{"pending"},"operations":operations,"issues":issues}),
        )
    }

    pub async fn finalize_data_save(
        self: &Arc<Self>,
        id: &str,
        input: GitDataSaveFinalizeInput,
        stop: watch::Receiver<bool>,
    ) -> Result<TaskRecord, ApiError> {
        validate_data_task(&input.save_task_id)?;
        validate_digest(&input.expected_journal_digest)?;
        self.submit(id, Operation::DataFinalize(input), stop)
    }

    pub(super) async fn finalize_data_save_inner(
        &self,
        project: &ProjectHandle,
        task_id: &str,
        input: GitDataSaveFinalizeInput,
        context: &mut Context,
    ) -> Result<Value, ApiError> {
        let _gate = read_gate_for_data(project, context).await?;
        let path = self.journal_path(&project.manifest.id, &input.save_task_id);
        let bytes = read_file(&path, MAX_METADATA)?;
        if sha256(&bytes) != input.expected_journal_digest {
            return Err(stale());
        }
        let mut journal: Journal = serde_json::from_slice(&bytes).map_err(|_| recovery())?;
        let checked = self.check_data_finalization(project, &journal)?;
        let candidate = checked.candidate;
        let outcome = if checked.committed {
            "committed"
        } else {
            "not_committed"
        };
        let finalize = (|| -> Result<Value, ApiError> {
            context.checkpoint()?;
            self.tasks.phase(task_id, "git-data-finalize")?;
            // No await/cancellation after the checked publication begins. A changed
            // path/index is retained rather than overwritten or silently retried.
            let publication = journal.data_publication.as_ref().ok_or_else(recovery)?;
            let git = project.root.join(".git");
            let index = git.join("index");
            if !checked.committed {
                journal.phase = "not-committed-release".into();
                atomic_json(&path, &journal)?;
                let lock = git.join("index.lock");
                if lock.try_exists()? {
                    let metadata = fs::symlink_metadata(&lock)?;
                    if !metadata.is_file()
                        || metadata.nlink() != 1
                        || (metadata.dev(), metadata.ino()) != publication.lock_identity
                        || content_digest(&lock)? != Some(publication.prepared_index.clone())
                        || index_fingerprint(&index)? != publication.previous_index
                    {
                        return Err(recovery());
                    }
                    fs::remove_file(&lock)?;
                    File::open(&git)?.sync_all()?;
                }
                journal.phase = "not-committed".into();
                atomic_json(&path, &journal)?;
                return Ok(
                    json!({"operation":"save-data-finalize","operationOutcome":"not_committed","saveTaskId":input.save_task_id,"commit":null,"liveDataChanged":false,"candidateRetained":true}),
                );
            }
            if content_digest(&index)? != Some(publication.prepared_index.clone()) {
                let lock = git.join("index.lock");
                let metadata = fs::symlink_metadata(&lock)?;
                if (metadata.dev(), metadata.ino()) != publication.lock_identity
                    || content_digest(&lock)? != Some(publication.prepared_index.clone())
                    || index_fingerprint(&index)? != publication.previous_index
                {
                    return Err(recovery());
                }
                fs::rename(&lock, &index)?;
                File::open(&git)?.sync_all()?;
            }
            self.publish_data_snapshot(project, &input.save_task_id, &journal, &candidate)?;
            journal.phase = "completed".into();
            atomic_json(&path, &journal)?;
            let warning = self
                .cleanup_data_save(project, &input.save_task_id, &journal)
                .err();
            Ok(
                json!({"operation":"save-data-finalize","operationOutcome":"committed","saveTaskId":input.save_task_id,"commit":journal.new_head,"database":data_result(Some(&candidate)),"liveDataChanged":false,"cleanupWarning":warning}),
            )
        })();
        finalize.map_err(|error| error.outcome(outcome).detail("pending", true))
    }

    fn check_data_finalization(
        &self,
        project: &ProjectHandle,
        journal: &Journal,
    ) -> Result<DataFinalization, ApiError> {
        let committed = journal
            .branch
            .as_deref()
            .and_then(|branch| read_current_ref(&project.root.join(".git"), branch).ok())
            .flatten()
            .as_ref()
            == journal.new_head.as_ref()
            && journal.new_head.is_some();
        self.check_data_finalization_inner(project, journal)
            .map_err(|error| {
                error
                    .outcome(if committed { "committed" } else { "unknown" })
                    .detail("pending", true)
            })
    }
    fn check_data_finalization_inner(
        &self,
        project: &ProjectHandle,
        journal: &Journal,
    ) -> Result<DataFinalization, ApiError> {
        if journal.schema_version != 1
            || journal.project_id != project.manifest.id
            || journal.operation != "save-data"
            || !matches!(
                journal.phase.as_str(),
                "prepared" | "ref-committed" | "not-committed-release"
            )
        {
            return Err(recovery());
        }
        validate_data_task(&journal.task_id)?;
        let publication = journal.data_publication.as_ref().ok_or_else(recovery)?;
        validate_digest(&publication.manifest_digest)?;
        validate_digest(&publication.prepared_index)?;
        let head = journal
            .new_head
            .as_deref()
            .filter(|head| valid_oid(head))
            .ok_or_else(recovery)?;
        let branch = journal.branch.as_deref().ok_or_else(recovery)?;
        one_ref(format!("{branch}\n").as_bytes())?;
        let git = project.root.join(".git");
        if identity(&git)? != publication.repository_identity {
            return Err(recovery());
        }
        if read_file(&git.join("HEAD"), 4096)? != format!("ref: {branch}\n").as_bytes() {
            return Err(recovery());
        }
        let current_head = read_current_ref(&git, branch)?;
        let committed = current_head.as_deref() == Some(head);
        if !committed
            && (!matches!(journal.phase.as_str(), "prepared" | "not-committed-release")
                || current_head != journal.old_head)
        {
            return Err(recovery().detail(
                "reason",
                "HEAD或分支已变化；无法确认本次提交结果，未覆盖现场。",
            ));
        }
        // The known lock may be ours. Ref/HEAD locks still indicate another writer.
        for name in [
            "HEAD.lock".to_owned(),
            format!("{branch}.lock"),
            "packed-refs.lock".into(),
        ] {
            if git.join(name).try_exists()? {
                return Err(recovery());
            }
        }
        let current = index_fingerprint(&git.join("index"))?;
        let pending = git.join("index.lock");
        if pending.try_exists()? {
            let metadata = fs::symlink_metadata(&pending)?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || (metadata.dev(), metadata.ino()) != publication.lock_identity
                || content_digest(&pending)? != Some(publication.prepared_index.clone())
                || current != publication.previous_index
            {
                return Err(recovery());
            }
        } else if committed {
            if content_digest(&git.join("index"))? != Some(publication.prepared_index.clone()) {
                return Err(recovery());
            }
        } else if journal.phase != "not-committed-release" || current != publication.previous_index
        {
            return Err(recovery());
        }
        let operation = self
            .journal_path(&project.manifest.id, &journal.task_id)
            .with_extension("files");
        if identity(&operation)? != publication.operation_identity {
            return Err(recovery());
        }
        let candidate = operation.join("data");
        let live = project.root.join(version_data::DIRECTORY);
        let selected = if candidate.try_exists()? {
            candidate
        } else {
            live.clone()
        };
        let (_stop_sender, stop) = watch::channel(false);
        let (_cancel_sender, cancel) = watch::channel(false);
        let verified = version_data::validate_snapshot(
            &selected,
            &project.manifest,
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )?;
        if verified.manifest_digest != publication.manifest_digest {
            return Err(recovery());
        }
        if selected != live && live.try_exists()? {
            let previous = version_data::validate_snapshot(
                &live,
                &project.manifest,
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )?;
            if Some(previous.manifest_digest) != publication.previous_manifest_digest {
                return Err(recovery());
            }
        }
        if !committed
            && (selected == live
                || operation.join("previous-data").try_exists()?
                || (!live.try_exists()? && publication.previous_manifest_digest.is_some()))
        {
            return Err(recovery());
        }
        // `publish_data_snapshot` verifies the displaced/current snapshot before
        // every move; a merely matching manifest never authorizes clobbering.
        Ok(DataFinalization {
            candidate: verified,
            committed,
        })
    }

    pub(super) fn publish_data_snapshot(
        &self,
        project: &ProjectHandle,
        task: &str,
        journal: &Journal,
        candidate: &version_data::PreparedDataSnapshot,
    ) -> Result<(), ApiError> {
        let publication = journal.data_publication.as_ref().ok_or_else(recovery)?;
        let operation = self
            .journal_path(&project.manifest.id, task)
            .with_extension("files");
        let prepared = operation.join("data");
        let previous = operation.join("previous-data");
        let live = project.root.join(version_data::DIRECTORY);
        if candidate.manifest_digest != publication.manifest_digest {
            return Err(recovery());
        }
        let (_stop_sender, stop) = watch::channel(false);
        let (_cancel_sender, cancel) = watch::channel(false);
        let verify = |path: &Path| {
            version_data::validate_snapshot(
                path,
                &project.manifest,
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )
        };
        if live.try_exists()? {
            let existing = verify(&live)?;
            if existing.manifest_digest == publication.manifest_digest {
                if prepared.try_exists()?
                    && (publication.previous_manifest_digest.as_ref()
                        != Some(&publication.manifest_digest)
                        || verify(&prepared)?.manifest_digest != publication.manifest_digest)
                {
                    return Err(recovery());
                }
                return Ok(());
            }
            if Some(&existing.manifest_digest) != publication.previous_manifest_digest.as_ref()
                || previous.try_exists()?
            {
                return Err(recovery());
            }
            rename_new(&live, &previous)?;
            File::open(&project.root)?.sync_all()?;
            File::open(&operation)?.sync_all()?;
            #[cfg(test)]
            if self.data_save_fault.load(Ordering::Acquire) == 3 {
                self.data_save_fault.store(0, Ordering::Release);
                return Err(recovery());
            }
        } else if publication.previous_manifest_digest.is_some() {
            let old = verify(&previous)?;
            if Some(&old.manifest_digest) != publication.previous_manifest_digest.as_ref() {
                return Err(recovery());
            }
        } else if previous.try_exists()? {
            return Err(recovery());
        }
        let ready = verify(&prepared)?;
        if ready.manifest_digest != publication.manifest_digest {
            return Err(recovery());
        }
        rename_new(&prepared, &live)?;
        File::open(&operation)?.sync_all()?;
        File::open(&project.root)?.sync_all()?;
        Ok(())
    }

    /// Delete only individually validated files from this completed task. A new
    /// unknown file is never reached by recursive deletion; nonempty directories
    /// remain with an explicit cleanup warning instead of becoming extra history.
    pub(super) fn cleanup_data_save(
        &self,
        project: &ProjectHandle,
        task: &str,
        journal: &Journal,
    ) -> Result<(), ApiError> {
        let publication = journal.data_publication.as_ref().ok_or_else(recovery)?;
        let operation = self
            .journal_path(&project.manifest.id, task)
            .with_extension("files");
        if identity(&operation)? != publication.operation_identity {
            return Err(recovery());
        }
        let mut expected = BTreeMap::<String, SourceFile>::new();
        for (path, file) in &publication.files {
            expected.insert(format!("source/{path}"), file.clone());
        }
        expected.insert(
            "index".into(),
            SourceFile {
                mode: "100644".into(),
                digest: publication.prepared_index.clone(),
                size: fs::metadata(operation.join("index"))?.len(),
            },
        );
        let mut extra_directories = vec!["source".to_owned()];
        let pending_data = operation.join("data");
        if pending_data.try_exists()? {
            let (_s, stop) = watch::channel(false);
            let (_c, cancel) = watch::channel(false);
            let pending = version_data::validate_snapshot(
                &pending_data,
                &project.manifest,
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )?;
            if pending.manifest_digest != publication.manifest_digest {
                return Err(recovery());
            }
            for (path, file) in version_data::tracked_files(&pending) {
                expected.insert(
                    format!(
                        "data/{}",
                        path.strip_prefix("version-data/").ok_or_else(recovery)?
                    ),
                    SourceFile {
                        mode: "100644".into(),
                        digest: file.sha256,
                        size: file.bytes,
                    },
                );
            }
            extra_directories.extend(["data".into(), "data/attachments".into()]);
            extra_directories.extend(
                pending
                    .manifest
                    .directories
                    .iter()
                    .map(|path| format!("data/attachments/{path}")),
            );
        }
        let previous = operation.join("previous-data");
        if previous.try_exists()? {
            let (_s, stop) = watch::channel(false);
            let (_c, cancel) = watch::channel(false);
            let old = version_data::validate_snapshot(
                &previous,
                &project.manifest,
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )?;
            if Some(&old.manifest_digest) != publication.previous_manifest_digest.as_ref() {
                return Err(recovery());
            }
            for (path, file) in version_data::tracked_files(&old) {
                expected.insert(
                    format!(
                        "previous-data/{}",
                        path.strip_prefix("version-data/").ok_or_else(recovery)?
                    ),
                    SourceFile {
                        mode: "100644".into(),
                        digest: file.sha256,
                        size: file.bytes,
                    },
                );
            }
            extra_directories.extend(["previous-data".into(), "previous-data/attachments".into()]);
            extra_directories.extend(
                old.manifest
                    .directories
                    .iter()
                    .map(|path| format!("previous-data/attachments/{path}")),
            );
        }
        let mut directories = BTreeSet::new();
        for path in expected.keys() {
            for ancestor in Path::new(path)
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
            {
                directories.insert(ancestor.to_str().ok_or_else(recovery)?.to_owned());
            }
        }
        directories.extend(extra_directories);
        let mut found = BTreeSet::new();
        let mut found_dirs = BTreeSet::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            for entry in fs::read_dir(operation.join(&relative))? {
                let entry = entry?;
                let path = relative.join(entry.file_name());
                let label = path.to_str().ok_or_else(recovery)?.to_owned();
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.is_dir() {
                    if !directories.contains(&label) {
                        return Err(recovery());
                    }
                    found_dirs.insert(label);
                    pending.push(path);
                } else if metadata.is_file() && metadata.nlink() == 1 {
                    if !expected.contains_key(&label) {
                        return Err(recovery());
                    }
                    found.insert(label);
                } else {
                    return Err(recovery());
                }
            }
        }
        if found != expected.keys().cloned().collect::<BTreeSet<_>>() {
            return Err(recovery());
        }
        let mut stamps = BTreeMap::new();
        for (path, file) in &expected {
            let absolute = operation.join(path);
            let before = fs::symlink_metadata(&absolute)?;
            let bytes = read_file(
                &absolute,
                version_data::SnapshotLimits::default().max_file_bytes,
            )?;
            if bytes.len() as u64 != file.size || sha256(&bytes) != file.digest {
                return Err(recovery());
            }
            stamps.insert(path.clone(), before);
        }
        for (path, before) in stamps {
            let absolute = operation.join(path);
            if !same_file(&before, &fs::symlink_metadata(&absolute)?) {
                return Err(recovery());
            }
            fs::remove_file(absolute)?;
        }
        let mut directories = found_dirs.into_iter().collect::<Vec<_>>();
        directories.sort_by_key(|path| std::cmp::Reverse(path.split('/').count()));
        for path in directories {
            fs::remove_dir(operation.join(path))?;
        }
        if identity(&operation)? != publication.operation_identity {
            return Err(recovery());
        }
        fs::remove_dir(&operation)?;
        File::open(operation.parent().unwrap())?.sync_all()?;
        Ok(())
    }
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
    json!({"state":if scope=="project-root"{"supported"}else if scope=="source-only"{"source-only"}else{"not-supported"},"canSave":false,"currentSavedSnapshot":snapshot.map(|data|json!({"manifestDigest":data.manifest_digest,"databaseBytes":data.manifest.database.bytes,"attachmentCount":data.manifest.attachments.len()})),"reason":if scope=="project-root"{"主动保存源码和数据时，在业务操作结束后生成SQLite一致性快照及完整附件；状态预览不读取当前活库。只在本机Git保存，不自动推送。"}else if scope=="source-only"{"现有Git仅覆盖源码目录，不能纳入项目数据库；未移动Git根或创建第二个仓库。"}else{"数据保存需要项目根的标准Git仓库；Node自有数据库不在本步范围。"}})
}
pub(super) fn data_result(snapshot: Option<&version_data::PreparedDataSnapshot>) -> Value {
    match snapshot {
        Some(data) => {
            json!({"state":"included","manifestDigest":data.manifest_digest,"databaseBytes":data.manifest.database.bytes,"attachmentCount":data.manifest.attachments.len(),"liveDataChanged":false})
        }
        None => database_boundary(),
    }
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
pub(super) async fn read_gate_for_data<'a>(
    project: &'a ProjectHandle,
    context: &mut Context,
) -> Result<tokio::sync::RwLockReadGuard<'a, ()>, ApiError> {
    tokio::select! {biased;_ = watch_stop(&mut context.stop)=>Err(interrupted()),_ = watch_stop(&mut context.cancel)=>Err(interrupted()),_ = tokio::time::sleep_until(context.deadline)=>Err(interrupted()),gate=project.operation_gate.read()=>Ok(gate)}
}
fn validate_data_task(task: &str) -> Result<(), ApiError> {
    if task.len() != 37
        || !task.starts_with("task_")
        || !task[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Err(input_error())
    } else {
        Ok(())
    }
}
fn read_current_ref(git: &Path, branch: &str) -> Result<Option<String>, ApiError> {
    one_ref(format!("{branch}\n").as_bytes())?;
    let loose = git.join(branch);
    check_no_symlinks(&loose, true)?;
    if loose.try_exists()? {
        return one_oid(&read_file(&loose, 4096)?).map(Some);
    }
    let packed = git.join("packed-refs");
    check_no_symlinks(&packed, true)?;
    if !packed.try_exists()? {
        return Ok(None);
    }
    let bytes = read_file(&packed, MAX_METADATA)?;
    for line in std::str::from_utf8(&bytes).map_err(|_| recovery())?.lines() {
        if let Some((oid, name)) = line.split_once(' ') {
            if name == branch && valid_oid(oid) {
                return Ok(Some(oid.into()));
            }
        }
    }
    Ok(None)
}
struct DataFinalization {
    candidate: version_data::PreparedDataSnapshot,
    committed: bool,
}
fn content_digest(path: &Path) -> Result<Option<String>, ApiError> {
    check_no_symlinks(path, true)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    Ok(Some(sha256(&read_file(path, MAX_METADATA)?)))
}

pub(crate) fn check_combined_entry_budget(count: usize) -> Result<(), ApiError> {
    if count > MAX_FILES {
        Err(error(
            "PROJECT_GIT_COMBINED_LIMIT",
            "源码和数据快照合计超过当前Git条目预算；没有提交平台无法再次读取的版本。",
        ))
    } else {
        Ok(())
    }
}
fn data_pending_error() -> ApiError {
    error(
        "PROJECT_GIT_DATA_RECOVERY_REQUIRED",
        "数据版本保存尚未完成核对；请先完成该任务的保存收尾，再进行开发、删除或工作区迁移。",
    )
}
fn data_records(workspace: &Path, id: &str) -> Result<Vec<(PathBuf, Journal, String)>, ApiError> {
    if !crate::projects::valid_id(id) {
        return Err(input_error());
    }
    let root = workspace.join(".platform/project-state/git").join(id);
    check_no_symlinks(&root, true)?;
    if !root.try_exists()? {
        return Ok(vec![]);
    }
    let mut output = vec![];
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        if output.len() >= 1000 {
            return Err(data_pending_error());
        }
        let bytes = read_file(&path, MAX_METADATA).map_err(|_| data_pending_error())?;
        let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| data_pending_error())?;
        if journal.schema_version != 1
            || journal.project_id != id
            || path.file_stem().and_then(|value| value.to_str()) != Some(journal.task_id.as_str())
        {
            return Err(data_pending_error());
        }
        if journal.operation == "save-data"
            && !matches!(journal.phase.as_str(), "completed" | "not-committed")
        {
            output.push((path, journal, sha256(&bytes)));
        }
    }
    Ok(output)
}
pub(crate) fn check_data_operations_available(workspace: &Path, id: &str) -> Result<(), ApiError> {
    if let Some((_, journal, _)) = data_records(workspace, id)?.into_iter().next() {
        return Err(data_pending_error().detail("saveTaskId", journal.task_id));
    }
    Ok(())
}
pub(crate) fn check_data_workspace_available(workspace: &Path) -> Result<(), ApiError> {
    let root = workspace.join(".platform/project-state/git");
    check_no_symlinks(&root, true)?;
    if !root.try_exists()? {
        return Ok(());
    }
    let mut count = 0;
    for entry in fs::read_dir(root)? {
        count += 1;
        if count > 1000 {
            return Err(data_pending_error());
        }
        let entry = entry?;
        let id = entry
            .file_name()
            .into_string()
            .map_err(|_| data_pending_error())?;
        check_no_symlinks(&entry.path(), false)?;
        if !fs::symlink_metadata(entry.path())?.is_dir() {
            return Err(data_pending_error());
        }
        check_data_operations_available(workspace, &id)?;
    }
    Ok(())
}
pub(crate) fn check_data_recovery_record(
    workspace: &Path,
    id: &str,
    input: &GitDataSaveFinalizeInput,
) -> Result<(), ApiError> {
    let records = data_records(workspace, id)?;
    if records.len() != 1 {
        return Err(data_pending_error());
    }
    let (_, journal, digest) = &records[0];
    if journal.task_id != input.save_task_id || digest != &input.expected_journal_digest {
        return Err(stale());
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_git_data_tests.rs"]
mod tests;

pub(super) fn recheck_current_data(
    project: &ProjectHandle,
    expected: Option<&version_data::PreparedDataSnapshot>,
) -> Result<(), ApiError> {
    let (_stop_sender, stop) = watch::channel(false);
    let (_cancel_sender, cancel) = watch::channel(false);
    let root = project.root.join(version_data::DIRECTORY);
    if let Some(expected) = expected {
        let current = version_data::validate_snapshot(
            &root,
            &project.manifest,
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )?;
        if current.manifest_digest != expected.manifest_digest {
            return Err(stale());
        }
    } else if root.try_exists()? {
        return Err(stale());
    }
    Ok(())
}
