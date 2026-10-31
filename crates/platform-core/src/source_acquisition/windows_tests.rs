use super::*;
use platform_native::windows::files as native;

fn sample() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-source-中文 space-")
        .tempdir()
        .unwrap()
}
fn row(mode: &str, path: &str, size: u64) -> Vec<u8> {
    format!("{mode} blob {} {size}\t{path}\0", "1".repeat(40)).into_bytes()
}

#[test]
fn windows_tree_names_fail_before_materialization_with_adaptation_details() {
    for path in [
        "CON",
        "dir/nul.txt",
        "AUX.tar.gz",
        "COM9.js",
        "LPT¹.txt",
        "bad?.js",
        "bad<name",
        "trailing.",
        "trailing ",
        "stream:name",
    ] {
        let issue = parse_tree(&row("100644", path, 1)).unwrap_err();
        assert_eq!(issue.code, "SOURCE_NEEDS_ADAPTATION", "{path}");
        assert_eq!(issue.details["executionState"], "not-executed");
    }
    for path in [
        "COM10.txt",
        "LPT0",
        "null.txt",
        ".temp",
        "目录/普通 file.txt",
    ] {
        assert_eq!(
            parse_tree(&row("100644", path, 1)).unwrap().len(),
            1,
            "{path}"
        );
    }
    for paths in [
        ["A/file", "a/other"],
        ["é/file", "e\u{301}/other"],
        ["a", "a/b"],
        ["Σ/file", "σ/other"],
    ] {
        assert!(
            parse_tree(
                &paths
                    .into_iter()
                    .flat_map(|p| row("100644", p, 1))
                    .collect::<Vec<_>>()
            )
            .is_err(),
            "{paths:?}"
        );
    }
    for mode in ["120000", "160000", "040000"] {
        assert!(parse_tree(&row(mode, "unsafe", 1)).is_err());
    }
    assert_eq!(
        parse_tree(&row("100644", "huge", MAX_BLOB + 1))
            .unwrap_err()
            .code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    assert!(parse_tree(&[0xff, 0]).is_err());
    assert!(parse_tree(b"unfinished").is_err());
    // Final sigma is a distinct legal name under the Windows ordinal rule;
    // Unicode uppercase expansion must not manufacture another collision.
    assert_eq!(
        parse_tree(&[row("100644", "Σ/file", 1), row("100644", "ς/other", 1)].concat())
            .unwrap()
            .len(),
        2
    );
    for paths in [["A/file", "a/other"], ["Σ/file", "σ/other"]] {
        let entries = paths.map(|path| TreeEntry {
            path: path.into(),
            oid: "1".repeat(40),
            size: 1,
            executable: false,
        });
        assert_eq!(
            policy_native::validate_tree(&entries).unwrap_err().code,
            "SOURCE_NEEDS_ADAPTATION"
        );
    }
}

#[test]
fn actual_ntfs_names_and_exclusive_file_creation_preserve_the_original_full_identity() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let first = root.join("Σ");
    let final_sigma = root.join("ς");
    fs::create_dir(&first).unwrap();
    let first_id = native::open_no_reparse(&first).unwrap().identity;
    match fs::create_dir(&final_sigma) {
        Ok(()) => {
            let other = native::open_no_reparse(&final_sigma).unwrap().identity;
            assert!(!first_id.same_object(&other));
            eprintln!("actual volume keeps capital/final sigma as different full FileIds");
        }
        Err(issue) if issue.kind() == std::io::ErrorKind::AlreadyExists => {
            let other = native::open_no_reparse(&final_sigma).unwrap().identity;
            assert!(first_id.same_object(&other));
            eprintln!("actual volume aliases capital/final sigma; exclusive creation remains final authority");
        }
        Err(issue) => panic!("actual directory creation could not be observed: {issue}"),
    }
    let path = root.join("Σ.txt");
    let mut original = files_native::create_source_file(&path).unwrap();
    original
        .write_all(b"original bytes must survive the alias")
        .unwrap();
    original.sync_all().unwrap();
    let original_id = native::identity(&original).unwrap();
    drop(original);
    let alias = root.join("σ.txt");
    let issue = files_native::create_source_file(&alias).unwrap_err();
    assert_eq!(issue.code, "SOURCE_NEEDS_ADAPTATION");
    assert!(original_id.same_object(&native::open_no_reparse(&alias).unwrap().identity));
    assert_eq!(
        fs::read(&path).unwrap(),
        b"original bytes must survive the alias"
    );
    assert_eq!(
        fs::read(&alias).unwrap(),
        b"original bytes must survive the alias"
    );
}

