use platform_core::ApiError;
use tauri::webview::DownloadEvent;

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：网页下载的写入归属与保存发布尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "native-webview-download")
    .outcome("not_started")
}

pub(super) fn reject(event: DownloadEvent<'_>, notify: impl FnOnce(&ApiError)) -> bool {
    if matches!(event, DownloadEvent::Requested { .. }) {
        notify(&unavailable());
    }
    // No request was admitted, so even success=true is not a delivery receipt.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, fs};
    use tauri::Url;

    #[test]
    fn request_reports_unavailable_without_rewriting_or_creating_a_destination() {
        let root = tempfile::tempdir().unwrap();
        let sentinel = root.path().join("existing-external-copy.txt");
        fs::write(&sentinel, b"preserve completed external delivery").unwrap();
        let mut destination = root.path().join("not-created.bin");
        let original = destination.clone();
        let notices = Cell::new(0);
        assert!(!reject(
            DownloadEvent::Requested {
                url: Url::parse("http://127.0.0.1:4321/p/prj_disposable_download/file").unwrap(),
                destination: &mut destination,
            },
            |error| {
                notices.set(notices.get() + 1);
                assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
                assert_eq!(error.details["platform"], "windows");
                assert_eq!(error.details["capability"], "native-webview-download");
                assert_eq!(error.details["operationOutcome"], "not_started");
                assert!(error.message.contains("Windows 暂不支持此功能"));
            },
        ));
        assert_eq!(notices.get(), 1);
        assert_eq!(destination, original);
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read(sentinel).unwrap(),
            b"preserve completed external delivery"
        );
    }

    #[test]
    fn unadmitted_finished_events_never_report_success_or_consume_external_files() {
        let root = tempfile::tempdir().unwrap();
        let external = root.path().join("unowned-engine-output.txt");
        fs::write(&external, b"unowned bytes").unwrap();
        for success in [false, true] {
            for path in [None, Some(external.clone())] {
                assert!(!reject(
                    DownloadEvent::Finished {
                        url: Url::parse("http://127.0.0.1:4321/p/prj_disposable_download/file")
                            .unwrap(),
                        path,
                        success,
                    },
                    |_| panic!("An unadmitted Finished event is not a new request"),
                ));
            }
        }
        assert_eq!(fs::read(external).unwrap(), b"unowned bytes");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
