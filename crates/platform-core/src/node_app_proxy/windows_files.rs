//! HANDLE-bound static files; the proxy's original URL and hard-link policy.
use platform_native::windows::files as native;
use std::{fs::File, io, path::Path};

pub(super) fn open_root(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    native::directory_file(path)
}

pub(super) fn open_file(root: &File, parts: &[String]) -> io::Result<File> {
    let (last, parents) = parts.split_last().ok_or(io::ErrorKind::InvalidInput)?;
    let (mut path, root_scope) = native::directory_scope(root)?;
    let mut scopes = vec![root_scope];
    for component in parents {
        path.push(component);
        scopes.push(native::pin_directory(&path)?);
    }
    // Root and each admitted directory stay pinned across the actual leaf open.
    // No owner/DACL check is added to ordinary project website files.
    native::open_read_file(&path.join(last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Read};

    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new(
            platform_native::windows::system_paths::SystemPaths::current()
                .unwrap()
                .system
                .join("cmd.exe"),
        )
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn real_bound_files_reject_junctions_and_hardlinks_without_acl_admission() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-node-proxy-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().join("ordinary-web");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/page.html"), "actual ordinary page").unwrap();
        let foreign = temporary.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("private.txt"), "foreign private").unwrap();
        fs::hard_link(foreign.join("private.txt"), root.join("hardlinked.txt")).unwrap();
        junction(&root.join("linked"), &foreign);
        let held = open_root(&root).unwrap();
        let mut content = String::new();
        open_file(&held, &["nested".into(), "page.html".into()])
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "actual ordinary page");
        assert!(open_file(&held, &["hardlinked.txt".into()]).is_err());
        assert!(open_file(&held, &["linked".into(), "private.txt".into()]).is_err());
        assert!(open_root(&root.join("linked")).is_err());
        assert_eq!(
            fs::read_to_string(foreign.join("private.txt")).unwrap(),
            "foreign private"
        );
    }

    #[test]
    fn real_long_unicode_tree_is_served_from_its_original_pinned_root() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-node-proxy-long-")
            .tempdir()
            .unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("网页 中文 space");
        fs::create_dir(&root).unwrap();
        let parts: Vec<String> = (0..8)
            .map(|i| format!("nested-{i}-中文 space-{}", "x".repeat(28)))
            .collect();
        let mut path = root.clone();
        for p in &parts {
            path.push(p);
            fs::create_dir(&path).unwrap();
        }
        fs::write(path.join("page.txt"), "long actual content").unwrap();
        let held = open_root(&root).unwrap();
        let mut request = parts;
        request.push("page.txt".into());
        let mut bytes = String::new();
        open_file(&held, &request)
            .unwrap()
            .read_to_string(&mut bytes)
            .unwrap();
        assert_eq!(bytes, "long actual content");
        let error = fs::rename(&root, root.with_file_name("displaced")).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32 | 33)));
        assert_eq!(
            fs::read_to_string(path.join("page.txt")).unwrap(),
            "long actual content"
        );
    }

    #[tokio::test]
    async fn actual_http_serves_policy_and_refuses_linked_content() {
        use axum::http::{header, Method, StatusCode};
        let f = crate::node_app_proxy::tests::fixture().await;
        let response = f.request(Method::GET, "/").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("script-src 'self' 'unsafe-inline' 'unsafe-eval' https:;"));
        assert!(response.text().await.unwrap().contains("Radio fixture"));
        let outside = f.root.parent().unwrap().join("foreign-private.txt");
        fs::write(&outside, "foreign private").unwrap();
        fs::hard_link(&outside, f.root.join("hardlinked.txt")).unwrap();
        let foreign = f.root.parent().unwrap().join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("private.txt"), "foreign private").unwrap();
        junction(&f.root.join("linked"), &foreign);
        for path in ["/hardlinked.txt", "/linked/private.txt"] {
            let response = f.request(Method::GET, path).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(!response.text().await.unwrap().contains("foreign private"));
        }
        f.proxy.stop().await.unwrap();
        assert_eq!(fs::read_to_string(outside).unwrap(), "foreign private");
    }
}
