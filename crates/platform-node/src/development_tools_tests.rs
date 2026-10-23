//! Opt-in tests copy fixed official payloads into new private disposable roots.
//! They never select, modify or execute an existing user project or global tool.
use super::*;
use platform_host::{WindowsSourceDirectory, WindowsWorkspace};
use std::{fs, fs::OpenOptions, process::Command};

const CHILD_FLAG: &str = "WINDOWS_DEVELOPMENT_ENV_CHILD";
const CHILD_ROOT: &str = "WINDOWS_DEVELOPMENT_ENV_FIXTURE";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
fn official_source(kind: &str) -> PathBuf {
    let key = format!(
        "WINDOWS_DEVELOPMENT_{}_TEST_ROOT",
        kind.to_ascii_uppercase()
    );
    std::env::var_os(key).map(PathBuf::from).unwrap_or_else(|| {
        repository()
            .join(".test-artifacts/windows-development-tools-20261023")
            .join(format!("official-{kind}-second"))
    })
}
fn copy_payload(kind: &str, source: &Path, destination: &Path) {
    let release = DevelopmentTools::release(kind).unwrap();
    let metadata = format!("development-{kind}.json");
    let expected: Value = serde_json::from_str(match kind {
        "node" => include_str!("../../../scripts/windows-development-node-release.json"),
        "git" => include_str!("../../../scripts/windows-development-git-release.json"),
        _ => unreachable!(),
    })
    .unwrap();
    let bytes = fs::read(source.join(&metadata)).unwrap_or_else(|error| {
        panic!("Fixed official {kind} fixture is unavailable ({error}); prepare the reviewed payload or set WINDOWS_DEVELOPMENT_{}_TEST_ROOT", kind.to_ascii_uppercase())
    });
    assert!(
        serde_json::from_slice::<Value>(&bytes).unwrap() == expected,
        "Only the fixed official fixture metadata is accepted"
    );
    for file in &release.files {
        let target = destination.join(&file.path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        assert!(
            fs::symlink_metadata(source.join(&file.path))
                .unwrap()
                .is_file(),
            "The prepared payload contains only ordinary source files"
        );
        fs::copy(source.join(&file.path), target).unwrap();
    }
    fs::write(destination.join(metadata), bytes).unwrap();
}
struct Fixture {
    parent: WindowsSourceDirectory,
    workspace: WindowsWorkspace,
    resources: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let artifacts = repository().join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let parent = WindowsSourceDirectory::create_private_in(
            &artifacts,
            &format!(
                "windows-node-disposable-development-tools-{}",
                uuid::Uuid::new_v4()
            ),
        )
        .unwrap();
        let workspace = platform_host::disposable_node_workspace(&parent.path().join(format!(
            "windows-node-disposable-data-{}",
            uuid::Uuid::new_v4()
        )))
        .unwrap();
        workspace
            .root()
            .create_directory("workspace")
            .unwrap()
            .create_directory(".platform")
            .unwrap();
        let resources = parent.create_private_directory("resources").unwrap();
        for kind in ["node", "git"] {
            let target = resources
                .create_private_directory(&format!("development-{kind}"))
                .unwrap();
            copy_payload(kind, &official_source(kind), target.path());
        }
        Self {
            resources: resources.path().to_owned(),
            parent,
            workspace,
        }
    }
    fn data(&self) -> WorkspaceDirectory {
        self.workspace.root()
    }
    fn tools(&self) -> DevelopmentTools {
        DevelopmentTools::new(self.resources.clone())
    }
    fn managed(&self, kind: &str) -> PathBuf {
        self.resources.join(format!("development-{kind}"))
    }
    fn explicit(&self, kind: &str, label: &str) -> PathBuf {
        let directory = self.parent.create_private_directory(label).unwrap();
        copy_payload(kind, &self.managed(kind), directory.path());
        directory.path().to_owned()
    }
    fn selection(&self) -> Value {
        serde_json::to_value(DevelopmentTools::selection(&self.data()).unwrap()).unwrap()
    }
}
fn executable(root: &Path, kind: &str) -> PathBuf {
    root.join(if kind == "node" {
        "node.exe"
    } else {
        "cmd/git.exe"
    })
}
fn assert_unavailable(result: Result<ToolLease>) {
    assert!(
        result.is_err(),
        "Invalid selection must not use another tool"
    );
}
fn assert_ready(value: &Value) {
    assert_eq!(
        value["ready"],
        true,
        "Doctor issue codes: {:?}",
        value["issues"]
            .as_array()
            .map(|a| a.iter().map(|v| v["code"].clone()).collect::<Vec<_>>())
    );
    assert_eq!(value["npm"]["version"], "11.19.1");
    assert_eq!(
        value["node"]["version"]
            .as_str()
            .unwrap()
            .trim_start_matches('v'),
        "24.22.1"
    );
    assert_eq!(
        value["git"]["version"]
            .as_str()
            .unwrap()
            .trim_start_matches("git version "),
        "2.56.0.windows.2"
    );
}

