use super::*;
use std::fs;

fn stage(root: &std::path::Path) -> PendingDownload {
    PendingDownload::create_at(&root.join("native-downloads"), "人工 中文 下载.txt").unwrap()
}
#[test]
fn only_bound_instance_origin_and_exact_origin_blobs_are_admitted() {
    let base = Url::parse("http://127.0.0.1:4321/_platform/bootstrap?token=fixture").unwrap();
    let current = base.join("/nested/page.html").unwrap();
    for value in [
        "http://127.0.0.1:4321/_platform/api/v1/file?path=a.txt",
        "http://127.0.0.1:4321/reports/a.pdf",
        "blob:http://127.0.0.1:4321/id",
    ] {
        assert!(
            permitted_url(&base, &current, &Url::parse(value).unwrap()),
            "{value}"
        );
    }
    for value in [
        "https://example.invalid/file",
        "http://127.0.0.1:4320/_platform/api/v1/file?path=a.txt",
        "http://127.0.0.1:4322/file",
        "http://localhost:4321/file",
        "http://user@127.0.0.1:4321/file",
        "http://127.0.0.1:4321/a%2fb",
        "http://127.0.0.1:4321/a%2Fb",
        "http://127.0.0.1:4321/a%5cb",
        "http://127.0.0.1:4321/a%5Cb",
        "blob:https://example.invalid/id",
        "blob:http://127.0.0.1:4322/id",
        "blob:http://user@127.0.0.1:4321/id",
        "blob:http://127.0.0.1:4321/id?query=1",
        "blob:http://127.0.0.1:4321/id#fragment",
    ] {
        assert!(
            !permitted_url(&base, &current, &Url::parse(value).unwrap()),
            "{value}"
        );
    }
    assert!(!permitted_url(
        &base,
        &Url::parse("http://127.0.0.1:4322/").unwrap(),
        &base.join("/file").unwrap()
    ));
}

#[test]
fn unadmitted_finished_never_consumes_an_external_file() {
    let temporary = tempfile::tempdir().unwrap();
    let external = temporary.path().join("unowned engine output.txt");
    fs::write(&external, b"unowned external bytes").unwrap();
    let pending = Mutex::new(HashMap::new());
    let url = Url::parse("http://127.0.0.1:4321/file").unwrap();
    assert!(take_finished(&pending, &url).is_none());
    assert_eq!(fs::read(&external).unwrap(), b"unowned external bytes");
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
}
#[test]
fn real_stage_waits_for_finished_and_keeps_unknown_children() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let mut stage = stage(&root);
    let directory = stage.file.parent().unwrap().to_path_buf();
    fs::write(&stage.file, b"unfinished engine output").unwrap();
    stage.engine_accepted.store(true, Ordering::Release);
    assert!(stage.open_file().is_err());
    stage.engine_finished.store(true, Ordering::Release);
    drop(stage.open_file().unwrap());
    fs::write(directory.join("unknown"), b"preserve unknown child").unwrap();
    assert!(stage.cleanup().is_err());
    assert_eq!(
        fs::read(directory.join("unknown")).unwrap(),
        b"preserve unknown child"
    );
    fs::remove_file(directory.join("unknown")).unwrap();
    drop(stage);
    assert!(!directory.exists());
}
#[test]
fn actual_payload_hardlink_remains_eligible_and_other_name_survives_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let stage = self::stage(&root);
    fs::write(&stage.file, b"completed engine bytes").unwrap();
    let other = root.join("external hardlink.txt");
    fs::hard_link(&stage.file, &other).unwrap();
    drop(stage.open_file().unwrap());
    drop(stage);
    assert_eq!(fs::read(other).unwrap(), b"completed engine bytes");
}

#[test]
fn callback_drop_without_finished_preserves_engine_payload_and_size_limit_is_real() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let stage = self::stage(&root);
    let file = stage.file.clone();
    fs::write(&file, b"unfinished engine bytes").unwrap();
    stage.engine_accepted.store(true, Ordering::Release);
    drop(stage);
    assert_eq!(fs::read(&file).unwrap(), b"unfinished engine bytes");
    let stage = self::stage(&root);
    let file = fs::File::create(&stage.file).unwrap();
    file.set_len(256 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert!(stage.open_file().is_err());
}
#[tokio::test]
async fn actual_finished_retires_engine_ticket_but_save_stage_keeps_io_busy() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let stage = stage(&root);
    fs::write(&stage.file, b"completed engine bytes").unwrap();
    let file = stage.file.clone();
    let engines = Arc::new(super::super::super::engine_downloads::EngineDownloads::default());
    let io = platform_core::io_lifetime::IoLifetime::default();
    let (lease, project, mut ticket) = engines
        .admit_project("prj_disposable", || io.admit())
        .unwrap();
    ticket.started();
    let engine = EngineDownload {
        resource: Some(project.hold(lease.hold(stage))),
        ticket: Some(ticket),
    };
    engines.close();
    io.close();
    io.seal();
    let stage = engine.finished();
    engines.wait_for_finished().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), io.drain())
            .await
            .is_err()
    );
    drop(stage);
    io.drain().await;
    assert!(!file.exists());
}
