//! Explicit local-directory onboarding. Inspection never mutates a directory;
//! initialization writes only into a confirmed empty, descriptor-bound root.
use super::*;
use crate::file_identity::PersistedIdentity;
use std::{
    ffi::{CStr, CString, OsString},
    fs::File,
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
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
    app_id: String,
    original: Preview,
    root: File,
    created: Vec<Created>,
    ready: Preview,
}
impl GeneratedDirectory {
    fn check(&self) -> Result<()> {
        verify_initialization_root(&self.original, &self.root, &self.created)
    }
}
impl Preview {
    fn response(&self, paths: &super::PromptPaths) -> Value {
        let prompt = super::directory_review_prompt(&self.source, self.state, &self.issues, paths);
        let mut response = json!({"sourcePath":self.source,"state":self.state,"expectedRevision":self.revision,"issues":self.issues,"reviewPrompt":prompt});
        if let Some(inspection) = &self.inspection {
            response["inspection"] = inspection.clone();
        }
        response
    }
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
fn c_name(value: &std::ffi::OsStr) -> Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| directory_error("NODE_DIRECTORY_UNSAFE", "目录条目名称无效。"))
}
fn opened(fd: i32) -> Result<File> {
    if fd < 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn child_directory(parent: &File, name: &str) -> Result<File> {
    let name = c_name(std::ffi::OsStr::new(name))?;
    opened(unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })
}
fn entries(root: &File) -> Result<Vec<OsString>> {
    struct Listing(*mut libc::DIR);
    impl Drop for Listing {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let fd = unsafe { libc::fcntl(root.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let listing = unsafe { libc::fdopendir(fd) };
    if listing.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(error.into());
    }
    let listing = Listing(listing);
    // dup shares the directory offset; always rewind before reading a snapshot.
    unsafe {
        libc::rewinddir(listing.0);
    }
    let mut names = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(listing.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                return Err(error.into());
            }
            break;
        }
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsString::from_vec(bytes.to_vec()));
        }
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