#[tokio::test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
async fn fixed_payloads_are_ready_and_lease_handles_refuse_leaf_writes() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let node = tools.lease(&fixture.data(), "node").unwrap();
    let git = tools.lease(&fixture.data(), "git").unwrap();
    assert_eq!(node.selection, "managed");
    assert_eq!(node.version, "24.22.1");
    assert_eq!(node.npm_version.as_deref(), Some("11.19.1"));
    assert_eq!(git.selection, "managed");
    assert_eq!(git.version, "2.56.0.windows.2");
    for path in [
        node.executable.clone(),
        node.npm_cli.clone().unwrap(),
        git.executable.clone(),
        fixture
            .managed("git")
            .join("ucrt64/bin/git-remote-https.exe"),
    ] {
        let error = OpenOptions::new().write(true).open(&path).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(32),
            "A retained leaf HANDLE must reject writable sharing"
        );
    }
    node.verify().unwrap();
    git.verify().unwrap();
    drop((node, git));
    assert_ready(&tools.doctor(&fixture.data()).await);
}

#[test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
fn lease_snapshot_rejects_unknown_files_and_empty_directories() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    for kind in ["node", "git"] {
        let lease = tools.lease(&fixture.data(), kind).unwrap();
        let added = lease.root.join("added-after-lease.txt");
        fs::write(&added, b"disposable post-lease change").unwrap();
        assert!(
            lease.verify().is_err(),
            "A file added after inventory must be rejected"
        );
        fs::remove_file(&added).unwrap();
        lease.verify().unwrap();
        let empty = lease.root.join("empty-added-after-lease");
        fs::create_dir(&empty).unwrap();
        assert!(
            lease.verify().is_err(),
            "An empty directory added after inventory must be rejected"
        );
        fs::remove_dir(&empty).unwrap();
        lease.verify().unwrap();
    }
}

#[test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
fn managed_added_missing_modified_and_replaced_metadata_never_fallback() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    for kind in ["node", "git"] {
        let target = fixture.managed(kind);
        let extra = target.join("unrecognized-file.txt");
        fs::write(&extra, b"new disposable evidence").unwrap();
        assert_unavailable(tools.lease(&fixture.data(), kind));
        fs::remove_file(&extra).unwrap();
        let notice = target.join(if kind == "node" {
            "LICENSE"
        } else {
            "LICENSE.txt"
        });
        let original = fs::read(&notice).unwrap();
        fs::remove_file(&notice).unwrap();
        assert_unavailable(tools.lease(&fixture.data(), kind));
        fs::write(&notice, &original).unwrap();
        fs::write(&notice, b"shortened disposable notice").unwrap();
        assert_unavailable(tools.lease(&fixture.data(), kind));
        fs::write(&notice, &original).unwrap();
        let metadata = target.join(format!("development-{kind}.json"));
        let original_metadata = fs::read(&metadata).unwrap();
        fs::write(&metadata, b"{}").unwrap();
        assert_unavailable(tools.lease(&fixture.data(), kind));
        fs::write(&metadata, original_metadata).unwrap();
        tools
            .lease(&fixture.data(), kind)
            .unwrap()
            .verify()
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
async fn explicit_choices_take_priority_and_tampering_does_not_use_managed() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let node_root = fixture.explicit("node", "explicit-node");
    let git_root = fixture.explicit("git", "explicit-git");
    assert_ready(
        &tools
            .configure(
                &fixture.data(),
                "node",
                Some(executable(&node_root, "node")),
            )
            .await
            .unwrap(),
    );
    assert_ready(
        &tools
            .configure(&fixture.data(), "git", Some(executable(&git_root, "git")))
            .await
            .unwrap(),
    );
    assert_eq!(fixture.selection()["revision"], 2);
    for (kind, root) in [("node", node_root), ("git", git_root)] {
        let lease = tools.lease(&fixture.data(), kind).unwrap();
        assert_eq!(lease.selection, "configured");
        assert_eq!(lease.root, root);
        drop(lease);
        let managed_notice = fixture.managed(kind).join(if kind == "node" {
            "LICENSE"
        } else {
            "LICENSE.txt"
        });
        let original = fs::read(&managed_notice).unwrap();
        fs::write(&managed_notice, b"temporarily invalid managed fixture").unwrap();
        assert_eq!(
            tools.lease(&fixture.data(), kind).unwrap().selection,
            "configured"
        );
        fs::write(&managed_notice, original).unwrap();
        fs::write(
            root.join("selected-only-extra.txt"),
            b"selected tree changed",
        )
        .unwrap();
        assert_unavailable(tools.lease(&fixture.data(), kind));
        assert_eq!(fixture.selection()["revision"], 2);
    }
}