#[test]
fn windows_source_reads_keep_single_link_and_bounded_snapshot_contracts() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let before = native::open_no_reparse(&root).unwrap();
    assert!(native::verify_private(&before.file).is_err());
    let declaration = root.join("local-app.json");
    fs::write(&declaration, b"{}").unwrap();
    assert_eq!(read_small(&declaration).unwrap(), b"{}");
    fs::hard_link(&declaration, root.join("other-link")).unwrap();
    assert!(read_small(&declaration).is_err());
    assert!(check_disk(&root).is_err());
    fs::remove_file(root.join("other-link")).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&declaration)
        .unwrap()
        .set_len(MAX_METADATA + 1)
        .unwrap();
    assert_eq!(
        read_small(&declaration).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    assert!(native::verify_private(&before.file).is_err());
}

#[test]
fn new_source_paths_are_private_exclusive_and_existing_parent_access_is_unchanged() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let parent = native::open_no_reparse(&root).unwrap();
    assert!(native::verify_private(&parent.file).is_err());
    private_dir(&root).unwrap();
    assert!(native::verify_private(&parent.file).is_err());
    let child = root.join("new/nested");
    private_dir(&child).unwrap();
    let directory = native::open_no_reparse(&child).unwrap();
    native::verify_private(&directory.file).unwrap();
    assert!(new_dir(&child).is_err());
    let path = child.join("blob.txt");
    let mut file = files_native::create_source_file(&path).unwrap();
    file.write_all(b"actual private source bytes").unwrap();
    file.sync_all().unwrap();
    files_native::verify_created_file(&path, &file).unwrap();
    native::verify_private(&file).unwrap();
    assert!(files_native::create_source_file(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"actual private source bytes");
    assert!(native::verify_private(&parent.file).is_err());
}

#[test]
fn windows_git_command_is_explicitly_isolated_and_preserves_native_arguments() {
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let tools = GitToolchain {
        git: ExecutableInfo {
            path: root.join("cmd/git.exe"),
            version: "2.56.0.windows.4".into(),
        },
        path: "selected-only;system-only".into(),
        exec_path: Some(root.join("libexec/git-core")),
        template_path: None,
    };
    let command = isolated_command(&tools, &root, &root, &root).unwrap();
    let observed = command.as_std();
    let values = observed
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.unwrap().to_os_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(values["PATH"], "selected-only;system-only");
    assert_eq!(values["GIT_CONFIG_GLOBAL"], "NUL");
    assert_eq!(values["GIT_CONFIG_SYSTEM"], "NUL");
    assert_eq!(values["GIT_ALLOW_PROTOCOL"], "https");
    assert_eq!(
        values["HOME"],
        crate::node_tool_layout::command_argument(&root).unwrap()
    );
    assert!(values.contains_key("SystemRoot"));
    let args = observed
        .get_args()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(args.iter().any(|value| value == "core.longpaths=true"));
    assert!(args.iter().any(|value| value == "core.autocrlf=false"));
    assert!(supported_version("2.56.0.windows.4"));
    assert!(!supported_version("2.56.0.windows."));
    assert!(!supported_version("2.57.0.windows.1"));
}