// Read only the public declaration beneath the already-bound directory. A
// symlink, FIFO, shared hard link or oversized file is review material, never a
// reason to follow a target or wait for a writer.
fn public_manifest(root: &File) -> Result<Vec<u8>> {
    let file = opened(unsafe {
        libc::openat(
            root.as_raw_fd(),
            c"panda.json".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > 65536 {
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
        digest.update(source.as_os_str().as_bytes());
        for name in &names {
            let metadata = fs::symlink_metadata(source.join(name))?;
            digest.update((name.as_bytes().len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            digest.update(metadata.dev().to_le_bytes());
            digest.update(metadata.ino().to_le_bytes());
            digest.update(metadata.mode().to_le_bytes());
            digest.update(metadata.len().to_le_bytes());
            digest.update(metadata.mtime().to_le_bytes());
            digest.update(metadata.mtime_nsec().to_le_bytes());
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
                            let metadata = fs::metadata(source.join(path))?;
                            digest.update(path.as_bytes());
                            digest.update(metadata.dev().to_le_bytes());
                            digest.update(metadata.ino().to_le_bytes());
                            digest.update(metadata.len().to_le_bytes());
                            digest.update(metadata.mtime().to_le_bytes());
                            digest.update(metadata.mtime_nsec().to_le_bytes());
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
        let mut response = preview.response(&self.prompt_paths());
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
        Ok(generated.ready.response(&self.prompt_paths()))
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
                let c = c_name(relative.file_name().ok_or_else(stale)?)?;
                if unsafe { libc::mkdirat(parent.as_raw_fd(), c.as_ptr(), 0o700) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                created_paths.push(relative.into());
                let opened = child_directory(
                    &parent,
                    relative
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(stale)?,
                )?;
                created.push(Created {
                    path: relative.into(),
                    directory: true,
                    identity: PersistedIdentity::from_file(&opened).map_err(|_| stale())?,
                    digest: None,
                    length: 0,
                });
                parent.sync_all()?;
            }
            for (relative, contents) in &files {
                before_create(Path::new(relative))?;
                verify_initialization_root(&preview, &root, &created)?;
                let relative = Path::new(relative);
                let parent = relative_parent(&root, relative)?;
                verify_parent(&preview, &parent, relative, &created)?;
                let file = c_name(relative.file_name().ok_or_else(stale)?)?;
                let mut output = opened(unsafe {
                    libc::openat(
                        parent.as_raw_fd(),
                        file.as_ptr(),
                        libc::O_WRONLY
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC,
                        0o600,
                    )
                })?;
                created_paths.push(relative.into());
                created.push(Created {
                    path: relative.into(),
                    directory: false,
                    identity: PersistedIdentity::from_file(&output).map_err(|_| stale())?,
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
                parent.sync_all()?;
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
        Ok(GeneratedDirectory {
            app_id: instance_id,
            original: preview,
            root,
            created,
            ready,
        })
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
        let cleanup: Result<()> = Ok(());
        let retained = rollback.is_err();
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
            || metadata.file_type().is_symlink()
            || (metadata.is_file() && metadata.nlink() != 1)
        {
            return Err(directory_error(
                "NODE_DIRECTORY_UNSAFE",
                "应用声明的实际路径须位于项目内，且文件不能共享硬链接。",
            ));
        }
    }
    Ok(())
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
/// Undo only entries whose captured identity and bytes still match this write.
/// Concurrent author edits and replacement directories are never removed.
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
            let name = c_name(entry.path.file_name().ok_or_else(stale)?)?;
            let file = opened(unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY
                        | libc::O_NOFOLLOW
                        | libc::O_NONBLOCK
                        | libc::O_CLOEXEC
                        | if entry.directory {
                            libc::O_DIRECTORY
                        } else {
                            0
                        },
                )
            })?;
            if !entry
                .identity
                .matches_captured(&PersistedIdentity::from_file(&file).map_err(|_| stale())?)
            {
                return Err(stale());
            }
            let metadata = file.metadata()?;
            if entry.directory {
                if !metadata.is_dir() || !entries(&file)?.is_empty() {
                    return Err(stale());
                }
            } else {
                if !metadata.is_file() || metadata.nlink() != 1 {
                    return Err(stale());
                }
                let mut bytes = Vec::new();
                file.take(entry.length as u64 + 1).read_to_end(&mut bytes)?;
                if Some(Sha256::digest(&bytes).to_vec()) != entry.digest {
                    return Err(stale());
                }
            }
            if unsafe {
                libc::unlinkat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    if entry.directory {
                        libc::AT_REMOVEDIR
                    } else {
                        0
                    },
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            parent.sync_all()?;
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
        let name = c_name(entry.path.file_name().ok_or_else(stale)?)?;
        let file = opened(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC
                    | if entry.directory {
                        libc::O_DIRECTORY
                    } else {
                        0
                    },
            )
        })?;
        let metadata = file.metadata()?;
        if if entry.directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file() || metadata.nlink() != 1
        } {
            return Err(stale());
        }
        let observed = PersistedIdentity::from_file(&file).map_err(|_| stale())?;
        if !entry.identity.matches_captured(&observed) {
            return Err(stale());
        }
        if entry.directory {
            let allowed = created
                .iter()
                .filter(|child| child.path.parent() == Some(entry.path.as_path()))
                .filter_map(|child| child.path.file_name())
                .map(std::ffi::OsStr::to_os_string)
                .collect::<std::collections::BTreeSet<_>>();
            if entries(&file)?
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                != allowed
            {
                return Err(stale());
            }
        } else {
            let mut bytes = Vec::new();
            file.take(entry.length as u64 + 1).read_to_end(&mut bytes)?;
            if Some(Sha256::digest(&bytes).to_vec()) != entry.digest {
                return Err(stale());
            }
        }
    }
    Ok(())
}
