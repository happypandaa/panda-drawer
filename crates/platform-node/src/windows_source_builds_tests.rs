//! Windows candidate build regression tests. All filesystem fixtures are disposable.

use super::*;
use crate::windows_sources::CaptureInput;
use platform_host::WindowsWorkspace;
use std::{
    fs,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION},
};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}
struct Fixture {
    parent: WindowsSourceDirectory,
    workspace: WindowsWorkspace,
    tasks: Arc<DevelopmentTasks>,
    sources: Arc<SourceManager>,
    builds: Arc<SourceBuilds>,
}
impl Fixture {
    fn new(official: bool) -> Self {
        let artifacts = repository().join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let path = artifacts.join(format!(
            "windows-node-disposable-bld-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        let workspace = platform_host::disposable_node_workspace(&path).unwrap();
        let parent = WindowsSourceDirectory::create_private_in(
            &artifacts,
            &format!(
                "windows-node-disposable-src-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            ),
        )
        .unwrap();
        workspace
            .root()
            .create_directory("workspace")
            .unwrap()
            .create_directory(".platform")
            .unwrap();
        let resources = if official {
            let resources = repository().join("apps/desktop/src-tauri/resources/windows");
            for kind in ["node", "git"] {
                let expected: Value = serde_json::from_str(match kind {
                    "node" => {
                        include_str!("../../../scripts/windows-development-node-release.json")
                    }
                    _ => include_str!("../../../scripts/windows-development-git-release.json"),
                })
                .unwrap();
                let actual: Value = serde_json::from_slice(
                    &fs::read(
                        resources.join(format!("development-{kind}/development-{kind}.json")),
                    )
                    .expect(
                        "prepare the fixed official Windows development resources before opting in",
                    ),
                )
                .unwrap();
                assert!(
                    actual == expected,
                    "only fixed reviewed official payload metadata is accepted"
                );
            }
            resources
        } else {
            parent.path().join("no-development-tools-used")
        };
        let tools = DevelopmentTools::new(resources);
        let tasks = DevelopmentTasks::new(workspace.root()).unwrap();
        let sources = SourceManager::new(workspace.root(), tools.clone(), tasks.clone()).unwrap();
        let builds = SourceBuilds::new(workspace.root(), tools, sources.clone(), tasks.clone());
        Self {
            parent,
            workspace,
            tasks,
            sources,
            builds,
        }
    }
    fn source(&self, label: &str, script: &str) -> WindowsSourceDirectory {
        let root = self.parent.create_private_directory(label).unwrap();
        root.write_new("project.json", &serde_json::to_vec(&json!({"manifestVersion":1,"id":"disposable_project","name":"Disposable Windows build","sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html","database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}})).unwrap()).unwrap();
        root.write_new("README.md", b"Disposable build fixture")
            .unwrap();
        let docs = root.create_private_directory("docs").unwrap();
        docs.write_new("database.md", b"No business data is used")
            .unwrap();
        docs.write_new("platform.md", b"Fixture only").unwrap();
        let app = root.create_private_directory("app").unwrap();
        app.write_new("package.json", br#"{"name":"disposable-build","version":"1.0.0","private":true,"scripts":{"build":"node build.cjs"}}"#).unwrap();
        app.write_new("package-lock.json", br#"{"name":"disposable-build","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"disposable-build","version":"1.0.0"}}}"#).unwrap();
        // With no dependency declarations, npm ci needs no registry access. The
        // build uses only Node's standard library and a separate output folder.
        app.write_new("build.cjs", script.as_bytes()).unwrap();
        root
    }
    async fn completed(&self, accepted: &Value) -> Value {
        let id = accepted["taskId"].as_str().unwrap();
        let deadline = Instant::now() + TIMEOUT + Duration::from_secs(30);
        loop {
            let value = self.tasks.get(id).unwrap();
            if value["final"] == true {
                return value;
            }
            assert!(Instant::now() < deadline, "task did not complete: {value}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    async fn capture(&self, source: &WindowsSourceDirectory) -> BuildInput {
        let inspection = self.sources.inspect_local(source.path()).unwrap();
        assert_eq!(
            inspection["inspection"]["kind"], "project-v1-source",
            "{inspection}"
        );
        let accepted = self
            .sources
            .capture(CaptureInput {
                source_path: source.path().into(),
                name: None,
            })
            .unwrap();
        let value = self.completed(&accepted).await;
        assert_eq!(value["status"], "succeeded", "{value}");
        let id = accepted["sourceId"].as_str().unwrap();
        let lease = self.sources.candidate(id).unwrap();
        assert_eq!(
            lease.inspection["kind"], "project-v1-source",
            "{}",
            lease.inspection
        );
        BuildInput {
            source_id: id.to_owned(),
            expected_digest: lease.content_digest.clone(),
            acknowledge_scripts: true,
        }
    }
    async fn submit(&self, input: BuildInput) -> Value {
        // A task's final event can precede dropping its capacity permit by one
        // scheduler turn; retry only this admission race, never the operation.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.builds.prepare(input.clone()).await {
                Ok(value) => return value,
                Err(error) if error.code == "SOURCE_BUILD_BUSY" && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("build admission failed: {error:?}"),
            }
        }
    }
    fn directory(&self, accepted: &Value) -> WorkspaceDirectory {
        platform_host::windows_development::area(&self.workspace.root(), "tasks", false)
            .unwrap()
            .unwrap()
            .open_directory(accepted["taskId"].as_str().unwrap())
            .unwrap()
    }
    async fn shutdown(&self) {
        self.builds.shutdown().await;
        self.sources.shutdown().await;
        self.tasks.shutdown().await;
    }
}
const GOOD_BUILD: &str = r#"const fs=require('fs'),path=require('path');
const out=process.argv[process.argv.indexOf('--outDir')+1];
if(!out)throw Error('missing explicit output folder');
fs.mkdirSync(out,{recursive:true});
fs.writeFileSync(path.join(out,'index.html'),'<!doctype html><html><body><script src="./app.js"></script></body></html>');
fs.writeFileSync(path.join(out,'app.js'),'document.body.dataset.fixture="ready";');
console.log('DISPOSABLE_BUILD_COMPLETE');
"#;

fn memory_inventory(paths: &[&str]) -> Inventory {
    Inventory {
        digest: "0".repeat(64),
        _guards: Vec::new(),
        files: paths
            .iter()
            .map(|path| {
                (
                    path.to_string(),
                    InputFile {
                        identity: WindowsObjectIdentity {
                            volume_serial: 1,
                            file_id128: [0; 16],
                        },
                        digest: "0".repeat(64),
                        bytes: 0,
                    },
                )
            })
            .collect(),
    }
}

#[test]
fn candidate_paths_reject_escaping_and_windows_alias_syntax() {
    for path in [
        "",
        ".",
        "../src",
        "src/../out",
        "C:/src",
        "src\\file",
        "src//file",
        "src/file.",
        "src/file ",
        "src/file:stream",
    ] {
        assert!(safe_path(path).is_err(), "accepted {path:?}");
    }
    assert!(safe_path("src/assets/index.html").is_ok());
    assert!(overlaps("App", "app/nested"));
    assert!(!overlaps("app", "application"));
}

#[test]
fn npm_lock_requires_supported_root_and_matching_all_root_dependency_sections() {
    let package = json!({"dependencies":{"a":"1.0.0"},"devDependencies":{"b":"2.0.0"},"optionalDependencies":{"c":"3.0.0"}});
    for version in [2, 3] {
        let mut lock = json!({"lockfileVersion":version,"packages":{"":package.clone()}});
        validate_lock(&package, &lock).unwrap();
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            lock["packages"][""] = package.clone();
            lock["packages"][""][section] = json!({"changed":"4.0.0"});
            assert_eq!(
                validate_lock(&package, &lock).unwrap_err().code,
                "LOCKFILE_MISMATCH"
            );
        }
    }
    for lock in [
        json!({"lockfileVersion":1,"packages":{"":{}}}),
        json!({"lockfileVersion":3,"packages":{}}),
        json!({"lockfileVersion":3,"packages":{"":null}}),
    ] {
        assert_eq!(
            validate_lock(&json!({}), &lock).unwrap_err().code,
            "LOCKFILE_INVALID"
        );
    }
}

#[test]
fn local_dependency_checks_use_package_bases_and_reject_escape_aliases_and_workspaces() {
    let files = memory_inventory(&[
        "vendor/sdk.tgz",
        "members/a/package.json",
        "members/b/package.json",
    ]);
    for spec in [
        "file:vendor/sdk.tgz",
        "link:members/a",
        "./members/b",
        "~1.2.12",
        "~2.3.3",
        "^1.0.0",
        "*",
    ] {
        validate_dependencies(&json!({"dependencies":{"x":spec}}), "", &files).unwrap();
    }
    // A nested package's ../b is relative to that package, rather than the lock.
    validate_dependencies(&json!({"lockfileVersion":3,"packages":{"":{},"members/a":{"dependencies":{"b":"file:../b"}}}}),"",&files).unwrap();
    for spec in [
        "file:../outside",
        "file:C:/outside",
        "file:\\server\\share",
        "file:/outside",
        "file:vendor/%2e%2e/secret",
        "file:missing",
        "~/outside",
        "link:members/missing",
        "git+file:///C:/outside",
        "Git+FiLe:///D:/outside",
    ] {
        assert!(
            validate_dependencies(&json!({"dependencies":{"x":spec}}), "", &files).is_err(),
            "{spec}"
        );
    }
    for value in [
        json!({"workspaces":["members/*"]}),
        json!({"packages":{"members/a":{"workspaces":[]}}}),
    ] {
        assert_eq!(
            validate_dependencies(&value, "", &files).unwrap_err().code,
            "SOURCE_BUILD_LINK_UNSUPPORTED"
        );
    }
    assert!(validate_dependencies(&json!({"packages":{"../outside":{}}}), "", &files).is_err());
    assert!(validate_dependencies(
        &json!({"packages":{"node_modules/x":{"link":true,"resolved":"../outside"}}}),
        "",
        &files
    )
    .is_err());
    assert!(validate_dependencies(
        &json!({"dependencies":{"x":{"version":"file:../outside"}}}),
        "",
        &files
    )
    .is_err());
}

#[tokio::test]
async fn acknowledgement_and_malformed_identity_reject_without_task_or_tool_creation() {
    let fixture = Fixture::new(false);
    let mut input = BuildInput {
        source_id: format!("src_{}", "1".repeat(32)),
        expected_digest: "1".repeat(64),
        acknowledge_scripts: false,
    };
    assert_eq!(
        fixture
            .builds
            .prepare(input.clone())
            .await
            .unwrap_err()
            .code,
        "SOURCE_BUILD_ACK_REQUIRED"
    );
    input.acknowledge_scripts = true;
    input.source_id = "src_../outside".into();
    assert_eq!(
        fixture
            .builds
            .prepare(input.clone())
            .await
            .unwrap_err()
            .code,
        "SOURCE_BUILD_INVALID"
    );
    input.source_id = format!("src_{}", "1".repeat(32));
    input.expected_digest = "A".repeat(64);
    assert_eq!(
        fixture.builds.prepare(input).await.unwrap_err().code,
        "SOURCE_BUILD_INVALID"
    );
    assert_eq!(fixture.tasks.list(), json!([]));
    assert!(
        platform_host::windows_development::area(&fixture.workspace.root(), "tasks", false)
            .unwrap()
            .is_none()
    );
    fixture.shutdown().await;
}

#[test]
fn ntfs_inventory_tracks_identity_and_bytes_excludes_generated_material_and_rejects_shrinkwrap() {
    let fixture = Fixture::new(false);
    let source = fixture.source("inventory", GOOD_BUILD);
    let app = source.open_directory("app").unwrap();
    app.create_private_directory("node_modules")
        .unwrap()
        .write_new("ignored.bin", b"generated")
        .unwrap();
    let first = inventory(&app, true).unwrap();
    assert!(!first.files.contains_key("node_modules/ignored.bin"));
    assert!(inventory(&app, false)
        .unwrap()
        .files
        .contains_key("node_modules/ignored.bin"));
    validate_materials(&app, &first, "build").unwrap();
    assert!(validate_materials(&app, &first, "missing").is_err());
    fs::write(app.path().join("build.cjs"), b"changed input").unwrap();
    let after = inventory(&app, true).unwrap();
    assert_ne!(after.digest, first.digest);
    assert_eq!(
        after.files["build.cjs"].identity,
        first.files["build.cjs"].identity
    );
    app.write_new(
        "npm-shrinkwrap.json",
        br#"{"lockfileVersion":3,"packages":{"":{}}}"#,
    )
    .unwrap();
    assert_eq!(
        validate_materials(&app, &inventory(&app, true).unwrap(), "build")
            .unwrap_err()
            .code,
        "SOURCE_BUILD_INVALID"
    );
}

#[test]
fn ntfs_budget_accepts_64_directory_levels_and_rejects_65_without_removing_evidence() {
    let fixture = Fixture::new(false);
    let root = fixture.parent.create_private_directory("depth").unwrap();
    let mut deepest = root.clone();
    for _ in 0..64 {
        deepest = deepest.create_private_directory("d").unwrap();
    }
    let mut entries = 0;
    disk_tree(&root, &mut 0, &mut entries).unwrap();
    assert_eq!(entries, 64, "the supported boundary must remain usable");
    let rejected = deepest.create_private_directory("d").unwrap();
    rejected
        .write_new("retained.txt", b"disposable depth-limit evidence")
        .unwrap();
    let error = disk_tree(&root, &mut 0, &mut 0).unwrap_err();
    assert_eq!(error.code, "SOURCE_BUILD_LIMIT");
    assert_eq!(error.details["operationOutcome"], "not_committed");
    assert_eq!(
        rejected.read_bounded("retained.txt", 1024).unwrap().bytes,
        b"disposable depth-limit evidence"
    );
    rejected.verify_binding().unwrap();
}

#[test]
fn ntfs_artifact_uses_shared_digest_and_pins_validated_html_css_and_javascript() {
    use platform_project_format::{tree_digest, TreeFile};
    let fixture = Fixture::new(false);
    let artifact = fixture
        .parent
        .create_private_directory("artifact-valid")
        .unwrap();
    let files: [(&str,&[u8]);7] = [
        ("index.html",br##"<!doctype html><html><a href="#anchor">ok</a><a href="https://example.invalid">web</a><link href="./a/style.css?v=1#x"><script src="./a/app.js"></script><img src="data:image/png;base64,AA=="></html>"##),
        ("a/app.js",b"import './module.js'; import('./module.js'); export {x} from './module.js';"),
        ("a/module.js",b"export const x=1;"),
        ("a/style.css",b"body{background:url('../image.bin')}"),
        ("image.bin",&[0,255,1,2]),
        ("a-.txt",b"whole path order differs from component order"),
        ("a/x.txt",b"component sorting must match main"),
    ];
    let a = artifact.create_private_directory("a").unwrap();
    for (path, bytes) in &files {
        if let Some(name) = path.strip_prefix("a/") {
            a.write_new(name, bytes).unwrap();
        } else {
            artifact.write_new(path, bytes).unwrap();
        }
    }
    let inventory = inventory(&artifact, false).unwrap();
    let logical: Vec<_> = files
        .iter()
        .map(|(path, bytes)| TreeFile {
            logical_path: path,
            bytes,
        })
        .collect();
    assert_eq!(inventory.digest, tree_digest(&logical));
    validate_artifact(&artifact, &inventory, "index.html").unwrap();
    assert!(
        validate_artifact(&artifact, &inventory, "a/app.js").is_err(),
        "the entry must be HTML"
    );
    let error = fs::OpenOptions::new()
        .write(true)
        .open(artifact.path().join("index.html"))
        .unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(32),
        "artifact validation retains read-only leaf HANDLEs"
    );
    drop(inventory);
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(artifact.path().join("index.html"))
        .is_ok());
}

#[test]
fn ntfs_artifact_rejects_base_absolute_escaping_missing_and_invalid_utf8_references() {
    let fixture = Fixture::new(false);
    type ArtifactCase<'a> = (&'a [u8], Option<(&'a str, &'a [u8])>);
    let cases: [ArtifactCase<'_>; 9] = [
        (b"<base href='./'><p>bad</p>", None),
        (b"<script src='/app.js'></script>", None),
        (b"<script src='//example.invalid/app.js'></script>", None),
        (b"<img src='../outside.png'>", None),
        (b"<img src='./missing.png'>", None),
        (&[0xff, 0xfe], None),
        (
            b"<link href='./style.css'>",
            Some(("style.css", b"body{background:url('../outside.png')}")),
        ),
        (
            b"<script src='./app.js'></script>",
            Some(("app.js", b"import('./missing.js')")),
        ),
        (
            b"<script src='./app.js'></script>",
            Some(("app.js", &[0xff])),
        ),
    ];
    for (index, (html, extra)) in cases.into_iter().enumerate() {
        let artifact = fixture
            .parent
            .create_private_directory(&format!("artifact-invalid-{index}"))
            .unwrap();
        artifact.write_new("index.html", html).unwrap();
        if let Some((name, bytes)) = extra {
            artifact.write_new(name, bytes).unwrap();
        }
        let snapshot = inventory(&artifact, false).unwrap();
        assert_eq!(
            validate_artifact(&artifact, &snapshot, "index.html")
                .unwrap_err()
                .code,
            "BUILD_ARTIFACT_INVALID",
            "case {index}"
        );
    }
}

#[test]
fn restart_marks_pending_build_interrupted_and_preserves_evidence_without_replay() {
    let fixture = Fixture::new(false);
    let root = platform_host::windows_development::area(&fixture.workspace.root(), "tasks", true)
        .unwrap()
        .unwrap();
    let id = format!("task_{}", uuid::Uuid::new_v4().simple());
    let source_id = format!("src_{}", uuid::Uuid::new_v4().simple());
    let directory = root.create_directory(&id).unwrap();
    directory
        .write_new(
            ".windows-task.json",
            &serde_json::to_vec(
                &json!({"schemaVersion":1,"taskId":id,"directoryIdentity":directory.identity()}),
            )
            .unwrap(),
        )
        .unwrap();
    directory.write_new("event-00000001.json",&serde_json::to_vec(&json!({"taskId":id,"sourceId":source_id,"projectId":null,"type":"sourceBuild","status":"running","phase":"building-candidate","final":false,"result":null,"error":null})).unwrap()).unwrap();
    directory
        .create_directory("candidate")
        .unwrap()
        .write_new("must-not-run.cjs", b"throw Error('never replay')")
        .unwrap();
    let reopened = DevelopmentTasks::new(fixture.workspace.root()).unwrap();
    let value = reopened.get(&id).unwrap();
    assert_eq!(value["status"], "interrupted");
    assert_eq!(value["final"], true);
    assert_eq!(value["error"]["code"], "TASK_INTERRUPTED");
    assert_eq!(value["error"]["details"]["operationOutcome"], "unknown");
    assert_eq!(
        fs::read(directory.path().join("candidate/must-not-run.cjs")).unwrap(),
        b"throw Error('never replay')"
    );
    assert!(!directory.path().join("candidate-ready.json").exists());
    assert!(!directory.path().join("artifact").exists());
    let count = directory.list().unwrap().len();
    drop(reopened);
    let twice = DevelopmentTasks::new(fixture.workspace.root()).unwrap();
    assert_eq!(twice.get(&id).unwrap()["status"], "interrupted");
    assert_eq!(
        directory.list().unwrap().len(),
        count,
        "reopening must not append a new retry"
    );
}

#[tokio::test]
#[ignore = "requires the prepared fixed official Node/npm and MinGit resources; no registry dependency is used"]
async fn official_zero_dependency_ci_build_failure_and_input_mutation_preserve_sources() {
    let fixture = Fixture::new(true);
    let source = fixture.source("good", GOOD_BUILD);
    let original = fs::read(source.path().join("app/build.cjs")).unwrap();
    let input = fixture.capture(&source).await;
    let mut stale = input.clone();
    stale.expected_digest = "0".repeat(64);
    let accepted = fixture.submit(stale).await;
    let failed = fixture.completed(&accepted).await;
    assert_eq!(
        failed["error"]["code"], "SOURCE_BUILD_SOURCE_CHANGED",
        "{failed}"
    );
    assert!(!fixture
        .directory(&accepted)
        .path()
        .join("candidate-ready.json")
        .exists());
    let accepted = fixture.submit(input).await;
    let result = fixture.completed(&accepted).await;
    assert_eq!(
        result["status"],
        "succeeded",
        "{}\n{}",
        result,
        fixture
            .tasks
            .logs(accepted["taskId"].as_str().unwrap(), 0)
            .unwrap()
    );
    let ready: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .directory(&accepted)
                .path()
                .join("candidate-ready.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(ready["sourceDigest"], result["result"]["sourceDigest"]);
    assert_eq!(ready["artifactDigest"], result["result"]["artifactDigest"]);
    assert_eq!(ready["installed"], false);
    assert_eq!(ready["published"], false);
    let artifact = PathBuf::from(result["result"]["artifactPath"].as_str().unwrap());
    assert!(fs::read_to_string(artifact.join("index.html"))
        .unwrap()
        .contains("./app.js"));
    assert_eq!(ready["files"].as_array().unwrap().len(), 2);
    assert_eq!(
        fs::read(source.path().join("app/build.cjs")).unwrap(),
        original
    );
    assert!(!source.path().join("app/node_modules").exists());
    assert!(!source.path().join("dist").exists());
    for (label, script, code) in [
        (
            "failure",
            "console.error('DISPOSABLE_FAILURE'); process.exit(23);",
            "SOURCE_BUILD_PROCESS_FAILED",
        ),
        (
            "mutation",
            "require('fs').writeFileSync('package.json','{\"name\":\"changed\"}');\n",
            "SOURCE_BUILD_INPUT_CHANGED",
        ),
    ] {
        let source = fixture.source(label, &format!("{script}\n{GOOD_BUILD}"));
        let before = fs::read(source.path().join("app/package.json")).unwrap();
        let input = fixture.capture(&source).await;
        let accepted = fixture.submit(input).await;
        let result = fixture.completed(&accepted).await;
        assert_eq!(result["status"], "failed", "{result}");
        assert_eq!(
            result["error"]["code"],
            code,
            "{}\n{}",
            result,
            fixture
                .tasks
                .logs(accepted["taskId"].as_str().unwrap(), 0)
                .unwrap()
        );
        assert!(fixture
            .directory(&accepted)
            .path()
            .join("candidate/build.cjs")
            .exists());
        assert!(!fixture
            .directory(&accepted)
            .path()
            .join("candidate-ready.json")
            .exists());
        assert_eq!(
            fs::read(source.path().join("app/package.json")).unwrap(),
            before
        );
    }
    assert_eq!(fixture.builds.list().unwrap().as_array().unwrap().len(), 4);
    fixture.shutdown().await;
    let reopened = DevelopmentTasks::new(fixture.workspace.root()).unwrap();
    assert_eq!(
        reopened.get(accepted["taskId"].as_str().unwrap()).unwrap()["status"],
        "succeeded"
    );
}

#[tokio::test]
#[ignore = "requires the prepared fixed official Node/npm and MinGit resources; launches disposable Node descendants"]
async fn official_build_cancellation_stops_parent_and_detached_child_then_accepts_next_build() {
    let fixture = Fixture::new(true);
    let source = fixture.source("cancel",r#"const fs=require('fs'),cp=require('child_process');
fs.mkdirSync('tmp',{recursive:true});
if(process.argv[2]==='child') {fs.writeFileSync('tmp/child.pid',String(process.pid));setInterval(()=>{},1000);}
else {fs.writeFileSync('tmp/parent.pid',String(process.pid));cp.spawn(process.execPath,[__filename,'child'],{detached:true,stdio:'ignore',windowsHide:true}).unref();setInterval(()=>{},1000);}
"#);
    let input = fixture.capture(&source).await;
    let accepted = fixture.submit(input).await;
    let directory = fixture.directory(&accepted);
    let deadline = Instant::now() + Duration::from_secs(90);
    let handles = loop {
        let pids: Option<Vec<u32>> = ["parent", "child"]
            .into_iter()
            .map(|name| {
                fs::read_to_string(directory.path().join(format!("candidate/tmp/{name}.pid")))
                    .ok()?
                    .parse()
                    .ok()
            })
            .collect();
        if let Some(pids) = pids {
            break pids
                .into_iter()
                .map(|pid| {
                    let handle = unsafe {
                        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid)
                    };
                    assert!(
                        !handle.is_null(),
                        "fixture process must be alive before cancellation: {}",
                        std::io::Error::last_os_error()
                    );
                    unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
                })
                .collect::<Vec<_>>();
        }
        let record = fixture
            .tasks
            .get(accepted["taskId"].as_str().unwrap())
            .unwrap();
        assert_ne!(
            record["final"], true,
            "process fixture ended before both descendants were live: {record}"
        );
        assert!(
            Instant::now() < deadline,
            "disposable process fixture did not become ready: {record}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    fixture
        .tasks
        .cancel(accepted["taskId"].as_str().unwrap())
        .unwrap();
    let result = fixture.completed(&accepted).await;
    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(
        result["error"]["code"], "SOURCE_BUILD_CANCELLED",
        "{result}"
    );
    for handle in &handles {
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 5_000) },
            WAIT_OBJECT_0,
            "a disposable descendant survived its finished cancellation task"
        );
    }
    assert!(!directory.path().join("candidate-ready.json").exists());
    assert!(directory.path().join("candidate/tmp/parent.pid").exists());
    assert!(!source.path().join("app/tmp").exists());
    let next = fixture.source("after-cancel", GOOD_BUILD);
    let input = fixture.capture(&next).await;
    let accepted = fixture.submit(input).await;
    let result = fixture.completed(&accepted).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    fixture.shutdown().await;
}

fn copy_template(source: &Path, destination: &WindowsSourceDirectory) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_str().unwrap().to_owned();
        if generated(&name) {
            continue;
        }
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "template must contain ordinary files");
        if kind.is_dir() {
            copy_template(
                &entry.path(),
                &destination.create_private_directory(&name).unwrap(),
            );
        } else {
            destination
                .write_new(&name, &fs::read(entry.path()).unwrap())
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires WINDOWS_SOURCE_TEMPLATE_TEST=1, fixed official resources and registry access for a fresh React/Vite/tsc npm ci"]
async fn official_react_template_ci_typecheck_and_vite_build_fresh_candidate() {
    assert_eq!(
        std::env::var("WINDOWS_SOURCE_TEMPLATE_TEST").as_deref(),
        Ok("1"),
        "explicitly opt in to this registry-backed template test"
    );
    let fixture = Fixture::new(true);
    let source = fixture.source("react", GOOD_BUILD);
    let app = source.open_directory("app").unwrap();
    // Replace only this newly created fixture's three placeholder files, then
    // copy the repository template verbatim, including its reviewed SDK tarball.
    for name in ["package.json", "package-lock.json", "build.cjs"] {
        fs::remove_file(app.path().join(name)).unwrap();
    }
    copy_template(&repository().join("templates/react/app"), &app);
    assert!(!app.path().join("node_modules").exists());
    let before = inventory(&app, true).unwrap().digest;
    let input = fixture.capture(&source).await;
    let accepted = fixture.submit(input).await;
    let result = fixture.completed(&accepted).await;
    assert_eq!(
        result["status"],
        "succeeded",
        "{}\n{}",
        result,
        fixture
            .tasks
            .logs(accepted["taskId"].as_str().unwrap(), 0)
            .unwrap()
    );
    let artifact = PathBuf::from(result["result"]["artifactPath"].as_str().unwrap());
    assert!(artifact.join("index.html").is_file());
    assert!(artifact.join("assets").is_dir());
    assert_eq!(inventory(&app, true).unwrap().digest, before);
    assert!(!app.path().join("node_modules").exists());
    assert!(!source.path().join("dist").exists());
    assert_eq!(result["result"]["installed"], false);
    assert_eq!(result["result"]["published"], false);
    fixture.shutdown().await;
}
