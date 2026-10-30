//! Explicit, bounded public excerpts for the user's external AI draft.
//! All content and recursive directory access stay beneath open directory
//! descriptors; path checks are not the boundary against symlink replacement.
use crate::{files, ApiError};
use platform_native::directory::{self as native, Kind, Stamp};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

const BUDGET: usize = 20 * 1024;
const FILE_READ_LIMIT: usize = 8192;
const TREE_BYTES: usize = 8192;
const TREE_ENTRIES: usize = 150;
const TREE_INSPECTIONS: usize = 500;
const TREE_LABEL: &str = "目录结构（最多3层）";

fn invalid(message: &str) -> ApiError {
    ApiError::new("PROJECT_REVIEW_UNAVAILABLE", message)
}
fn changed() -> ApiError {
    invalid("摘录时项目文件夹或文件发生变化，请重新准备。")
}
fn same_object(left: &Stamp, right: &Stamp) -> bool {
    left.identity == right.identity
}
fn unchanged(left: &Stamp, right: &Stamp) -> bool {
    left == right
}
fn stamp(file: &File) -> Result<Stamp, ApiError> {
    native::stamp(file).map_err(files::error)
}

pub fn review_root(source: &Path, data: &Path, workspace: &Path) -> Result<PathBuf, ApiError> {
    if !source.is_absolute() {
        return Err(invalid("请选择完整的本机项目路径。"));
    }
    let selected = files::root(source)?;
    let source = source.canonicalize()?;
    let current = files::root(&source)?;
    if !same_object(&stamp(&selected)?, &stamp(&current)?) {
        return Err(changed());
    }
    let data = data.canonicalize()?;
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_owned());
    let projects = workspace.join("project-sources");
    let author_source = source.starts_with(&projects) && source != projects;
    if data.starts_with(&source)
        || source.starts_with(&data) && !author_source
        || source.starts_with(&workspace) && !author_source
        || source.components().any(|part| matches!(part, Component::Normal(name) if name == ".platform" || name == ".git" || name == "node_modules"))
        || source.file_name().is_some_and(|name| excluded(&name.to_string_lossy()))
    { return Err(invalid("请选择项目源码文件夹，不能读取平台私有目录或应用数据。")); }
    Ok(source)
}

fn excluded(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".git"
            | ".platform"
            | "node_modules"
            | "data"
            | "uploads"
            | "attachments"
            | "credentials"
            | "secrets"
            | "cache"
            | "backups"
            | "trash"
            | ".venv"
            | "venv"
            | "target"
            | ".next"
    ) || lower.starts_with('.') && lower != ".env.example"
        || [".sqlite", ".sqlite3", ".db", ".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

fn utf8_prefix(text: &str, max: usize) -> &str {
    let mut length = text.len().min(max);
    while !text.is_char_boundary(length) {
        length -= 1;
    }
    &text[..length]
}

struct Excerpt {
    content: String,
    truncated: bool,
}
fn public_file(root: &File, relative: &str, remaining: usize) -> Result<Option<Excerpt>, ApiError> {
    if files::parts(relative).is_err()
        || Path::new(relative)
            .components()
            .any(|part| excluded(&part.as_os_str().to_string_lossy()))
    {
        return Ok(None);
    }
    let mut file = match files::read(root, relative) {
        Ok(file) => file,
        Err(error) if error.code == "FILE_NOT_FOUND" => return Ok(None),
        Err(error) => return Err(error),
    };
    let before = stamp(&file)?;
    let limit = remaining.min(FILE_READ_LIMIT);
    let mut bytes = Vec::new();
    (&mut file)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    let after = stamp(&file)?;
    // Reopen by the same anchored root to reject renames and replacement even
    // when the original descriptor itself was not modified.
    let current = files::read(root, relative)?;
    if !unchanged(&before, &after) || !unchanged(&before, &stamp(&current)?) {
        return Err(changed());
    }
    let mut truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            truncated = true;
            std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return Ok(None),
    };
    let template = Path::new(relative)
        .file_name()
        .is_some_and(|name| name == ".env.example");
    let mut content = String::new();
    for (index, line) in text.lines().enumerate() {
        if index >= 200 {
            truncated = true;
            break;
        }
        let lower = line.to_ascii_lowercase();
        let line = if template {
            // Only names survive, including for unknown business variables.
            line.split_once('=')
                .map(|(name, _)| format!("{name}=<在平台设置中填写>"))
                .unwrap_or_default()
        } else if [
            "api_key",
            "apikey",
            "password",
            "private_key",
            "access_token",
            "client_secret",
        ]
        .iter()
        .any(|key| lower.contains(key))
        {
            "[含凭据相关字段的行已省略]".into()
        } else {
            line.into()
        };
        let separator = usize::from(!content.is_empty());
        let available = remaining.saturating_sub(content.len() + separator);
        if line.len() > available {
            truncated = true;
            if available > 0 {
                if separator != 0 {
                    content.push('\n');
                }
                content.push_str(utf8_prefix(&line, available));
            }
            break;
        }
        if separator != 0 {
            content.push('\n');
        }
        content.push_str(&line);
    }
    Ok(Some(Excerpt { content, truncated }))
}

