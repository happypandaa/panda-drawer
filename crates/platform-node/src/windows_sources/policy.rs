//! Portable source transport/path policies, matched to the integration baseline.
use super::*;
use unicode_normalization::UnicodeNormalization;

pub(super) fn local_excluded(path: &str, output: Option<&str>) -> bool {
    let folded = path.to_ascii_lowercase();
    if output.is_some_and(|output| {
        let output = output.to_ascii_lowercase();
        folded == output || folded.starts_with(&format!("{output}/"))
    }) {
        return true;
    }
    folded.split('/').any(|name| {
        matches!(
            name,
            ".git"
                | ".hg"
                | ".svn"
                | ".platform"
                | "node_modules"
                | "data"
                | ".data"
                | "attachments"
                | "config"
                | "configuration"
                | "credentials"
                | "secrets"
                | "tokens"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".npmrc"
                | ".netrc"
                | ".git-credentials"
                | ".pypirc"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa"
                | "cache"
                | ".cache"
                | ".vite"
                | ".next"
                | ".nuxt"
                | "coverage"
                | "tmp"
                | ".tmp"
                | "temp"
                | "runs"
                | "backups"
                | "trash"
                | ".ds_store"
        ) || name.starts_with(".env")
            || [
                ".pem",
                ".key",
                ".p12",
                ".pfx",
                ".sqlite",
                ".sqlite3",
                ".db",
                ".sqlite-wal",
                ".sqlite-shm",
                ".sqlite3-wal",
                ".sqlite3-shm",
                ".db-wal",
                ".db-shm",
                "-wal",
                "-shm",
                "-journal",
            ]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    })
}
pub(super) fn validate_url(value: &str) -> Result<(String, String, String)> {
    let bad = || {
        error(
            "SOURCE_URL_INVALID",
            "请输入标准 GitHub 或 Gitee 的公开 HTTPS 仓库地址，不包含凭据、端口、查询或额外路径。",
        )
    };
    let (host, provider, path) = if let Some(path) = value.strip_prefix("https://github.com/") {
        ("github.com", "github", path)
    } else if let Some(path) = value.strip_prefix("https://gitee.com/") {
        ("gitee.com", "gitee", path)
    } else {
        return Err(bad());
    };
    if value.len() > 1024 {
        return Err(bad());
    }
    let mut parts = path.strip_suffix('/').unwrap_or(path).split('/');
    let owner = parts.next().ok_or_else(bad)?;
    let repo = parts.next().ok_or_else(bad)?;
    let safe = |s: &str| {
        (1..=255).contains(&s.len())
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    if parts.next().is_some() || !safe(owner) || !safe(repo) {
        return Err(bad());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe(repo) {
        return Err(bad());
    }
    Ok((
        format!("https://{host}/{owner}/{repo}.git"),
        provider.into(),
        repo.into(),
    ))
}
pub(super) fn parse_commit(bytes: &[u8]) -> Result<String> {
    let s = std::str::from_utf8(bytes).map_err(|_| invalid("Git 未返回有效提交。"))?;
    let oid = s
        .strip_suffix('\n')
        .ok_or_else(|| invalid("Git 未返回有效提交。"))?;
    if !valid_oid(oid) {
        return Err(invalid("Git 未返回有效提交。"));
    }
    Ok(oid.into())
}
pub(super) fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64)
        && oid
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && !oid.bytes().all(|b| b == b'0')
}
#[derive(Debug)]
pub(super) struct TreeEntry {
    pub path: String,
    pub oid: String,
    pub size: u64,
    pub executable: bool,
}
pub(super) fn parse_tree(bytes: &[u8]) -> Result<Vec<TreeEntry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.ends_with(&[0]) {
        return Err(invalid("Git 文件树不完整。"));
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut ancestors = BTreeMap::<String, String>::new();
    let mut total = 0u64;
    for record in bytes[..bytes.len() - 1].split(|b| *b == 0) {
        let record = std::str::from_utf8(record).map_err(|_| invalid("仅支持 UTF-8 文件路径。"))?;
        let (header, path) = record
            .split_once('\t')
            .ok_or_else(|| invalid("Git 文件树无效。"))?;
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4
            || !matches!(fields[0], "100644" | "100755")
            || fields[1] != "blob"
            || !valid_oid(fields[2])
        {
            return Err(invalid("本步不获取符号链接、子模块或非普通文件。"));
        }
        let size = fields[3]
            .parse::<u64>()
            .map_err(|_| invalid("Git 文件大小无效。"))?;
        if size > MAX_BLOB {
            return Err(limit());
        }
        total = total.checked_add(size).ok_or_else(limit)?;
        if total > MAX_TREE || result.len() >= MAX_FILES {
            return Err(limit());
        }
        validate_tree_path(path)?;
        let mut prefix = String::new();
        for component in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let normalized = prefix
                .nfd()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if let Some(previous) = ancestors.insert(normalized.clone(), prefix.clone()) {
                if previous != prefix {
                    return Err(invalid("仓库路径在本机存在大小写或 Unicode 冲突。"));
                }
            }
            if prefix != path && seen.contains(&normalized) {
                return Err(invalid("仓库文件与目录路径冲突。"));
            }
        }
        let normalized = path.nfd().flat_map(char::to_lowercase).collect::<String>();
        if !seen.insert(normalized.clone())
            || ancestors
                .range(format!("{normalized}/")..)
                .next()
                .is_some_and(|(p, _)| p.starts_with(&format!("{normalized}/")))
        {
            return Err(invalid("仓库文件路径重复或冲突。"));
        }
        result.push(TreeEntry {
            path: path.into(),
            oid: fields[2].into(),
            size,
            executable: fields[0] == "100755",
        });
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}
pub(super) fn validate_tree_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 1024
        || path.split('/').count() > 32
        || path.chars().any(|c| c.is_control() || c == '\\')
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(invalid("仓库包含不安全或超限路径。"));
    }
    for part in path.split('/') {
        let folded = part
            .nfd()
            .filter(|c| !matches!(c, '\u{200c}' | '\u{200d}' | '\u{feff}'))
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || folded == ".git"
            || folded.starts_with("git~")
            || part.contains(['<', '>', '"', '|', '?', '*'])
            || folded.contains(':')
            || windows_device_name(&folded)
        {
            return Err(invalid("仓库包含保留或不安全路径。"));
        }
    }
    Ok(())
}

