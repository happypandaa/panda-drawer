//! Local source versions in a Node application's one authoritative repository.
//! Callers hold the Node source-binding/operation lease throughout each call.
//! This shares the web Git process isolation and transactional plumbing, but uses
//! a Node source policy: it never manufactures a web project or another Git root.
use super::*;

const NODE_POLICY: &str = "node-source-v1";

#[path = "node_development_restore.rs"]
mod restore;
pub(crate) use restore::{
    require_clear as require_source_restore_clear,
    require_control_clear as require_source_restore_control_clear,
};
pub use restore::{
    NodeRestoreApplyInput, NodeRestorePreviewInput, NodeRestoreRecoverInput, NodeRestoreScope,
};

pub struct NodeGit {
    environment: GitEnvironmentManager,
    stop: Option<watch::Receiver<bool>>,
    process_scope: Option<(
        crate::owned_process::OwnedProcessRegistry,
        Arc<dyn Send + Sync>,
    )>,
}

impl NodeGit {
    pub fn new(environment: GitEnvironmentManager) -> Self {
        Self {
            environment,
            stop: None,
            process_scope: None,
        }
    }

    pub fn with_stop(mut self, stop: watch::Receiver<bool>) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Bind only the current operation's source/task owner. A long-lived
    /// manager must not retain a phase read gate across later publication.
    pub(crate) fn with_process_scope(
        mut self,
        processes: crate::owned_process::OwnedProcessRegistry,
        owner: Arc<dyn Send + Sync>,
    ) -> Self {
        self.process_scope = Some((processes, owner));
        self
    }

    fn context(&self) -> Result<(Context, [watch::Sender<bool>; 2]), ApiError> {
        let (mut context, senders) = node_context()?;
        if let Some(stop) = &self.stop {
            context.stop = stop.clone();
        }
        if let Some((processes, owner)) = &self.process_scope {
            context.bind_keep_alive(processes.clone(), owner.clone());
        }
        Ok((context, senders))
    }

    pub async fn status(&self, root: &Path) -> Result<ProjectGitStatus, ApiError> {
        let (mut context, _senders) = self.context()?;
        match context.resolve(&self.environment).await {
            Ok(tools) => Ok(inspect_node(root, Some(&tools), &mut context, None)
                .await?
                .view),
            Err(issue)
                if issue.code.starts_with("PROCESS_")
                    || issue.details["processResourceClosed"] == false =>
            {
                Err(issue)
            }
            Err(issue) => {
                let mut snapshot = inspect_node(root, None, &mut context, None).await?;
                snapshot.view.issues.push(issue);
                finalize_capabilities(&mut snapshot.view);
                Ok(snapshot.view)
            }
        }
    }

    pub async fn initialize(&self, root: &Path, input: GitInitInput) -> Result<Value, ApiError> {
        validate_digest(&input.expected_snapshot)?;
        if input.initial_branch != "main" {
            return Err(input_error());
        }
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        let snapshot = inspect_node(root, Some(&tools), &mut context, None).await?;
        expected(&snapshot, &input.expected_snapshot, None, false)?;
        if !snapshot.view.can_initialize {
            return Err(blocked(&snapshot.view));
        }
        let candidate = context.root().join("candidate");
        let template = context.root().join("empty-template");
        private_dir(&template)?;
        let mut command = git_command(&tools, context.root(), None);
        command
            .args([
                "init",
                "--quiet",
                "--initial-branch=main",
                "--object-format=sha1",
                "--template",
            ])
            .arg(&template)
            .arg(&candidate);
        context.run(command, vec![]).await?.success()?;
        private_dir(&candidate.join(".git/info"))?;
        fs::write(candidate.join(".git/info/exclude"), "/data/\n/config/\n/configuration/\n/credentials/\n/release/\n**/node_modules/\n**/.env*\n!**/.env.example\n**/.npmrc\n**/.netrc\n**/.cache/\n**/uploads/\n**/logs/\n**/*.pem\n**/*.key\n")?;
        sync_ordinary_tree(&candidate.join(".git"))?;
        let verify = inspect_node(root, Some(&tools), &mut context, None).await?;
        expected(&verify, &input.expected_snapshot, None, false)?;
        context.checkpoint()?;
        rename_new(&candidate.join(".git"), &root.join(".git"))?;
        let result = json!({"operation":"initialize","operationOutcome":"committed","repositoryRoot":root,"branch":"main","head":null});
        File::open(root)?.sync_all().map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        let status = inspect_node(root, Some(&tools), &mut context, None)
            .await
            .map_err(|issue| issue.outcome("committed").detail("result", result.clone()))?
            .view;
        Ok(with_status(result, status))
    }

