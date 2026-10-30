//! Original Unix workspace filesystem operations and admission policy.
use super::Entry;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

fn synced_path(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let mut roots = vec![
        home.join("Library/Mobile Documents"),
        home.join("Library/CloudStorage"),
        home.join("Dropbox"),
        home.join("Google Drive"),
        home.join("OneDrive"),
    ];
    if let Ok(bytes) = fs::read(home.join(".dropbox/info.json")) {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(accounts) = value.as_object() {
                for account in accounts.values() {
                    if let Some(p) = account["path"].as_str() {
                        roots.push(PathBuf::from(p));
                    }
                }
            }
        }
    }
    roots.iter().any(|r| path.starts_with(r))
}
pub(super) fn filesystem(path: &Path) -> Result<Value> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "macos")]
    let (name, local) = (
        unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        stat.f_flags & (libc::MNT_LOCAL as u32) != 0,
    );
    #[cfg(not(target_os = "macos"))]
    let (name, local) = (format!("{}", stat.f_type), true);
    let available = stat.f_bavail.saturating_mul(stat.f_bsize as u64);
    Ok(json!({"type":name,"local":local,"availableBytes":available}))
}

pub(super) fn require_unsynced(path: &Path) -> Result<()> {
    if synced_path(path) {
        bail!("Cloud synchronization directories are not supported as active workspaces")
    }
    Ok(())
}

pub(super) fn require_supported_filesystem(fsinfo: &Value) -> Result<()> {
    #[cfg(not(test))]
    if fsinfo["type"] != "apfs" || fsinfo["local"] != true {
        bail!("Migration targets must be local APFS disks")
    }
    #[cfg(test)]
    if cfg!(target_os = "macos") && (fsinfo["type"] != "apfs" || fsinfo["local"] != true) {
        bail!("Migration targets must be local APFS disks")
    }
    Ok(())
}

pub(super) fn collect(root: &Path, excluded: &BTreeSet<PathBuf>) -> Result<Vec<Entry>> {
    fn walk(
        root: &Path,
        relative: &Path,
        excluded: &BTreeSet<PathBuf>,
        out: &mut Vec<Entry>,
    ) -> Result<()> {
        let mut entries =
            fs::read_dir(root.join(relative))?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let rel = relative.join(e.file_name());
            if excluded.iter().any(|p| rel == *p || rel.starts_with(p)) {
                continue;
            }
            let path = rel
                .to_str()
                .context("Workspace contains a filename that is not valid UTF-8")?
                .to_owned();
            let meta = fs::symlink_metadata(e.path())?;
            let mode = meta.permissions().mode() & 0o777;
            if meta.file_type().is_symlink() {
                out.push(Entry {
                    path,
                    kind: "symlink".into(),
                    bytes: 0,
                    digest: None,
                    target: Some(fs::read_link(e.path())?),
                    mode,
                });
            } else if meta.is_dir() {
                out.push(Entry {
                    path,
                    kind: "directory".into(),
                    bytes: 0,
                    digest: None,
                    target: None,
                    mode,
                });
                walk(root, &rel, excluded, out)?;
            } else if meta.is_file() {
                let mut f = File::open(e.path())?;
                let mut hash = Sha256::new();
                let mut buf = [0; 128 * 1024];
                loop {
                    let n = f.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buf[..n]);
                }
                out.push(Entry {
                    path,
                    kind: "file".into(),
                    bytes: meta.len(),
                    digest: Some(format!("{:x}", hash.finalize())),
                    target: None,
                    mode,
                });
            } else {
                bail!(
                    "Workspace contains a live socket or unsupported file: {}",
                    e.path().display()
                )
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(root, Path::new(""), excluded, &mut out)?;
    Ok(out)
}
pub(super) fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    fs::create_dir(target)?;
    fs::set_permissions(target, fs::Permissions::from_mode(0o700))?;
    for e in entries {
        let from = source.join(&e.path);
        let to = target.join(&e.path);
        match e.kind.as_str() {
            "directory" => {
                fs::create_dir(&to)?;
            }
            "symlink" => symlink(e.target.as_ref().context("Missing symlink target")?, &to)?,
            "file" => {
                let m = fs::symlink_metadata(&from)?;
                if !m.is_file() || m.file_type().is_symlink() {
                    bail!("Source changed while copying: {}", from.display())
                }
                let mut input = platform_native::filesystem::open_read(
                    &from,
                    platform_native::filesystem::ReadOptions::NoLinks,
                )?;
                let mut output = platform_native::filesystem::create_new_private_file(&to, false)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                fs::set_permissions(&to, fs::Permissions::from_mode(e.mode))?;
            }
            _ => bail!("Unknown migration entry kind"),
        }
    }
    for e in entries.iter().rev().filter(|e| e.kind == "directory") {
        let p = target.join(&e.path);
        File::open(&p)?.sync_all()?;
        fs::set_permissions(p, fs::Permissions::from_mode(e.mode))?;
    }
    File::open(target)?.sync_all()?;
    Ok(())
}

pub(super) fn append_phase_log(path: &Path, phase: &str) -> Result<(), crate::ApiError> {
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(log, "{phase}")?;
    log.sync_all()?;
    Ok(())
}
