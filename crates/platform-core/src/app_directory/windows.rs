//! Windows counterpart of the original directory onboarding policy. External
//! roots are observed, never repaired; new entries use private native creation.
use super::*;
use crate::file_identity::PersistedIdentity;
use platform_native::windows::{files as native, removal};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::Read,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
};

const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryInitializeInput {
    pub expected_revision: String,
    pub name: String,
}

struct Preview {
    source: PathBuf,
    identity: PersistedIdentity,
    revision: String,
    state: &'static str,
    inspection: Option<Value>,
    issues: Vec<String>,
    manifest_digest: Option<String>,
}
struct Created {
    path: PathBuf,
    directory: bool,
    identity: PersistedIdentity,
    digest: Option<Vec<u8>>,
    length: usize,
}
struct GeneratedDirectory {
    // Retain the verified root until the initialization response is built.
    _root: File,
    ready: Preview,
}
impl Preview {
    fn response(&self, requested_source: &Path, paths: &super::PromptPaths) -> Value {
        let prompt =
            super::directory_review_prompt(requested_source, self.state, &self.issues, paths);
        let mut response = json!({"sourcePath":requested_source,"state":self.state,"expectedRevision":self.revision,"issues":self.issues,"reviewPrompt":prompt});
        if let Some(inspection) = &self.inspection {
            response["inspection"] = inspection.clone();
        }
        response
    }
}

fn requested_source_response(mut response: Value, requested_source: &Path) -> Value {
    // Folder-picker/CLI round trips retain the caller's exact representation.
    // Canonical paths, full FileIds and preview revisions still authorize every
    // operation internally. Never strip a verbatim namespace to display it:
    // doing so could change a trailing-dot or other special Windows name.
    response["sourcePath"] = json!(requested_source);
    response
}

fn directory_error(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message)
}
pub(super) fn stale() -> ApiError {
    directory_error(
        "NODE_DIRECTORY_CHANGED",
        "目录或应用声明已变化，请重新检查并确认后再继续。",
    )
    .outcome("not_started")
}
fn utf16_bytes(value: &OsStr) -> Vec<u8> {
    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}
