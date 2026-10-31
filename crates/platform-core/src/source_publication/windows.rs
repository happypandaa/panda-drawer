//! Windows publication of the immutable Git download into its editable author
//! directory. Existing directories have no ownership/ACL admission; only fresh
//! output and journal files receive the native private creation policy.
use super::*;
use crate::{file_identity::PersistedIdentity, files};
use platform_native::{
    tool_files::Facts,
    windows::{files as native, removal},
};
use std::{ffi::OsString, fs::File};

const AUTHOR_ENTRIES: usize = 50_000;
const AUTHOR_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Publication {
    schema_version: u32,
    source_id: String,
    source_task_id: String,
    source_digest: String,
    author_digest: String,
    app_id: String,
    directory: String,
    phase: String,
    payload: String,
    payload_identity: PersistedIdentity,
}

struct Anchor {
    path: PathBuf,
    file: File,
    // Ancestors stay pinned. The leaf allows DELETE sharing so retaining the
    // exact payload HANDLE does not obstruct its own exclusive publication.
    _scope: native::CheckedPath,
}
impl Anchor {
    fn open(path: PathBuf) -> Result<Self, ApiError> {
        let scope = native::open_no_reparse(&path).map_err(files::error)?;
        if !scope.identity.directory {
            return Err(invalid("源码发布位置不是普通目录。"));
        }
        Ok(Self {
            path,
            file: scope.file.try_clone()?,
            _scope: scope,
        })
    }
    fn child(&self, name: &str, create: bool) -> Result<Self, ApiError> {
        single_name(name)?;
        let (parent, _scope) = native::directory_scope(&self.file).map_err(files::error)?;
        let path = parent.join(name);
        if create {
            match native::create_private_directory(&path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(files::error(error)),
            }
            native::sync_directory(&self.file).map_err(files::error)?;
        }
        let mut child = Self::open(path)?;
        // Keep the caller-visible path, as on Unix. Operations resolve the
        // held HANDLE again and verify this original name before publishing.
        child.path = self.path.join(name);
        child.verify()?;
        Ok(child)
    }
    fn verify(&self) -> Result<(), ApiError> {
        let current = native::open_no_reparse(&self.path).map_err(files::error)?;
        if !current.identity.directory
            || !native::identity(&self.file)
                .map_err(files::error)?
                .same_object(&current.identity)
        {
            return Err(invalid("源码发布目录身份已变化，请保留现场。"));
        }
        Ok(())
    }
}
fn single_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(invalid("源码发布目录无效。"));
    }
    // A Git tree component is bounded by actual NTFS UTF-16 units, matching
    // source materialization. The attachment API's UTF-8 byte budget is a
    // separate contract and must not reject an already valid source name.
    super::super::policy_native::validate_names(name)
}
fn open_child(parent: &File, name: &std::ffi::OsStr) -> Result<File, ApiError> {
    let (parent, _scope) = native::directory_scope(parent).map_err(files::error)?;
    let path = parent.join(name);
    let checked = native::open_no_reparse(&path).map_err(files::error)?;
    if checked.identity.directory {
        return Ok(checked.file);
    }
    let file = native::open_read_file(&path).map_err(files::error)?;
    if !checked
        .identity
        .same_object(&native::identity(&file).map_err(files::error)?)
    {
        return Err(invalid("源码条目身份已变化，未发布。"));
    }
    Ok(file)
}
fn exists_at(parent: &File, name: &str) -> Result<bool, ApiError> {
    single_name(name)?;
    let (parent, _scope) = native::directory_scope(parent).map_err(files::error)?;
    // Like fstatat(AT_SYMLINK_NOFOLLOW), existence observes a reparse leaf
    // itself. It grants no authority to read, copy or replace that leaf.
    match removal::inspect(&parent.join(name)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(files::error(error)),
    }
}
fn names(directory: &File) -> Result<Vec<OsString>, ApiError> {
    let (path, _scope) = native::directory_scope(directory).map_err(files::error)?;
    let mut result = Vec::new();
    for entry in fs::read_dir(path)? {
        if result.len() >= AUTHOR_ENTRIES {
            return Err(limit());
        }
        result.push(entry?.file_name());
    }
    result.sort();
    Ok(result)
}
fn create_child_directory(parent: &File, name: &str) -> Result<File, ApiError> {
    single_name(name)?;
    let (parent, _scope) = native::directory_scope(parent).map_err(files::error)?;
    let path = parent.join(name);
    native::create_private_directory(&path).map_err(files::error)?;
    let checked = native::open_no_reparse(&path).map_err(files::error)?;
    if !checked.identity.directory {
        return Err(invalid("源码复制位置不是普通目录。"));
    }
    Ok(checked.file)
}
fn create_child_file(parent: &File, name: &str) -> Result<File, ApiError> {
    single_name(name)?;
    let (parent, _scope) = native::directory_scope(parent).map_err(files::error)?;
    // Pin the actual held parent through exclusive native creation. Every
    // subsequent write/permission change/flush uses this original file HANDLE.
    native::create_private_file(&parent.join(name)).map_err(files::error)
}
fn author_tree(
    directory: &File,
    target: Option<&File>,
    relative: &str,
    depth: usize,
    budget: &mut (usize, u64),
    tree: &mut Sha256,
) -> Result<(), ApiError> {
    if depth > 40 {
        return Err(limit());
    }
    let entries = names(directory)?;
    for name in &entries {
        budget.0 += 1;
        if budget.0 > AUTHOR_ENTRIES {
            return Err(limit());
        }
        let label = name
            .to_str()
            .ok_or_else(|| invalid("源码文件名不是 UTF-8。"))?;
        if label.chars().any(char::is_control) {
            return Err(invalid("源码文件名包含控制字符。"));
        }
        let path = if relative.is_empty() {
            label.to_owned()
        } else {
            format!("{relative}/{label}")
        };
        let mut input = open_child(directory, name)?;
        let before = Facts::from_file(&input)?;
        tree.update(path.as_bytes());
        tree.update([0]);
        if before.is_dir() {
            tree.update(b"directory");
            if let Some(target) = target {
                let child = create_child_directory(target, label)?;
                author_tree(&input, Some(&child), &path, depth + 1, budget, tree)?;
                native::sync_directory(&child).map_err(files::error)?;
            } else {
                author_tree(&input, None, &path, depth + 1, budget, tree)?;
            }
        } else {
            budget.1 = budget.1.checked_add(before.len()).ok_or_else(limit)?;
            if budget.1 > AUTHOR_BYTES || before.len() > AUTHOR_BYTES {
                return Err(limit());
            }
            let permissions = input.metadata()?.permissions();
            let mut output = target
                .map(|target| create_child_file(target, label))
                .transpose()?;
            let mut digest = Sha256::new();
            let mut size = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                size += read as u64;
                if size > before.len() {
                    return Err(invalid("复制时源码文件增长，未发布。"));
                }
                digest.update(&buffer[..read]);
                if let Some(output) = output.as_mut() {
                    output.write_all(&buffer[..read])?;
                }
            }
            if size != before.len() || !before.same_modified_permissions(&Facts::from_file(&input)?)
            {
                return Err(invalid("复制时源码内容变化，未发布。"));
            }
            if let Some(output) = output {
                output.set_permissions(permissions)?;
                // The original writable output HANDLE performs the real flush;
                // reopening a read-only copy cannot stand in for that receipt.
                output.sync_all()?;
            }
            tree.update(b"file");
            tree.update(size.to_le_bytes());
            // Windows has no Unix executable permission. Git's executable bit
            // remains in its own index; do not invent a host permission bit.
            tree.update([0]);
            tree.update(digest.finalize());
        }
        let current = open_child(directory, name)?;
        if !before.same_modified_permissions(&Facts::from_file(&current)?) {
            return Err(invalid("复制时源码条目身份变化，未发布。"));
        }
    }
    if entries != names(directory)? {
        return Err(invalid("复制时源码目录条目变化，未发布。"));
    }
    if let Some(target) = target {
        native::sync_directory(target).map_err(files::error)?;
    }
    Ok(())
}
fn author_digest(directory: &File, target: Option<&File>) -> Result<String, ApiError> {
    Ok(author_snapshot(directory, target)?.0)
}
fn author_snapshot(directory: &File, target: Option<&File>) -> Result<(String, u64), ApiError> {
    let mut digest = Sha256::new();
    let mut budget = (0, 0);
    author_tree(directory, target, "", 0, &mut budget, &mut digest)?;
    Ok((format!("{:x}", digest.finalize()), budget.1))
}
fn require_copy_space(directory: &File, bytes: u64) -> Result<(), ApiError> {
    let (path, _scope) = native::directory_scope(directory).map_err(files::error)?;
    if fs2::available_space(path)? < bytes + MIN_FREE {
        return Err(error(
            "SOURCE_PUBLICATION_SPACE_LOW",
            "剩余空间不足以准备源码并保留运行余量，请释放空间后重试。",
        ));
    }
    Ok(())
}
fn record_bytes(receipt: &File) -> Result<Option<Vec<u8>>, ApiError> {
    if !exists_at(receipt, "publication.json")? {
        return Ok(None);
    }
    let file = files::read(receipt, "publication.json")?;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    Ok(Some(bytes))
}
fn save_publication(
    receipt: &Anchor,
    expected: &mut Option<Vec<u8>>,
    publication: &Publication,
) -> Result<(), ApiError> {
    receipt.verify()?;
    if record_bytes(&receipt.file)? != *expected {
        return Err(invalid("源码发布记录被其他操作修改，请保留现场。"));
    }
    let bytes = serde_json::to_vec(publication)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    let temporary = format!(".publication-{}.json", Uuid::new_v4().simple());
    let (parent, _scope) = native::directory_scope(&receipt.file).map_err(files::error)?;
    let mut file = native::create_private_file_for_publication(&parent.join(&temporary))
        .map_err(files::error)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if record_bytes(&receipt.file)? != *expected {
        return Err(invalid("源码发布记录发生变化，未替换记录。"));
    }
    // Native success is this journal's commit point. There is no fallible
    // post-rename observation that could misreport its replacement as absent.
    native::rename_private_file_handle(&file, &parent.join("publication.json"), true)
        .map_err(files::error)?;
    *expected = Some(bytes);
    Ok(())
}
fn fresh_payload(receipt: &Anchor) -> Result<(String, Anchor), ApiError> {
    let name = format!("payload_{}", Uuid::new_v4().simple());
    let (parent, _scope) = native::directory_scope(&receipt.file).map_err(files::error)?;
    native::create_private_directory(&parent.join(&name)).map_err(files::error)?;
    native::sync_directory(&receipt.file).map_err(files::error)?;
    Ok((name.clone(), receipt.child(&name, false)?))
}
fn publication_base(repository: &str, app_id: &str) -> String {
    let base: String = repository.chars().take(80).collect();
    if base.is_empty() {
        "project".into()
    } else if single_name(&base).is_ok() {
        base
    } else {
        // A valid provider name may be a Windows device name or end in a dot.
        // Map only our newly allocated local directory; the public Git URL and
        // repository contents keep their original names and policy.
        crate::project_directory_name::readable_directory_name(&base, app_id)
    }
}
fn available_name(root: &File, base: &str) -> Result<String, ApiError> {
    for number in 1..100_000 {
        let name = if number == 1 {
            base.to_owned()
        } else {
            format!("{base}-{number}")
        };
        if !exists_at(root, &name)? {
            return Ok(name);
        }
    }
    Err(invalid("同名项目目录过多，请先核对项目位置。"))
}
fn publication_identity(publication: &Publication, directory: &File) -> Result<(), ApiError> {
    let observed =
        PersistedIdentity::from_file(directory).map_err(|_| invalid("无法核对源码目录身份。"))?;
    if !publication.payload_identity.matches_captured(&observed) {
        return Err(invalid("源码目录与发布记录不一致，未覆盖文件。"));
    }
    Ok(())
}
fn uncertain(error: &ApiError) -> bool {
    matches!(
        error
            .details
            .get("operationOutcome")
            .and_then(serde_json::Value::as_str),
        Some("unknown" | "committed")
    )
}
fn with_publication_reservation(
    ownership: &crate::node_project_ownership::NodeProjectOwnership,
    app: &str,
    publication: &Publication,
    publish: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if let Err(error) = ownership.reserve_published_source_identity(
        app,
        &publication.directory,
        &publication.payload_identity,
    ) {
        ownership.forget_unpublished_source_identity(
            app,
            &publication.directory,
            &publication.payload_identity,
        )?;
        return Err(error);
    }
    let result = publish();
    if result.as_ref().is_err_and(|error| !uncertain(error)) {
        ownership.forget_unpublished_source_identity(
            app,
            &publication.directory,
            &publication.payload_identity,
        )?;
    }
    result
}
fn publish_exclusive(
    from: &File,
    payload: &str,
    to: &File,
    name: &str,
    expected: &PersistedIdentity,
) -> Result<(), ApiError> {
    single_name(payload)?;
    single_name(name)?;
    let (source, _source_scope) = native::directory_scope(from).map_err(files::error)?;
    let (destination, _destination_scope) = native::directory_scope(to).map_err(files::error)?;
    let source = source.join(payload);
    let checked = native::open_no_reparse(&source).map_err(files::error)?;
    if !checked.identity.directory || !expected.matches_open_runtime(&checked.file)? {
        return Err(invalid("源码目录与发布记录不一致，未覆盖文件。"));
    }
    native::rename_directory_noreplace_expected(&source, &destination.join(name), &checked.identity)
        .map_err(|error| {
            if native::publication_outcome_unknown(&error) {
                files::error(error).outcome("unknown")
            } else {
                invalid("项目目录已有内容或无法发布；原内容与本次现场均保留。")
                    .detail("cause", error.to_string())
            }
        })
}
impl SourceAcquisitionManager {
    /// Reconcile only durable publication facts after its real writer stopped.
    /// This never creates, repairs, adopts, moves or deletes a project path.
    pub(crate) fn reconcile_author_publication(
        &self,
        source_id: &str,
        app_id: &str,
        task_id: &str,
    ) -> Result<Option<PathBuf>, ApiError> {
        let mut remaining_paths = Vec::<PathBuf>::new();
        (|| {
            if Uuid::parse_str(app_id)
                .map(|id| id.to_string() != app_id)
                .unwrap_or(true)
                || !super::super::source_id(source_id)
                || !crate::projects::valid_id(task_id)
            {
                return Err(invalid("源码发布恢复标识无效。"));
            }
            let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
                self.paths.data_root().into(),
                self.paths.workspace_root().into(),
            );
            // Existing ownership verifies original scope identities and the
            // original volume when a known name is absent. It also returns
            // absent reservations, so a missing target cannot erase authority.
            let owned = ownership.owned_paths(app_id);
            let root = self.paths.workspace_root().join("project-sources");
            let has_authors = |paths: &[PathBuf]| {
                paths
                    .iter()
                    .any(|path| path.parent() == Some(root.as_path()))
            };
            let receipt_path = self
                .paths
                .workspace_root()
                .join(".platform/source-publications")
                .join(app_id);
            remaining_paths.push(receipt_path.clone());
            let receipt = match Anchor::open(receipt_path.clone()) {
                Ok(receipt) => receipt,
                Err(error) if error.code == "FILE_NOT_FOUND" => {
                    let owned = owned?;
                    return if !has_authors(&owned) {
                        Ok(None)
                    } else {
                        Err(error)
                    };
                }
                Err(error) => return Err(error),
            };
            let Some(bytes) = record_bytes(&receipt.file)? else {
                let owned = owned?;
                if !owned.contains(&receipt_path) {
                    return Err(invalid("源码发布现场缺少原归属，未接管。"));
                }
                return if !has_authors(&owned) {
                    Ok(None)
                } else {
                    Err(invalid("源码发布记录缺失，已有发布预留不能当作未提交。"))
                };
            };
            let publication: Publication = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("源码发布记录无法读取，请保留现场。"))?;
            if publication.schema_version != 1
                || publication.app_id != app_id
                || publication.source_id != source_id
                || publication.source_task_id != task_id
                || !matches!(
                    publication.phase.as_str(),
                    "copying" | "ready" | "published"
                )
                || single_name(&publication.directory).is_err()
                || !publication.payload.starts_with("payload_")
                || single_name(&publication.payload).is_err()
                || publication.author_digest.len() != 64
            {
                return Err(invalid("源码发布记录与原导入不符，请保留现场。"));
            }
            let destination = root.join(&publication.directory);
            // These validated names are display-only diagnostics. Reading the
            // plan does not grant deletion authority, even when the ownership
            // check below fails because the real target is now foreign.
            remaining_paths.push(receipt_path.join(&publication.payload));
            remaining_paths.push(destination.clone());
            let owned = owned?;
            if !owned.contains(&receipt_path) {
                return Err(invalid("源码发布现场缺少原归属，未接管。"));
            }
            let target = match native::open_no_reparse(&destination) {
                Ok(target) => Some(target),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(files::error(error)),
            };
            let payload = if exists_at(&receipt.file, &publication.payload)? {
                let payload = receipt.child(&publication.payload, false)?;
                publication_identity(&publication, &payload.file)?;
                Some(payload)
            } else {
                None
            };
            if let Some(target) = target {
                if target.identity.directory
                    && publication
                        .payload_identity
                        .matches_open_runtime(&target.file)?
                {
                    if payload.is_some() || !owned.contains(&destination) {
                        return Err(invalid("源码发布身份或预留不一致，请保留现场。"));
                    }
                    receipt.verify()?;
                    return Ok(Some(destination));
                }
                // An unrelated target can never be adopted or cleaned. A
                // matching payload still proves our source is unpublished;
                // owned_paths above rejects a conflicting author reservation.
                if payload.is_some() {
                    return Ok(None);
                }
                return Err(invalid("源码发布目标被替换，原复制现场也已缺失。"));
            }
            if payload.is_some() {
                return Ok(None);
            }
            Err(invalid("源码发布目标与复制现场均缺失，提交状态待核对。"))
        })()
        .map_err(|error: ApiError| {
            error
                .outcome("unknown")
                .detail("remainingPaths", serde_json::json!(remaining_paths))
        })
    }

    pub(crate) fn publish_author_source(
        &self,
        source: &SourceCandidate,
        app_id: &str,
        lease_id: &str,
    ) -> Result<PathBuf, ApiError> {
        if Uuid::parse_str(app_id)
            .map(|id| id.to_string() != app_id)
            .unwrap_or(true)
        {
            return Err(invalid("应用标识无效。"));
        }
        if self.owner_app_id(&source.source_id)? != app_id {
            return Err(invalid("来源只属于原项目，不能发布给另一个实例。"));
        }
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let digest = source
            .content_digest
            .as_deref()
            .ok_or_else(|| invalid("源码摘要缺失。"))?;
        let source_task = source.task_id.clone();
        let source = self.verify_ready_leased(&source.source_id, digest, lease_id)?;
        if source.task_id != source_task {
            return Err(invalid("源码任务身份已变化，未发布。"));
        }
        let (url, provider, repository) = validate_url(&source.url)?;
        if source.provider != provider || source.url != url {
            return Err(invalid("只有本次 Git 添加成功的原始源码可以发布。"));
        }
        let original = Anchor::open(
            source
                .source_path
                .clone()
                .ok_or_else(|| invalid("源码位置缺失。"))?,
        )?;
        // A downloaded Git source must still contain its own ordinary Git root.
        original.child(".git", false)?;
        let (full_digest, copy_bytes) = author_snapshot(&original.file, None)?;
        original.verify()?;
        let workspace = Anchor::open(self.paths.workspace_root().into())?;
        let root = workspace.child("project-sources", true)?;
        ownership.record_publication(app_id)?;
        let receipt = workspace
            .child(".platform", false)?
            .child("source-publications", true)?
            .child(app_id, true)?;
        ownership.record_publication(app_id)?;
        let mut bytes = record_bytes(&receipt.file)?;
        let base = publication_base(&repository, app_id);
        let mut publication = if let Some(bytes) = &bytes {
            let publication: Publication = serde_json::from_slice(bytes)
                .map_err(|_| invalid("源码发布记录无法读取，请保留现场。"))?;
            if publication.schema_version != 1
                || publication.app_id != app_id
                || publication.source_id != source.source_id
                || publication.source_task_id != source.task_id
                || publication.source_digest != digest
                || !matches!(
                    publication.phase.as_str(),
                    "copying" | "ready" | "published"
                )
                || single_name(&publication.directory).is_err()
                || !publication.payload.starts_with("payload_")
                || single_name(&publication.payload).is_err()
                || publication.author_digest.len() != 64
            {
                return Err(invalid("源码发布记录与本次应用不一致，请保留现场。"));
            }
            publication
        } else {
            let (payload, stage) = fresh_payload(&receipt)?;
            let publication = Publication {
                schema_version: 1,
                source_id: source.source_id.clone(),
                source_task_id: source.task_id.clone(),
                source_digest: digest.into(),
                author_digest: full_digest.clone(),
                app_id: app_id.into(),
                directory: available_name(&root.file, &base)?,
                phase: "copying".into(),
                payload,
                payload_identity: PersistedIdentity::from_file(&stage.file)
                    .map_err(|_| invalid("无法核对复制目录身份。"))?,
            };
            save_publication(&receipt, &mut bytes, &publication)?;
            publication
        };
        if exists_at(&root.file, &publication.directory)? {
            if let Ok(destination) = root.child(&publication.directory, false) {
                if publication_identity(&publication, &destination.file).is_ok() {
                    // The published identity is already present. Later author
                    // edits are accepted and every remaining error is after
                    // commit, including a journal reply that cannot be saved.
                    return (|| {
                        root.verify()?;
                        receipt.verify()?;
                        workspace.verify()?;
                        publication.phase = "published".into();
                        save_publication(&receipt, &mut bytes, &publication)?;
                        ownership.reserve_published_source_identity(
                            app_id,
                            &publication.directory,
                            &publication.payload_identity,
                        )?;
                        Ok(destination.path)
                    })()
                    .map_err(|error: ApiError| error.outcome("committed"));
                }
            }
            if !exists_at(&receipt.file, &publication.payload)? {
                return Err(
                    invalid("发布后的源码位置被替换，请保留现场，不会再复制一份。")
                        .outcome("unknown"),
                );
            }
            ownership.forget_unpublished_source_identity(
                app_id,
                &publication.directory,
                &publication.payload_identity,
            )?;
            publication.directory = available_name(&root.file, &base)?;
            save_publication(&receipt, &mut bytes, &publication)?;
        }
        if publication.author_digest != full_digest {
            return Err(invalid("源码或 Git 元信息已变化，未继续发布。"));
        }
        if !exists_at(&receipt.file, &publication.payload)? {
            return Err(
                invalid("源码发布位置与复制现场均缺失，请保留现场核对。").outcome("unknown")
            );
        }
        let stage = receipt.child(&publication.payload, false)?;
        publication_identity(&publication, &stage.file)?;
        if publication.phase == "copying" {
            if author_digest(&stage.file, None)? != publication.author_digest
                && !names(&stage.file)?.is_empty()
            {
                return Err(invalid("源码复制尚未完成，请清理本次临时材料后重新添加。"));
            }
            if names(&stage.file)?.is_empty() {
                require_copy_space(&stage.file, copy_bytes)?;
                let copied = author_digest(&original.file, Some(&stage.file))?;
                if copied != publication.author_digest {
                    return Err(invalid("复制时源码变化，临时现场保留。"));
                }
            }
            if author_digest(&stage.file, None)? != publication.author_digest
                || author_digest(&original.file, None)? != publication.author_digest
            {
                return Err(invalid("源码副本核验不一致，未发布。"));
            }
            stage.verify()?;
            original.verify()?;
            self.verify_ready_leased(&source.source_id, digest, lease_id)?;
            publication.phase = "ready".into();
            save_publication(&receipt, &mut bytes, &publication)?;
        }
        if author_digest(&stage.file, None)? != publication.author_digest {
            return Err(invalid("复制现场已变化，未发布。"));
        }
        workspace.verify()?;
        root.verify()?;
        receipt.verify()?;
        stage.verify()?;
        original.verify()?;
        with_publication_reservation(&ownership, app_id, &publication, || {
            publish_exclusive(
                &receipt.file,
                &publication.payload,
                &root.file,
                &publication.directory,
                &publication.payload_identity,
            )
        })?;
        (|| {
            let destination = root.child(&publication.directory, false)?;
            publication_identity(&publication, &destination.file)?;
            root.verify()?;
            receipt.verify()?;
            workspace.verify()?;
            publication.phase = "published".into();
            save_publication(&receipt, &mut bytes, &publication)?;
            ownership.reserve_published_source_identity(
                app_id,
                &publication.directory,
                &publication.payload_identity,
            )?;
            Ok(destination.path)
        })()
        .map_err(|error: ApiError| error.outcome("committed"))
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
