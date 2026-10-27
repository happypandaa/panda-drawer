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

// Restore fixtures emulate an external author using ordinary Git commands.
impl NodeGit {
    pub(crate) async fn fixture_command(&self, root: &Path, args: &[&str]) -> Value {
        let tools = self.environment.resolve().await.unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let mut command = node_command(&tools, temporary.path(), Some(root));
        command
            .args(args)
            .env("GIT_AUTHOR_NAME", "Disposable external author")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Disposable external author")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
        let output = command.output().await.unwrap();
        assert!(
            output.status.success(),
            "external Git fixture command failed: {args:?}"
        );
        json!({"stdout":String::from_utf8(output.stdout).unwrap()})
    }
    pub(crate) async fn fixture_initialize(
        &self,
        root: &Path,
        _input: GitInitInput,
    ) -> Result<Value, ApiError> {
        self.fixture_command(root, &["init", "--quiet", "--initial-branch=main"])
            .await;
        fs::write(root.join(".git/info/exclude"),"/data/\n/configuration/\n/credentials/\n/release/\n**/node_modules/\n**/.env*\n!**/.env.example\n**/.npmrc\n").unwrap();
        Ok(json!({"operationOutcome":"committed","status":self.status(root).await?}))
    }
    pub(crate) async fn fixture_save(
        &self,
        root: &Path,
        input: GitSaveInput,
    ) -> Result<Value, ApiError> {
        self.fixture_command(root, &["add", "--all", "--", "."])
            .await;
        self.fixture_command(
            root,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "--no-gpg-sign",
                "-m",
                &input.message,
            ],
        )
        .await;
        let status = self.status(root).await?;
        Ok(
            json!({"operationOutcome":"committed","commit":status.repository.head.as_ref().and_then(|head|head.oid.clone()),"status":status}),
        )
    }
}

#[path = "node_git_owned_process_tests.rs"]
mod owned_process;
