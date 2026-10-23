use super::*;
use std::os::unix::fs::symlink;

fn source() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-node-source");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("local-app.json"),
        r#"{"id":"disposable-node-source"}"#,
    )
    .unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"name":"disposable-node-source","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(root.join("src/server.mjs"), "console.log('first');\n").unwrap();
    (temp, root)
}

#[test]
fn scope_keeps_node_inputs_and_excludes_private_runtime_material() {
    let (_temp, root) = source();
    for directory in [
        "public",
        "dist",
        "data",
        "config",
        "credentials",
        "node_modules",
        "release",
    ] {
        fs::create_dir(root.join(directory)).unwrap();
        fs::write(root.join(directory).join("example.json"), "{}").unwrap();
    }
    fs::write(root.join(".env.local"), "DISPOSABLE_SECRET=do-not-save").unwrap();
    let scan = scan_node(&root).unwrap();
    assert!(scan.files.contains_key("dist/example.json"));
    assert!(scan.files.contains_key("public/example.json"));
    assert!(scan.files.contains_key("local-app.json"));
    assert!(!scan.files.keys().any(|path| node_protected(path)));
    assert_eq!(scan.excluded.len(), 6);
    assert!(scan.issues.is_empty());
    assert!(scan.unclassified.is_empty());
}

#[test]
fn unknown_roots_and_linked_source_are_explicitly_blocked() {
    let (temp, root) = source();
    fs::create_dir(root.join("unclassified-material")).unwrap();
    fs::write(temp.path().join("outside.txt"), "do not read").unwrap();
    symlink(temp.path().join("outside.txt"), root.join("src/link.txt")).unwrap();
    let scan = scan_node(&root).unwrap();
    assert_eq!(scan.unclassified, ["unclassified-material"]);
    assert_eq!(scan.issues.len(), 1);
    assert!(!scan.files.contains_key("src/link.txt"));
    assert!(scan_node(&root.join("..")).is_err());
}

#[test]
fn root_and_nested_git_links_never_become_source() {
    let (temp, root) = source();
    symlink(temp.path(), root.join("src/.git")).unwrap();
    let scan = scan_node(&root).unwrap();
    assert_eq!(scan.markers.len(), 1);
    assert_eq!(scan.markers[0].kind, "unsafe");
    assert!(!scan.files.contains_key("src/.git"));
    symlink(&root, temp.path().join("root-link")).unwrap();
    assert!(scan_node(&temp.path().join("root-link")).is_err());
}

#[test]
fn source_snapshot_changes_for_bytes_modes_and_scope() {
    let (_temp, root) = source();
    let initial = node_source_digest(&scan_node(&root).unwrap()).unwrap();
    fs::write(root.join("src/server.mjs"), "console.log('second');\n").unwrap();
    let changed = node_source_digest(&scan_node(&root).unwrap()).unwrap();
    assert_ne!(initial, changed);
    fs::set_permissions(
        root.join("src/server.mjs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_ne!(
        changed,
        node_source_digest(&scan_node(&root).unwrap()).unwrap()
    );
}

#[test]
fn declared_build_output_is_excluded_even_when_named_like_source_input() {
    let (_temp, root) = source();
    fs::write(root.join("local-app.json"), r#"{"development":{"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"dist"}}"#).unwrap();
    fs::create_dir(root.join("dist")).unwrap();
    fs::write(root.join("dist/server.mjs"), "generated output").unwrap();
    let scan = scan_node(&root).unwrap();
    assert!(!scan.files.contains_key("dist/server.mjs"));
    assert_eq!(scan.output_dir.as_deref(), Some("dist"));
    assert!(scan.excluded.iter().any(|entry| entry.path == "dist"));
}

#[test]
fn incomplete_git_operation_journals_require_explicit_inspection() {
    let (_temp, root) = source();
    let git_dir = root.join(".git");
    let operation = git_dir.join("local-projects-node-operations/disposable-interruption");
    fs::create_dir_all(&operation).unwrap();
    fs::write(
        operation.join("journal.json"),
        r#"{"schemaVersion":1,"phase":"prepared"}"#,
    )
    .unwrap();
    let mut issues = vec![];
    pending_node_operations(&git_dir, &mut issues).unwrap();
    assert_eq!(issues[0].code, "NODE_GIT_RECOVERY_REQUIRED");
    fs::write(
        operation.join("journal.json"),
        r#"{"schemaVersion":1,"phase":"completed"}"#,
    )
    .unwrap();
    let mut issues = vec![];
    pending_node_operations(&git_dir, &mut issues).unwrap();
    assert!(issues.is_empty());
}

#[tokio::test]
async fn runtime_stop_prevents_git_resolution_or_source_operation() {
    let (temp, root) = source();
    let (stop, receiver) = watch::channel(true);
    let manager = NodeGit::new(GitEnvironmentManager::new(
        temp.path().canonicalize().unwrap(),
        None,
    ))
    .with_stop(receiver);
    assert_eq!(
        manager.status(&root).await.unwrap_err().code,
        "PROJECT_GIT_INTERRUPTED"
    );
    assert!(!root.join(".git").exists());
    drop(stop);
}

async fn fixture_git(temp: &Path) -> (NodeGit, GitToolchain) {
    let git_root = std::env::var_os("LOCAL_PROJECTS_TEST_GIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../apps/desktop/src-tauri/resources/development-git")
        });
    let environment = GitEnvironmentManager::new(
        temp.canonicalize().unwrap(),
        Some(git_root.canonicalize().unwrap()),
    );
    let tools = environment.resolve().await.unwrap();
    (NodeGit::new(environment), tools)
}