fn update_metadata(digest: &mut Sha256, path: &Path, include_mode: bool) -> Result<()> {
    // Like lstat in the original preview, arbitrary top-level entries are
    // observed themselves. A junction is review material, never followed.
    let entry = removal::inspect(path)?;
    let metadata = entry.file.metadata()?;
    digest.update(entry.identity.volume_serial.to_le_bytes());
    digest.update(entry.identity.file_id);
    if include_mode {
        digest.update(metadata.file_attributes().to_le_bytes());
    }
    digest.update(metadata.len().to_le_bytes());
    digest.update(metadata.last_write_time().to_le_bytes());
    Ok(())
}
fn child_path(parent: &File, name: &OsStr) -> Result<(PathBuf, native::PinnedDirectory)> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(stale());
    }
    let (directory, scope) = native::directory_scope(parent)?;
    Ok((directory.join(name), scope))
}
fn child_directory(parent: &File, name: &str) -> Result<File> {
    let (path, _scope) = child_path(parent, OsStr::new(name))?;
    native::directory_file(&path).map_err(Into::into)
}
fn create_child_directory(parent: &File, name: &OsStr) -> Result<()> {
    let (path, _scope) = child_path(parent, name)?;
    native::create_private_directory(&path).map_err(|error| creation_error(&path, error))
}
fn create_child_file(parent: &File, name: &OsStr) -> Result<File> {
    let (path, _scope) = child_path(parent, name)?;
    native::create_private_file(&path).map_err(|error| creation_error(&path, error))
}
fn creation_error(path: &Path, error: std::io::Error) -> ApiError {
    let occupied = error.kind() == std::io::ErrorKind::AlreadyExists;
    let issue = ApiError::from(error);
    // A failed native confirmation after creation cannot identify the new name
    // as ours. Preserve it and its original generated-root ownership receipt.
    // An ordinary exclusive-create collision still belongs to its other writer.
    if !occupied
        && !matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        issue
            .detail("generatedEntryUnconfirmed", true)
            .detail("unconfirmedCreatedPath", json!(path))
    } else {
        issue
    }
}
fn entries(root: &File) -> Result<Vec<OsString>> {
    let (path, _scope) = native::directory_scope(root)?;
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        names.push(entry?.file_name());
        if names.len() > MAX_ENTRIES {
            return Err(directory_error(
                "NODE_DIRECTORY_LIMIT",
                "目录顶层条目超过检查预算，请选择具体应用目录。",
            ));
        }
    }
    names.sort();
    Ok(names)
}
fn public_manifest(root: &File) -> Result<Vec<u8>> {
    let (path, _scope) = child_path(root, OsStr::new("panda.json"))?;
    let file = native::open_read_file(&path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 65536 {
        return Err(directory_error(
            "NODE_DIRECTORY_DECLARATION_UNSAFE",
            "panda.json 必须为不共享硬链接且不超过64KiB的普通文件；当前文件未读取。",
        ));
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(directory_error(
            "NODE_DIRECTORY_DECLARATION_UNSAFE",
            "panda.json 超过64KiB检查预算。",
        ));
    }
    Ok(bytes)
}

impl AppDirectories {
    fn directory_path(&self, source: &Path) -> Result<PathBuf> {
        ordinary_path(source, true)?;
        let source = source.canonicalize()?;
        crate::project_tool_boundary::require_source_excludes_global_tools(
            &self.data_root,
            &source,
        )?;
        let data = self.data_root.canonicalize()?;
        if source.starts_with(&data) || data.starts_with(&source) {
            return Err(directory_error(
                "NODE_DIRECTORY_UNSAFE",
                "请选择独立的外部目录，不能使用平台数据目录、受管应用目录或其父目录。",
            ));
        }
        // Compare physical ancestor identities as well as pathname spelling.
        let source_identity = PersistedIdentity::capture(&source).map_err(|_| stale())?;
        let data_identity = PersistedIdentity::capture(&data).map_err(|_| stale())?;
        for ancestor in source.ancestors() {
            if data_identity.matches_path(ancestor).map_err(|_| stale())? {
                return Err(directory_error(
                    "NODE_DIRECTORY_UNSAFE",
                    "所选目录位于平台受管数据中。",
                ));
            }
        }
        for ancestor in data.ancestors() {
            if source_identity
                .matches_path(ancestor)
                .map_err(|_| stale())?
            {
                return Err(directory_error(
                    "NODE_DIRECTORY_UNSAFE",
                    "所选目录包含平台受管数据。",
                ));
            }
        }
        Ok(source)
    }
    fn directory_preview(&self, source: &Path) -> Result<Preview> {
        let source = self.directory_path(source)?;
        let root = crate::files::root(&source)?;
        let identity = PersistedIdentity::from_file(&root).map_err(|_| stale())?;
        let names = entries(&root)?;
        let mut digest = Sha256::new();
        digest.update(b"local-directory-preview-v1\0");
        digest.update(serde_json::to_vec(&identity)?);
        digest.update(utf16_bytes(source.as_os_str()));
        for name in &names {
            let bytes = utf16_bytes(name);
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
            update_metadata(&mut digest, &source.join(name), true)?;
        }
        let mut state = if names.is_empty() {
            "empty"
        } else {
            "needs-review"
        };
        let mut inspection = None;
        let mut issues = Vec::new();
        let manifest_present = names.iter().any(|name| name == "panda.json");
        let manifest = if manifest_present {
            // A declaration is public metadata; other arbitrary file contents,
            // dependency manifests and local credentials are never read here.
            match public_manifest(&root) {
                Ok(bytes) => Some(bytes),
                Err(_) => {
                    issues.push("panda.json 无法安全读取：它可能为链接、非普通文件、过大或不可访问；未读取其目标或超限内容，请让 AI 只读检查。".into());
                    None
                }
            }
        } else {
            None
        };
        if let Some(bytes) = &manifest {
            digest.update(bytes);
            let inspected = match PandaManifest::parse_runtime(bytes) {
                Ok(parsed) => check_declared_paths(&source, &parsed).and_then(|()| {
                    parsed
                        .inspect_local(&source)
                        .map_err(|error| fail(error.to_string()))
                        .and_then(|value| serde_json::to_value(value).map_err(ApiError::from))
                }),
                Err(error) => Err(fail(error.to_string())),
            };
            match inspected {
                Ok(value) => {
                    for file in value["checkedFiles"].as_array().into_iter().flatten() {
                        if let Some(path) = file["path"].as_str() {
                            digest.update(path.as_bytes());
                            update_metadata(
                                &mut digest,
                                &source.join(path).canonicalize()?,
                                false,
                            )?;
                        }
                    }
                    state = "ready";
                    inspection = Some(value);
                }
                Err(error) => issues.push(error.message),
            }
            if public_manifest(&root).map_err(|_| stale())? != *bytes {
                return Err(stale());
            }
        } else if !names.is_empty() && !manifest_present {
            issues
                .push("目录非空，且未找到受支持的 panda.json；请让 AI 先只读检查现有工程。".into());
        }
        if entries(&root)? != names || !identity.matches_path(&source).map_err(|_| stale())? {
            return Err(stale());
        }
        let manifest_digest = manifest
            .as_ref()
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        Ok(Preview {
            source,
            identity,
            revision: format!("directory_{:x}", digest.finalize()),
            state,
            inspection,
            issues,
            manifest_digest,
        })
    }
    pub fn inspect_directory(&self, source: &Path) -> Result<Value> {
        let requested_source = source;
        let source = self.directory_path(source)?;
        let identity = PersistedIdentity::capture(&source).map_err(|_| stale())?;
        // An existing folder remains the same application even when its
        // declaration was changed or a large dependency tree was added later.
        if let Some(id) = self.runtime.store().registered_source(&source, &identity)? {
            let name = PandaManifest::load(&source)
                .map(|manifest| manifest.app.name)
                .unwrap_or_else(|_| id.clone());
            return Ok(
                json!({"sourcePath":source,"state":"needs-review","expectedRevision":format!("registered:{id}"),"issues":[],"reviewPrompt":"这个文件夹已经添加，请打开已有应用。","category":"insufficient","existingApp":{"id":id,"name":name}}),
            );
        }
        let preview = self.directory_preview(&source)?;
        let mut response = preview.response(requested_source, &self.prompt_paths());
        if preview.state == "needs-review" {
            let has = |name: &str| {
                fs::symlink_metadata(preview.source.join(name)).is_ok_and(|m| m.is_file())
            };
            if !has("package.json")
                && !has("panda.json")
                && ((has("pyproject.toml") || has("requirements.txt"))
                    && (has("main.py") || has("app.py"))
                    || has("pom.xml")
                    || has("build.gradle"))
            {
                response["category"] = json!("unsupported");
                response["reasonCodes"] = json!(["EXTERNAL_RUNTIME"]);
                response["issues"] = json!(["检测到 Python／Java 工程入口，当前平台不提供这些运行环境；请让 AI 评估是否能保留主要功能并调整。"]);
            } else {
                response["category"] = json!("needs-changes");
                response["reasonCodes"] = if has("panda.json") {
                    json!(["APPLICATION_MATERIALS"])
                } else {
                    json!(["MISSING_DECLARATION"])
                };
            }
        }
        Ok(response)
    }
    pub fn initialize_directory(
        &self,
        source: &Path,
        input: DirectoryInitializeInput,
    ) -> Result<Value> {
        let _scope = self.runtime.source_registration_scope()?;
        let _guard = self
            .directory_changes
            .lock()
            .map_err(|_| fail("目录初始化锁不可用。"))?;
        let generated = self.generate_directory_with_hook(source, input, |_| Ok(()))?;
        Ok(generated.ready.response(source, &self.prompt_paths()))
    }
    fn generate_directory_with_hook(
        &self,
        source: &Path,
        input: DirectoryInitializeInput,
        mut before_create: impl FnMut(&Path) -> Result<()>,
    ) -> Result<GeneratedDirectory> {
        let preview = self.directory_preview(source)?;
        if input.expected_revision.is_empty() || input.expected_revision != preview.revision {
            return Err(stale());
        }
        if preview.state != "empty" {
            return Err(directory_error(
                "NODE_DIRECTORY_NOT_EMPTY",
                "目录已包含文件，请重新检查；初始化不会覆盖任何现有内容。",
            )
            .outcome("not_started"));
        }
        let name = input.name.trim();
        if name.is_empty() || name.len() > 240 || name.contains('\0') {
            return Err(directory_error(
                "NODE_DIRECTORY_NAME_INVALID",
                "应用名称须为非空文本，最多240个UTF-8字节。",
            ));
        }
        let root = crate::files::root(&preview.source)?;
        let actual = PersistedIdentity::from_file(&root).map_err(|_| stale())?;
        if !preview.identity.matches_captured(&actual) || !entries(&root)?.is_empty() {
            return Err(stale());
        }
        crate::app_source_ownership::require_new_web_project_location(
            &self.data_root,
            &preview.source,
        )?;
        let instance_id = Uuid::new_v4().to_string();
        let (directories, files) = self.starter_files(name)?;
        let mut created_paths = Vec::<PathBuf>::new();
        let mut created = Vec::<Created>::new();
        let result = (|| -> Result<()> {
            for directory in &directories {
                before_create(Path::new(directory))?;
                verify_initialization_root(&preview, &root, &created)?;
                let relative = Path::new(directory);
                let parent = relative_parent(&root, relative)?;
                verify_parent(&preview, &parent, relative, &created)?;
                create_child_directory(&parent, relative.file_name().ok_or_else(stale)?)?;
                created_paths.push(relative.into());
                let opened = child_directory(
                    &parent,
                    relative
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(stale)?,
                )
                .map_err(|error| error.detail("generatedEntryUnconfirmed", true))?;
                created.push(Created {
                    path: relative.into(),
                    directory: true,
                    identity: PersistedIdentity::from_file(&opened)
                        .map_err(|_| stale().detail("generatedEntryUnconfirmed", true))?,
                    digest: None,
                    length: 0,
                });
                native::sync_directory(&parent)?;
            }
            for (relative, contents) in &files {
                before_create(Path::new(relative))?;
                verify_initialization_root(&preview, &root, &created)?;
                let relative = Path::new(relative);
                let parent = relative_parent(&root, relative)?;
                verify_parent(&preview, &parent, relative, &created)?;
                let mut output =
                    create_child_file(&parent, relative.file_name().ok_or_else(stale)?)?;
                created_paths.push(relative.into());
                created.push(Created {
                    path: relative.into(),
                    directory: false,
                    identity: PersistedIdentity::from_file(&output)
                        .map_err(|_| stale().detail("generatedEntryUnconfirmed", true))?,
                    digest: Some(Sha256::digest([]).to_vec()),
                    length: 0,
                });
                let entry = created.last_mut().ok_or_else(stale)?;
                while entry.length < contents.len() {
                    let written = output.write(&contents[entry.length..])?;
                    if written == 0 {
                        return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
                    }
                    entry.length += written;
                    entry.digest = Some(Sha256::digest(&contents[..entry.length]).to_vec());
                }
                output.sync_all()?;
                native::sync_directory(&parent)?;
            }
            verify_initialization_root(&preview, &root, &created)?;
            Ok(())
        })();
        if let Err(error) = result {
            return Err(self.initialization_failed(
                error,
                &instance_id,
                &preview,
                &root,
                &created,
                &created_paths,
            ));
        }
        let ready = self
            .directory_preview(&preview.source)
            .and_then(|ready| {
                if ready.state == "ready" {
                    // Inspection must not turn a concurrent valid edit into
                    // content accepted as part of the generated starter.
                    verify_initialization_root(&preview, &root, &created)?;
                    Ok(ready)
                } else {
                    Err(directory_error(
                        "NODE_DIRECTORY_CHANGED",
                        "起始文件已生成，但目录随后变化；请保留文件并重新检查。",
                    ))
                }
            })
            .map_err(|error| {
                self.initialization_failed(
                    error,
                    &instance_id,
                    &preview,
                    &root,
                    &created,
                    &created_paths,
                )
            })?;
        Ok(GeneratedDirectory { _root: root, ready })
    }
    fn initialization_failed(
        &self,
        error: ApiError,
        id: &str,
        preview: &Preview,
        root: &File,
        created: &[Created],
        paths: &[PathBuf],
    ) -> ApiError {
        let rollback = rollback_generated_entries(preview, root, created);
        let unconfirmed = error.details["generatedEntryUnconfirmed"] == true;
        let cleanup: Result<()> = Ok(());
        let retained = rollback.is_err() || unconfirmed;
        error
            .outcome(if retained {
                "initialization_incomplete"
            } else {
                "not_started"
            })
            .detail("createdPaths", json!(paths))
            .detail("partialFilesRetained", retained)
            .detail("sourcePath", json!(preview.source))
            .detail("appId", id)
            .detail("rootIdentity", json!(preview.identity))
            .detail(
                "cleanupError",
                rollback
                    .err()
                    .or_else(|| cleanup.err())
                    .map(|error| json!(error)),
            )
    }

    pub async fn register_directory(
        &self,
        source: PathBuf,
        expected_revision: String,
    ) -> Result<Value> {
        let _scope = self.runtime.source_registration_scope()?;
        let preview = self.directory_preview(&source)?;
        if preview.state != "ready"
            || preview.revision != expected_revision
            || expected_revision.is_empty()
        {
            return Err(stale());
        }
        if self.directory_preview(&source)?.revision != preview.revision {
            return Err(stale());
        }
        let manifest_digest = preview.manifest_digest.ok_or_else(stale)?;
        let project = self
            .runtime
            .store()
            .register_local_checked(&preview.source, &preview.identity, &manifest_digest)
            .await?;
        Ok(serde_json::to_value(project)?)
    }
}

fn relative_parent(root: &File, relative: &Path) -> Result<File> {
    let mut parent = root.try_clone()?;
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        let name = match component {
            Component::Normal(name) => name.to_str().ok_or_else(stale)?,
            _ => return Err(stale()),
        };
        parent = child_directory(&parent, name)?;
    }
    Ok(parent)
}
fn verify_parent(
    preview: &Preview,
    parent: &File,
    relative: &Path,
    created: &[Created],
) -> Result<()> {
    let path = relative.parent().unwrap_or(Path::new(""));
    let expected = if path.as_os_str().is_empty() {
        &preview.identity
    } else {
        &created
            .iter()
            .find(|entry| entry.directory && entry.path == path)
            .ok_or_else(stale)?
            .identity
    };
    let observed = PersistedIdentity::from_file(parent).map_err(|_| stale())?;
    if !expected.matches_captured(&observed) {
        return Err(stale());
    }
    Ok(())
}

