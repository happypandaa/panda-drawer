//! Synthetic complete trees exercise the real verifier and are never executed.
use super::*;
use serde_json::json;
use std::collections::BTreeMap;

fn sample(omit: Option<&str>) -> (tempfile::TempDir, PathBuf, String) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-managed-mingit-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("development-git");
    let mut files = BTreeMap::from([
        ("LICENSE.txt", "synthetic license"),
        ("cmd/git.exe", "synthetic Git entry, never executed"),
        ("ucrt64/bin/git.exe", "synthetic Git engine, never executed"),
        ("ucrt64/libexec/git-core/git-sh-setup", "synthetic helper"),
        (
            "ucrt64/share/git-core/templates/description",
            "synthetic template",
        ),
    ]);
    if let Some(omit) = omit {
        assert!(files.remove(omit).is_some());
    }
    let mut hash = Sha256::new();
    let mut total = 0u64;
    for (name, bytes) in &files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        hash.update(format!(
            "{name}\0{}\0{:x}\n",
            bytes.len(),
            Sha256::digest(bytes)
        ));
        total += bytes.len() as u64;
    }
    let source = json!({
        "schemaVersion":1,"gitVersion":"synthetic.windows.1","platform":"win32","arch":"x64",
        "distribution":"MinGit","source":{},
        "license":{"path":"LICENSE.txt","size":files["LICENSE.txt"].len(),"sha256":format!("{:x}",Sha256::digest(files["LICENSE.txt"]))},
        "layout":{"git":"cmd/git.exe","execPath":"ucrt64/libexec/git-core","templatesPath":"ucrt64/share/git-core/templates"},
        "tree":{"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1","fileCount":files.len(),"totalBytes":total,"sha256":format!("{:x}",hash.finalize())},
        "executables":["cmd/git.exe","ucrt64/bin/git.exe"]
    }).to_string();
    fs::write(root.join(METADATA_FILE), &source).unwrap();
    (temporary, root, source)
}

#[test]
fn fixed_windows_release_has_its_own_native_schema_and_paths() {
    let source = release(RELEASE).unwrap();
    assert_eq!(source.platform, "win32");
    assert_eq!(source.arch, "x64");
    assert_eq!(source.distribution, "MinGit");
    assert_eq!(source.git_version, "2.56.0.windows.4");
    assert_eq!(source.tree.file_count, 375);
    assert_eq!(source.tree.total_bytes, 96_237_338);
    assert_eq!(source.license.path, "LICENSE.txt");
    let value: serde_json::Value = serde_json::from_str(RELEASE).unwrap();
    assert!(value.get("build").is_none());
    assert!(value.get("signature").is_none());
    let root = Path::new(r"C:\安装 中文 space\development-git");
    assert_eq!(executable_path(root), root.join("cmd/git.exe"));
    assert_eq!(
        layout::exec_path(root),
        root.join("ucrt64/libexec/git-core")
    );
    assert_eq!(
        layout::template_path(root),
        root.join("ucrt64/share/git-core/templates")
    );
    assert_eq!(
        release(include_str!(
            "../../../../scripts/development-git-release.json"
        ))
        .err()
        .unwrap()
        .code,
        "MANAGED_GIT_INVALID"
    );
}

#[test]
fn windows_complete_tree_verification_relocates_and_preserves_read_only_bytes() {
    let (_temporary, root, source) = sample(None);
    let verified = verify_sync(root.clone(), &source).unwrap();
    assert_eq!(verified.git, root.join("cmd/git.exe"));
    assert_eq!(verified.exec_path, root.join("ucrt64/libexec/git-core"));
    assert_eq!(
        verified.template_path,
        root.join("ucrt64/share/git-core/templates")
    );
    let moved = root.with_file_name("moved 中文 space");
    fs::rename(&root, &moved).unwrap();
    let git = moved.join("cmd/git.exe");
    let original = fs::metadata(&git).unwrap().permissions();
    let mut permissions = original.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&git, permissions).unwrap();
    verify_sync(moved.clone(), &source).unwrap();
    assert!(fs::metadata(&git).unwrap().permissions().readonly());
    fs::set_permissions(&git, original).unwrap();
    assert_eq!(
        fs::read_to_string(git).unwrap(),
        "synthetic Git entry, never executed"
    );
}

#[test]
fn changed_missing_extra_metadata_and_empty_directory_entries_fail_without_repair() {
    for change in ["changed", "missing", "extra", "metadata", "empty-directory"] {
        let (_temporary, root, source) = sample(None);
        let git = root.join("cmd/git.exe");
        match change {
            "changed" => fs::write(&git, b"retain changed entry").unwrap(),
            "missing" => fs::remove_file(&git).unwrap(),
            "extra" => fs::write(root.join("foreign"), b"retain extra bytes").unwrap(),
            "metadata" => fs::write(root.join(METADATA_FILE), b"{}").unwrap(),
            "empty-directory" => fs::create_dir(root.join("empty")).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            verify_sync(root.clone(), &source).is_err(),
            "accepted {change}"
        );
        if change == "changed" {
            assert_eq!(fs::read(&git).unwrap(), b"retain changed entry");
        }
        if change == "extra" {
            assert_eq!(
                fs::read(root.join("foreign")).unwrap(),
                b"retain extra bytes"
            );
        }
        if change == "empty-directory" {
            assert!(root.join("empty").is_dir());
        }
    }
}

#[test]
fn the_native_schema_rejects_foreign_layouts_and_synthesized_mac_signature_fields() {
    for change in [
        "platform",
        "arch",
        "layout",
        "distribution",
        "license",
        "executables",
        "signature",
        "build",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(RELEASE).unwrap();
        match change {
            "platform" => value["platform"] = json!("darwin"),
            "arch" => value["arch"] = json!("arm64"),
            "layout" => value["layout"]["git"] = json!("bin/git"),
            "distribution" => value["distribution"] = json!("other"),
            "license" => value["license"]["sha256"] = json!("invalid"),
            "executables" => value["executables"] = json!(["cmd/git.exe"]),
            "signature" => value["signature"] = json!({}),
            "build" => value["build"] = json!({}),
            _ => unreachable!(),
        }
        let expected = if change == "platform" || change == "arch" {
            "MANAGED_GIT_PLATFORM_UNSUPPORTED"
        } else {
            "MANAGED_GIT_INVALID"
        };
        assert_eq!(
            release(&value.to_string()).err().unwrap().code,
            expected,
            "{change}"
        );
    }
}

#[test]
fn a_matching_tree_still_requires_actual_helper_and_template_directories() {
    for omit in [
        "ucrt64/libexec/git-core/git-sh-setup",
        "ucrt64/share/git-core/templates/description",
    ] {
        let (_temporary, root, source) = sample(Some(omit));
        let error = verify_sync(root, &source).unwrap_err();
        assert_eq!(error.code, "MANAGED_GIT_INVALID");
        assert_eq!(error.message, "私有 Git 缺少必要的辅助程序或模板目录。");
    }
}
