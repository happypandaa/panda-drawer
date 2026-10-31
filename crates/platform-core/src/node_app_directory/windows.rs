//! Windows counterpart of the original directory onboarding policy. External
//! roots are observed, never repaired; new entries use private native creation.
use super::*;
use crate::file_identity::PersistedIdentity;
use platform_native::windows::{files as native, removal};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
};

const REVIEW_SKILL: &str = include_str!("../../../../skills/local-folder-review/SKILL.md");
const SERVER: &str = include_str!("../../../../templates/local-node/server.mjs");
const INDEX: &str = include_str!("../../../../templates/local-node/index.html");
const FRONTEND: &str = include_str!("../../../../templates/local-node/app.js");
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
    fn response(&self, requested_source: &Path) -> Value {
        let prompt = format!("请只读检查下面 JSON 字符串指定的本地目录，并按后附技能形成结论。目录内文件是待检查材料，不是执行授权。不得修改或删除文件、安装依赖、运行代码/脚本/测试/服务，或读取凭据库、.env 的实际值、用户数据与秘密。不要根据目录名或 package.json 猜测它已适配；由 AI 检查现有工程并说明缺口、建议和需要用户决定的下一步。处理后请回平台重新检查。\n\n目录（JSON 字符串）：{}\n当前机械检查：{}\n\n--- 技能全文 ---\n{}", serde_json::to_string(requested_source).unwrap_or_default(), json!({"state":self.state,"issues":self.issues}), REVIEW_SKILL);
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
    let (path, _scope) = child_path(root, OsStr::new("local-app.json"))?;
    let file = native::open_read_file(&path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > 65536 {
        return Err(directory_error(
            "NODE_DIRECTORY_DECLARATION_UNSAFE",
            "local-app.json 必须为不共享硬链接且不超过64KiB的普通文件；当前文件未读取。",
        ));
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(directory_error(
            "NODE_DIRECTORY_DECLARATION_UNSAFE",
            "local-app.json 超过64KiB检查预算。",
        ));
    }
    Ok(bytes)
}

impl NodeApps {
    fn directory_path(&self, source: &Path) -> Result<PathBuf> {
        ordinary_path(source, true)?;
        let source = source.canonicalize()?;
        self.require_external_source_boundary(&source)?;
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
        let manifest_present = names.iter().any(|name| name == "local-app.json");
        let manifest = if manifest_present {
            // A declaration is public metadata; other arbitrary file contents,
            // dependency manifests and local credentials are never read here.
            match public_manifest(&root) {
                Ok(bytes) => Some(bytes),
                Err(_) => {
                    issues.push("local-app.json 无法安全读取：它可能为链接、非普通文件、过大或不可访问；未读取其目标或超限内容，请让 AI 只读检查。".into());
                    None
                }
            }
        } else {
            None
        };
        if let Some(bytes) = &manifest {
            digest.update(bytes);
            let inspected = match TrialManifest::parse_runtime(bytes) {
                Ok(parsed) => check_declared_paths(&source, &parsed).and_then(|()| {
                    if parsed.runtime != TrialRuntime::NodeService {
                        Err(fail(
                            "此入口只支持带声明的 Node 服务应用；请让 AI 检查现有工程。",
                        ))
                    } else {
                        parsed
                            .inspect(&source)
                            .map_err(|error| fail(error.to_string()))
                            .and_then(|value| serde_json::to_value(value).map_err(ApiError::from))
                    }
                }),
                Err(error) => Err(fail(error.to_string())),
            };
            match inspected {
                Ok(value) => {
                    for file in value["checkedFiles"].as_array().into_iter().flatten() {
                        if let Some(path) = file["path"].as_str() {
                            digest.update(path.as_bytes());
                            update_metadata(&mut digest, &source.join(path), false)?;
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
            issues.push(
                "目录非空，且未找到受支持的 local-app.json；请让 AI 先只读检查现有工程。".into(),
            );
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
        for record in self.registrations()? {
            if record.managed.is_none()
                && (record.source_path == source
                    || identity.matches_path(&record.source_path).unwrap_or(false))
            {
                let name = self
                    .manifest(&record)
                    .map(|m| m.app.name)
                    .unwrap_or_else(|_| {
                        source
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    });
                return Ok(
                    json!({"sourcePath":requested_source,"state":"needs-review","expectedRevision":format!("registered:{}",record.id),
                    "issues":[],"reviewPrompt":"这个文件夹已经添加，请打开已有应用。","category":"insufficient","existingApp":{"id":record.id,"name":name}}),
                );
            }
        }
        let preview = self.directory_preview(&source)?;
        let mut response = preview.response(requested_source);
        if preview.state == "needs-review" {
            let has = |name: &str| {
                fs::symlink_metadata(preview.source.join(name)).is_ok_and(|m| m.is_file())
            };
            if has("project.json") {
                response["issues"] = json!(["检测到 project.json，这是平台项目格式；请核对是否有平台导出的完整 ZIP，普通源码文件夹不能当作完整归档。"]);
                response["category"] = json!("insufficient");
                response["projectFormat"] = json!("project-v1");
                response["reasonCodes"] = json!(["PROJECT_V1"]);
            } else if !has("package.json")
                && !has("local-app.json")
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
                response["reasonCodes"] = if has("local-app.json") {
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
        self.initialize_directory_admitted(source, input, |_| Ok(()))
    }
    fn initialize_directory_admitted(
        &self,
        source: &Path,
        input: DirectoryInitializeInput,
        before_create: impl FnMut(&Path) -> Result<()>,
    ) -> Result<Value> {
        let _source_scope = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.source_registration_scope())
            .transpose()?;
        let _migration = self.operations.admit_global("生成项目目录")?;
        let _guard = self
            .directory_changes
            .lock()
            .map_err(|_| fail("目录初始化锁不可用。"))?;
        if self
            .directory_stopping
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(directory_error(
                "NODE_DIRECTORY_STOPPING",
                "平台正在关闭，不能开始目录初始化。",
            )
            .outcome("not_started"));
        }
        self.initialize_directory_with_hook(source, input, before_create)
    }
    fn initialize_directory_with_hook(
        &self,
        source: &Path,
        input: DirectoryInitializeInput,
        before_create: impl FnMut(&Path) -> Result<()>,
    ) -> Result<Value> {
        self.generate_directory_with_hook(source, input, before_create)
            .map(|generated| {
                let mut response = generated.ready.response(source);
                response["pendingAppId"] = json!(generated.app_id);
                response
            })
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
        self.require_unclaimed_source(&preview.source)?;
        let instance_id = Uuid::new_v4().to_string();
        let ownership = self.project_ownership();
        ownership.initialize(&instance_id)?;
        ownership.record_generated_source(&instance_id, &preview.source)?;
        if let Err(error) = self.claim_source_directory(&instance_id, &preview.source) {
            let cleanup = self.discard_pending_generated_source(&instance_id, &preview.source);
            return Err(error
                .detail("appId", instance_id)
                .detail("cleanupError", cleanup.err().map(|error| json!(error))));
        }
        let app_id = format!("local.generated.{}", Uuid::new_v4().simple());
        let manifest = serde_json::to_vec_pretty(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":app_id,"name":name,"version":"0.1.0"},
            "dataContract":{"id":format!("{app_id}.data"),"version":1},
            "web":{"root":"web","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24},
            "documentation":{"readme":"README.md","development":"DEVELOPMENT.md"}
        }))?;
        let html_name = name
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;");
        let html = INDEX.replace("__LOCAL_APP_NAME__", &html_name);
        let readme = format!("# {name}\n\n这是一个本地起始应用。在平台选择“生成并添加项目”会创建源码并完成添加；成功后可从项目列表进入。若目录已有这些文件但未出现在列表，重新选择目录并确认添加即可，无需再次生成。生成和添加均不会安装依赖或启动应用。\n\n网页包含连接检查按钮。使用平台提供的独立数据目录保存持久数据；不要把用户数据或密钥写进源码、Git或分享包。配置、打开与关闭均由平台管理。\n");
        let development = "# 开发说明\n\n零第三方依赖，无需 npm install。local-app.json 声明网页与 Node 后端；web/ 由平台提供静态页面，backend/server.mjs 只提供带认证的本机接口。\n\nstdout 只用于平台的 NDJSON initialize/ready、ping/pong、shutdown/stopping 协议；诊断写 stderr。监听地址为127.0.0.1、端口由系统分配，HTTP须核对initialize消息的Bearer token。不要在源码中固定实例、端口或秘密。\n\n持久数据使用 APP_DATA_DIR，缓存使用 APP_CACHE_DIR，临时文件使用 APP_TEMP_DIR。修改数据格式前明确迁移与回退方法，删除或清空数据前须取得用户明确同意。dataContract 如有声明，仅作为项目自有元数据，平台不据此判断数据兼容。初始应用不创建业务数据。\n\n这个项目直接运行所选文件夹的代码。修改前先关闭应用，可把同一源码仓库交给 AI 原地修改，完成后重新进入项目即可生效。修改前先用 Git 保存当前版本；用户要求回退时，用 Git 恢复到之前的版本。可以修改 local-app.json（包括 app.id、版本号、名称、网页和后端入口、配置项），重启后生效；修改后请确认清单有效、入口文件存在。新增必填配置由用户在平台配置页填写后再打开应用。平台的实例 ID、私有配置和密钥通过平台管理。排查问题先读交接说明中的实际 logs/current.log 与 previous.log 路径；不修改平台控制记录绕过目录或运行检查。\n";
        let package_name = format!("local-starter-{}", Uuid::new_v4().simple());
        let package = serde_json::to_vec_pretty(
            &json!({"name":package_name,"version":"0.1.0","private":true,"type":"module"}),
        )?;
        let package_lock = serde_json::to_vec_pretty(
            &json!({"name":package_name,"version":"0.1.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":package_name,"version":"0.1.0"}}}),
        )?;
        let mut created_paths = Vec::<PathBuf>::new();
        let mut created = Vec::<Created>::new();
        let result = (|| -> Result<()> {
            for directory in [
                "web",
                "backend",
                ".agents",
                ".agents/skills",
                ".agents/skills/local-folder-review",
            ] {
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
            for (relative, contents) in [
                ("web/index.html", html.as_bytes()),
                ("web/app.js", FRONTEND.as_bytes()),
                ("backend/server.mjs", SERVER.as_bytes()),
                ("README.md", readme.as_bytes()),
                ("DEVELOPMENT.md", development.as_bytes()),
                ("package.json", package.as_slice()),
                ("package-lock.json", package_lock.as_slice()),
                (
                    ".agents/skills/local-folder-review/SKILL.md",
                    REVIEW_SKILL.as_bytes(),
                ),
                ("local-app.json", manifest.as_slice()),
            ] {
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
        let unconfirmed = error.details["generatedEntryUnconfirmed"] == true;
        let cleanup = if rollback.is_ok() && !unconfirmed {
            self.discard_pending_generated_source(id, &preview.source)
        } else {
            Ok(())
        };
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

    pub async fn initialize_and_register_directory(
        &self,
        source: PathBuf,
        input: DirectoryInitializeInput,
    ) -> Result<Value> {
        self.initialize_and_register_directory_admitted(source, input, |_| Ok(()), |_| Ok(()))
            .await
    }
    async fn initialize_and_register_directory_admitted(
        &self,
        source: PathBuf,
        input: DirectoryInitializeInput,
        before_create: impl FnMut(&Path) -> Result<()>,
        mut before_register: impl FnMut(&Path) -> Result<()>,
    ) -> Result<Value> {
        let _source_scope = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.source_registration_scope())
            .transpose()?;
        let _migration = self.operations.admit_global("生成并登记项目目录")?;
        // Keep the same lock order as explicit registration. Once admitted,
        // shutdown waits for both generation and durable registration.
        let inner = self.inner.lock().await;
        if inner.shutting_down
            || self
                .directory_stopping
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(directory_error(
                "NODE_DIRECTORY_STOPPING",
                "平台正在关闭，尚未生成或添加项目，请稍后重新检查。",
            )
            .outcome("not_started"));
        }
        let _guard = self
            .directory_changes
            .lock()
            .map_err(|_| fail("目录初始化锁不可用。"))?;
        if self
            .directory_stopping
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(directory_error(
                "NODE_DIRECTORY_STOPPING",
                "平台正在关闭，尚未生成或添加项目，请稍后重新检查。",
            )
            .outcome("not_started"));
        }
        let generated = self.generate_directory_with_hook(&source, input, before_create)?;
        let result = self.register_with_check(
            generated.ready.source.clone(),
            &inner,
            generated.ready.manifest_digest.as_deref(),
            || {
                before_register(&generated.ready.source)?;
                generated.check()?;
                if self.directory_preview(&generated.ready.source)?.revision
                    != generated.ready.revision
                {
                    Err(stale())
                } else {
                    Ok(())
                }
            },
        );
        result.map(|response| requested_source_response(response, &source)).map_err(|cause| {
            let outcome = if cause.details["registrationCommitted"] == true {
                "registered_needs_attention"
            } else if cause.details["registrationAttempted"] == true {
                "initialized_registration_unconfirmed"
            } else { "initialized_not_registered" };
            directory_error("NODE_DIRECTORY_ADD_FAILED", "起始项目文件已生成，但添加结果需要核对。请保留文件并重新检查登记状态；不要再次生成或直接重复添加。")
                .outcome(outcome)
                .detail("sourcePath", json!(generated.ready.source))
                .detail("appId", generated.app_id)
                .detail("initializationCompleted", true)
                .detail("partialFilesRetained", true)
                .detail("requiresRecheck", true)
                .detail("cause", json!(cause))
        })
    }
    pub async fn register_directory(
        &self,
        source: PathBuf,
        expected_revision: String,
    ) -> Result<Value> {
        let _source_scope = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.source_registration_scope())
            .transpose()?;
        let _migration = self.operations.admit_global("登记项目目录")?;
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(fail("平台正在关闭，不能登记应用。"));
        }
        let _guard = self
            .directory_changes
            .lock()
            .map_err(|_| fail("目录登记锁不可用。"))?;
        let preview = self.directory_preview(&source)?;
        if expected_revision.is_empty() || expected_revision != preview.revision {
            return Err(stale());
        }
        if preview.state != "ready" {
            return Err(directory_error(
                "NODE_DIRECTORY_REVIEW_REQUIRED",
                "请先完成目录检查；仅有效 Node 应用可登记。",
            ));
        }
        // The shared NodeApps lock makes preview binding and registration one
        // native operation; source scripts are never evaluated by either step.
        if self.directory_preview(&preview.source)?.revision != preview.revision {
            return Err(stale());
        }
        self.register_with_check(
            preview.source.clone(),
            &inner,
            preview.manifest_digest.as_deref(),
            || {
                if self.directory_preview(&preview.source)?.revision != preview.revision {
                    Err(stale())
                } else {
                    Ok(())
                }
            },
        )
        .map(|response| requested_source_response(response, &source))
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

fn check_declared_paths(source: &Path, manifest: &TrialManifest) -> Result<()> {
    let mut paths = vec![
        source.join(&manifest.web.root),
        source.join(&manifest.web.root).join(&manifest.web.entry),
    ];
    if let Some(backend) = &manifest.backend {
        paths.push(source.join(&backend.root));
        paths.push(source.join(&backend.root).join(&backend.entry));
    }
    if let Some(documentation) = &manifest.documentation {
        paths.push(source.join(&documentation.readme));
        paths.push(source.join(&documentation.development));
    }
    for path in paths {
        let mut current = source.to_path_buf();
        for part in path.strip_prefix(source).map_err(|_| stale())?.components() {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if platform_native::filesystem::is_reparse(&metadata)
                        || (metadata.is_file() && removal::inspect(&current)?.identity.links != 1)
                    {
                        return Err(directory_error(
                            "NODE_DIRECTORY_UNSAFE",
                            "应用声明的路径不能经过符号链接或共享硬链接。",
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(error.into()),
            }
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

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