fn check_declared_paths(source: &Path, manifest: &PandaManifest) -> Result<()> {
    let source = source.canonicalize()?;
    let inspection = manifest
        .inspect_local(&source)
        .map_err(|error| fail(error.to_string()))?;
    let paths: Vec<_> = inspection["checkedFiles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["path"].as_str())
        .map(|path| source.join(path))
        .collect();
    for path in paths {
        let resolved = path.canonicalize()?;
        let metadata = fs::symlink_metadata(&resolved)?;
        if !resolved.starts_with(&source)
            || platform_native::filesystem::is_reparse(&metadata)
            || (metadata.is_file() && removal::inspect(&resolved)?.identity.links != 1)
        {
            return Err(directory_error(
                "NODE_DIRECTORY_UNSAFE",
                "应用声明的实际路径须位于项目内，且文件不能共享硬链接。",
            ));
        }
    }
    Ok(())
}

fn open_created(parent: &File, entry: &Created) -> Result<File> {
    let (path, _scope) = child_path(parent, entry.path.file_name().ok_or_else(stale)?)?;
    let file = if entry.directory {
        let opened = native::open_no_reparse(&path)?;
        if !opened.identity.directory {
            return Err(stale());
        }
        opened.file
    } else {
        native::open_read_file(&path)?
    };
    if !entry.identity.matches_open_runtime(&file)? {
        return Err(stale());
    }
    Ok(file)
}
fn verify_created_bytes(file: &File, entry: &Created) -> Result<()> {
    let mut bytes = Vec::new();
    file.take(entry.length as u64 + 1).read_to_end(&mut bytes)?;
    if Some(Sha256::digest(&bytes).to_vec()) != entry.digest {
        return Err(stale());
    }
    Ok(())
}
fn require_absent(path: &Path) -> Result<()> {
    for delay in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if delay != 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if !matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                return Err(error.into())
            }
            _ => (),
        }
    }
    Err(stale().detail("remainingPath", json!(path)))
}

