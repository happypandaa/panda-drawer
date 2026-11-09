//! Read-only descriptions of existing source, runtime, data and control boundaries.
//!
//! These paths describe boundaries, not authorization to modify, relocate or clean them.
//! Git discovery only observes `.git` markers: it never reads their contents or repository
//! configuration, executes Git, certifies a repository, or claims database coverage.
//! Filesystem observations can become stale; mutating operations must validate again.

use crate::panda_manifest::PandaManifest;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Present,
    Missing,
}

/// Check every existing component, including parents of the supplied root, without
/// canonicalizing through a symbolic link. Missing paths are useful boundary descriptions.
fn inspect_path(path: &Path, directory: bool) -> Result<Presence, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("路径不是安全的绝对路径：{}", path.display()));
    }
    let components = platform_native::filesystem::path_components(path);
    let mut parts = components.iter().peekable();
    while let Some(current) = parts.next() {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                if platform_native::filesystem::is_reparse(&metadata) {
                    return Err(format!("未跟随符号链接：{}", current.display()));
                }
                let needs_directory = parts.peek().is_some() || directory;
                if (needs_directory && !metadata.is_dir())
                    || (!needs_directory && !metadata.is_file())
                {
                    return Err(format!("路径类型不符合预期：{}", current.display()));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Presence::Missing),
            Err(error) => {
                return Err(format!("无法检查路径 {}：{}", current.display(), error));
            }
        }
    }
    Ok(Presence::Present)
}

fn checked_boundary(path: PathBuf, directory: bool, issues: &mut Vec<String>) -> Option<PathBuf> {
    match inspect_path(&path, directory) {
        Ok(_) => Some(path),
        Err(reason) => {
            issues.push(reason);
            None
        }
    }
}

fn source_description(path: Option<&Path>, development_source: bool) -> Value {
    let Some(path) = path else {
        return json!({"state":"unverified","path":null,"reason":"源码路径未通过校验，未检查或派生源码位置。"});
    };
    match inspect_path(path, true) {
        Ok(Presence::Present) if development_source => json!({
            "state":"available", "path":path,
            "reason":"项目清单指定的源码目录存在；构建材料和版本能力另行检查。"
        }),
        Ok(Presence::Present) => json!({
            "state":"unverified", "path":path,
            "reason":"当前登记的是外部应用运行目录，尚未确认其中包含开发源码或受支持的构建声明。"
        }),
        Ok(Presence::Missing) => json!({
            "state":"missing", "path":path,
            "reason":"登记的源码或应用目录已失联；未创建替代目录。"
        }),
        Err(reason) => json!({"state":"unverified","path":null,"reason":reason}),
    }
}

fn inspect_git_marker(
    directory: &Path,
    scope: &str,
    seen: &mut HashSet<PathBuf>,
    markers: &mut Vec<Value>,
    issues: &mut Vec<String>,
) {
    if !seen.insert(directory.to_path_buf()) {
        return;
    }
    match inspect_path(directory, true) {
        Ok(Presence::Present) => (),
        Ok(Presence::Missing) => return,
        Err(reason) => {
            issues.push(reason);
            return;
        }
    }
    let marker = directory.join(".git");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            let kind = if platform_native::filesystem::is_reparse(&metadata) {
                "unsafe"
            } else if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "unsafe"
            };
            if kind == "unsafe" {
                issues.push(format!(
                    "Git 标记是链接或特殊文件，未读取或跟随：{}",
                    marker.display()
                ));
            }
            markers.push(json!({"path":marker,"kind":kind,"scope":scope}));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => issues.push(format!("无法检查 Git 标记 {}：{}", marker.display(), error)),
    }
}

fn versioning(project: Option<&Path>, source: Option<&Path>, mut issues: Vec<String>) -> Value {
    let mut seen = HashSet::new();
    let mut markers = Vec::new();
    for (directory, scope) in [(project, "project"), (source, "source")] {
        if let Some(directory) = directory {
            inspect_git_marker(directory, scope, &mut seen, &mut markers, &mut issues);
        }
    }
    for path in [project, source].into_iter().flatten() {
        for ancestor in path.ancestors().skip(1) {
            inspect_git_marker(ancestor, "ancestor", &mut seen, &mut markers, &mut issues);
        }
    }
    json!({
        "state":"not-integrated",
        "reason":"本报告仅浅查目录及祖先的 .git 标记，未确认仓库有效性、工作区状态或纳管范围；代码保存与回退由外部 AI 使用项目自己的 Git 完成。",
        "markers":markers,
        "inspectionIssues":issues,
        "database":{
            "state":if project.is_some() {"not-inspected"} else {"not-supported"},
            "reason":if project.is_some() {"平台不备份或校验业务数据；是否将 data/ 提交到 Git 由用户与 AI 决定。发现标记不代表数据已保存。"} else {"Node 自有业务数据不属于源码 Git；不能从目录标记推断数据库已纳入版本。"}
        }
    })
}

