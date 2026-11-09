//! Disposable, opt-in cross-manager recovery tests. The test process retains a
//! rescue Child; a fresh manager has no runtime handle. This simulates the
//! manager ownership boundary, not a physical platform crash or system reboot.

use super::*;
use crate::node_process_identity::RUN_IDENTITY_ENV;
use sha2::{Digest, Sha256};
use std::{
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::OnceLock,
    time::Duration,
};

fn official_node() -> &'static Path {
    static NODE: OnceLock<PathBuf> = OnceLock::new();
    NODE.get_or_init(|| {
        let path = fs::canonicalize(
            std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE")
                .expect("explicit private Node path for opt-in recovery tests"),
        )
        .unwrap();
        let release: Value = serde_json::from_str(include_str!(
            "../../../../scripts/node-runtime-release.json"
        ))
        .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&path).unwrap())),
            release["binary"]["sha256"].as_str().unwrap(),
            "recovery tests require the pinned official Node binary"
        );
        path
    })
}

/// This Child belongs only to the fixture. The fresh unified backend owner manager never
/// receives it. Keeping it unreaped provides a safe rescue if an assertion fails.
struct RescueNode {
    child: Child,
    pid: u32,
    nonce: String,
    reaped: bool,
}

impl RescueNode {
    fn spawn() -> Self {
        let nonce = Uuid::new_v4().simple().to_string();
        let child = Command::new(official_node())
            .args([
                "-e",
                "setTimeout(() => process.exit(0), 30000); setInterval(() => {}, 1000);",
            ])
            .env_clear()
            .env(RUN_IDENTITY_ENV, &nonce)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        Self {
            child,
            pid,
            nonce,
            reaped: false,
        }
    }