/// Delete just the original created name after checking its full identity and
/// written prefix. Foreign replacements, edits and children remain untouched.
fn rollback_generated_entries(preview: &Preview, root: &File, created: &[Created]) -> Result<()> {
    if !preview
        .identity
        .matches_path(&preview.source)
        .map_err(|_| stale())?
    {
        return Err(stale());
    }
    let mut incomplete = false;
    for entry in created.iter().rev() {
        let result = (|| -> Result<()> {
            let parent = relative_parent(root, &entry.path)?;
            verify_parent(preview, &parent, &entry.path, created)?;
            let file = open_created(&parent, entry)?;
            if entry.directory {
                if !entries(&file)?.is_empty() {
                    return Err(stale());
                }
            } else {
                verify_created_bytes(&file, entry)?;
            }
            let identity = native::identity(&file)?;
            let (path, _scope) = child_path(&parent, entry.path.file_name().ok_or_else(stale)?)?;
            drop(file);
            removal::remove(&path, &identity)?;
            require_absent(&path)?;
            native::sync_directory(&parent)?;
            Ok(())
        })();
        incomplete |= result.is_err();
    }
    if incomplete {
        Err(stale())
    } else {
        Ok(())
    }
}

fn verify_initialization_root(preview: &Preview, root: &File, created: &[Created]) -> Result<()> {
    if !preview
        .identity
        .matches_path(&preview.source)
        .map_err(|_| stale())?
    {
        return Err(stale());
    }
    let expected = created
        .iter()
        .filter_map(|entry| entry.path.components().next())
        .map(|part| part.as_os_str().to_os_string())
        .collect::<std::collections::BTreeSet<_>>();
    if entries(root)?
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        != expected
    {
        return Err(stale());
    }
    for entry in created {
        let parent = relative_parent(root, &entry.path)?;
        verify_parent(preview, &parent, &entry.path, created)?;
        let file = open_created(&parent, entry)?;
        if entry.directory {
            let allowed = created
                .iter()
                .filter(|child| child.path.parent() == Some(entry.path.as_path()))
                .filter_map(|child| child.path.file_name())
                .map(OsStr::to_os_string)
                .collect::<std::collections::BTreeSet<_>>();
            if entries(&file)?
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != allowed
            {
                return Err(stale());
            }
        } else {
            verify_created_bytes(&file, entry)?;
        }
    }
    Ok(())
}
