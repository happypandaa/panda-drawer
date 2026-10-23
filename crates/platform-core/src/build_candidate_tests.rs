use super::*;
use std::os::unix::fs::symlink;

fn write(root: &Path, path: &str, value: Value) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
fn fixture(root: &Path) {
    write(
        root,
        "package.json",
        json!({
            "dependencies":{"tiny":"file:vendor/tiny", "registry-package":"~1.0.0"},
            "workspaces":["packages/*"]
        }),
    );
    write(
        root,
        "vendor/tiny/package.json",
        json!({"name":"tiny","version":"1.0.0"}),
    );
    write(
        root,
        "packages/one/package.json",
        json!({
            "name":"one","version":"1.0.0", "dependencies":{"tiny":"file:../../vendor/tiny"}
        }),
    );
    write(
        root,
        "package-lock.json",
        json!({"lockfileVersion":3,"packages":{
            "":{"dependencies":{"tiny":"file:vendor/tiny"},"workspaces":["packages/*"]},
            "node_modules/tiny":{"link":true,"resolved":"vendor/tiny"},
            "node_modules/one":{"link":true,"resolved":"packages/one"},
            "packages/one":{"dependencies":{"tiny":"file:../../vendor/tiny"}},
            "vendor/tiny":{"version":"1.0.0"}
        }}),
    );
}

#[test]
fn copy_preserves_executables_and_empty_directories_without_sharing_files_or_git() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::write(source.join("helper.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(source.join("helper.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::hard_link(source.join("helper.sh"), source.join("alias.sh")).unwrap();
    fs::write(source.join(".git"), "gitdir: /never-read-this").unwrap();
    symlink(root.join("not-present"), source.join("node_modules")).unwrap();
    copy_source(&source, &target).unwrap();
    assert!(target.join("empty").is_dir());
    assert!(!target.join(".git").exists());
    assert!(fs::symlink_metadata(target.join("node_modules")).is_err());
    for name in ["helper.sh", "alias.sh"] {
        let copied = fs::metadata(target.join(name)).unwrap();
        assert_eq!(copied.mode() & 0o777, 0o700);
        assert_eq!(copied.nlink(), 1);
        assert_ne!(copied.ino(), fs::metadata(source.join(name)).unwrap().ino());
    }
    fs::write(target.join("helper.sh"), "changed candidate").unwrap();
    assert_eq!(
        fs::read(source.join("helper.sh")).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    symlink(&root, source.join("ordinary-link")).unwrap();
    assert_eq!(
        copy_source(&source, &root.join("rejected"))
            .unwrap_err()
            .code,
        "BUILD_INPUT_UNSAFE"
    );
}

#[test]
fn local_file_and_workspace_graph_must_close_inside_source_including_nested_locks() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    fixture(&root);
    validate_local_dependencies(&root).unwrap();
    let root_lock: Value =
        serde_json::from_slice(&fs::read(root.join("package-lock.json")).unwrap()).unwrap();
    let mut invalid_locks = vec![];
    let mut lock = root_lock.clone();
    lock["packages"]["node_modules/transitive"] = json!({"resolved":"file:../outside.tgz"});
    invalid_locks.push(lock);
    let mut lock = root_lock.clone();
    lock["packages"]["node_modules/one"]["resolved"] = json!("../outside");
    invalid_locks.push(lock);
    let mut lock = root_lock.clone();
    lock["packages"]["../outside"] = json!({"version":"1.0.0"});
    invalid_locks.push(lock);
    let mut lock = root_lock.clone();
    lock["dependencies"] =
        json!({"outer":{"dependencies":{"inner":{"version":"file:../../outside"}}}});
    invalid_locks.push(lock);
    let mut lock = root_lock;
    lock["packages"]["packages/one"]["dependencies"]["tiny"] = json!("file:../../../outside");
    invalid_locks.push(lock);
    for lock in invalid_locks {
        write(&root, "package-lock.json", lock);
        assert_eq!(
            validate_local_dependencies(&root).unwrap_err().code,
            "BUILD_LOCAL_DEPENDENCY_UNSUPPORTED"
        );
    }
}

#[test]
fn external_paths_ambiguous_workspaces_and_overriding_shrinkwrap_fail_before_install() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    for package in [
        json!({"dependencies":{"tiny":"file:../outside"}}),
        json!({"dependencies":{"tiny":"file:/tmp/outside"}}),
        json!({"dependencies":{"tiny":"file:vendor/%2e%2e/outside"}}),
        json!({"dependencies":{"tiny":"file:node_modules/old"}}),
        json!({"workspaces":["../outside"]}),
        json!({"workspaces":["{packages,../outside}/*"]}),
    ] {
        fixture(&root);
        write(&root, "package.json", package);
        assert_eq!(
            validate_local_dependencies(&root).unwrap_err().code,
            "BUILD_LOCAL_DEPENDENCY_UNSUPPORTED"
        );
    }
    fixture(&root);
    write(
        temp.path(),
        "npm-shrinkwrap.json",
        json!({"lockfileVersion":3}),
    );
    assert_eq!(
        validate_local_dependencies(&root).unwrap_err().code,
        "BUILD_LOCAL_DEPENDENCY_UNSUPPORTED"
    );
}

#[test]
fn command_stops_git_discovery_at_parent_and_removes_node_path() {
    let mut command = Command::new("unused");
    isolate_git_environment(&mut command, Path::new("/private/candidate/app")).unwrap();
    let env: std::collections::HashMap<_, _> = command.as_std().get_envs().collect();
    assert_eq!(
        env.get(std::ffi::OsStr::new("GIT_CEILING_DIRECTORIES")),
        Some(&Some(std::ffi::OsStr::new("/private/candidate")))
    );
    assert_eq!(env.get(std::ffi::OsStr::new("NODE_PATH")), Some(&None));
}
