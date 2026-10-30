use super::*;
use serde_json::json;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

fn sample() -> TempDir {
    tempfile::Builder::new()
        .prefix("disposable-source-acquisition-")
        .tempdir()
        .unwrap()
}
fn row(mode: &str, path: &str, size: u64) -> Vec<u8> {
    format!("{mode} blob {} {size}\t{path}\0", "1".repeat(40)).into_bytes()
}

#[test]
fn tree_validation_rejects_links_paths_collisions_and_size_without_materializing() {
    let files =
        parse_tree(&[row("100644", "a.txt", 0), row("100755", "目录/run.sh", 4)].concat()).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[1].executable);
    for path in [
        "../bad",
        "/absolute",
        ".git/config",
        "dir/.GiT/config",
        "a//b",
        "a\\b",
        "a\nb",
        "a/./b",
        "trailing.",
        "dir:stream/file",
    ] {
        assert!(parse_tree(&row("100644", path, 1)).is_err(), "{path}");
    }
    for mode in ["120000", "160000", "040000"] {
        assert!(parse_tree(&row(mode, "unsafe", 1)).is_err());
    }
    for paths in [
        ["A/file", "a/other"],
        ["é/file", "e\u{301}/other"],
        ["a", "a/b"],
        ["a/b", "a"],
        ["same", "same"],
    ] {
        assert!(
            parse_tree(
                &paths
                    .iter()
                    .flat_map(|p| row("100644", p, 1))
                    .collect::<Vec<_>>()
            )
            .is_err(),
            "{paths:?}"
        );
    }
    assert!(parse_tree(&row("100644", "large", MAX_BLOB + 1)).is_err());
    assert!(parse_tree(&[0xff, 0]).is_err());
    assert!(parse_tree(b"unfinished").is_err());
    assert!(parse_tree(&[]).unwrap().is_empty());
}

#[test]
fn local_inspection_is_metadata_only_and_does_not_register_or_read_secrets() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    fs::write(
        root.join("package.json"),
        "{\"scripts\":{\"postinstall\":\"touch must-not-run\"}}",
    )
    .unwrap();
    symlink("/this-secret-file-must-not-be-opened", root.join(".env")).unwrap();
    let generic = inspect_directory(&root).unwrap();
    assert_eq!(generic.kind, "needs-adaptation");
    assert_eq!(generic.execution_state, "not-executed");
    assert!(!root.join("must-not-run").exists());
    fs::create_dir(root.join("app")).unwrap();
    fs::write(root.join("app/package.json"), "{}").unwrap();
    fs::write(
        root.join("app/package-lock.json"),
        "{\"lockfileVersion\":3}",
    )
    .unwrap();
    let manifest = json!({"manifestVersion":1,"id":"prj_disposable","name":"Disposable source","description":"","sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html","database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}});
    fs::write(
        root.join("project.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let info = inspect_directory(&root).unwrap();
    assert_eq!(info.kind, "project-v1-source");
    assert!(!root.join("data").exists());
    assert!(!root.join("dist").exists());
    fs::remove_file(root.join("project.json")).unwrap();
    fs::create_dir(root.join("public")).unwrap();
    fs::write(root.join("public/index.html"), "<!doctype html>").unwrap();
    fs::create_dir(root.join("server")).unwrap();
    fs::write(
        root.join("server/managed.mjs"),
        "throw new Error('must not run');",
    )
    .unwrap();
    fs::write(root.join("local-app.json"),serde_json::to_vec(&json!({"packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service","app":{"id":"org.example.disposable","name":"Disposable Node","version":"1.0.0"},"web":{"root":"public","entry":"index.html","routing":"files"},"backend":{"root":"server","entry":"managed.mjs","protocolVersion":1,"nodeMajor":24}})).unwrap()).unwrap();
    assert_eq!(
        inspect_directory(&root).unwrap().kind,
        "node-service-source"
    );
    assert!(!root.join("configuration").exists());
    assert!(inspect_directory(Path::new("relative")).is_err());
}

#[test]
fn local_declaration_symlink_cannot_open_external_file() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let external = root.join("external-secret");
    fs::write(&external, "not a project").unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    symlink(&external, source.join("project.json")).unwrap();
    let inspection = inspect_directory(&source).unwrap();
    assert_eq!(inspection.kind, "needs-adaptation");
    assert!(!inspection.issues.is_empty());
    assert_eq!(fs::read(external).unwrap(), b"not a project");
}

#[allow(clippy::type_complexity)]
pub(super) fn fixture() -> (
    TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    Arc<BuildManager>,
    Arc<SourceAcquisitionManager>,
) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let git =
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes());
    let manager =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    (temp, paths, store, tasks, manager)
}

fn real_toolchain() -> GitToolchain {
    let git = PathBuf::from(
        std::env::var_os("PLATFORM_TEST_SOURCE_GIT")
            .expect("PLATFORM_TEST_SOURCE_GIT must name official disposable Git"),
    );
    let git = git.canonicalize().unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(git.starts_with(workspace.join(".test-artifacts")));
    assert!(git.components().any(|part| part
        .as_os_str()
        .to_string_lossy()
        .starts_with("disposable-")));
    GitToolchain {
        git: ExecutableInfo {
            path: git.clone(),
            version: "2.56.0".into(),
        },
        path: format!(
            "{}:/usr/bin:/bin:/usr/sbin:/sbin",
            git.parent().unwrap().display()
        ),
        exec_path: None,
        template_path: None,
    }
}

