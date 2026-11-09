//! Reads the current author files through both manifest and native file guards.
use crate::{files, panda_manifest::PandaManifest, ApiError};
use std::{fs::File, path::Path};

pub(crate) enum Lookup {
    Found(File),
    Missing,
}
/// Missing is separate from a forbidden path so SPA fallback never masks a
/// protected path, an escaping alias, a special file, or a native open failure.
pub(crate) fn lookup(
    project: &Path,
    manifest: &PandaManifest,
    relative: &str,
) -> Result<Lookup, ApiError> {
    let denied = || ApiError::new("FILE_NOT_FOUND", "网页路径不存在或不允许访问。");
    let web_root = manifest.resolve_web_root(project).map_err(|_| denied())?;
    let mut actual = web_root.clone();
    let parts: Vec<_> = relative.split('/').collect();
    if parts.is_empty()
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.contains('\\')
                || part.contains(':')
                || part.contains('\0')
        })
    {
        return Err(denied());
    }
    for (index, part) in parts.iter().enumerate() {
        actual.push(part);
        crate::panda_manifest::ensure_web_path(project, &actual, "web.file")
            .map_err(|_| denied())?;
        match std::fs::symlink_metadata(&actual) {
            Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Lookup::Missing)
            }
            Err(_) => return Err(denied()),
            Ok(_) => {}
        }
        actual = actual.canonicalize().map_err(|_| denied())?;
        if !actual.starts_with(&web_root) {
            return Err(denied());
        }
        crate::panda_manifest::ensure_web_path(project, &actual, "web.file")
            .map_err(|_| denied())?;
        let metadata = std::fs::metadata(&actual).map_err(|_| denied())?;
        if index + 1 < parts.len() {
            if !metadata.is_dir() {
                return Err(denied());
            }
        } else if !metadata.is_file() {
            return Err(denied());
        }
    }
    Ok(Lookup::Found(open(project, manifest, relative)?))
}

pub(crate) fn open(
    project: &Path,
    manifest: &PandaManifest,
    relative: &str,
) -> Result<File, ApiError> {
    let web_root = manifest
        .resolve_web_root(project)
        .map_err(|error| ApiError::new("WEB_ROOT_INVALID", error.to_string()))?;
    let resolved = crate::panda_manifest::checked_project_child(&web_root, relative, false)
        .map_err(|_| ApiError::new("FILE_NOT_FOUND", "没有找到这个网页文件。"))?;
    crate::panda_manifest::ensure_web_path(project, &resolved, "web.file")
        .map_err(|_| ApiError::new("FILE_NOT_FOUND", "没有找到这个网页文件。"))?;
    let relative = resolved
        .strip_prefix(&web_root)
        .ok()
        .and_then(Path::to_str)
        .ok_or_else(|| ApiError::new("FILE_NOT_FOUND", "网页文件不在声明目录内。"))?;
    let relative = relative.replace('\\', "/");
    // The native open walks the resolved components under a retained directory
    // handle without following links and checks the final file's link count.
    files::read(&files::root(&web_root)?, &relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Read};
    fn manifest(root: &str) -> PandaManifest {
        PandaManifest::parse(
            &serde_json::to_vec(&serde_json::json!({
                "manifestVersion":2,"app":{"name":"Disposable web file boundary"},
                "web":{"root":root,"entry":"index.html"}
            }))
            .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn current_files_are_read_without_snapshots_and_business_files_remain_private() {
        let temporary = tempfile::Builder::new()
            .prefix("disposable-current-web-files-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::write(root.join("index.html"), "first").unwrap();
        let declaration = manifest(".");
        let mut first = String::new();
        open(&root, &declaration, "index.html")
            .unwrap()
            .read_to_string(&mut first)
            .unwrap();
        assert_eq!(first, "first");
        fs::write(root.join("index.html"), "second").unwrap();
        let mut second = String::new();
        open(&root, &declaration, "index.html")
            .unwrap()
            .read_to_string(&mut second)
            .unwrap();
        assert_eq!(second, "second");
        for relative in [
            "data/main.sqlite",
            "node_modules/pkg/index.js",
            ".private/secret",
            "panda.json",
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "private").unwrap();
            assert!(open(&root, &declaration, relative).is_err(), "{relative}");
        }
        fs::create_dir_all(root.join("assets/data")).unwrap();
        fs::write(root.join("assets/data/public.json"), "public").unwrap();
        assert!(open(&root, &declaration, "assets/data/public.json").is_ok());
        fs::hard_link(root.join("index.html"), root.join("linked.html")).unwrap();
        assert!(open(&root, &declaration, "linked.html").is_err());
        assert!(open(&root, &declaration, "../outside").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn internal_aliases_work_but_never_expose_protected_or_external_files() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::Builder::new()
            .prefix("disposable-linked-web-files-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::create_dir(root.join("public")).unwrap();
        fs::write(root.join("public/index.html"), "public").unwrap();
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data/private.txt"), "private").unwrap();
        symlink(
            root.join("public/index.html"),
            root.join("public/alias.html"),
        )
        .unwrap();
        symlink(
            root.join("data/private.txt"),
            root.join("public/innocent.txt"),
        )
        .unwrap();
        symlink(root.join("public"), root.join("web-alias")).unwrap();
        let declaration = manifest("web-alias");
        assert!(open(&root, &declaration, "alias.html").is_ok());
        assert!(open(&root, &declaration, "innocent.txt").is_err());
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("external.txt"), "outside").unwrap();
        symlink(
            outside.path().join("external.txt"),
            root.join("public/external.txt"),
        )
        .unwrap();
        assert!(open(&root, &declaration, "external.txt").is_err());
    }
}