    fn identity(&self) -> RuntimeProcessIdentity {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(identity) =
                node_process_identity::capture(self.pid, official_node(), &self.nonce)
            {
                return identity;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "disposable Node identity never became observable"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn reap_stopped(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if self.child.try_wait().unwrap().is_some() {
                self.reaped = true;
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "disposable Node did not exit after a confirmed stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn end_direct_child(&mut self) {
        self.child.kill().unwrap();
        self.reap_stopped();
    }
}

impl Drop for RescueNode {
    fn drop(&mut self) {
        if !self.reaped {
            // Only our retained, unreaped direct Child reserves this group ID.
            // No PID read from active.json is used for fixture rescue cleanup.
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    slot: PathBuf,
    id: String,
}

impl Fixture {
    async fn create() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().canonicalize().unwrap();
        let root = workspace.join("disposable-platform-data");
        fs::create_dir(&root).unwrap();
        let source = workspace.join("disposable-orphan-source");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "<p>Disposable orphan fixture</p>",
        )
        .unwrap();
        fs::write(
            source.join("server/fixture.mjs"),
            "// Never launched through unified backend owner in this test.\n",
        )
        .unwrap();
        fs::write(source.join("source-sentinel"), b"preserve external source").unwrap();
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&json!({
                "manifestVersion":2,
                "app":{"name":"Disposable orphan recovery","version":"1"},
                "web":{"root":"public","entry":"index.html"},
                "backend":{"protocolVersion":1,"root":"server","entry":"fixture.mjs","nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap();
        let id = Uuid::new_v4().to_string();
        let slot = root.join("apps").join(&id);
        fs::create_dir_all(slot.join("runs")).unwrap();
        fs::write(
            slot.join("registration.json"),
            b"untouched registration sentinel",
        )
        .unwrap();
        fs::create_dir(source.join("data")).unwrap();
        fs::write(source.join("data/data-sentinel"), b"preserve instance data").unwrap();
        Self {
            _temporary: temporary,
            source,
            slot,
            id,
        }
    }

    fn write_marker(&self, identity: RuntimeProcessIdentity) {
        let mut marker =
            RunMarker::launching(&self.id, self.slot.join("runs/disposable-prior-run"));
        marker.bind(identity);
        let _owner = StartIntentOwnership::create(&self.slot.join("active.json"), &marker).unwrap();
    }

    fn assert_preserved(&self) {
        assert_eq!(
            fs::read(self.source.join("source-sentinel")).unwrap(),
            b"preserve external source"
        );
        assert_eq!(
            fs::read(self.source.join("data/data-sentinel")).unwrap(),
            b"preserve instance data"
        );
        assert!(self.slot.join("registration.json").is_file());
    }
}

#[tokio::test]
#[ignore = "Requires the pinned private Node path in NODE_RUNTIME_TRIAL_TEST_NODE"]
async fn fresh_manager_stops_verified_orphan_without_touching_another_node_or_app_files() {
    let fixture = Fixture::create().await;
    let mut orphan = RescueNode::spawn();
    let identity = orphan.identity();
    let independent = RescueNode::spawn();
    let independent_identity = independent.identity();
    fixture.write_marker(identity.clone());
    assert!(matches!(
        inspect(&fixture.slot, &fixture.id, boot_time_or_unknown())
            .unwrap()
            .state,
        RunState::Orphaned(_)
    ));
    assert_eq!(
        inspect(&fixture.slot, &fixture.id, boot_time_or_unknown())
            .unwrap()
            .view()
            .unwrap()["canStop"],
        true
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        super::super::recover(&fixture.slot, &fixture.id),
    )
    .await
    .unwrap()
    .unwrap();
    orphan.reap_stopped();
    assert_eq!(
        node_process_identity::observe(&identity).unwrap(),
        Observation::Stopped
    );
    assert!(!fixture.slot.join("active.json").exists());
    fixture.assert_preserved();
    assert_eq!(
        node_process_identity::observe(&independent_identity).unwrap(),
        Observation::Matching
    );
    assert!(super::super::status(&fixture.slot, &fixture.id)
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "Requires the pinned private Node path in NODE_RUNTIME_TRIAL_TEST_NODE"]
async fn fresh_manager_retires_v2_marker_only_after_the_recorded_group_is_empty() {
    let fixture = Fixture::create().await;
    let mut previous = RescueNode::spawn();
    let identity = previous.identity();
    fixture.write_marker(identity.clone());
    previous.end_direct_child();
    assert_eq!(
        node_process_identity::observe(&identity).unwrap(),
        Observation::Stopped
    );
    super::super::recover(&fixture.slot, &fixture.id)
        .await
        .unwrap();
    assert!(!fixture.slot.join("active.json").exists());
    fixture.assert_preserved();
}

#[tokio::test]
#[ignore = "Requires the pinned private Node path in NODE_RUNTIME_TRIAL_TEST_NODE"]
async fn marker_with_mismatched_live_pid_stays_unknown_and_never_signals_that_process() {
    let fixture = Fixture::create().await;
    let mut previous = RescueNode::spawn();
    let mut stale = previous.identity();
    previous.end_direct_child();
    let independent = RescueNode::spawn();
    let independent_identity = independent.identity();
    // Simulate a disk marker whose numeric PID/PGID now point to an unrelated
    // live process. This deliberately does not claim to force kernel PID reuse.
    stale.pid = independent_identity.pid;
    stale.pgid = independent_identity.pgid;
    fixture.write_marker(stale);
    let marker_before = fs::read(fixture.slot.join("active.json")).unwrap();
    let detail = super::super::status(&fixture.slot, &fixture.id)
        .unwrap()
        .unwrap();
    assert_eq!(detail["state"], "unconfirmed");
    assert_eq!(detail["canStop"], false);
    assert!(super::super::recover(&fixture.slot, &fixture.id)
        .await
        .is_err());
    assert_eq!(
        fs::read(fixture.slot.join("active.json")).unwrap(),
        marker_before
    );
    assert_eq!(
        node_process_identity::observe(&independent_identity).unwrap(),
        Observation::Matching
    );
    fixture.assert_preserved();
}