    pub async fn save(&self, root: &Path, input: GitSaveInput) -> Result<Value, ApiError> {
        validate_save(&input)?;
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        let snapshot = inspect_node(root, Some(&tools), &mut context, None).await?;
        expected(
            &snapshot,
            &input.expected_snapshot,
            input.expected_head.as_deref(),
            true,
        )?;
        if !snapshot.view.can_save {
            return Err(blocked(&snapshot.view));
        }
        if snapshot.view.changes.is_empty() {
            return Err(error("PROJECT_GIT_NO_CHANGES", "没有需要保存的源码改动。"));
        }
        let branch = snapshot
            .view
            .repository
            .head
            .as_ref()
            .and_then(|head| head.branch.clone())
            .ok_or_else(input_error)?;
        let git_dir = root.join(".git");
        let mut lock = IndexLock::acquire(&git_dir.join("index.lock"))?;
        context.retain(lock.retained.clone());
        let verify = inspect_node(root, Some(&tools), &mut context, Some(&lock)).await?;
        expected(
            &verify,
            &input.expected_snapshot,
            input.expected_head.as_deref(),
            true,
        )?;
        // Retain a private operation journal and candidate index if publication
        // becomes uncertain. It is inside this existing repository, not a repo.
        let operation = git_dir
            .join("local-projects-node-operations")
            .join(uuid::Uuid::new_v4().to_string());
        private_dir(&operation)?;
        // Source filenames such as yarn.lock are ordinary inputs. Keep their
        // disposable copies outside .git so repository lock discovery cannot
        // mistake them for another Git operation's metadata locks.
        let source = context.root().join("source");
        private_dir(&source)?;
        for (path, entry) in &snapshot.files {
            context.checkpoint()?;
            let bytes = read_file(&checked_relative(root, path, false)?, MAX_BLOB)?;
            if sha256(&bytes) != entry.digest {
                return Err(stale());
            }
            let destination = source.join(path);
            private_dir(destination.parent().ok_or_else(unsafe_path)?)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if entry.mode == "100755" { 0o700 } else { 0o600 })
                .open(destination)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        let index = operation.join("candidate-index");
        let mut command = node_command(&tools, context.root(), Some(root));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["read-tree", "--empty"]);
        context.run(command, vec![]).await?.success()?;
        let hashes = node_hashes(
            &tools,
            &mut context,
            Some(root),
            &source,
            snapshot.files.keys(),
            true,
        )
        .await?;
        let mut entries = vec![];
        for (path, entry) in &snapshot.files {
            entries.extend_from_slice(
                format!(
                    "{} {}\t",
                    entry.mode,
                    hashes.get(path).ok_or_else(output_invalid)?
                )
                .as_bytes(),
            );
            entries.extend_from_slice(path.as_bytes());
            entries.push(0);
        }
        let mut command = node_command(&tools, context.root(), Some(root));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["update-index", "-z", "--index-info"]);
        context.run(command, entries).await?.success()?;
        let mut command = node_command(&tools, context.root(), Some(root));
        command.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = one_oid(&context.run(command, vec![]).await?.success()?)?;
        let mut command = node_command(&tools, context.root(), Some(root));
        command
            .env("GIT_INDEX_FILE", &index)
            .args(["commit-tree", &tree, "--no-gpg-sign"]);
        if let Some(parent) = &input.expected_head {
            command.args(["-p", parent]);
        }
        author_environment(&mut command, &input.author);
        let commit = one_oid(
            &context
                .run(command, input.message.as_bytes().to_vec())
                .await?
                .success()?,
        )?;
        let verify = inspect_node(root, Some(&tools), &mut context, Some(&lock)).await?;
        expected(
            &verify,
            &input.expected_snapshot,
            input.expected_head.as_deref(),
            true,
        )?;
        lock.write(&read_file(&index, MAX_METADATA)?)?;
        let journal = operation.join("journal.json");
        let mut record = json!({"schemaVersion":1,"operation":"save","phase":"prepared","root":root,
            "previousHead":input.expected_head,"candidateCommit":commit,"branch":branch,"snapshotDigest":input.expected_snapshot});
        atomic_json(&journal, &record)?;
        let mut command = node_command(&tools, context.root(), Some(root));
        command.args([
            "update-ref",
            "--create-reflog",
            "-m",
            "Local Projects: save Node source",
            "--stdin",
            "-z",
        ]);
        author_environment(&mut command, &input.author);
        lock.preserve = true;
        let outcome = context
            .commit_ref(command, &commit, input.expected_head.as_deref(), || {
                if read_file(&git_dir.join("HEAD"), 4096)? != format!("ref: {branch}\n").as_bytes()
                    || repository_digest(&git_dir)? != snapshot.repository_digest
                    || node_source_digest(&scan_node(root)?)? != snapshot.source_digest
                    || node_root_identity(root)? != snapshot.root_identity
                {
                    return Err(stale());
                }
                Ok(())
            })
            .await;
        if let Err(issue) = outcome {
            if issue.details["safeAbort"] == true {
                lock.preserve = false;
                record["phase"] = json!("not-committed");
                atomic_json(&journal, &record)?;
                return Err(issue.outcome("not_committed"));
            }
            let published = issue.details["commitAcknowledged"] == true;
            return Err(issue
                .outcome(if published { "committed" } else { "unknown" })
                .detail("journal", json!(journal))
                .detail("candidateCommit", commit));
        }
        record["phase"] = json!("ref-committed");
        let result = json!({"operation":"save","operationOutcome":"committed","repositoryRoot":root,"commit":commit,"parent":input.expected_head,"branch":branch,"message":input.message,"author":input.author});
        if atomic_json(&journal, &record).is_err() || lock.publish(&git_dir.join("index")).is_err()
        {
            return Err(recovery()
                .outcome("committed")
                .detail("result", result)
                .detail("journal", json!(journal)));
        }
        record["phase"] = json!("completed");
        atomic_json(&journal, &record).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        // Only this operation's disposable snapshots are removed after success.
        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_file(index);
        let status = inspect_node(root, Some(&tools), &mut context, None)
            .await
            .map_err(|issue| issue.outcome("committed").detail("result", result.clone()))?
            .view;
        Ok(with_status(result, status))
    }

    pub async fn versions(&self, root: &Path, limit: u32) -> Result<ProjectGitVersions, ApiError> {
        if !(1..=100).contains(&limit) {
            return Err(input_error());
        }
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        let snapshot = inspect_node(root, Some(&tools), &mut context, None).await?;
        if !snapshot.view.can_list_versions {
            return Err(blocked(&snapshot.view));
        }
        let head = snapshot
            .view
            .repository
            .head
            .clone()
            .ok_or_else(output_invalid)?;
        let mut command = node_command(&tools, context.root(), Some(root));
        command
            .args([
                "log",
                "--no-show-signature",
                "--no-decorate",
                "--no-notes",
                "--no-use-mailmap",
                "--encoding=UTF-8",
                "-z",
                "--format=%H%x00%P%x00%an%x00%ae%x00%aI%x00%s",
            ])
            .arg(format!("--max-count={}", limit + 1))
            .arg(head.oid.as_ref().ok_or_else(output_invalid)?)
            .arg("--");
        let mut versions = parse_versions(
            &context.run(command, vec![]).await?.success()?,
            limit as usize + 1,
        )?;
        if versions.first().map(|version| &version.oid) != head.oid.as_ref() {
            return Err(output_invalid());
        }
        let verify = inspect_node(root, Some(&tools), &mut context, None).await?;
        expected(
            &verify,
            &snapshot.view.snapshot_digest,
            head.oid.as_deref(),
            true,
        )?;
        let has_more = versions.len() > limit as usize;
        versions.truncate(limit as usize);
        Ok(ProjectGitVersions {
            repository_root: root.to_path_buf(),
            head,
            versions,
            has_more,
            limit,
            git: tools.git,
            database: node_boundary(),
        })
    }

    pub async fn create_branch(
        &self,
        root: &Path,
        input: GitCreateBranchInput,
    ) -> Result<Value, ApiError> {
        validate_branch_name(&input.name)?;
        validate_digest(&input.expected_snapshot)?;
        if !valid_oid(&input.expected_head)
            || one_ref(format!("{}\n", input.expected_branch).as_bytes()).is_err()
        {
            return Err(input_error());
        }
        let (mut context, _senders) = self.context()?;
        let tools = context.resolve(&self.environment).await?;
        if !branch_version_supported(&tools.git.version) {
            return Err(branch_version_issue());
        }
        let snapshot = inspect_node(root, Some(&tools), &mut context, None).await?;
        expected(
            &snapshot,
            &input.expected_snapshot,
            Some(&input.expected_head),
            true,
        )?;
        if snapshot
            .view
            .repository
            .head
            .as_ref()
            .and_then(|head| head.branch.as_deref())
            != Some(&input.expected_branch)
        {
            return Err(stale());
        }
        if !snapshot.view.can_create_branch {
            return Err(blocked(&snapshot.view));
        }
        let git_dir = root.join(".git");
        let mut index_lock = IndexLock::acquire(&git_dir.join("index.lock"))?;
        context.retain(index_lock.retained.clone());
        let verify = inspect_node(root, Some(&tools), &mut context, Some(&index_lock)).await?;
        expected(
            &verify,
            &input.expected_snapshot,
            Some(&input.expected_head),
            true,
        )?;
        let old_lock = Arc::new(RefLock::acquire(
            &git_dir.join(format!("{}.lock", input.expected_branch)),
        )?);
        context.retain(old_lock.clone());
        let branch = format!("refs/heads/{}", input.name);
        let mut command = node_command(&tools, context.root(), Some(root));
        command.args(["show-ref", "--verify", "--quiet", &branch]);
        match context.run(command, vec![]).await?.code {
            1 => (),
            0 => return Err(error("PROJECT_GIT_BRANCH_EXISTS", "分支已存在。")),
            _ => return Err(git_failed()),
        }
        let operation = git_dir
            .join("local-projects-node-operations")
            .join(uuid::Uuid::new_v4().to_string());
        private_dir(&operation)?;
        let journal = operation.join("journal.json");
        let mut record = json!({"schemaVersion":1,"operation":"create-branch","phase":"prepared","root":root,"previousBranch":input.expected_branch,"branch":branch,"head":input.expected_head,"snapshotDigest":input.expected_snapshot});
        atomic_json(&journal, &record)?;
        let mut command = node_command(&tools, context.root(), Some(root));
        command.args([
            "update-ref",
            "--create-reflog",
            "-m",
            "Local Projects: create Node source branch",
            "--stdin",
            "-z",
        ]);
        author_environment(
            &mut command,
            &GitAuthor {
                name: "Local Projects".into(),
                email: "local-projects@localhost".into(),
            },
        );
        let commands = format!("start\0create {branch}\0{}\0option no-deref\0symref-update HEAD\0{branch}\0oid\0{}\0prepare\0", input.expected_head, input.expected_head).into_bytes();
        index_lock.preserve = true;
        old_lock.preserve.store(true, Ordering::Release);
        let outcome = context
            .commit_transaction(command, commands, || {
                if !old_lock.matches()
                    || !index_lock.matches(&index_lock.path)
                    || index_fingerprint(&git_dir.join("index"))? != index_lock.original_index
                    || read_file(&git_dir.join("HEAD"), 4096)?
                        != format!("ref: {}\n", input.expected_branch).as_bytes()
                    || repository_digest(&git_dir)? != snapshot.repository_digest
                    || node_source_digest(&scan_node(root)?)? != snapshot.source_digest
                    || node_root_identity(root)? != snapshot.root_identity
                {
                    return Err(stale());
                }
                Ok(())
            })
            .await;
        if let Err(issue) = outcome {
            if issue.details["safeAbort"] == true {
                index_lock.preserve = false;
                old_lock.preserve.store(false, Ordering::Release);
                record["phase"] = json!("not-committed");
                atomic_json(&journal, &record)?;
                return Err(issue.outcome("not_committed"));
            }
            index_lock.preserve = true;
            old_lock.preserve.store(true, Ordering::Release);
            let published = issue.details["commitAcknowledged"] == true;
            return Err(issue
                .outcome(if published { "committed" } else { "unknown" })
                .detail("journal", json!(journal)));
        }
        record["phase"] = json!("completed");
        let result = json!({"operation":"create-branch","operationOutcome":"committed","repositoryRoot":root,"head":input.expected_head,"previousBranch":input.expected_branch,"branch":branch,"name":input.name});
        atomic_json(&journal, &record).map_err(|_| {
            recovery()
                .outcome("committed")
                .detail("result", result.clone())
        })?;
        index_lock.preserve = false;
        old_lock.preserve.store(false, Ordering::Release);
        drop(old_lock);
        drop(index_lock);
        // All processes have actually closed. Release the context's retained
        // lock FDs before inspecting the final view, so our own locks cannot
        // make a successful branch appear permanently busy.
        drop(context);
        let (mut context, _status_senders) = self
            .context()
            .map_err(|issue| issue.outcome("committed").detail("result", result.clone()))?;
        let status = inspect_node(root, Some(&tools), &mut context, None)
            .await
            .map_err(|issue| issue.outcome("committed").detail("result", result.clone()))?
            .view;
        Ok(with_status(result, status))
    }
}