#[tokio::test]
#[ignore = "Opt-in pinned PANDA_W4_GIT_SAMPLE_ROOT; entirely disposable offline Git repository"]
async fn actual_windows_git_blobs_fsck_index_and_required_filters_are_non_executing() {
    let selected = PathBuf::from(
        std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT").expect("explicit pinned MinGit sample"),
    );
    crate::managed_git::verify(selected.clone()).await.unwrap();
    let temporary = sample();
    let root = temporary.path().canonicalize().unwrap();
    let identity = crate::file_identity::PersistedIdentity::capture(&root).unwrap();
    let repository = root.join("repository");
    let home = root.join("home");
    let template = root.join("template");
    for path in [&repository, &home, &template] {
        new_dir(path).unwrap();
    }
    let (stop, _) = watch::channel(false);
    let (cancel, _) = watch::channel(false);
    let writer = crate::durable_writer::WriterOwner::platform(&root).unwrap();
    let processes =
        crate::owned_process::OwnedProcessRegistry::default().with_writer_owner(writer.clone());
    let mut control = RunControl {
        stop: stop.subscribe(),
        cancel: cancel.subscribe(),
        deadline: Instant::now() + Duration::from_secs(60),
        monitor: root.clone(),
        processes: processes.clone(),
        scope: None,
        hooks: Default::default(),
    };
    let environment = GitEnvironmentManager::with_processes(
        root.join("platform-data"),
        Some(selected),
        processes.clone(),
    );
    let tools = resolve_source_git(&environment, &control).await.unwrap();
    let mut init = isolated_command(&tools, &repository, &home, &template).unwrap();
    init.args(["init", "--initial-branch=main"]);
    execute(init, &mut control, MAX_OUTPUT).await.unwrap();
    fs::write(
        repository.join(".gitattributes"),
        "*.txt filter=must-not-run\n",
    )
    .unwrap();
    fs::write(repository.join("safe.txt"), "中文 raw materialized bytes\n").unwrap();
    fs::write(repository.join("script.sh"), "#!/bin/sh\nexit 99\n").unwrap();
    let mut add = isolated_command(&tools, &repository, &home, &template).unwrap();
    add.args(["add", "--", ".gitattributes", "safe.txt", "script.sh"]);
    execute(add, &mut control, MAX_OUTPUT).await.unwrap();
    let mut commit = isolated_command(&tools, &repository, &home, &template).unwrap();
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
    let mut fsck = isolated_command(&tools, &repository, &home, &template).unwrap();
    fsck.args(["fsck", "--full", "--strict", "--no-reflogs"]);
    execute(fsck, &mut control, MAX_OUTPUT).await.unwrap();
    let mut list = isolated_command(&tools, &repository, &home, &template).unwrap();
    list.args(["ls-tree", "-r", "-z", "--long", "HEAD"]);
    let entries = parse_tree(&execute(list, &mut control, MAX_TREE_OUTPUT).await.unwrap()).unwrap();
    for args in [
        vec![
            "config",
            "filter.must-not-run.smudge",
            "must-not-execute-filter-command",
        ],
        vec!["config", "filter.must-not-run.required", "true"],
    ] {
        let mut config = isolated_command(&tools, &repository, &home, &template).unwrap();
        config.args(args);
        execute(config, &mut control, MAX_OUTPUT).await.unwrap();
    }
    for name in [".gitattributes", "safe.txt", "script.sh"] {
        fs::remove_file(repository.join(name)).unwrap();
    }
    let digest = materialize(
        &tools,
        &repository,
        &home,
        &template,
        &entries,
        &mut control,
    )
    .await
    .unwrap();
    assert_eq!(directory_digest(&repository).unwrap(), digest);
    let mut index = isolated_command(&tools, &repository, &home, &template).unwrap();
    index.args(["read-tree", "HEAD"]);
    execute(index, &mut control, MAX_OUTPUT).await.unwrap();
    assert_eq!(
        fs::read_to_string(repository.join("safe.txt")).unwrap(),
        "中文 raw materialized bytes\n"
    );
    assert!(processes.shutdown().await.is_empty());
    assert!(processes
        .snapshot()
        .iter()
        .all(|process| process.resource_closed()));
    crate::durable_writer::require_owner_clear(&writer).unwrap();
    drop(control);
    drop(environment);
    crate::history_cleanup::remove_owned_directory(&root, &identity).unwrap();
    assert!(!root.exists());
}
