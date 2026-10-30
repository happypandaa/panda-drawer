//! Owner descriptors only validate syntax; they do not admit any writer.
//! Writing and closure checks stay closed until durable markers are connected
//! to actual Windows Job/I/O ownership. A guessed boot UUID or elapsed delay
//! cannot prove writer closure.
use crate::ApiError;
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

fn lexical_path(data: &Path) -> PathBuf {
    // Rust treats '/' inside a verbatim path as part of a normal component.
    // Parse Windows separators before checking traversal, without a lossy UTF-8
    // conversion or PathBuf::push that could collapse an original '..'.
    let units: Vec<u16> = data
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                unit
            }
        })
        .collect();
    PathBuf::from(OsString::from_wide(&units))
}

fn invalid_owner() -> ApiError {
    ApiError::new(
        "PROJECT_WRITER_UNCONFIRMED",
        "尚不能确认项目写入进程已结束，暂不能继续。若平台曾异常退出，请重新启动电脑后重试；仅重启平台不能证明旧进程已结束。",
    )
    .outcome("unknown")
}

fn unavailable() -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 暂不支持此功能：持久写入进程归属核对尚未完成。",
    )
    .detail("platform", "windows")
    .detail("capability", "durable-process-writer")
    .outcome("not_started")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Node,
    Web,
    Platform,
}