fn with_status(mut result: Value, status: ProjectGitStatus) -> Value {
    result["status"] = json!(status);
    result
}
fn node_boundary() -> Value {
    json!({"state":"not-supported","reason":"仅管理源码；数据、密钥和运行目录不纳入 Git。"})
}
fn node_context() -> Result<(Context, [watch::Sender<bool>; 2]), ApiError> {
    let (stop, receiver) = watch::channel(false);
    let (cancel, cancelled) = watch::channel(false);
    Ok((Context::new(receiver, cancelled)?, [stop, cancel]))
}
fn node_command(tools: &GitToolchain, private: &Path, root: Option<&Path>) -> Command {
    let mut command = git_command(tools, private, None);
    if let Some(root) = root {
        command
            .current_dir(root)
            .arg(format!("--git-dir={}", root.join(".git").display()))
            .arg(format!("--work-tree={}", root.display()));
    }
    command
}
fn expected(
    snapshot: &NodeSnapshot,
    digest: &str,
    head: Option<&str>,
    compare_head: bool,
) -> Result<(), ApiError> {
    if snapshot.view.snapshot_digest != digest || (compare_head && head_oid(&snapshot.view) != head)
    {
        Err(stale())
    } else {
        Ok(())
    }
}
fn validate_save(input: &GitSaveInput) -> Result<(), ApiError> {
    validate_digest(&input.expected_snapshot)?;
    if input
        .expected_head
        .as_ref()
        .is_some_and(|oid| !valid_oid(oid))
        || input.message.trim().is_empty()
        || input.message.len() > 4000
        || input
            .message
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        || input.author.name.trim().is_empty()
        || input.author.name.len() > 200
        || input
            .author
            .name
            .chars()
            .any(|c| c.is_control() || "<>".contains(c))
        || input.author.email.len() > 254
        || !input.author.email.contains('@')
        || input
            .author
            .email
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>".contains(c))
    {
        return Err(input_error());
    }
    Ok(())
}

