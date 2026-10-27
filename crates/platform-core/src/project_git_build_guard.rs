//! Read-only build admission, not a clean-worktree requirement or a Git repair.
//! No marker means no Git selection/probe. A marked project is checked through a
//! private index copy; Git never opens its live repository or writes its index.
use super::*;

pub(crate) struct BuildGitGuard {
    repository: Option<Repository>,
    tools: Option<GitToolchain>,
    digest: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
struct Repository {
    git_dir: PathBuf,
    identity: String,
}
impl BuildGitGuard {
    #[cfg(test)]
    pub(crate) async fn capture(
        project: &ProjectHandle,
        environment: &GitEnvironmentManager,
        stop: watch::Receiver<bool>,
    ) -> Result<Self, ApiError> {
        Self::capture_with_keep_alive(
            project,
            environment,
            stop,
            crate::owned_process::OwnedProcessRegistry::default(),
            Arc::new(()),
        )
        .await
    }
    pub(crate) async fn capture_with_keep_alive(
        project: &ProjectHandle,
        environment: &GitEnvironmentManager,
        stop: watch::Receiver<bool>,
        processes: crate::owned_process::OwnedProcessRegistry,
        keep_alive: Arc<dyn Send + Sync>,
    ) -> Result<Self, ApiError> {
        let repository = discover(project).map_err(unavailable)?;
        let Some(repo) = &repository else {
            return Ok(Self {
                repository,
                tools: None,
                digest: None,
            });
        };
        // In-progress operations are actionable even when the selected tool is
        // unavailable. Never describe a resolved-but-uncommitted merge as clean.
        repository_ready(&repo.git_dir).map_err(unavailable)?;
        let (_cancel, receiver) = watch::channel(false);
        let mut context = Context::new(stop, receiver).map_err(unavailable)?;
        context.bind_keep_alive(processes, keep_alive);
        let tools = context.resolve(environment).await.map_err(unavailable)?;
        let digest = inspect_index(repo, &tools, &mut context)
            .await
            .map_err(unavailable)?;
        if discover(project).map_err(unavailable)? != repository {
            return Err(changed());
        }
        Ok(Self {
            repository,
            tools: Some(tools),
            digest: Some(digest),
        })
    }
    #[cfg(test)]
    pub(crate) async fn recheck(
        &self,
        project: &ProjectHandle,
        stop: watch::Receiver<bool>,
    ) -> Result<(), ApiError> {
        self.recheck_with_keep_alive(
            project,
            stop,
            crate::owned_process::OwnedProcessRegistry::default(),
            Arc::new(()),
        )
        .await
    }
    pub(crate) async fn recheck_with_keep_alive(
        &self,
        project: &ProjectHandle,
        stop: watch::Receiver<bool>,
        processes: crate::owned_process::OwnedProcessRegistry,
        keep_alive: Arc<dyn Send + Sync>,
    ) -> Result<(), ApiError> {
        let repository = discover(project).map_err(unavailable)?;
        if repository != self.repository {
            return Err(changed());
        }
        if let Some(repo) = &repository {
            let (_cancel, receiver) = watch::channel(false);
            let mut context = Context::new(stop, receiver).map_err(unavailable)?;
            context.bind_keep_alive(processes, keep_alive);
            let digest = inspect_index(repo, self.tools.as_ref().unwrap(), &mut context)
                .await
                .map_err(unavailable)?;
            if Some(digest) != self.digest || discover(project).map_err(unavailable)? != repository
            {
                return Err(changed());
            }
        }
        Ok(())
    }
}
fn changed() -> ApiError {
    error(
        "BUILD_GIT_STATE_CHANGED",
        "构建期间 Git 仓库、索引或配置发生变化；未发布候选产物，请结束 Git 操作后重新构建。",
    )
}
pub(super) fn unavailable(cause: ApiError) -> ApiError {
    if cause.code.starts_with("BUILD_GIT_") || cause.code.starts_with("PROCESS_") {
        return cause;
    }
    let mut issue = if cause.code == "PROJECT_GIT_INTERRUPTED" {
        error(
            "BUILD_INTERRUPTED",
            "平台正在退出，已停止构建的 Git 冲突检查。",
        )
    } else if cause.code == "PROJECT_GIT_TIMEOUT" {
        error(
            "BUILD_GIT_CHECK_TIMEOUT",
            "构建的 Git 冲突检查超过 120 秒内部限制；未发布产物。",
        )
    } else {
        error("BUILD_GIT_CHECK_UNAVAILABLE", "无法安全确认项目 Git 索引没有冲突；未执行 Git 修复或改用另一份工具，请检查仓库及 Git 工具设置。")
            .detail("causeCode", cause.code.as_str())
    };
    for field in ["processResourceClosed", "processClosed", "operationOutcome"] {
        if let Some(value) = cause.details.get(field) {
            issue = issue.detail(field, value.clone());
        }
    }
    issue
}
fn discover(project: &ProjectHandle) -> Result<Option<Repository>, ApiError> {
    check_no_symlinks(&project.root, false)?;
    let source = checked_relative(&project.root, &project.manifest.source_dir, false)?;
    let mut markers = Vec::new();
    marker(&project.root.join(".git"), "project", &mut markers)?;
    for ancestor in project.root.ancestors().skip(1) {
        marker(&ancestor.join(".git"), "ancestor", &mut markers)?;
    }
    for ancestor in source
        .ancestors()
        .skip(1)
        .take_while(|path| *path != project.root)
    {
        if !ancestor.starts_with(&project.root) {
            return Err(unsafe_path());
        }
        marker(&ancestor.join(".git"), "source-ancestor", &mut markers)?;
    }
    // Names/metadata only. Do not open excluded credentials or dependency trees.
    // Keep this iterative and impose no extra source file/depth allowance: an
    // unversioned project keeps the existing build-input scanner's contract.
    let mut pending = vec![source.clone()];
    while let Some(directory) = pending.pop() {
        check_no_symlinks(&directory, false)?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&project.root)
                .map_err(|_| unsafe_path())?
                .to_str()
                .ok_or_else(unsafe_path)?;
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(".git")
            {
                marker(&path, "source", &mut markers)?;
                continue;
            }
            if protected(relative, &project.manifest).is_some() {
                continue;
            }
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                pending.push(path);
            }
        }
    }
    markers.sort_by(|left, right| left.path.cmp(&right.path));
    markers.dedup_by(|left, right| left.path == right.path);
    if markers.is_empty() {
        return Ok(None);
    }
    if markers.len() != 1
        || markers[0].kind != "directory"
        || ![project.root.join(".git"), source.join(".git")].contains(&markers[0].path)
    {
        return Err(error(
            "BUILD_GIT_CHECK_UNAVAILABLE",
            "发现祖先、嵌套、多个仓库或非普通 .git 目录；本步无法安全核对冲突，未迁移或修改仓库。",
        ));
    }
    let git_dir = markers.remove(0).path;
    check_no_symlinks(&git_dir, false)?;
    Ok(Some(Repository {
        identity: identity(&git_dir)?,
        git_dir,
    }))
}
fn repository_ready(git_dir: &Path) -> Result<(), ApiError> {
    check_no_symlinks(git_dir, false)?;
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        match fs::symlink_metadata(git_dir.join(name)) {
            Ok(_) => return Err(error("BUILD_GIT_OPERATION_IN_PROGRESS", "项目有尚未结束的合并、变基、拣选、撤销或二分操作；请在原 Git 工具中完成或退出该操作后再构建，平台未更改现场。")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(unsafe_path()),
        }
    }
    let mut issues = Vec::new();
    inspect_repository_files(git_dir, &mut issues)?;
    if let Some(issue) = issues.into_iter().next() {
        return Err(issue);
    }
    // A stray empty .git directory is not evidence of a valid Git repository.
    let head = read_file(&git_dir.join("HEAD"), 4096)?;
    let head = std::str::from_utf8(&head)
        .map_err(|_| unsafe_path())?
        .trim_end_matches('\n');
    if !valid_oid(head)
        && !head
            .strip_prefix("ref: refs/")
            .is_some_and(|name| valid_relative(name).is_ok())
    {
        return Err(unsafe_path());
    }
    read_file(&git_dir.join("config"), MAX_METADATA)?;
    for directory in ["objects", "refs"] {
        let path = git_dir.join(directory);
        check_no_symlinks(&path, false)?;
        if !fs::symlink_metadata(path)?.is_dir() {
            return Err(unsafe_path());
        }
    }
    Ok(())
}
async fn inspect_index(
    repo: &Repository,
    tools: &GitToolchain,
    context: &mut Context,
) -> Result<String, ApiError> {
    context.checkpoint()?;
    repository_ready(&repo.git_dir)?;
    let before = repository_digest(&repo.git_dir)?;
    let git_dir = context.root().join("index-check.git");
    private_dir(&git_dir.join("objects"))?;
    private_dir(&git_dir.join("refs"))?;
    // Even config --no-includes receives a verified private copy, so a live
    // config replaced with a link after validation cannot redirect Git's read.
    fs::write(
        git_dir.join("config"),
        read_file(&repo.git_dir.join("config"), MAX_METADATA)?,
    )?;
    let mut issues = Vec::new();
    check_config_path(&git_dir, tools, context, &mut issues).await?;
    if let Some(issue) = issues.into_iter().next() {
        return Err(issue);
    }
    fs::write(git_dir.join("HEAD"), b"ref: refs/heads/build-check\n")?;
    fs::write(
        git_dir.join("config"),
        b"[core]\nrepositoryformatversion = 0\nbare = false\n",
    )?;
    let index = repo.git_dir.join("index");
    match fs::symlink_metadata(&index) {
        Ok(_) => fs::write(git_dir.join("index"), read_file(&index, MAX_METADATA)?)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(unsafe_path()),
    }
    let mut command = git_command(tools, context.root(), None);
    command
        .arg(format!("--git-dir={}", git_dir.display()))
        .arg(format!(
            "--work-tree={}",
            context.root().join("home").display()
        ))
        .args(["ls-files", "--unmerged", "-z"]);
    let result = context.run(command, Vec::new()).await?;
    if result.code != 0 {
        return Err(git_failed());
    }
    // Git emits complete NUL-delimited stage entries. No path or raw output is
    // returned/logged; conflicts anywhere in this repository block publication.
    if !result.stdout.is_empty() {
        validate_unmerged(&result.stdout)?;
        return Err(error("BUILD_GIT_CONFLICT", "项目 Git 索引仍有未解决的文件冲突（包括源码目录外的文件）；未发布候选产物，请先解决冲突。"));
    }
    repository_ready(&repo.git_dir)?;
    if before != repository_digest(&repo.git_dir)? {
        return Err(changed());
    }
    context.checkpoint()?;
    Ok(before)
}

fn validate_unmerged(bytes: &[u8]) -> Result<(), ApiError> {
    if !bytes.ends_with(&[0]) {
        return Err(output_invalid());
    }
    for row in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let tab = row
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(output_invalid)?;
        let header = std::str::from_utf8(&row[..tab]).map_err(|_| output_invalid())?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3
            || !matches!(fields[0], "100644" | "100755" | "120000" | "160000")
            || !valid_oid(fields[1])
            || !matches!(fields[2], "1" | "2" | "3")
            || row[tab + 1..].is_empty()
        {
            return Err(output_invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_git_build_guard_tests.rs"]
mod tests;