fn project_build(source: Option<&Path>, issues: &mut Vec<String>) -> Value {
    let Some(source) = source else {
        return json!({"state":"unavailable","reason":"源码位置未通过校验，当前不能使用平台构建。"});
    };
    for name in ["package.json", "package-lock.json"] {
        match inspect_path(&source.join(name), false) {
            Ok(Presence::Present) => (),
            Ok(Presence::Missing) => {
                return json!({"state":"unavailable","reason":format!("缺少构建材料 {}；已有运行产物的使用不因此要求重新构建。", name)});
            }
            Err(reason) => {
                issues.push(reason);
                return json!({"state":"unavailable","reason":"构建材料路径未通过安全检查，当前不能使用平台构建。"});
            }
        }
    }
    json!({
        "state":"platform-build",
        "reason":"已存在项目构建声明、package.json 和 package-lock.json，可进入现有平台构建流程；尚未校验内容、依赖、工具或构建结果。"
    })
}

pub fn project_layout(root: &Path, manifest: &PandaManifest) -> Value {
    let mut issues = Vec::new();
    let root = if let Err(error) = manifest.validate() {
        issues.push(error.to_string());
        None
    } else {
        checked_boundary(root.to_path_buf(), true, &mut issues)
    };
    let source = root.clone();
    let web = root
        .as_ref()
        .and_then(|root| match manifest.resolve_web_root(root) {
            Ok(web) => Some(web),
            Err(error) => {
                issues.push(error.to_string());
                None
            }
        });
    let backend = if manifest.has_backend() {
        root.as_ref()
            .and_then(|root| match manifest.resolve_backend(root) {
                Ok((_, backend)) => Some(backend),
                Err(error) => {
                    issues.push(error.to_string());
                    None
                }
            })
    } else {
        None
    };
    let data = root
        .as_ref()
        .and_then(|root| checked_boundary(root.join("data"), true, &mut issues));
    let database = if manifest.platform.is_some() {
        root.as_ref()
            .and_then(|root| checked_boundary(root.join("data/main.sqlite"), false, &mut issues))
    } else {
        None
    };
    let attachments = if manifest.platform.is_some() {
        root.as_ref()
            .and_then(|root| checked_boundary(root.join("data/attachments"), true, &mut issues))
    } else {
        None
    };
    let build_source = manifest.development.as_ref().and_then(|development| {
        root.as_ref().and_then(|root| {
            crate::panda_manifest::checked_project_child(root, &development.source_dir, true).ok()
        })
    });
    let build = if manifest.development.is_some() {
        project_build(build_source.as_deref(), &mut issues)
    } else {
        json!({"state":"unavailable","reason":"未声明平台构建；直接运行现有网页文件。"})
    };
    json!({
        "kind":"app",
        "source":source_description(source.as_deref(), true),
        "runtime":{"webRoot":web,"backendRoot":backend,"external":false},
        "data":{"root":data,"database":database,"attachments":attachments},
        "configurationPath":null,"controlPath":null,"build":build,
        "versioning":versioning(root.as_deref(), source.as_deref(), issues)
    })
}