struct NodeScan {
    files: BTreeMap<String, SourceFile>,
    entries: BTreeSet<String>,
    excluded: Vec<GitExcludedPath>,
    unclassified: Vec<String>,
    markers: Vec<GitMarker>,
    issues: Vec<ApiError>,
    output_dir: Option<String>,
}
struct NodeSnapshot {
    view: ProjectGitStatus,
    files: BTreeMap<String, SourceFile>,
    source_digest: String,
    repository_digest: String,
    root_identity: String,
}

fn node_root_identity(root: &Path) -> Result<String, ApiError> {
    if !root.is_absolute()
        || root
            .to_str()
            .is_none_or(|p| p.contains('\\') || p.contains(':') || p.chars().any(char::is_control))
        || root
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(unsafe_path());
    }
    check_no_symlinks(root, false)?;
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(unsafe_path());
    }
    identity(root)
}

fn node_protected(path: &str) -> bool {
    path.split('/').any(|part| {
        let lower = part.to_ascii_lowercase();
        matches!(
            lower.as_str(),
            "data"
                | ".data"
                | "config"
                | "configuration"
                | "credentials"
                | "secrets"
                | "config.json"
                | "configuration.json"
                | "credentials.json"
                | "secrets.json"
                | "node_modules"
                | ".platform"
                | "release"
                | ".cache"
                | "cache"
                | ".vite"
                | ".next"
                | ".nuxt"
                | "coverage"
                | ".ssh"
                | "uploads"
                | "attachments"
                | "storage"
                | "logs"
                | "runs"
                | "backups"
                | "trash"
                | ".hg"
                | ".svn"
                | ".npmrc"
                | ".netrc"
                | ".ds_store"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa" // Only this conventional, public configuration template is source.
                           // It is not a content scanner: authors must never put real keys here.
        ) || (lower.starts_with(".env") && lower != ".env.example")
            || lower.ends_with(".local.json")
            || lower.ends_with("-wal")
            || lower.ends_with("-shm")
            || lower.ends_with("-journal")
            || [
                ".pem", ".key", ".p12", ".pfx", ".sqlite", ".sqlite3", ".db", ".log",
            ]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
    })
}
fn node_included(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or("");
    if path.contains('/') {
        return matches!(
            first,
            "src"
                | "app"
                | "server"
                | "client"
                | "web"
                | "frontend"
                | "backend"
                | "public"
                | "dist"
                | "assets"
                | "components"
                | "lib"
                | "scripts"
                | "docs"
                | "test"
                | "tests"
                | "packages"
                | "shared"
                | "types"
                | "bin"
                | ".agents"
                | ".github"
        );
    }
    matches!(
        first,
        "local-app.json"
            | "package.json"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | ".gitignore"
            | ".gitattributes"
            | ".editorconfig"
            | ".nvmrc"
            | ".node-version"
            | ".env.example"
            | "LICENSE"
            | "Dockerfile"
            | "Makefile"
    ) || Path::new(first)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension,
                "js" | "mjs"
                    | "cjs"
                    | "ts"
                    | "mts"
                    | "cts"
                    | "tsx"
                    | "jsx"
                    | "html"
                    | "css"
                    | "scss"
                    | "md"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "toml"
                    | "sh"
            )
        })
}