#[tokio::test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
async fn invalid_npm_layout_git_path_and_managed_reset_preserve_saved_selection() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let good = fixture.explicit("node", "good-explicit-node");
    assert_ready(
        &tools
            .configure(&fixture.data(), "node", Some(executable(&good, "node")))
            .await
            .unwrap(),
    );
    let before = fixture.selection();
    let invalid_npm = fixture.explicit("node", "invalid-explicit-npm");
    fs::write(
        invalid_npm.join("node_modules/npm/bin/npm-cli.js"),
        b"process.exit(43);\n",
    )
    .unwrap();
    assert!(
        tools
            .configure(
                &fixture.data(),
                "node",
                Some(executable(&invalid_npm, "node"))
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), before);
    let invalid_git = fixture.explicit("git", "invalid-explicit-git");
    fs::create_dir(invalid_git.join("other")).unwrap();
    fs::copy(
        invalid_git.join("cmd/git.exe"),
        invalid_git.join("other/git.exe"),
    )
    .unwrap();
    assert!(
        tools
            .configure(
                &fixture.data(),
                "git",
                Some(invalid_git.join("other/git.exe"))
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), before);
    let missing = fixture.parent.path().join("missing/node.exe");
    assert!(
        tools
            .configure(&fixture.data(), "node", Some(missing))
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), before);
    fs::write(
        fixture.managed("node").join("LICENSE"),
        b"invalid managed reset target",
    )
    .unwrap();
    assert!(
        tools
            .configure(&fixture.data(), "node", None)
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), before);
    assert_eq!(tools.lease(&fixture.data(), "node").unwrap().root, good);
}

#[tokio::test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
async fn corrupted_or_noncontiguous_saved_selection_is_not_treated_as_default() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let explicit = fixture.explicit("node", "explicit-node");
    tools
        .configure(&fixture.data(), "node", Some(executable(&explicit, "node")))
        .await
        .unwrap();
    let area =
        platform_host::windows_development::area(&fixture.data(), "development-tools", false)
            .unwrap()
            .unwrap();
    let path = area.path().join("selection-00000001.json");
    let original = fs::read(&path).unwrap();
    let mut malformed: Value = serde_json::from_slice(&original).unwrap();
    malformed["node"]["paths"]["npmCli"] = json!("node.exe");
    fs::write(&path, serde_json::to_vec(&malformed).unwrap()).unwrap();
    assert_unavailable(tools.lease(&fixture.data(), "node"));
    fs::write(&path, &original).unwrap();
    fs::write(&path, b"{incomplete disposable record").unwrap();
    assert_unavailable(tools.lease(&fixture.data(), "node"));
    assert_unavailable(tools.lease(&fixture.data(), "git"));
    fs::write(&path, original).unwrap();
    area.write_new("selection-00000003.json", b"{}").unwrap();
    assert_unavailable(tools.lease(&fixture.data(), "node"));
    assert_unavailable(tools.lease(&fixture.data(), "git"));
}

async fn command(
    tool: &ToolLease,
    args: Vec<OsString>,
    cwd: &Path,
    environment: BTreeMap<OsString, OsString>,
) -> platform_host::windows_command::CommandOutput {
    let (_sender, receiver) = watch::channel(false);
    platform_host::windows_command::run(
        platform_host::windows_command::CommandSpec {
            executable: tool.executable.clone(),
            args,
            working_directory: cwd.to_owned(),
            environment,
            stdin: Vec::new(),
            timeout: Duration::from_secs(15),
            stdout_limit: 16384,
            stderr_limit: 16384,
        },
        receiver,
    )
    .await
    .unwrap()
}
#[test]
#[ignore = "requires prepared fixed official Windows development Node and MinGit payloads"]
fn native_children_do_not_inherit_user_configuration_node_options_or_credentials() {
    let fixture = Fixture::new();
    let hostile = fixture
        .parent
        .create_private_directory("hostile-user-home")
        .unwrap();
    hostile
        .write_new(
            ".gitconfig",
            b"[credential]\n helper = !must-not-run\n[core]\n hooksPath = Z:/artificial-hooks\n",
        )
        .unwrap();
    hostile.write_new(".npmrc", b"registry=https://artificial-registry.invalid/\n//registry.npmjs.org/:_authToken=ARTIFICIAL-NOT-A-USER-TOKEN\n").unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "development_tools::development_tools_tests::environment_child_entrypoint_only",
            "--nocapture",
        ])
        .env(CHILD_FLAG, "1")
        .env(CHILD_ROOT, fixture.parent.path())
        .env(
            "NODE_OPTIONS",
            "--require=Z:/artificial-only/must-not-load.cjs",
        )
        .env("HOME", hostile.path())
        .env("USERPROFILE", hostile.path())
        .env("GIT_CONFIG_GLOBAL", hostile.path().join(".gitconfig"))
        .env("NPM_CONFIG_USERCONFIG", hostile.path().join(".npmrc"))
        .env(
            "NPM_CONFIG_REGISTRY",
            "https://artificial-registry.invalid/",
        )
        .env("NPM_TOKEN", "ARTIFICIAL-NPM-TOKEN")
        .env("GITHUB_TOKEN", "ARTIFICIAL-GIT-TOKEN")
        .env("AWS_SECRET_ACCESS_KEY", "ARTIFICIAL-AWS-KEY")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "credential.helper")
        .env("GIT_CONFIG_VALUE_0", "!must-not-run")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Disposable child failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[ignore = "helper entry point; the environment-isolation test invokes this with artificial values"]