fn open_directory(parent: &File, name: &str) -> Result<File, ApiError> {
    native::child_directory(parent, std::ffi::OsStr::new(name)).map_err(files::error)
}

#[derive(Default)]
struct Tree {
    scanned: usize,
    entries: usize,
    content: String,
    truncated: bool,
}
impl Tree {
    fn full(&self) -> bool {
        self.scanned >= TREE_INSPECTIONS
            || self.entries >= TREE_ENTRIES
            || self.content.len() >= TREE_BYTES
    }
}
fn tree(directory: &File, prefix: &str, depth: usize, output: &mut Tree) -> Result<(), ApiError> {
    if depth > 3 {
        output.truncated = true;
        return Ok(());
    }
    let before = stamp(directory)?;
    let mut listing = native::Names::open(directory).map_err(files::error)?;
    loop {
        if output.full() {
            output.truncated = true;
            break;
        }
        let Some(entry) = listing.next_name().map_err(files::error)? else {
            break;
        };
        output.scanned += 1;
        let Some(name) = entry.to_str() else {
            output.truncated = true;
            continue;
        };
        if excluded(name) || name.chars().any(char::is_control) || name.contains('\\') {
            continue;
        }
        let metadata = native::inspect(directory, &entry).map_err(files::error)?;
        let kind = metadata.kind;
        if kind == Kind::Other {
            continue;
        }
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let line = format!(
            "{relative}{}",
            if kind == Kind::Directory { "/" } else { "" }
        );
        let separator = usize::from(!output.content.is_empty());
        if output.content.len() + separator + line.len() > TREE_BYTES {
            output.truncated = true;
            break;
        }
        if separator != 0 {
            output.content.push('\n');
        }
        output.content.push_str(&line);
        output.entries += 1;
        if kind == Kind::Directory {
            if depth >= 3 {
                output.truncated = true;
                continue;
            }
            let child = open_directory(directory, name)?;
            let opened = stamp(&child)?;
            if metadata.identity.as_ref() != Some(&opened.identity) {
                return Err(changed());
            }
            tree(&child, &relative, depth + 1, output)?;
            let current = open_directory(directory, name)?;
            if !same_object(&opened, &stamp(&current)?) {
                return Err(changed());
            }
        }
    }
    if !unchanged(&before, &stamp(directory)?) {
        return Err(changed());
    }
    Ok(())
}