pub(crate) fn source_path_allowed(path: &str) -> bool {
    !node_protected(path) && node_included(path)
}
fn scan_node(root: &Path) -> Result<NodeScan, ApiError> {
    node_root_identity(root)?;
    let manifest: Value =
        serde_json::from_slice(&read_file(&root.join("local-app.json"), MAX_METADATA)?).map_err(
            |_| {
                error(
                    "NODE_GIT_MANIFEST_INVALID",
                    "源码目录的 local-app.json 无法读取。",
                )
            },
        )?;
    let output_dir = match manifest.get("development") {
        None | Some(Value::Null) => None,
        Some(development) => {
            let declaration: crate::node_app_manifest::NodeDevelopmentDecl =
                serde_json::from_value(development.clone())
                    .map_err(|_| error("NODE_GIT_MANIFEST_INVALID", "开发构建配置无效。"))?;
            declaration
                .validate()
                .map_err(|_| error("NODE_GIT_MANIFEST_INVALID", "开发构建配置无效。"))?;
            (declaration.output_dir != ".").then_some(declaration.output_dir)
        }
    };
    let mut scan = NodeScan {
        files: BTreeMap::new(),
        entries: BTreeSet::new(),
        excluded: vec![],
        unclassified: vec![],
        markers: vec![],
        issues: vec![],
        output_dir,
    };
    fn walk(
        root: &Path,
        dir: &Path,
        scan: &mut NodeScan,
        count: &mut usize,
        total: &mut u64,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 64 {
            return Err(limit());
        }
        check_no_symlinks(dir, false)?;
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            *count += 1;
            if *count > MAX_FILES {
                return Err(limit());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| unsafe_path())?
                .to_str()
                .ok_or_else(unsafe_path)?
                .to_owned();
            valid_relative(&relative)?;
            scan.entries.insert(relative.clone());
            let metadata = fs::symlink_metadata(&path)?;
            if path.file_name().is_some_and(|name| name == ".git") {
                if depth > 0 {
                    marker(&path, "source", &mut scan.markers)?;
                }
                continue;
            }
            if node_protected(&relative)
                || scan.output_dir.as_ref().is_some_and(|output| {
                    relative == *output || relative.starts_with(&format!("{output}/"))
                })
            {
                scan.excluded.push(GitExcludedPath {
                    path: relative,
                    reason: "本机数据、密钥、依赖或运行产物".into(),
                });
                continue;
            }
            if metadata.file_type().is_symlink()
                || (!metadata.is_dir() && (!metadata.is_file() || metadata.nlink() != 1))
            {
                scan.issues.push(unsafe_path().detail("path", relative));
                continue;
            }
            let allowed = node_included(&relative)
                || (metadata.is_dir() && node_included(&format!("{relative}/file")));
            if !allowed {
                scan.unclassified.push(relative);
                continue;
            }
            if metadata.is_dir() {
                walk(root, &path, scan, count, total, depth + 1)?;
            } else {
                if metadata.len() > MAX_BLOB {
                    return Err(limit());
                }
                *total += metadata.len();
                if *total > MAX_BYTES {
                    return Err(limit());
                }
                let bytes = read_file(&path, MAX_BLOB)?;
                scan.files.insert(
                    relative,
                    SourceFile {
                        mode: if metadata.mode() & 0o111 != 0 {
                            "100755"
                        } else {
                            "100644"
                        }
                        .into(),
                        digest: sha256(&bytes),
                        size: bytes.len() as u64,
                    },
                );
            }
        }
        Ok(())
    }
    walk(root, root, &mut scan, &mut 0, &mut 0, 0)?;
    scan.excluded.sort_by(|a, b| a.path.cmp(&b.path));
    scan.unclassified.sort();
    scan.markers.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(scan)
}
fn node_source_digest(scan: &NodeScan) -> Result<String, ApiError> {
    Ok(sha256(&serde_json::to_vec(
        &json!({"files":scan.files,"excluded":scan.excluded,"unclassified":scan.unclassified,"markers":scan.markers,"issues":scan.issues,"outputDir":scan.output_dir}),
    )?))
}