fn environment_child_entrypoint_only() {
    if std::env::var(CHILD_FLAG).as_deref() != Ok("1") {
        return;
    }
    let path = PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap());
    assert_eq!(path.parent().unwrap(), repository().join(".test-artifacts"));
    assert!(
        path.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("windows-node-disposable-development-tools-")
    );
    let parent = WindowsSourceDirectory::open_private(&path).unwrap();
    let workspace = platform_host::disposable_node_workspace(&path.join(format!(
        "windows-node-disposable-child-data-{}",
        uuid::Uuid::new_v4()
    )))
    .unwrap();
    workspace
        .root()
        .create_directory("workspace")
        .unwrap()
        .create_directory(".platform")
        .unwrap();
    let tools = DevelopmentTools::new(path.join("resources"));
    let home = parent
        .create_private_directory("clean-command-home")
        .unwrap();
    let temporary = parent
        .create_private_directory("clean-command-temporary")
        .unwrap();
    home.write_new("user.npmrc", b"").unwrap();
    home.write_new("global.npmrc", b"").unwrap();
    home.write_new(
        "package.json",
        b"{\"name\":\"disposable-environment-fixture\",\"version\":\"1.0.0\"}",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let node = tools.lease(&workspace.root(), "node").unwrap();
        let git = tools.lease(&workspace.root(), "git").unwrap();
        let environment = node.environment(home.path(), temporary.path(), None).unwrap();
        for key in ["NODE_OPTIONS", "NPM_TOKEN", "GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "HTTPS_PROXY", "GIT_CONFIG_COUNT", "GIT_CONFIG_KEY_0", "NPM_CONFIG_REGISTRY"] {
            assert!(!environment.keys().any(|name| name.to_string_lossy().eq_ignore_ascii_case(key)));
        }
        let script = "process.stdout.write(JSON.stringify({nodeOptions:process.env.NODE_OPTIONS??null,npmToken:process.env.NPM_TOKEN??null,gitToken:process.env.GITHUB_TOKEN??null,aws:process.env.AWS_SECRET_ACCESS_KEY??null,proxy:process.env.HTTPS_PROXY??null,home:process.env.HOME,npmUser:process.env.NPM_CONFIG_USERCONFIG}))";
        let output = command(&node, vec!["--eval".into(), script.into()], home.path(), environment.clone()).await;
        assert!(output.success(), "Isolated Node failed");
        let value: Value = serde_json::from_slice(&output.stdout.bytes).unwrap();
        for key in ["nodeOptions", "npmToken", "gitToken", "aws", "proxy"] { assert!(value[key].is_null()); }
        assert_eq!(value["home"], json!(home.path())); assert_eq!(value["npmUser"], json!(home.path().join("user.npmrc")));
        let npm = command(&node, vec![node.npm_cli.as_ref().unwrap().as_os_str().into(), "config".into(), "get".into(), "registry".into()], home.path(), environment).await;
        assert!(npm.success(), "Isolated npm configuration read failed");
        assert_eq!(String::from_utf8_lossy(&npm.stdout.bytes).trim(), "https://registry.npmjs.org/");
        let git = command(&git, vec!["config".into(), "--global".into(), "--list".into(), "--show-origin".into()], home.path(), git.environment(home.path(), temporary.path(), Some(&node)).unwrap()).await;
        assert!(git.success(), "Isolated Git configuration read failed"); assert!(git.stdout.bytes.is_empty());
    });
}