/// A validated description of the caller's scope, without a marker, process,
/// filesystem claim or successful closure receipt. The fields remain unused
/// by admission until the corresponding Windows ownership backend exists.
#[derive(Clone)]
pub(crate) struct WriterOwner {
    _data: PathBuf,
    _kind: Kind,
    _id: String,
}
pub(crate) enum WriterGuard {}
impl WriterOwner {
    pub(crate) fn node(data: &Path, id: &str) -> Result<Self, ApiError> {
        Self::new(data, Kind::Node, id)
    }
    pub(crate) fn web(data: &Path, id: &str) -> Result<Self, ApiError> {
        Self::new(data, Kind::Web, id)
    }
    pub(crate) fn platform(data: &Path) -> Result<Self, ApiError> {
        Self::new(data, Kind::Platform, "platform")
    }
    fn new(data: &Path, kind: Kind, id: &str) -> Result<Self, ApiError> {
        let valid = match kind {
            Kind::Node => Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id),
            Kind::Web => id.starts_with("prj_") && crate::projects::valid_id(id),
            Kind::Platform => id == "platform",
        };
        // Prefix is the Windows counterpart of a Unix absolute path's root.
        // Keep the existing descriptor contract: no existence, root-directory,
        // device, permissions or ownership claims are made by construction.
        let syntax = lexical_path(data);
        if !valid
            || !syntax.is_absolute()
            || syntax.components().any(|part| {
                !matches!(
                    part,
                    Component::Prefix(_) | Component::RootDir | Component::Normal(_)
                )
            })
        {
            return Err(invalid_owner());
        }
        Ok(Self {
            _data: data.into(),
            _kind: kind,
            _id: id.into(),
        })
    }
    pub(crate) fn begin(&self, _: &str) -> Result<WriterGuard, ApiError> {
        Err(unavailable())
    }
    #[cfg(test)]
    pub(crate) fn fixture_mark_unconfirmed(&self) -> Result<PathBuf, ApiError> {
        Err(unavailable())
    }
}
impl WriterGuard {
    pub(crate) fn finish(&mut self) -> Result<(), ApiError> {
        match *self {}
    }
}
pub(crate) fn require_owner_clear(_: &WriterOwner) -> Result<(), ApiError> {
    Err(unavailable())
}
pub(crate) fn require_all_clear(_: &Path) -> Result<(), ApiError> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_descriptors_keep_the_existing_kind_and_id_contract() {
        let data = Path::new(r"C:\panda-disposable-writer-descriptor");
        let node_id = "50bfcf0c-5102-4a27-8446-b3c7312e2bc1";
        let node = WriterOwner::node(data, node_id).unwrap();
        assert_eq!(node._data, data);
        assert_eq!(node._kind, Kind::Node);
        assert_eq!(node._id, node_id);

        let web_id = "prj_disposable-writer_1";
        let web = WriterOwner::web(data, web_id).unwrap();
        assert_eq!(web._kind, Kind::Web);
        assert_eq!(web._id, web_id);
        let platform = WriterOwner::platform(data).unwrap();
        assert_eq!(platform._kind, Kind::Platform);
        assert_eq!(platform._id, "platform");

        for id in [
            "",
            "50BFCF0C-5102-4A27-8446-B3C7312E2BC1",
            "50bfcf0c51024a278446b3c7312e2bc1",
            "{50bfcf0c-5102-4a27-8446-b3c7312e2bc1}",
            "urn:uuid:50bfcf0c-5102-4a27-8446-b3c7312e2bc1",
        ] {
            let error = WriterOwner::node(data, id).err().unwrap();
            assert_eq!(error.code, "PROJECT_WRITER_UNCONFIRMED", "{id}");
            assert_eq!(error.details["operationOutcome"], "unknown");
        }
        let too_long = format!("prj_{}", "a".repeat(61));
        for id in ["", "project", "prj_Upper", "prj_../escape", &too_long] {
            assert_eq!(
                WriterOwner::web(data, id).err().unwrap().code,
                "PROJECT_WRITER_UNCONFIRMED",
                "{id}"
            );
        }
        let longest = format!("prj_{}", "a".repeat(60));
        assert!(WriterOwner::web(data, &longest).is_ok());
        // The shared predicate accepts this short identifier; construction
        // must not add a stronger Windows project-ID policy.
        assert!(WriterOwner::web(data, "prj_").is_ok());
        assert!(WriterOwner::new(data, Kind::Platform, "other").is_err());
    }

    #[test]
    fn native_absolute_syntax_does_not_add_filesystem_admission_rules() {
        for data in [
            r"C:\",
            r"C:\panda-disposable\missing",
            r"C:/panda-disposable\missing",
            r"\\panda-disposable-host\share\missing",
            r"\\?\C:\panda-disposable\missing",
        ] {
            assert!(WriterOwner::platform(Path::new(data)).is_ok(), "{data}");
        }
        for data in [
            "",
            "relative",
            r"C:relative",
            r"\current-drive-relative",
            r"C:\panda-disposable\..\escape",
            r"C:\panda-disposable/../escape",
            r"\\panda-disposable-host\share\..\escape",
        ] {
            assert_eq!(
                WriterOwner::platform(Path::new(data)).err().unwrap().code,
                "PROJECT_WRITER_UNCONFIRMED",
                "{data}"
            );
        }
    }

    #[test]
    fn raw_verbatim_separators_cannot_hide_parent_components_or_lose_utf16() {
        for value in [
            r"\\?\C:\panda-disposable/../escape",
            r"\\?\C:\panda-disposable/..\escape",
            r"\\?\C:\panda-disposable\../escape",
            r"\\?\UNC\panda-disposable-host\share\panda/../escape",
            r"\\?\UNC\panda-disposable-host\share\panda/..\escape",
        ] {
            // Do not use join/push: verbatim PathBuf joins may normalize the
            // original traversal before the constructor gets to inspect it.
            let raw: Vec<u16> = value.encode_utf16().collect();
            let path = PathBuf::from(OsString::from_wide(&raw));
            assert_eq!(
                WriterOwner::platform(&path).err().unwrap().code,
                "PROJECT_WRITER_UNCONFIRMED",
                "{value}"
            );
        }

        let mut raw: Vec<u16> = r"\\?\C:\panda-disposable/中文-".encode_utf16().collect();
        raw.push(0xd800);
        let path = PathBuf::from(OsString::from_wide(&raw));
        let owner = WriterOwner::platform(&path).unwrap();
        assert_eq!(
            owner._data.as_os_str().encode_wide().collect::<Vec<_>>(),
            raw
        );
        assert!(lexical_path(&path)
            .as_os_str()
            .encode_wide()
            .any(|unit| unit == 0xd800));
        assert!(!lexical_path(&path)
            .as_os_str()
            .encode_wide()
            .any(|unit| unit == u16::from(b'/')));
    }

    #[test]
    fn construction_and_closed_operations_preserve_disposable_material() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-writer-descriptor-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let retained = root.join("retained-evidence");
        std::fs::write(&retained, b"disposable retained evidence").unwrap();
        let missing = root.join("missing-owner-root");
        let owners = [
            WriterOwner::platform(&root).unwrap(),
            WriterOwner::web(&missing, "prj_disposablewriter").unwrap(),
            WriterOwner::node(&missing, "50bfcf0c-5102-4a27-8446-b3c7312e2bc1").unwrap(),
        ];
        for owner in owners {
            for error in [
                owner.begin("disposable-writer").err().unwrap(),
                require_owner_clear(&owner).unwrap_err(),
                owner.fixture_mark_unconfirmed().unwrap_err(),
            ] {
                assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
                assert_eq!(error.details["capability"], "durable-process-writer");
                assert_eq!(error.details["operationOutcome"], "not_started");
            }
        }
        assert_eq!(
            require_all_clear(&root).unwrap_err().code,
            "PLATFORM_FEATURE_UNAVAILABLE"
        );
        assert_eq!(
            std::fs::read(&retained).unwrap(),
            b"disposable retained evidence"
        );
        assert!(!missing.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
}