async fn inspect_node(
    root: &Path,
    tools: Option<&GitToolchain>,
    context: &mut Context,
    lock: Option<&IndexLock>,
) -> Result<NodeSnapshot, ApiError> {
    context.checkpoint()?;
    let root_identity = node_root_identity(root)?;
    let mut scan = scan_node(root)?;
    let source_digest = node_source_digest(&scan)?;
    let mut markers = scan.markers.clone();
    marker(&root.join(".git"), "project", &mut markers)?;
    for ancestor in root.ancestors().skip(1) {
        marker(&ancestor.join(".git"), "ancestor", &mut markers)?;
    }
    markers.sort_by(|a, b| a.path.cmp(&b.path));
    let root_marker = markers.iter().find(|marker| marker.scope == "project");
    let nested_marker = markers.iter().any(|marker| marker.scope == "source");
    let state = if markers.iter().any(|m| m.kind == "unsafe") {
        "unsafe"
    } else if nested_marker && markers.len() > 1 {
        "ambiguous"
    } else if root_marker.is_some_and(|marker| marker.kind == "directory") && !nested_marker {
        // A bound application's own ordinary .git is authoritative. An outer
        // repository belongs to another project and is never discovered by our
        // commands, which always pass this exact --git-dir/--work-tree pair.
        // Without an own repository, ancestors still prevent initialization.
        "project-root"
    } else if let Some(m) = markers.first() {
        if m.scope == "project" && m.kind == "directory" {
            "project-root"
        } else {
            "unsupported"
        }
    } else {
        "none"
    };
    let mut view = ProjectGitStatus {
        repository: GitRepositoryState {
            state: state.into(),
            root: (state == "project-root").then(|| root.to_path_buf()),
            markers,
            head: None,
            index_state: "not-inspected".into(),
            git: tools.map(|t| t.git.clone()),
        },
        scope: GitSourceScope {
            policy: NODE_POLICY.into(),
            included_roots: scan
                .files
                .keys()
                .filter_map(|path| path.split('/').next())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            included_paths: scan.files.keys().cloned().collect(),
            excluded: scan.excluded.clone(),
            unclassified_paths: scan.unclassified.clone(),
            protected_tracked_paths: vec![],
            complete: scan.issues.is_empty(),
        },
        changes: vec![],
        snapshot_digest: String::new(),
        can_initialize: false,
        can_save: false,
        can_list_versions: false,
        can_create_branch: false,
        branch_creation_issue: None,
        source_restore: Default::default(),
        database: node_boundary(),
        issues: std::mem::take(&mut scan.issues),
    };
    let mut heads = BTreeMap::new();
    let mut index = BTreeMap::new();
    let mut repository = String::new();
    let mut ready = state == "none";
    if !matches!(state, "none" | "project-root") {
        view.issues.push(error(
            "PROJECT_GIT_SCOPE_UNSUPPORTED",
            "源码存在父级、嵌套或非普通 Git 仓库；请直接打开原仓库处理。",
        ));
    }
    if state == "project-root" {
        let git_dir = root.join(".git");
        inspect_repository_files(&git_dir, lock, &mut view.issues)?;
        pending_node_operations(&git_dir, &mut view.issues)?;
        view.source_restore = restore::guard_state(root);
        if let Err(issue) = restore::require_clear(root) {
            view.issues.push(issue);
        }
        repository = repository_digest(&git_dir)?;
        if let Some(tools) = tools.filter(|_| view.issues.is_empty()) {
            check_config_path(&git_dir, tools, context, &mut view.issues).await?;
            ready = view.issues.is_empty();
        }
        if let Some(tools) = tools.filter(|_| ready) {
            let mut command = node_command(tools, context.root(), Some(root));
            command.args(["symbolic-ref", "--quiet", "HEAD"]);
            let output = context.run(command, vec![]).await?;
            let branch = match output.code {
                0 => Some(one_ref(&output.stdout)?),
                1 => None,
                _ => return Err(git_failed()),
            };
            let mut command = node_command(tools, context.root(), Some(root));
            command.args(["rev-parse", "--verify", "HEAD^{commit}"]);
            let output = context.run(command, vec![]).await?;
            let oid = if output.code == 0 {
                Some(one_oid(&output.stdout)?)
            } else if let Some(branch) = &branch {
                let mut command = node_command(tools, context.root(), Some(root));
                command.args(["show-ref", "--verify", "--quiet", branch]);
                if context.run(command, vec![]).await?.code != 1 {
                    return Err(git_failed());
                }
                None
            } else {
                return Err(git_failed());
            };
            view.repository.head = Some(GitHead {
                oid: oid.clone(),
                branch,
            });
            if oid.is_some() {
                let mut command = node_command(tools, context.root(), Some(root));
                command.args(["ls-tree", "-r", "-z", "--full-tree", "HEAD"]);
                heads = parse_tree(
                    &context.run(command, vec![]).await?.success()?,
                    false,
                    &mut view.issues,
                )?;
            }
            let mut command = node_command(tools, context.root(), Some(root));
            command.args(["ls-files", "--stage", "-z"]);
            index = parse_tree(
                &context.run(command, vec![]).await?.success()?,
                true,
                &mut view.issues,
            )?;
            view.repository.index_state = if index == heads {
                "matched"
            } else {
                "staged-changes"
            }
            .into();
            if index != heads {
                view.issues.push(error(
                    "PROJECT_GIT_INDEX_NOT_CLEAN",
                    "请先处理已有暂存或冲突内容。",
                ));
            }
            if view
                .repository
                .head
                .as_ref()
                .is_some_and(|head| head.branch.is_none())
            {
                view.issues
                    .push(error("PROJECT_GIT_DETACHED", "当前未位于本地分支。"));
            }
            for path in heads.keys().chain(index.keys()) {
                if node_protected(path)
                    || scan.output_dir.as_ref().is_some_and(|output| {
                        path == output || path.starts_with(&format!("{output}/"))
                    })
                {
                    view.scope.protected_tracked_paths.push(path.clone());
                } else if !node_included(path) {
                    view.scope.unclassified_paths.push(path.clone());
                }
            }
            view.scope.protected_tracked_paths.sort();
            view.scope.protected_tracked_paths.dedup();
            if !view.scope.protected_tracked_paths.is_empty() {
                view.issues.push(error(
                    "PROJECT_GIT_PROTECTED_TRACKED",
                    "仓库已跟踪数据、密钥、依赖或运行产物；请先在原 Git 中处理。",
                ));
            }
            node_repository_options(root, tools, context, &mut view.issues).await?;
            let mut untracked: Vec<_> = scan
                .files
                .keys()
                .filter(|path| !heads.contains_key(*path) && !index.contains_key(*path))
                .cloned()
                .collect();
            // Unknown top-level material is never read or silently admitted.
            // Existing ignore rules can explicitly keep it outside this repo's
            // source scope (for example Radio's private user/work directories).
            untracked.extend(
                scan.unclassified
                    .iter()
                    .filter(|path| !heads.contains_key(*path) && !index.contains_key(*path))
                    .cloned(),
            );
            if !untracked.is_empty() {
                let mut input = vec![];
                for path in &untracked {
                    input.extend_from_slice(path.as_bytes());
                    input.push(0);
                }
                let mut command = node_command(tools, context.root(), Some(root));
                command.args(["check-ignore", "-z", "--stdin"]);
                let output = context.run(command, input).await?;
                if !matches!(output.code, 0 | 1) {
                    return Err(git_failed());
                }
                for path in parse_paths(&output.stdout)? {
                    if !untracked.contains(&path) {
                        return Err(output_invalid());
                    }
                    scan.files.remove(&path);
                    view.scope
                        .unclassified_paths
                        .retain(|unclassified| unclassified != &path);
                    view.scope.excluded.push(GitExcludedPath {
                        path,
                        reason: "Git 忽略规则".into(),
                    });
                }
            }
            node_attributes(
                root,
                tools,
                context,
                scan.files.keys().chain(heads.keys()),
                &mut view.issues,
            )
            .await?;
        }
    }
    if let Some(tools) = tools.filter(|_| ready) {
        let hashes = node_hashes(
            tools,
            context,
            (state == "project-root").then_some(root),
            root,
            scan.files.keys(),
            false,
        )
        .await?;
        for (path, entry) in &scan.files {
            let target = (
                entry.mode.clone(),
                hashes.get(path).ok_or_else(output_invalid)?.clone(),
            );
            if heads.get(path) != Some(&target) {
                view.changes.push(GitChange {
                    path: path.clone(),
                    kind: if heads.contains_key(path) {
                        "modify"
                    } else {
                        "add"
                    }
                    .into(),
                    staged: index.get(path) != heads.get(path),
                });
            }
        }
        for path in heads.keys().filter(|path| !scan.files.contains_key(*path)) {
            view.changes.push(GitChange {
                path: path.clone(),
                kind: "delete".into(),
                staged: index.get(path) != heads.get(path),
            });
        }
    }
    if node_source_digest(&scan_node(root)?)? != source_digest
        || node_root_identity(root)? != root_identity
        || (state == "project-root" && repository_digest(&root.join(".git"))? != repository)
    {
        return Err(stale());
    }
    view.scope.unclassified_paths.sort();
    view.scope.unclassified_paths.dedup();
    view.scope.included_paths = scan.files.keys().cloned().collect();
    view.scope.excluded.sort_by(|a, b| a.path.cmp(&b.path));
    view.scope.complete = view.scope.complete
        && view.scope.unclassified_paths.is_empty()
        && view.scope.protected_tracked_paths.is_empty();
    if !view.scope.unclassified_paths.is_empty() {
        view.issues.push(error(
            "PROJECT_GIT_SCOPE_INCOMPLETE",
            "存在尚未归类的开发文件；请先确认源码范围。",
        ));
    }
    view.changes.sort_by(|a, b| a.path.cmp(&b.path));
    view.snapshot_digest = sha256(&serde_json::to_vec(
        &json!({"policy":NODE_POLICY,"root":root,"identity":root_identity,"source":source_digest,"repository":view.repository,"repositoryDigest":repository,"index":index,"head":heads,"scope":view.scope}),
    )?);
    finalize_capabilities(&mut view);
    Ok(NodeSnapshot {
        view,
        files: scan.files,
        source_digest,
        repository_digest: repository,
        root_identity,
    })
}