#[cfg(test)]
fn node_layout(source: &Path, slot: &Path, manifest: Option<&PandaManifest>) -> Value {
    let mut issues = Vec::new();
    let source = checked_boundary(source.to_path_buf(), true, &mut issues);
    let slot = checked_boundary(slot.to_path_buf(), true, &mut issues);
    let mut web = None;
    let mut backend = None;
    if let (Some(source), Some(manifest)) = (&source, manifest) {
        match manifest.inspect(source) {
            Ok(_) => {
                web = manifest.resolve_web_root(source).ok();
                backend = manifest
                    .backend
                    .as_ref()
                    .and_then(|_| manifest.resolve_backend(source).ok().map(|(_, root)| root));
            }
            Err(error) => issues.push(format!(
                "应用声明或声明文件未通过校验，未派生运行路径：{}",
                error
            )),
        }
    } else if manifest.is_none() {
        issues.push("未提供已解析的应用声明，无法确定网页与后端运行路径。".into());
    }
    let data = source
        .as_ref()
        .and_then(|source| checked_boundary(source.join("data"), true, &mut issues));
    let configuration = slot.as_ref().and_then(|slot| {
        checked_boundary(
            slot.join("configuration/configuration.json"),
            false,
            &mut issues,
        )
    });
    json!({
        "kind":"local-node-source",
        "source":source_description(source.as_deref(), false),
        "runtime":{"webRoot":web,"backendRoot":backend,"external":true},
        "data":{"root":data,"database":null,"attachments":null},
        "configurationPath":configuration,
        "controlPath":slot,
        "build":{
            "state":"unavailable",
            "reason":"本机 Node 接入只登记外部运行目录，尚未提供受支持的源码构建流程。"
        },
        "versioning":versioning(None, source.as_deref(), issues)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{Builder, TempDir};

    fn disposable() -> (TempDir, PathBuf) {
        let temp = Builder::new()
            .prefix("disposable-object-layout-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }

    fn project(root: &Path) -> PandaManifest {
        for directory in ["app", "dist", "data/attachments", "docs"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("data/main.sqlite"), "disposable database bytes").unwrap();
        PandaManifest::parse(
            &serde_json::to_vec(&json!({
                "manifestVersion":2,"app":{"name":"可丢弃布局样本"},
                "web":{"root":"dist","entry":"index.html"},"platform":{"sdkApiVersion":1},
                "development":{"sourceDir":"app","build":{"kind":"npm","script":"build"}}
            }))
            .unwrap(),
        )
        .unwrap()
    }

    fn node_manifest(root: &Path) -> PandaManifest {
        for directory in ["public", "server"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("public/index.html"), "disposable page").unwrap();
        fs::write(root.join("server/main.mjs"), "never executed").unwrap();
        PandaManifest::parse(
            &serde_json::to_vec(&json!({
                "manifestVersion":2,"app":{"name":"可丢弃 Node 样本","version":"1"},
                "web":{"root":"public","entry":"index.html"},
                "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn source_only_git_is_a_marker_and_never_claims_database_coverage() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        fs::create_dir(root.join(".git")).unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(layout["source"]["state"], "available");
        assert_eq!(
            layout["versioning"]["markers"],
            json!([
                {"path":root.join(".git"),"kind":"directory","scope":"project"}
            ])
        );
        assert_eq!(layout["versioning"]["state"], "not-integrated");
        assert_eq!(layout["versioning"]["database"]["state"], "not-inspected");
        assert_eq!(layout["build"]["state"], "unavailable");
    }

    #[test]
    fn parent_git_is_not_reported_as_a_project_repository() {
        let (_temp, parent) = disposable();
        let root = parent.join("project");
        let manifest = project(&root);
        fs::create_dir(parent.join(".git")).unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(
            layout["versioning"]["markers"],
            json!([
                {"path":parent.join(".git"),"kind":"directory","scope":"ancestor"}
            ])
        );
    }

    #[test]
    fn git_file_contents_are_not_parsed_and_inspection_preserves_files() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        let marker = root.join(".git");
        let contents = b"not a gitdir pointer\0private bytes";
        fs::write(&marker, contents).unwrap();
        let database = fs::read(root.join("data/main.sqlite")).unwrap();
        let before = fs::metadata(&marker).unwrap().modified().unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(layout["versioning"]["markers"][0]["kind"], "file");
        assert_eq!(fs::read(&marker).unwrap(), contents);
        assert_eq!(fs::metadata(&marker).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read(root.join("data/main.sqlite")).unwrap(), database);
        assert!(!root.join("app/package.json").exists());
        assert!(!root.join(".platform").exists());
    }

    #[cfg(unix)]
    #[test]
    fn git_symlink_is_reported_without_following_it() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        std::os::unix::fs::symlink("missing-private-target", root.join(".git")).unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(layout["versioning"]["markers"][0]["kind"], "unsafe");
        assert!(!layout["versioning"]["inspectionIssues"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(fs::symlink_metadata(root.join(".git"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn missing_source_is_not_recreated_or_buildable() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        fs::remove_dir(root.join("app")).unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(layout["source"]["state"], "available");
        assert_eq!(layout["build"]["state"], "unavailable");
        assert!(!root.join("app").exists());
        assert_eq!(layout["runtime"]["webRoot"], json!(root.join("dist")));
    }

    #[test]
    fn build_entry_requires_both_ordinary_package_files() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        fs::write(root.join("app/package.json"), "not parsed by layout").unwrap();
        assert_eq!(
            project_layout(&root, &manifest)["build"]["state"],
            "unavailable"
        );
        fs::write(root.join("app/package-lock.json"), "not parsed by layout").unwrap();
        assert_eq!(
            project_layout(&root, &manifest)["build"]["state"],
            "platform-build"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_build_material_does_not_enable_platform_build() {
        let (_temp, root) = disposable();
        let manifest = project(&root);
        fs::write(root.join("private-package"), "private bytes").unwrap();
        std::os::unix::fs::symlink(root.join("private-package"), root.join("app/package.json"))
            .unwrap();
        fs::write(root.join("app/package-lock.json"), "disposable lock").unwrap();
        let layout = project_layout(&root, &manifest);
        assert_eq!(layout["source"]["state"], "available");
        assert_eq!(layout["build"]["state"], "unavailable");
        assert!(!layout["versioning"]["inspectionIssues"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            fs::read(root.join("private-package")).unwrap(),
            b"private bytes"
        );
    }

    #[test]
    fn node_runtime_directory_is_not_confirmed_development_source() {
        let (_temp, root) = disposable();
        let source = root.join("external");
        let slot = root.join("slot");
        let manifest = node_manifest(&source);
        fs::create_dir(&slot).unwrap();
        let layout = node_layout(&source, &slot, Some(&manifest));
        assert_eq!(layout["source"]["state"], "unverified");
        assert_eq!(layout["runtime"]["webRoot"], json!(source.join("public")));
        assert_eq!(
            layout["runtime"]["backendRoot"],
            json!(source.join("server"))
        );
        assert_eq!(layout["runtime"]["external"], true);
        assert_eq!(
            layout["configurationPath"],
            json!(slot.join("configuration/configuration.json"))
        );
        assert_eq!(layout["data"]["root"], json!(source.join("data")));
        assert!(!source.join("data").exists());
        assert!(layout["data"]["database"].is_null());
        assert_eq!(layout["build"]["state"], "unavailable");
        assert_eq!(fs::read_dir(&slot).unwrap().count(), 0);
    }

    #[test]
    fn absent_or_invalid_node_declaration_never_invents_runtime_paths() {
        let (_temp, root) = disposable();
        let mut manifest = node_manifest(&root);
        for declaration in [None, Some(&manifest)] {
            let layout = node_layout(&root.join("missing"), &root.join("slot"), declaration);
            assert_eq!(layout["source"]["state"], "missing");
            assert!(layout["runtime"]["webRoot"].is_null());
            assert!(layout["runtime"]["backendRoot"].is_null());
        }
        manifest.web.root = "../outside".into();
        let layout = node_layout(&root, &root.join("slot"), Some(&manifest));
        assert!(layout["runtime"]["webRoot"].is_null());
        assert!(layout["runtime"]["backendRoot"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_in_input_parent_prevents_all_derived_access() {
        let (_temp, root) = disposable();
        let real = root.join("real");
        let manifest = project(&real.join("project"));
        fs::create_dir(real.join("project/.git")).unwrap();
        std::os::unix::fs::symlink(&real, root.join("linked")).unwrap();
        let layout = project_layout(&root.join("linked/project"), &manifest);
        assert_eq!(layout["source"]["state"], "unverified");
        assert!(layout["source"]["path"].is_null());
        assert!(layout["data"]["root"].is_null());
        assert!(layout["controlPath"].is_null());
        assert!(layout["runtime"]["webRoot"].is_null());
        assert!(layout["versioning"]["markers"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!layout["versioning"]["inspectionIssues"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn node_slot_link_never_exposes_derived_private_paths() {
        let (_temp, root) = disposable();
        let source = root.join("source");
        let manifest = node_manifest(&source);
        let private = root.join("private");
        fs::create_dir(&private).unwrap();
        std::os::unix::fs::symlink(&private, root.join("linked-slot")).unwrap();
        let layout = node_layout(&source, &root.join("linked-slot"), Some(&manifest));
        // Project data belongs to the independently checked author source,
        // while an unsafe private slot never exposes its control/config paths.
        assert_eq!(layout["data"]["root"], json!(source.join("data")));
        assert!(!source.join("data").exists());
        assert!(layout["configurationPath"].is_null());
        assert!(layout["controlPath"].is_null());
        assert_eq!(layout["runtime"]["webRoot"], json!(source.join("public")));
        assert_eq!(fs::read_dir(&private).unwrap().count(), 0);
    }

    #[test]
    fn invalid_project_manifest_cannot_escape_the_project() {
        let (_temp, root) = disposable();
        let mut manifest = project(&root);
        manifest.web.root = "../outside".into();
        let layout = project_layout(&root, &manifest);
        assert!(layout["source"]["path"].is_null());
        assert!(layout["runtime"]["webRoot"].is_null());
        assert!(layout["data"]["database"].is_null());
        assert_eq!(layout["build"]["state"], "unavailable");
    }
}