#[tokio::test]
#[ignore = "Opt-in: official disposable PLATFORM_TEST_SOURCE_GIT; offline, no user repository/configuration"]
async fn actual_git_blob_materialization_index_and_filters_are_non_executing() {
    let toolchain = real_toolchain();
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let repository = root.join("repository");
    new_dir(&repository).unwrap();
    let home = root.join("home");
    new_dir(&home).unwrap();
    let template = root.join("template");
    new_dir(&template).unwrap();
    let (stop, _) = watch::channel(false);
    let (cancel, _) = watch::channel(false);
    let mut control = RunControl {
        stop: stop.subscribe(),
        cancel: cancel.subscribe(),
        deadline: Instant::now() + Duration::from_secs(30),
        monitor: root.clone(),
        processes: Default::default(),
        scope: None,
        hooks: Default::default(),
    };
    let mut init = isolated_command(&toolchain, &repository, &home, &template);
    init.args(["init", "--initial-branch=main"]);
    execute(init, &mut control, MAX_OUTPUT).await.unwrap();
    fs::write(
        repository.join(".gitattributes"),
        "*.txt filter=must-not-run\n",
    )
    .unwrap();
    fs::write(repository.join("safe.txt"), "中文 materialized exactly\n").unwrap();
    fs::write(
        repository.join("script.sh"),
        "#!/bin/sh\ntouch must-not-execute\n",
    )
    .unwrap();
    fs::set_permissions(
        repository.join("script.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut add = isolated_command(&toolchain, &repository, &home, &template);
    add.args(["add", "--", ".gitattributes", "safe.txt", "script.sh"]);
    execute(add, &mut control, MAX_OUTPUT).await.unwrap();
    let mut commit = isolated_command(&toolchain, &repository, &home, &template);
    commit.args([
        "-c",
        "user.name=Disposable",
        "-c",
        "user.email=disposable@example.invalid",
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-m",
        "disposable source",
    ]);
    execute(commit, &mut control, MAX_OUTPUT).await.unwrap();
    let mut list = isolated_command(&toolchain, &repository, &home, &template);
    list.args(["ls-tree", "-r", "-z", "--long", "HEAD"]);
    let entries = parse_tree(&execute(list, &mut control, MAX_TREE_OUTPUT).await.unwrap()).unwrap();
    // Configure a hostile filter in this test-owned repository. Raw cat-file and
    // read-tree must still never run it. No production network exception exists.
    let mut config = isolated_command(&toolchain, &repository, &home, &template);
    config.args([
        "config",
        "filter.must-not-run.smudge",
        "touch must-not-execute",
    ]);
    execute(config, &mut control, MAX_OUTPUT).await.unwrap();
    for name in [".gitattributes", "safe.txt", "script.sh"] {
        fs::remove_file(repository.join(name)).unwrap();
    }
    let digest = materialize(
        &toolchain,
        &repository,
        &home,
        &template,
        &entries,
        &mut control,
    )
    .await
    .unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(directory_digest(&repository).unwrap(), digest);
    let mut index = isolated_command(&toolchain, &repository, &home, &template);
    index.args(["read-tree", "HEAD"]);
    execute(index, &mut control, MAX_OUTPUT).await.unwrap();
    assert_eq!(
        fs::read_to_string(repository.join("safe.txt")).unwrap(),
        "中文 materialized exactly\n"
    );
    assert!(!repository.join("must-not-execute").exists());
    let mut status = isolated_command(&toolchain, &repository, &home, &template);
    status.args(["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(execute(status, &mut control, MAX_OUTPUT)
        .await
        .unwrap()
        .is_empty());
    fs::set_permissions(
        repository.join("script.sh"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_ne!(
        directory_digest(&repository).unwrap(),
        digest,
        "executable permissions participate in source identity"
    );
    assert!(control.processes.shutdown().await.is_empty());
    assert!(control
        .processes
        .snapshot()
        .iter()
        .all(|process| process.resource_closed()));
}

#[test]
fn disk_monitor_and_manifest_reads_reject_limits_and_hard_links() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let large = root.join("disposable-sparse-payload");
    fs::File::create(&large)
        .unwrap()
        .set_len(MAX_DISK + 1)
        .unwrap();
    assert_eq!(check_disk(&root).unwrap_err().code, "SOURCE_LIMIT_EXCEEDED");
    fs::remove_file(&large).unwrap();
    let declaration = root.join("project.json");
    fs::File::create(&declaration)
        .unwrap()
        .set_len(MAX_METADATA + 1)
        .unwrap();
    assert_eq!(
        read_small(&declaration).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    fs::remove_file(&declaration).unwrap();
    fs::write(&declaration, "{}").unwrap();
    fs::hard_link(&declaration, root.join("second-link")).unwrap();
    assert!(read_small(&declaration).is_err());
    assert!(check_disk(&root).is_err());
}

#[test]
fn tree_limits_include_file_count_total_bytes_and_hfs_git_aliases() {
    let too_many = (0..=MAX_FILES)
        .flat_map(|i| row("100644", &format!("file-{i}"), 0))
        .collect::<Vec<_>>();
    assert_eq!(
        parse_tree(&too_many).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    let too_large = (0..5)
        .flat_map(|i| row("100644", &format!("file-{i}"), MAX_BLOB))
        .collect::<Vec<_>>();
    assert_eq!(
        parse_tree(&too_large).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    assert!(parse_tree(&row("100644", ".g\u{200c}it/config", 1)).is_err());
}