async fn node_hashes<'a>(
    tools: &GitToolchain,
    context: &mut Context,
    repository: Option<&Path>,
    root: &Path,
    paths: impl Iterator<Item = &'a String>,
    write: bool,
) -> Result<BTreeMap<String, String>, ApiError> {
    let paths: Vec<_> = paths.collect();
    if paths.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut input = vec![];
    for path in &paths {
        let full = checked_relative(root, path, false)?;
        let value = full.to_str().ok_or_else(unsafe_path)?;
        input.extend_from_slice(
            format!("\"{}\"\n", value.replace('\\', "\\\\").replace('"', "\\\"")).as_bytes(),
        );
    }
    let mut command = node_command(tools, context.root(), repository);
    command.args(["hash-object", "--no-filters", "--stdin-paths"]);
    if write {
        command.arg("-w");
    }
    let output = context.run(command, input).await?.success()?;
    let hashes: Vec<_> = std::str::from_utf8(&output)
        .map_err(|_| output_invalid())?
        .lines()
        .collect();
    if hashes.len() != paths.len() {
        return Err(output_invalid());
    }
    paths
        .into_iter()
        .zip(hashes)
        .map(|(path, oid)| {
            if valid_oid(oid) {
                Ok((path.clone(), oid.to_owned()))
            } else {
                Err(output_invalid())
            }
        })
        .collect()
}
async fn node_repository_options(
    root: &Path,
    tools: &GitToolchain,
    context: &mut Context,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    for (flag, value) in [
        ("--show-object-format", "sha1"),
        ("--show-ref-format", "files"),
    ] {
        let mut command = node_command(tools, context.root(), Some(root));
        command.args(["rev-parse", flag]);
        if context.run(command, vec![]).await?.success()? != format!("{value}\n").as_bytes() {
            issues.push(error(
                "PROJECT_GIT_REPOSITORY_UNSUPPORTED",
                "当前仓库对象或引用格式尚不支持。",
            ));
        }
    }
    let mut command = node_command(tools, context.root(), Some(root));
    command.args(["ls-files", "-v", "-z"]);
    for entry in context
        .run(command, vec![])
        .await?
        .success()?
        .split(|b| *b == 0)
        .filter(|e| !e.is_empty())
    {
        if entry.len() < 3 || entry[1] != b' ' {
            return Err(output_invalid());
        }
        if entry[0].is_ascii_lowercase() || entry[0] == b'S' {
            issues.push(error(
                "PROJECT_GIT_INDEX_UNSUPPORTED",
                "索引包含跳过或忽略更新标记。",
            ));
            break;
        }
    }
    Ok(())
}
async fn node_attributes<'a>(
    root: &Path,
    tools: &GitToolchain,
    context: &mut Context,
    paths: impl Iterator<Item = &'a String>,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    let paths: BTreeSet<_> = paths.collect();
    if paths.is_empty() {
        return Ok(());
    }
    let mut input = vec![];
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut command = node_command(tools, context.root(), Some(root));
    command.args([
        "check-attr",
        "-z",
        "--stdin",
        "filter",
        "text",
        "eol",
        "ident",
        "working-tree-encoding",
    ]);
    let output = context.run(command, input).await?.success()?;
    let fields: Vec<_> = output.split(|b| *b == 0).collect();
    if fields.last() != Some(&b"".as_slice()) || !(fields.len() - 1).is_multiple_of(3) {
        return Err(output_invalid());
    }
    if fields[..fields.len() - 1]
        .chunks(3)
        .any(|row| !matches!(row[2], b"unspecified" | b"unset"))
    {
        issues.push(error(
            "PROJECT_GIT_ATTRIBUTES_UNSUPPORTED",
            "存在内容转换属性；请在原 Git 工具中处理。",
        ));
    }
    Ok(())
}

fn pending_node_operations(git_dir: &Path, issues: &mut Vec<ApiError>) -> Result<(), ApiError> {
    let operations = git_dir.join("local-projects-node-operations");
    check_no_symlinks(&operations, true)?;
    if !operations.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(operations)? {
        let journal = entry?.path().join("journal.json");
        check_no_symlinks(&journal, true)?;
        if !journal.exists() {
            continue;
        }
        let record: Value = serde_json::from_slice(&read_file(&journal, MAX_METADATA)?)
            .map_err(|_| recovery().detail("journal", json!(journal)))?;
        if record["schemaVersion"] != 1
            || !matches!(
                record["phase"].as_str(),
                Some("completed" | "not-committed")
            )
        {
            issues.push(
                error(
                    "NODE_GIT_RECOVERY_REQUIRED",
                    "上次源码版本操作尚待核对，请先检查保留的 Git 记录。",
                )
                .detail("journal", json!(journal)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "node_development_git_tests.rs"]
mod tests;