pub fn context(source: &Path, data: &Path, workspace: &Path) -> Result<Value, ApiError> {
    let path = review_root(source, data, workspace)?;
    let root = files::root(&path)?;
    let original = stamp(&root)?;
    let mut listing = Tree::default();
    tree(&root, "", 1, &mut listing)?;
    let mut used = TREE_LABEL.len() + listing.content.len();
    let mut truncated = listing.truncated;
    let mut excerpts = vec![json!({"path":TREE_LABEL,"content":listing.content})];
    let mut paths = vec!["local-app.json".to_owned(), "package.json".to_owned()];
    if let Some(excerpt) = public_file(&root, "local-app.json", FILE_READ_LIMIT)? {
        if let Ok(manifest) = serde_json::from_str::<Value>(&excerpt.content) {
            for key in ["backend", "web"] {
                if let (Some(folder), Some(entry)) = (
                    manifest[key]["root"].as_str(),
                    manifest[key]["entry"].as_str(),
                ) {
                    let extension = Path::new(entry)
                        .extension()
                        .and_then(|value| value.to_str());
                    if !matches!(
                        (key, extension),
                        ("backend", Some("js" | "mjs" | "cjs")) | ("web", Some("html" | "htm"))
                    ) {
                        continue;
                    }
                    let relative = if folder == "." {
                        entry.into()
                    } else {
                        format!("{folder}/{entry}")
                    };
                    if !paths.contains(&relative) {
                        paths.push(relative);
                    }
                }
            }
        }
    }
    paths.extend(["README.md".to_owned(), ".env.example".to_owned()]);
    for relative in paths {
        let Some(remaining) = BUDGET.checked_sub(used + relative.len()) else {
            truncated = true;
            break;
        };
        if remaining == 0 {
            truncated = true;
            break;
        }
        if let Some(excerpt) = public_file(&root, &relative, remaining)? {
            used += relative.len() + excerpt.content.len();
            truncated |= excerpt.truncated;
            excerpts.push(json!({"path":relative,"content":excerpt.content}));
        }
    }
    let current = files::root(&path)?;
    if !unchanged(&original, &stamp(&current)?) {
        return Err(changed());
    }
    Ok(
        json!({"sourcePath":path,"files":excerpts,"totalBytes":used,"truncated":truncated,
        "notice":"这是公开材料摘录，不是完整工程。已排除已知私有路径，不能保证识别写在源码里的所有秘密；请预览后选择要复制的文件。"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let data = base.join("platform-data");
        let project = base.join("disposable-project");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&project).unwrap();
        (temporary, data, project)
    }
    fn assert_budget(result: &Value) {
        let actual: usize = result["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                file["path"].as_str().unwrap().len() + file["content"].as_str().unwrap().len()
            })
            .sum();
        assert_eq!(result["totalBytes"].as_u64().unwrap() as usize, actual);
        assert!(
            actual <= BUDGET,
            "final path+content UTF-8 budget exceeded: {actual}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn excerpts_exclude_private_data_links_and_template_values() {
        let (_temporary, data, project) = fixture();
        for name in [".env", "accounts.db", "README.md", ".env.example"] {
            fs::write(project.join(name), "API_KEY=private-fixture\npublic text").unwrap();
        }
        fs::create_dir(project.join("uploads")).unwrap();
        fs::write(project.join("uploads/private.txt"), "private-fixture").unwrap();
        std::os::unix::fs::symlink(project.join(".env"), project.join("package.json")).unwrap();
        assert!(context(&project, &data, &data.join("workspace")).is_err());
        fs::remove_file(project.join("package.json")).unwrap();
        let result = context(&project, &data, &data.join("workspace")).unwrap();
        let text = result.to_string();
        assert!(!text.contains("private-fixture"));
        assert!(!text.contains("accounts.db"));
        assert!(!text.contains("private.txt"));
        assert!(text.contains("在平台设置中填写"));
        assert_budget(&result);
        assert!(review_root(&data, &data, &data.join("workspace")).is_err());
    }
    #[test]
    fn long_tree_and_expanding_redaction_share_the_final_utf8_budget() {
        let (_temporary, data, project) = fixture();
        for index in 0..200 {
            fs::write(
                project.join(format!("{index:03}-{}", "x".repeat(230))),
                "public",
            )
            .unwrap();
        }
        fs::write(project.join("README.md"), "公开内容".repeat(3000)).unwrap();
        fs::write(
            project.join(".env.example"),
            (0..200)
                .map(|index| format!("K{index}=private-fixture"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let result = context(&project, &data, &data.join("workspace")).unwrap();
        assert_budget(&result);
        assert_eq!(result["truncated"], true);
        assert!(!result.to_string().contains("private-fixture"));
        let excerpt = public_file(&files::root(&project).unwrap(), ".env.example", 600)
            .unwrap()
            .unwrap();
        assert!(excerpt.content.len() <= 600);
        assert!(excerpt.truncated);
        assert!(std::str::from_utf8(excerpt.content.as_bytes()).is_ok());
        assert!(!excerpt.content.contains("private-fixture"));
    }
    #[test]
    fn excluded_entries_are_scanned_with_a_bound_before_collecting_or_recursing() {
        let (_temporary, data, project) = fixture();
        for index in 0..600 {
            fs::write(project.join(format!(".private-{index}")), "private-fixture").unwrap();
        }
        let mut output = Tree::default();
        tree(&files::root(&project).unwrap(), "", 1, &mut output).unwrap();
        assert_eq!(output.scanned, TREE_INSPECTIONS);
        assert_eq!(output.entries, 0);
        assert!(output.truncated);
        let result = context(&project, &data, &data.join("workspace")).unwrap();
        assert_budget(&result);
        assert!(!result.to_string().contains("private-fixture"));
    }
    #[cfg(unix)]
    #[test]
    fn declared_entry_cannot_read_through_a_symlinked_parent() {
        let (_temporary, data, project) = fixture();
        let private = data.join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("server.mjs"), "private-fixture").unwrap();
        fs::write(
            project.join("local-app.json"),
            r#"{"backend":{"root":"backend","entry":"server.mjs"}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&private, project.join("backend")).unwrap();
        assert!(context(&project, &data, &data.join("workspace")).is_err());
    }
    #[test]
    fn declarations_cannot_turn_arbitrary_business_json_into_source_excerpts() {
        let (_temporary, data, project) = fixture();
        fs::write(project.join("local-app.json"), r#"{"backend":{"root":".","entry":"records.json"},"web":{"root":".","entry":"notes.txt"}}"#).unwrap();
        fs::write(project.join("records.json"), "private-record-fixture").unwrap();
        fs::write(project.join("notes.txt"), "private-note-fixture").unwrap();
        let result = context(&project, &data, &data.join("workspace")).unwrap();
        assert!(!result.to_string().contains("private-record-fixture"));
        assert!(!result.to_string().contains("private-note-fixture"));
        assert_budget(&result);
    }
    #[cfg(unix)]
    #[test]
    fn opened_directories_stay_anchored_when_parent_paths_are_replaced() {
        let (_temporary, data, project) = fixture();
        let backend = project.join("backend");
        fs::create_dir(&backend).unwrap();
        fs::write(backend.join("server.mjs"), "public-fixture").unwrap();
        let directory = files::root(&project).unwrap();
        let child = open_directory(&directory, "backend").unwrap();
        let private = data.join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("server.mjs"), "private-fixture").unwrap();
        fs::rename(&backend, project.join("previous-backend")).unwrap();
        std::os::unix::fs::symlink(&private, &backend).unwrap();
        assert!(open_directory(&directory, "backend").is_err());
        assert!(public_file(&directory, "backend/server.mjs", 1024).is_err());
        let excerpt = public_file(&child, "server.mjs", 1024).unwrap().unwrap();
        assert_eq!(excerpt.content, "public-fixture");
    }
    #[test]
    fn platform_private_roots_are_rejected_but_published_author_sources_are_allowed() {
        let (_temporary, data, _project) = fixture();
        let workspace = data.join("workspace");
        let author = workspace.join("project-sources/example");
        fs::create_dir_all(&author).unwrap();
        for root in [&data, &workspace, &workspace.join("project-sources")] {
            assert!(review_root(root, &data, &workspace).is_err());
        }
        assert_eq!(review_root(&author, &data, &workspace).unwrap(), author);
        let private = data.join("node-apps/application");
        fs::create_dir_all(&private).unwrap();
        assert!(review_root(&private, &data, &workspace).is_err());
    }

    #[cfg(windows)]
    fn junction(source: &Path, destination: &Path) {
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(destination)
            .arg(source)
            .output()
            .unwrap();
        assert!(result.status.success());
    }

    #[cfg(windows)]
    #[test]
    fn windows_excerpts_exclude_private_files_hardlinks_and_template_values() {
        let (_temporary, data, project) = fixture();
        for name in [".env", "accounts.db", "README.md", ".env.example"] {
            fs::write(project.join(name), "API_KEY=private-fixture\npublic text").unwrap();
        }
        fs::create_dir(project.join("uploads")).unwrap();
        fs::write(project.join("uploads/private.txt"), "private-fixture").unwrap();
        fs::hard_link(project.join(".env"), project.join("package.json")).unwrap();
        assert!(context(&project, &data, &data.join("workspace")).is_err());
        fs::remove_file(project.join("package.json")).unwrap();
        let result = context(&project, &data, &data.join("workspace")).unwrap();
        let text = result.to_string();
        for private in ["private-fixture", "accounts.db", "private.txt"] {
            assert!(!text.contains(private));
        }
        assert!(text.contains("在平台设置中填写"));
        assert_budget(&result);
    }

    #[cfg(windows)]
    #[test]
    fn windows_declared_entry_cannot_read_through_a_directory_junction() {
        let (_temporary, data, project) = fixture();
        let private = data.join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("server.mjs"), "private-fixture").unwrap();
        fs::write(
            project.join("local-app.json"),
            r#"{"backend":{"root":"backend","entry":"server.mjs"}}"#,
        )
        .unwrap();
        let backend = project.join("backend");
        junction(&private, &backend);
        assert!(context(&project, &data, &data.join("workspace")).is_err());
        fs::remove_dir(backend).unwrap();
        assert_eq!(
            fs::read(private.join("server.mjs")).unwrap(),
            b"private-fixture"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_held_directory_cannot_be_replaced_and_reopening_rejects_a_later_junction() {
        let (_temporary, data, project) = fixture();
        let backend = project.join("backend");
        fs::create_dir(&backend).unwrap();
        fs::write(backend.join("server.mjs"), "public-fixture").unwrap();
        let root = files::root(&project).unwrap();
        let child = open_directory(&root, "backend").unwrap();
        assert!(fs::rename(&backend, project.join("previous-backend")).is_err());
        assert_eq!(
            public_file(&child, "server.mjs", 1024)
                .unwrap()
                .unwrap()
                .content,
            "public-fixture"
        );
        drop(child);
        fs::rename(&backend, project.join("previous-backend")).unwrap();
        let private = data.join("private");
        fs::create_dir(&private).unwrap();
        fs::write(private.join("server.mjs"), "private-fixture").unwrap();
        junction(&private, &backend);
        assert!(open_directory(&root, "backend").is_err());
        assert!(public_file(&root, "backend/server.mjs", 1024).is_err());
        fs::remove_dir(backend).unwrap();
    }
}