fn windows_device_name(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or(value);
    matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || ["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

pub(super) fn inspect_directory(root: &WindowsSourceDirectory) -> Result<Value> {
    let mut result = json!({"kind":"needs-adaptation","name":null,"nextStep":"review-and-adapt-source","executionState":"not-executed","issues":[]});
    let entries = root.source_entries(MAX_ENTRIES).map_err(fs_error)?;
    let has = |name: &str| entries.iter().any(|entry| entry.name == name);
    let mut issues = Vec::new();
    if has("project.json") && has("local-app.json") {
        issues.push(invalid("目录同时包含两种项目声明，需明确接入类型。"));
    } else if has("project.json") {
        let inspected = (|| -> Result<String> {
            let manifest: platform_project_format::ProjectManifest = serde_json::from_value(
                optional_json(root, "project.json")?
                    .ok_or_else(|| invalid("project.json缺失。"))?,
            )?;
            if manifest.manifest_version != 1
                || manifest.sdk_api_version != 1
                || !valid_id(&manifest.id)
                || manifest.name.trim().is_empty()
            {
                return Err(invalid("项目声明版本、ID或名称无效。"));
            }
            for path in [
                &manifest.source_dir,
                &manifest.output_dir,
                &manifest.entry,
                &manifest.database,
                &manifest.attachments_dir,
            ] {
                validate_tree_path(path)?;
            }
            let overlaps = |left: &str, right: &str| {
                left == right
                    || left.starts_with(&format!("{right}/"))
                    || right.starts_with(&format!("{left}/"))
            };
            if overlaps(&manifest.source_dir, &manifest.output_dir)
                || ["data", "meta", ".platform"].iter().any(|fixed| {
                    overlaps(&manifest.source_dir, fixed) || overlaps(&manifest.output_dir, fixed)
                })
                || !manifest
                    .entry
                    .starts_with(&format!("{}/", manifest.output_dir))
                || !manifest.database.starts_with("data/")
                || !manifest.attachments_dir.starts_with("data/")
                || manifest.database == manifest.attachments_dir
                || manifest
                    .database
                    .starts_with(&format!("{}/", manifest.attachments_dir))
                || manifest.build["script"].as_str().is_none_or(str::is_empty)
            {
                return Err(invalid("项目的源码、产物、业务数据或构建声明不受支持。"));
            }
            for name in ["readme", "database", "platform"] {
                validate_tree_path(
                    manifest.docs[name]
                        .as_str()
                        .ok_or_else(|| invalid("项目文档声明不完整。"))?,
                )?;
            }
            platform_project_format::runtime_policy::RuntimePolicy::parse(
                &manifest.runtime_requirements,
            )?;
            let source = relative_directory(root, &manifest.source_dir)?;
            for name in ["package.json", "package-lock.json"] {
                optional_json(&source, name)?
                    .ok_or_else(|| invalid("项目缺少有效的npm构建材料。"))?;
            }
            Ok(manifest.name)
        })();
        match inspected {
            Ok(name) => {
                result["kind"] = json!("project-v1-source");
                result["name"] = json!(name);
                result["nextStep"] = json!("review-project-source-before-integration");
            }
            Err(issue) => issues.push(error("SOURCE_DECLARATION_INVALID", &issue.message)),
        }
    } else if has("local-app.json") {
        let inspected = (|| -> Result<crate::node_app_manifest::TrialManifest> {
            let bytes =
                optional_bytes(root, "local-app.json")?.ok_or_else(|| invalid("应用声明缺失。"))?;
            let manifest = crate::node_app_manifest::TrialManifest::parse(&bytes)
                .map_err(|_| invalid("应用声明格式无效。"))?;
            manifest
                .inspect(root.path())
                .map_err(|_| invalid("应用声明所需文件不完整。"))?;
            Ok(manifest)
        })();
        match inspected {
            Ok(manifest)
                if manifest.runtime == crate::node_app_manifest::TrialRuntime::NodeService =>
            {
                result["kind"] = json!("node-service-source");
                result["name"] = json!(manifest.app.name);
                result["nextStep"] = json!("prepare-node-runtime-before-registration");
            }
            _ => issues.push(invalid("应用声明或所声明文件不完整；本步不提供成品安装。")),
        }
    } else {
        for name in ["package.json", "package-lock.json"] {
            if has(name) && optional_json(root, name).is_err() {
                issues.push(invalid("npm声明文件无效或超限。"));
            }
        }
        issues.push(invalid(
            "尚无受支持的项目声明；源码获取不自动推断安装或启动方式。",
        ));
    }
    root.verify_binding().map_err(fs_error)?;
    result["issues"] = json!(issues);
    Ok(result)
}

pub(super) fn local_dependencies(snapshot: &Snapshot, control: &Control) -> Result<Vec<ApiError>> {
    fn target(base: &str, spec: &str, snapshot: &Snapshot, pattern: bool) -> Result<()> {
        if spec.is_empty()
            || spec.starts_with(['/', '~'])
            || spec.contains(['\\', '%', ':'])
            || spec.chars().any(char::is_control)
        {
            return Err(invalid(
                "本机依赖路径不受支持或越界；请将依赖材料放进所选源码目录。",
            ));
        }
        let mut path: Vec<_> = base.split('/').filter(|s| !s.is_empty()).collect();
        let mut wildcard = false;
        for part in spec.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if wildcard || path.pop().is_none() {
                        return Err(invalid("本机依赖指向所选源码以外；未接入。"));
                    }
                }
                part => {
                    if pattern && part.contains(['*', '?', '[', '{']) {
                        wildcard = true;
                    } else if !wildcard {
                        path.push(part);
                    }
                }
            }
        }
        let path = path.join("/");
        if !path.is_empty()
            && !snapshot
                .files
                .keys()
                .any(|file| file == &path || file.starts_with(&format!("{path}/")))
        {
            return Err(invalid("本机依赖材料不存在或已被保护规则排除；未接入。"));
        }
        Ok(())
    }
    fn spec(base: &str, value: &str, snapshot: &Snapshot, force: bool) -> Result<()> {
        let lower = value.to_ascii_lowercase();
        if lower.starts_with("file:") || lower.starts_with("link:") {
            target(base, &value[5..], snapshot, false)
        } else if force
            || value.starts_with(['.', '/'])
            || value.starts_with("~/")
            || value.contains('\\')
            || (value.len() > 1 && value.as_bytes()[1] == b':')
        {
            target(base, value, snapshot, false)
        } else {
            Ok(())
        }
    }
    fn dependencies(value: &Value, base: &str, snapshot: &Snapshot) -> Result<()> {
        if let Some(value) = value.as_str() {
            spec(base, value, snapshot, false)?;
        } else if let Some(values) = value.as_object() {
            for value in values.values() {
                dependencies(value, base, snapshot)?;
            }
        }
        Ok(())
    }
    fn walk(value: &Value, base: &str, snapshot: &Snapshot) -> Result<()> {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "dependencies"
                        | "devDependencies"
                        | "optionalDependencies"
                        | "peerDependencies"
                        | "overrides"
                ) {
                    dependencies(value, base, snapshot)?;
                }
                if matches!(key.as_str(), "resolved" | "version") {
                    if let Some(value) = value.as_str() {
                        spec(
                            base,
                            value,
                            snapshot,
                            key == "resolved"
                                && object.get("link").and_then(Value::as_bool) == Some(true),
                        )?;
                    }
                }
                if key == "workspaces" {
                    if let Some(entries) = value
                        .as_array()
                        .or_else(|| value.get("packages").and_then(Value::as_array))
                    {
                        for value in entries.iter().filter_map(Value::as_str) {
                            target(base, value, snapshot, true)?;
                        }
                    }
                }
                walk(value, base, snapshot)?;
            }
        } else if let Some(values) = value.as_array() {
            for value in values {
                walk(value, base, snapshot)?;
            }
        }
        Ok(())
    }
    let mut issues = Vec::new();
    for path in snapshot.files.keys() {
        if !matches!(
            path.rsplit('/').next(),
            Some("package.json" | "package-lock.json" | "npm-shrinkwrap.json")
        ) {
            continue;
        }
        control.check()?;
        let bytes = snapshot.read(path)?;
        if bytes.len() > MAX_METADATA {
            issues.push(invalid("存在超限npm声明，需适配后才能安装。"));
            continue;
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => walk(
                &value,
                path.rsplit_once('/').map(|p| p.0).unwrap_or(""),
                snapshot,
            )?,
            Err(_) => issues.push(invalid("存在无法检查的npm声明，需适配后才能安装。")),
        }
    }
    if snapshot.excluded.iter().any(|path| {
        path.split('/')
            .any(|name| name.eq_ignore_ascii_case("config"))
    }) {
        issues.push(invalid(
            "config目录按本机配置保护规则排除；若其中是运行代码，请调整源码布局后重新接入。",
        ));
    }
    issues.truncate(2);
    Ok(issues)
}