fn save_input(view: &ProjectGitStatus) -> GitSaveInput {
    GitSaveInput {
        expected_head: head_oid(view).map(str::to_owned),
        expected_snapshot: view.snapshot_digest.clone(),
        message: "Save disposable Node source".into(),
        author: GitAuthor {
            name: "Disposable Node Test".into(),
            email: "disposable@example.invalid".into(),
        },
    }
}

async fn initialize(manager: &NodeGit, root: &Path) -> ProjectGitStatus {
    let view = manager.status(root).await.unwrap();
    assert!(view.can_initialize, "{view:?}");
    let done = manager
        .initialize(
            root,
            GitInitInput {
                expected_snapshot: view.snapshot_digest,
                initial_branch: "main".into(),
            },
        )
        .await
        .unwrap();
    serde_json::from_value(done["status"].clone()).unwrap()
}

/// Uses only the explicit verified private Git payload and disposable sources.
#[tokio::test]
#[ignore = "requires the packaged private Git payload; set LOCAL_PROJECTS_TEST_GIT_ROOT when elsewhere"]
async fn local_versions_preserve_existing_history_remotes_data_and_never_run_hooks() {
    let (temp, root) = source();
    let (manager, tools) = fixture_git(temp.path()).await;
    let initial = initialize(&manager, &root).await;
    assert!(initial.can_save);
    fs::create_dir(root.join(".git/hooks")).unwrap();
    fs::write(
        root.join(".git/hooks/pre-commit"),
        "#!/bin/sh\ntouch ../hook-ran\n",
    )
    .unwrap();
    fs::set_permissions(
        root.join(".git/hooks/pre-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(
        root.join(".git/hooks/reference-transaction"),
        "#!/bin/sh\ntouch ../reference-hook-ran\n",
    )
    .unwrap();
    fs::set_permissions(
        root.join(".git/hooks/reference-transaction"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    // A user's remote configuration remains byte-for-byte intact. No operation
    // below invokes fetch, push, SSH, credential helpers or commit hooks.
    let config = root.join(".git/config");
    let mut bytes = fs::read(&config).unwrap();
    bytes.extend_from_slice(b"\n[remote \"origin\"]\n\turl = https://example.invalid/disposable-node.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n");
    fs::write(&config, &bytes).unwrap();
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/database.db"), b"disposable database bytes").unwrap();
    fs::write(root.join(".env"), b"SYNTHETIC_SECRET=must-not-save").unwrap();
    fs::write(root.join("yarn.lock"), "# Disposable lockfile\n").unwrap();
    fs::write(root.join(".env.example"), "EXAMPLE_API_KEY=fill-your-own\n").unwrap();
    fs::create_dir(root.join("work")).unwrap();
    fs::write(
        root.join("work/disposable-private-note.md"),
        "do not include",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "work/\n").unwrap();
    let preview = manager.status(&root).await.unwrap();
    assert!(preview
        .scope
        .excluded
        .iter()
        .any(|entry| entry.path == "work"));
    assert!(preview
        .scope
        .included_paths
        .contains(&".env.example".to_owned()));
    assert!(!preview
        .scope
        .included_paths
        .contains(&"work/disposable-private-note.md".to_owned()));
    let done = manager.save(&root, save_input(&preview)).await.unwrap();
    let first = done["commit"].as_str().unwrap().to_owned();
    let clean = manager.status(&root).await.unwrap();
    assert!(clean.changes.is_empty(), "{clean:?}");
    assert!(!root.join("hook-ran").exists());
    assert!(!root.join("reference-hook-ran").exists());
    assert_eq!(fs::read(&config).unwrap(), bytes);
    assert_eq!(
        fs::read(root.join("data/database.db")).unwrap(),
        b"disposable database bytes"
    );
    let history = manager.versions(&root, 10).await.unwrap();
    assert_eq!(history.versions.len(), 1);
    assert_eq!(history.versions[0].oid, first);
    fs::write(root.join("src/server.mjs"), "console.log('second');\n").unwrap();
    assert_eq!(
        manager
            .save(&root, save_input(&clean))
            .await
            .unwrap_err()
            .code,
        "PROJECT_GIT_STALE_PREVIEW"
    );
    let second = manager.status(&root).await.unwrap();
    manager.save(&root, save_input(&second)).await.unwrap();
    let history = manager.versions(&root, 1).await.unwrap();
    assert!(history.has_more);
    assert_eq!(history.versions[0].parents, [first]);
    let clean = manager.status(&root).await.unwrap();
    assert_eq!(
        clean.can_create_branch,
        branch_version_supported(&tools.git.version)
    );
    if clean.can_create_branch {
        let head = clean.repository.head.as_ref().unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let branch = manager
            .create_branch(
                &root,
                GitCreateBranchInput {
                    name: "custom/disposable".into(),
                    expected_head: head.oid.clone().unwrap(),
                    expected_branch: head.branch.clone().unwrap(),
                    expected_snapshot: clean.snapshot_digest.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(branch["branch"], "refs/heads/custom/disposable");
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert!(!root.join("reference-hook-ran").exists());
    }
}

#[tokio::test]
#[ignore = "requires the packaged private Git payload"]
async fn protection_filters_nested_repositories_and_stale_head_fail_closed() {
    let (temp, root) = source();
    let (manager, tools) = fixture_git(temp.path()).await;
    let initial = initialize(&manager, &root).await;
    manager.save(&root, save_input(&initial)).await.unwrap();
    let clean = manager.status(&root).await.unwrap();
    let mut stale_input = save_input(&clean);
    stale_input.expected_head = Some("1".repeat(40));
    assert_eq!(
        manager.save(&root, stale_input).await.unwrap_err().code,
        "PROJECT_GIT_STALE_PREVIEW"
    );
    fs::write(root.join(".gitattributes"), "*.mjs filter=unsafe\n").unwrap();
    let config = root.join(".git/config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("\n[filter \"unsafe\"]\n\tclean = touch filter-ran\n\trequired = true\n");
    fs::write(config, text).unwrap();
    let filtered = manager.status(&root).await.unwrap();
    assert!(!filtered.can_save);
    assert!(filtered
        .issues
        .iter()
        .any(|e| e.code == "PROJECT_GIT_ATTRIBUTES_UNSUPPORTED"));
    assert!(!root.join("filter-ran").exists());
    fs::remove_file(root.join(".gitattributes")).unwrap();
    // Deliberately stage a synthetic protected path using the already-isolated
    // low-level test command; no real application source is involved.
    fs::create_dir(root.join("data")).unwrap();
    fs::write(root.join("data/sample.json"), "{}").unwrap();
    let (mut context, _senders) = node_context().unwrap();
    let mut command = node_command(&tools, context.root(), Some(&root));
    command.args(["add", "--force", "--", "data/sample.json"]);
    context
        .run(command, vec![])
        .await
        .unwrap()
        .success()
        .unwrap();
    let protected = manager.status(&root).await.unwrap();
    assert!(!protected.can_save);
    assert_eq!(
        protected.scope.protected_tracked_paths,
        ["data/sample.json"]
    );
    fs::create_dir(root.join("src/.git")).unwrap();
    let nested = manager.status(&root).await.unwrap();
    assert_eq!(nested.repository.state, "ambiguous");
    assert!(!nested.can_save);
}

#[tokio::test]
#[ignore = "requires the packaged private Git payload"]
async fn initialization_rejects_changed_preview_and_never_wraps_an_ancestor_repo() {
    let (temp, root) = source();
    let (manager, _) = fixture_git(temp.path()).await;
    let preview = manager.status(&root).await.unwrap();
    fs::write(root.join("src/server.mjs"), "changed after preview").unwrap();
    let issue = manager
        .initialize(
            &root,
            GitInitInput {
                expected_snapshot: preview.snapshot_digest,
                initial_branch: "main".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(issue.code, "PROJECT_GIT_STALE_PREVIEW");
    assert!(!root.join(".git").exists());
    fs::create_dir(temp.path().join(".git")).unwrap();
    let ancestor = manager.status(&root).await.unwrap();
    assert!(!ancestor.can_initialize);
    assert_eq!(ancestor.repository.state, "unsupported");
    assert!(!root.join(".git").exists());
    fs::remove_dir(temp.path().join(".git")).unwrap();
    initialize(&manager, &root).await;
    fs::create_dir(temp.path().join(".git")).unwrap();
    let explicit = manager.status(&root).await.unwrap();
    assert_eq!(explicit.repository.state, "project-root");
    assert!(
        explicit.can_save,
        "a bound own repository must not inherit an outer repository: {explicit:?}"
    );
    fs::create_dir(root.join("src/.git")).unwrap();
    assert_eq!(
        manager.status(&root).await.unwrap().repository.state,
        "ambiguous"
    );
}
