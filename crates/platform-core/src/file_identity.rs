//! Persistent file identities are distinct from the device/inode pairs used by
//! descriptor-relative operations while the process is running. On macOS a
//! device number can change after reboot or remount; the volume UUID cannot be
//! replaced by that temporary number in an on-disk identity.
#[cfg(unix)]
use anyhow::Context;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::{
    ffi::CString,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
};
use std::{
    fs::File,
    io,
    path::{Component, Path},
};
use uuid::Uuid;

/// There is deliberately no `PartialEq`: persistent matching and comparison of
/// open objects have different rules. Missing UUIDs remain strict legacy
/// device/inode identities until a caller explicitly verifies and upgrades them.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) windows_file_id: Option<[u8; 16]>,
}

impl<'de> Deserialize<'de> for PersistedIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Object {
            device: u64,
            inode: u64,
            #[serde(default)]
            volume_uuid: Option<String>,
            #[serde(default)]
            windows_file_id: Option<[u8; 16]>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Object(Object),
            LegacyPair((u64, u64)),
        }
        Ok(match Representation::deserialize(deserializer)? {
            Representation::Object(value) => Self {
                device: value.device,
                inode: value.inode,
                volume_uuid: value.volume_uuid,
                windows_file_id: value.windows_file_id,
            },
            Representation::LegacyPair((device, inode)) => Self {
                device,
                inode,
                volume_uuid: None,
                windows_file_id: None,
            },
        })
    }
}

impl PersistedIdentity {
    /// The reviewed directory's native identity, for deletion preview revisions.
    /// Unix keeps its original serialized fields; Windows includes all 128 bits.
    pub(crate) fn deletion_revision_value(&self) -> serde_json::Value {
        #[cfg(unix)]
        {
            serde_json::json!({"inode":self.inode,
                "volume":self.volume_uuid,"device":if self.volume_uuid.is_none(){Some(self.device)}else{None}})
        }
        #[cfg(windows)]
        {
            serde_json::json!({"device":self.device,"windowsFileId":self.windows_file_id})
        }
    }

    pub(crate) fn capture(path: &Path) -> Result<Self> {
        capture(path)
    }

