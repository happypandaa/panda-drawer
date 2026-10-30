//! Archive permission observations are native data, never an ownership proof.
//! Existing Unix manifests retain the exact numeric mode representation.
use serde::{Deserialize, Serialize};
use std::{fs::Metadata, io, path::Path};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ArchivePermissions {
    Unix(u32),
    Windows {
        #[serde(rename = "windowsReadOnly")]
        read_only: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// Relative to the archived project, without the `project/` ZIP prefix.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default = "default_mode")]
    pub mode: ArchivePermissions,
}
fn default_mode() -> ArchivePermissions {
    ArchivePermissions::Unix(0o600)
}

impl ArchivePermissions {
    pub(crate) fn capture(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Self::Unix(metadata.permissions().mode() & 0o777)
        }
        #[cfg(windows)]
        {
            Self::Windows {
                read_only: metadata.permissions().readonly(),
            }
        }
    }

    pub(crate) fn valid(self) -> bool {
        match self {
            Self::Unix(mode) => mode & !0o777 == 0,
            Self::Windows { .. } => true,
        }
    }

    pub(crate) fn unix_mode(self) -> Option<u32> {
        match self {
            Self::Unix(mode) => Some(mode & 0o777),
            Self::Windows { .. } => None,
        }
    }

    pub(crate) fn apply(self, path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = match self {
                Self::Unix(mode) => mode & 0o777,
                // Windows archives carry no execute bits. Imported files keep
                // the private creation policy and the observed read-only flag.
                Self::Windows { read_only: true } => 0o400,
                Self::Windows { read_only: false } => 0o600,
            };
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        }
        #[cfg(windows)]
        {
            let read_only = match self {
                Self::Unix(mode) => mode & 0o222 == 0,
                Self::Windows { read_only } => read_only,
            };
            platform_native::windows::files::set_read_only(path, read_only)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_wire_modes_keep_their_original_numbers_and_validation() {
        for mode in [0, 0o400, 0o600, 0o644, 0o755, 0o777] {
            let permissions = ArchivePermissions::Unix(mode);
            assert_eq!(
                serde_json::to_value(permissions).unwrap(),
                serde_json::json!(mode)
            );
            assert_eq!(
                serde_json::from_value::<ArchivePermissions>(serde_json::json!(mode)).unwrap(),
                permissions
            );
            assert!(permissions.valid());
        }
        assert!(!ArchivePermissions::Unix(0o1000).valid());
        for invalid in [
            serde_json::Value::Null,
            serde_json::json!(-1),
            serde_json::json!("600"),
            serde_json::json!({"windowsReadOnly":null}),
        ] {
            assert!(serde_json::from_value::<ArchivePermissions>(invalid).is_err());
        }
        let windows = ArchivePermissions::Windows { read_only: true };
        assert_eq!(
            serde_json::to_value(windows).unwrap(),
            serde_json::json!({"windowsReadOnly":true})
        );
        assert_eq!(
            serde_json::from_value::<ArchivePermissions>(serde_json::to_value(windows).unwrap())
                .unwrap(),
            windows
        );
        assert_eq!(windows.unix_mode(), None);
    }

    #[test]
    fn existing_file_entry_wire_shape_and_missing_mode_default_are_preserved() {
        let entry = FileEntry {
            path: "disposable.txt".into(),
            bytes: 1,
            sha256: "wire-format-only".into(),
            mode: ArchivePermissions::Unix(0o600),
        };
        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"path":"disposable.txt","bytes":1,"sha256":"wire-format-only","mode":384}"#
        );
        let without_mode =
            serde_json::json!({"path":"disposable.txt","bytes":1,"sha256":"wire-format-only"});
        assert_eq!(
            serde_json::from_value::<FileEntry>(without_mode).unwrap(),
            entry
        );
        let mut explicit_null = serde_json::to_value(&entry).unwrap();
        explicit_null["mode"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<FileEntry>(explicit_null).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn actual_read_only_roundtrip_preserves_private_access_identity_and_bytes() {
        use platform_native::windows::files;
        use std::io::Write;
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-archive-permissions-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("disposable-input.txt");
        let mut file = files::create_private_file(&path).unwrap();
        file.write_all(b"disposable archive content").unwrap();
        file.sync_all().unwrap();
        let before = files::identity(&file).unwrap();
        drop(file);
        let writable = ArchivePermissions::capture(&std::fs::metadata(&path).unwrap());
        assert_eq!(writable, ArchivePermissions::Windows { read_only: false });
        ArchivePermissions::Windows { read_only: true }
            .apply(&path)
            .unwrap();
        let read_only = ArchivePermissions::capture(&std::fs::metadata(&path).unwrap());
        // Always clear the disposable attribute before assertions/TempDir drop.
        let restore = writable.apply(&path);
        assert_eq!(read_only, ArchivePermissions::Windows { read_only: true });
        restore.unwrap();
        let after = files::open_no_reparse(&path).unwrap();
        assert!(before.same_object(&after.identity));
        files::verify_private(&after.file).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"disposable archive content");
        ArchivePermissions::Unix(0o400).apply(&path).unwrap();
        let imported = std::fs::metadata(&path).unwrap().permissions().readonly();
        writable.apply(&path).unwrap();
        assert!(imported);
        assert!(writable.apply(&root.join("missing")).is_err());
        assert!(!root.join("missing").exists());
    }
}