    /// Use an already-open descriptor when a mutation's safety boundary is the
    /// descriptor itself. This does not establish that any path still names it.
    pub(crate) fn from_file(file: &File) -> Result<Self> {
        #[cfg(windows)]
        {
            let identity = platform_native::windows::files::identity(file)?;
            Ok(Self {
                device: identity.volume_serial,
                inode: u64::from_le_bytes(identity.file_id[..8].try_into().unwrap()),
                volume_uuid: None,
                windows_file_id: Some(identity.file_id),
            })
        }
        #[cfg(unix)]
        {
            let metadata = file.metadata()?;
            if !metadata.is_dir() && !metadata.is_file() {
                bail!("Persistent identity requires an ordinary file or directory")
            }
            #[cfg(target_os = "macos")]
            let volume_uuid = Some(volume_uuid(file)?);
            #[cfg(not(target_os = "macos"))]
            let volume_uuid = None;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                volume_uuid,
                windows_file_id: None,
            })
        }
    }

    pub(crate) fn is_legacy(&self) -> bool {
        self.volume_uuid.is_none() && self.windows_file_id.is_none()
    }

    pub(crate) fn legacy_pair(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    /// Runtime checks continue to use the current device number. This must not
    /// be used as an equivalence relation for records spanning boots.
    pub(crate) fn same_runtime_object(&self, observed: &Self) -> bool {
        self.legacy_pair() == observed.legacy_pair()
            && self.windows_file_id == observed.windows_file_id
    }

    /// Compare a retained runtime identity to an already open object without
    /// acquiring any new persistent volume-binding authority on Unix.
    #[cfg(windows)]
    pub(crate) fn matches_open_runtime(&self, file: &File) -> io::Result<bool> {
        #[cfg(unix)]
        {
            let metadata = file.metadata()?;
            Ok(self.device == metadata.dev() && self.inode == metadata.ino())
        }
        #[cfg(windows)]
        {
            let observed = platform_native::windows::files::identity(file)?;
            Ok(self.windows_file_id == Some(observed.file_id)
                && self.device == observed.volume_serial
                && self.volume_uuid.is_none())
        }
    }

    /// A stable record requires the same non-nil volume UUID and inode. A legacy
    /// record never gains permission to ignore a mismatching device number.
    pub(crate) fn matches_captured(&self, observed: &Self) -> bool {
        if self.windows_file_id.is_some() || observed.windows_file_id.is_some() {
            return self.windows_file_id.is_some()
                && self.windows_file_id == observed.windows_file_id
                && self.device == observed.device
                && self.volume_uuid.is_none()
                && observed.volume_uuid.is_none();
        }
        if self.inode != observed.inode {
            return false;
        }
        match &self.volume_uuid {
            Some(expected) => {
                let Some(expected) = valid_uuid(expected) else {
                    return false;
                };
                observed.volume_uuid.as_deref().and_then(valid_uuid) == Some(expected)
            }
            None => self.same_runtime_object(observed),
        }
    }

    pub(crate) fn matches_path(&self, path: &Path) -> Result<bool> {
        let observed = capture(path)?;
        if self.matches_captured(&observed) {
            return Ok(true);
        }
        if self.is_legacy() && observed.windows_file_id.is_none() && self.inode == observed.inode {
            // Receipts are created only by the explicit, independently audited
            // offline repair. Reading one does not bind or mutate a journal.
            return crate::identity_upgrade::legacy_identity_matches(path, self.device, self.inode);
        }
        Ok(false)
    }

    /// Missing material is an absent target only while its expected volume can
    /// still be observed. An unplugged old workspace must not count as deletion.
    pub(crate) fn absent_on_expected_volume(&self, path: &Path) -> Result<bool> {
        #[cfg(windows)]
        if self.windows_file_id.is_none() || self.volume_uuid.is_some() {
            bail!("A complete Windows file identity is required to verify absence")
        }
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            bail!("Expected an absolute material path without traversal")
        }
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        for ancestor in path.ancestors().skip(1) {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if metadata.is_dir() => {
                    let observed = Self::capture(ancestor)?;
                    let same_volume = match &self.volume_uuid {
                        Some(expected) => valid_uuid(expected).is_some_and(|expected| {
                            observed.volume_uuid.as_deref().and_then(valid_uuid) == Some(expected)
                        }),
                        None => self.device == observed.device,
                    };
                    if !same_volume {
                        bail!("The material's expected volume is unavailable or a different volume is mounted")
                    }
                    return Ok(true);
                }
                Ok(_) => {
                    bail!("The material's nearest existing ancestor is not an ordinary directory")
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        bail!("The material's expected volume cannot be verified")
    }
}

#[cfg(all(test, windows))]
mod revision_tests {
    use super::*;

    #[test]
    fn deletion_revision_includes_the_high_half_of_the_native_file_id() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-delete-revision-")
            .tempdir()
            .unwrap();
        let original = PersistedIdentity::capture(temporary.path()).unwrap();
        let mut different = original.clone();
        different.windows_file_id.as_mut().unwrap()[15] ^= 1;
        assert_eq!(original.legacy_pair(), different.legacy_pair());
        assert_ne!(
            original.deletion_revision_value(),
            different.deletion_revision_value()
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::fs;

    #[test]
    fn windows_identity_roundtrip_keeps_all_128_bits_and_rejects_old_windows_pairs() {
        let temp = tempfile::Builder::new()
            .prefix("panda-core-identity-中文 space-")
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap().join("disposable.txt");
        fs::write(&path, b"disposable identity").unwrap();
        let captured = PersistedIdentity::capture(&path).unwrap();
        assert!(!captured.is_legacy());
        assert!(captured.volume_uuid.is_none());
        assert!(captured.windows_file_id.is_some());
        let value = serde_json::to_value(&captured).unwrap();
        assert!(value.get("volumeUuid").is_none());
        assert_eq!(value["windowsFileId"].as_array().unwrap().len(), 16);
        let decoded: PersistedIdentity = serde_json::from_value(value).unwrap();
        assert!(decoded.matches_path(&path).unwrap());
        let mut changed = decoded.clone();
        changed.windows_file_id.as_mut().unwrap()[15] ^= 1;
        assert_eq!(changed.inode, decoded.inode);
        assert!(!changed.matches_captured(&decoded));
        let opened = fs::File::open(&path).unwrap();
        assert!(decoded.matches_open_runtime(&opened).unwrap());
        assert!(!changed.matches_open_runtime(&opened).unwrap());
        let old: PersistedIdentity = serde_json::from_value(
            serde_json::json!({"device":captured.device,"inode":captured.inode}),
        )
        .unwrap();
        assert!(!old.matches_path(&path).unwrap());
        assert!(!old.matches_open_runtime(&opened).unwrap());
        assert!(old
            .absent_on_expected_volume(&path.with_file_name("missing-disposable"))
            .is_err());
        let unavailable = PersistedIdentity {
            device: 0,
            inode: 0,
            volume_uuid: None,
            windows_file_id: None,
        };
        assert_eq!(
            serde_json::to_value(&unavailable).unwrap(),
            serde_json::json!({"device":0,"inode":0})
        );
        assert!(!unavailable.matches_path(&path).unwrap());
        assert!(unavailable
            .absent_on_expected_volume(&path.with_file_name("missing-disposable"))
            .is_err());
    }

    #[test]
    fn windows_identity_detects_replacement_and_checks_volume_before_accepting_absence() {
        let temp = tempfile::Builder::new()
            .prefix("panda-core-identity-disposable-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("disposable.txt");
        fs::write(&path, b"original disposable file").unwrap();
        let captured = PersistedIdentity::capture(&path).unwrap();
        fs::rename(&path, root.join("retained.txt")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(!captured.matches_path(&path).unwrap());
        fs::remove_file(&path).unwrap();
        assert!(captured.absent_on_expected_volume(&path).unwrap());
        let mut wrong_volume = captured.clone();
        wrong_volume.device ^= 1;
        assert!(wrong_volume.absent_on_expected_volume(&path).is_err());
    }
}

fn valid_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value).ok().filter(|uuid| !uuid.is_nil())
}

/// Open every path component without following symlinks, capture the final
/// descriptor, and re-open the chain to reject ancestors replaced while the
/// volume attributes were read. Mutating callers must still retain their own
/// descriptor-relative protection after this point-in-time observation.
#[cfg(unix)]
pub(crate) fn capture(path: &Path) -> Result<PersistedIdentity> {
    let opened = OpenedPath::open(path)?;
    let identity = PersistedIdentity::from_file(&opened.file)?;
    opened.verify_path(path)?;
    Ok(identity)
}

#[cfg(windows)]
pub(crate) fn capture(path: &Path) -> Result<PersistedIdentity> {
    let opened = platform_native::windows::files::open_no_reparse(path)?;
    PersistedIdentity::from_file(&opened.file)
}

#[cfg(unix)]
struct OpenedPath {
    file: File,
    chain: Vec<(u64, u64)>,
}

#[cfg(unix)]
impl OpenedPath {
    fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("Persistent identity requires an absolute path")
        }
        let mut file = opened(unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        let mut chain = vec![runtime_pair(&file)?];
        let components: Vec<_> = path.components().collect();
        for (index, part) in components.iter().enumerate() {
            match part {
                Component::RootDir => continue,
                Component::Normal(name) => {
                    let name = CString::new(name.as_bytes())?;
                    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
                    if index + 1 < components.len() {
                        flags |= libc::O_DIRECTORY;
                    } else {
                        // Do not block on a FIFO that was substituted for a
                        // file; reject every non-ordinary object after opening.
                        flags |= libc::O_NONBLOCK;
                    }
                    file = opened(unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) })
                        .with_context(|| format!("Cannot safely open {}", path.display()))?;
                    let metadata = file.metadata()?;
                    if !metadata.is_dir() && !metadata.is_file() {
                        bail!("Persistent identity requires an ordinary file or directory")
                    }
                    chain.push((metadata.dev(), metadata.ino()));
                }
                _ => bail!("Persistent identity requires a normalized absolute path"),
            }
        }
        Ok(Self { file, chain })
    }

    fn verify_path(&self, path: &Path) -> Result<()> {
        let current = Self::open(path)?;
        if self.chain != current.chain {
            bail!(
                "Path or ancestor changed while capturing identity: {}",
                path.display()
            )
        }
        Ok(())
    }
}

#[cfg(unix)]
fn opened(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(unix)]
fn runtime_pair(file: &File) -> io::Result<(u64, u64)> {
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(target_os = "macos")]
fn volume_uuid(file: &File) -> Result<String> {
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT as _,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // getattrlist attributes are packed with four-byte alignment: the returned
    // u32 byte count is followed by the volume's sixteen UUID bytes.
    let mut buffer = [0u8; 20];
    let status = unsafe {
        libc::fgetattrlist(
            file.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error()).context("Cannot read stable volume UUID");
    }
    if u32::from_ne_bytes(buffer[..4].try_into().unwrap()) != buffer.len() as u32 {
        bail!("Stable volume UUID response has an invalid length")
    }
    let uuid = Uuid::from_bytes(buffer[4..].try_into().unwrap());
    if uuid.is_nil() {
        bail!("Filesystem does not provide a stable non-nil volume UUID")
    }
    Ok(uuid.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    fn identity(device: u64, inode: u64, volume_uuid: Option<&str>) -> PersistedIdentity {
        PersistedIdentity {
            device,
            inode,
            volume_uuid: volume_uuid.map(str::to_owned),
            windows_file_id: None,
        }
    }
    #[test]
    fn missing_material_requires_its_expected_volume_and_never_follows_an_ancestor_link() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let file = root.join("disposable-known-file");
        fs::write(&file, b"known").unwrap();
        let expected = PersistedIdentity::capture(&file).unwrap();
        assert!(!expected.absent_on_expected_volume(&file).unwrap());
        fs::remove_file(&file).unwrap();
        assert!(expected.absent_on_expected_volume(&file).unwrap());
        let mut wrong = expected.clone();
        if wrong.volume_uuid.is_some() {
            wrong.volume_uuid = Some(Uuid::new_v4().to_string());
        } else {
            wrong.device ^= 1;
        }
        assert!(wrong.absent_on_expected_volume(&file).is_err());
        let linked = root.join("linked");
        symlink(&root, &linked).unwrap();
        assert!(expected
            .absent_on_expected_volume(&linked.join("missing"))
            .is_err());
    }

    #[test]
    fn stable_identity_survives_device_renumbering_but_not_volume_or_inode_changes() {
        let first = identity(11, 42, Some("6b4045a5-f1cb-472d-9581-70d079b17a6f"));
        let remounted = identity(13, 42, first.volume_uuid.as_deref());
        assert!(first.matches_captured(&remounted));
        assert!(!first.same_runtime_object(&remounted));
        assert!(!first.matches_captured(&identity(11, 43, first.volume_uuid.as_deref())));
        assert!(!first.matches_captured(&identity(
            11,
            42,
            Some("0875f92b-f48e-46d1-b569-e0bd2a076c5c")
        )));
        assert!(!first.matches_captured(&identity(11, 42, None)));
    }

    #[test]
    fn legacy_json_keeps_strict_device_matching_and_does_not_upgrade_itself() {
        let first: PersistedIdentity = serde_json::from_str(r#"{"device":11,"inode":42}"#).unwrap();
        assert!(first.is_legacy());
        assert!(first.matches_captured(&identity(11, 42, None)));
        assert!(!first.matches_captured(&identity(13, 42, None)));
        assert!(!first.matches_captured(&identity(
            13,
            42,
            Some("6b4045a5-f1cb-472d-9581-70d079b17a6f")
        )));
        assert_eq!(serde_json::to_value(&first).unwrap()["device"], 11);
        assert!(serde_json::to_value(&first)
            .unwrap()
            .get("volumeUuid")
            .is_none());
        let pair: PersistedIdentity = serde_json::from_str("[11,42]").unwrap();
        assert!(pair.is_legacy());
        assert!(pair.matches_captured(&first));
        assert!(serde_json::to_value(&pair).unwrap().is_object());
    }

    #[test]
    fn invalid_or_nil_volume_uuid_is_never_accepted() {
        for value in ["invalid", "00000000-0000-0000-0000-000000000000"] {
            let invalid = identity(11, 42, Some(value));
            assert!(!invalid.matches_captured(&invalid));
        }
    }

    #[test]
    fn captures_ordinary_files_and_directories_and_rejects_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join("disposable-identity-directory");
        fs::create_dir(&directory).unwrap();
        let first = capture(&directory).unwrap();
        assert!(first.matches_path(&directory).unwrap());
        #[cfg(target_os = "macos")]
        assert!(first.volume_uuid.is_some());
        let path = directory.join("sample.txt");
        fs::write(&path, b"disposable identity verification").unwrap();
        let first_file = capture(&path).unwrap();
        assert!(first_file.matches_path(&path).unwrap());
        fs::rename(&path, directory.join("retained.txt")).unwrap();
        fs::write(&path, b"different object").unwrap();
        assert!(!first_file.matches_path(&path).unwrap());
    }

    #[test]
    fn rejects_final_and_ancestor_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/sample.txt"), b"sample").unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();
        symlink(root.join("real/sample.txt"), root.join("file-link")).unwrap();
        assert!(capture(&root.join("link")).is_err());
        assert!(capture(&root.join("link/sample.txt")).is_err());
        assert!(capture(&root.join("file-link")).is_err());
    }

    #[test]
    fn detects_ancestor_replacement_even_if_leaf_identity_is_unchanged() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let ancestor = root.join("ancestor");
        fs::create_dir(&ancestor).unwrap();
        let path = ancestor.join("sample.txt");
        fs::write(&path, b"retained sample").unwrap();
        let opened = OpenedPath::open(&path).unwrap();
        fs::rename(&ancestor, root.join("retained-ancestor")).unwrap();
        fs::create_dir(&ancestor).unwrap();
        // The same leaf inode can be reached through a substituted parent;
        // validating only the final descriptor would miss this replacement.
        fs::hard_link(root.join("retained-ancestor/sample.txt"), &path).unwrap();
        assert_eq!(
            runtime_pair(&opened.file).unwrap(),
            runtime_pair(&File::open(&path).unwrap()).unwrap()
        );
        assert!(opened.verify_path(&path).is_err());
    }
}
