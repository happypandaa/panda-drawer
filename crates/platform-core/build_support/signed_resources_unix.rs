//! Unix file identity/no-follow checks. This module is never loaded on a
//! non-Unix host; portable schema/path checks and explicit refusal stubs remain
//! available to default development builds there.

use super::*;
use std::{
    fs::{self, Metadata, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mode() == b.mode()
        && a.nlink() == b.nlink()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

fn chain(root: &Path) -> Result<Vec<(PathBuf, Metadata)>, String> {
    validate_input_path(root)?;
    let mut path = PathBuf::new();
    let mut found = Vec::new();
    for part in root.components() {
        path.push(part.as_os_str());
        let info = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !info.is_dir() || info.file_type().is_symlink() {
            return Err("Resource directory chain contains a link or non-directory".into());
        }
        found.push((path.clone(), info));
    }
    Ok(found)
}

fn ordinary_file(path: &Path, max: u64) -> Result<(Vec<u8>, Metadata), String> {
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file()
        || before.file_type().is_symlink()
        || before.nlink() != 1
        || before.len() > max
    {
        return Err("Expected bounded ordinary non-linked resource file".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !unchanged(&before, &file.metadata().map_err(|e| e.to_string())?) {
        return Err("Resource changed while opening".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != before.len()
        || !unchanged(&before, &file.metadata().map_err(|e| e.to_string())?)
        || !unchanged(
            &before,
            &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
        )
    {
        return Err("Resource changed while reading".into());
    }
    Ok((bytes, before))
}

pub fn read_manifest(path: &Path) -> Result<Vec<u8>, String> {
    validate_input_path(path)?;
    let parents = chain(path.parent().ok_or("Manifest parent missing")?)?;
    let (bytes, info) = ordinary_file(path, MAX_MANIFEST_BYTES as u64)?;
    if ![0o444, 0o644].contains(&(info.mode() & 0o7777)) {
        return Err("Signed manifest permissions must be 0444 or 0644".into());
    }
    check_parents(&parents)?;
    Ok(bytes)
}

fn check_parents(parents: &[(PathBuf, Metadata)]) -> Result<(), String> {
    for (path, before) in parents {
        let after = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || !after.is_dir()
            || after.file_type().is_symlink()
        {
            return Err("Resource parent changed during validation".into());
        }
    }
    Ok(())
}

fn macho(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..4),
        Some(
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
    )
}

// Protocol v1 is bound to thin ARM64 macOS tools and a 15.0 application floor.
// These structural checks do not inspect or authenticate a code signature.
fn verify_macho_target(bytes: &[u8]) -> Result<(), String> {
    fn word(bytes: &[u8], offset: usize) -> Result<u32, String> {
        let bytes: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or("Truncated Mach-O header or command")?
            .try_into()
            .unwrap();
        Ok(u32::from_le_bytes(bytes))
    }
    if bytes.len() < 32
        || bytes[..4] != [0xcf, 0xfa, 0xed, 0xfe]
        || word(bytes, 4)? != 0x0100_000c
        || ![2, 6, 8].contains(&word(bytes, 12)?)
    {
        return Err("Signed tool is not a supported thin ARM64 Mach-O".into());
    }
    let count = word(bytes, 16)? as usize;
    let length = word(bytes, 20)? as usize;
    let end = 32usize
        .checked_add(length)
        .filter(|n| *n <= bytes.len())
        .ok_or("Mach-O command range exceeds file")?;
    if count == 0 || count > 4096 || length > 16 * 1024 * 1024 {
        return Err("Mach-O command budget exceeded or minimum OS absent".into());
    }
    let mut cursor = 32usize;
    let mut minimum = None;
    for _ in 0..count {
        let command = word(bytes, cursor)?;
        let size = word(bytes, cursor + 4)? as usize;
        let next = cursor
            .checked_add(size)
            .filter(|n| *n <= end)
            .ok_or("Mach-O command exceeds header range")?;
        if size < 8 || !size.is_multiple_of(4) {
            return Err("Invalid Mach-O command size".into());
        }
        let version = if command == 0x32 {
            if size < 24 || word(bytes, cursor + 8)? != 1 {
                return Err("Mach-O build platform is not macOS".into());
            }
            Some(word(bytes, cursor + 12)?)
        } else if command == 0x24 {
            if size < 16 {
                return Err("Invalid Mach-O minimum OS command".into());
            }
            Some(word(bytes, cursor + 8)?)
        } else {
            None
        };
        if let Some(version) = version {
            if minimum.replace(version).is_some() || version == 0 || version > 0x000f_0000 {
                return Err("Mach-O minimum OS is missing, duplicate or above profile 15.0".into());
            }
        }
        cursor = next;
    }
    if cursor != end || minimum.is_none() {
        return Err("Mach-O commands/minimum OS do not match the compiled target policy".into());
    }
    Ok(())
}

pub fn verify_payload(root: &Path, payload: &Payload) -> Result<(), String> {
    let parents = chain(root)?;
    let expected: BTreeMap<_, _> = payload.entries.iter().map(|x| (x.path(), x)).collect();
    let mut observed = BTreeSet::new();
    let mut evidence = Vec::new();
    let mut pending = vec![(PathBuf::new(), 0usize)];
    let mut bytes_total = 0u64;
    while let Some((relative, depth)) = pending.pop() {
        if depth > 64 {
            return Err("Resource directory depth exceeded".into());
        }
        let directory = root.join(&relative);
        let before = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !before.is_dir() || before.file_type().is_symlink() {
            return Err("Resource directory type changed".into());
        }
        let expected_mode = if relative.as_os_str().is_empty() {
            payload.root_mode
        } else {
            let name = relative.to_str().ok_or("Non-ASCII resource path")?;
            match expected.get(name) {
                Some(Entry::Directory { mode, .. }) => *mode,
                _ => return Err("Unexpected resource directory".into()),
            }
        };
        if before.mode() & 0o7777 != expected_mode {
            return Err("Resource directory permissions mismatch".into());
        }
        for item in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let child = relative.join(item.map_err(|e| e.to_string())?.file_name());
            let name = child
                .to_str()
                .filter(|p| self::relative(p))
                .ok_or("Unsafe or non-ASCII resource path")?;
            if !observed.insert(name.to_owned()) || observed.len() > MAX_ENTRIES {
                return Err("Resource entry count exceeded".into());
            }
            let entry = expected
                .get(name)
                .ok_or("Unexpected resource file/directory")?;
            let path = root.join(&child);
            let info = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if info.mode() & 0o7777 != entry.mode() {
                return Err("Resource permissions mismatch".into());
            }
            match entry {
                Entry::Directory { .. } => {
                    if !info.is_dir() || info.file_type().is_symlink() {
                        return Err("Resource expected directory".into());
                    }
                    pending.push((child, depth + 1));
                }
                Entry::File {
                    size,
                    sha256,
                    signature,
                    ..
                } => {
                    if info.len() != *size {
                        return Err("Resource size mismatch".into());
                    }
                    bytes_total = bytes_total
                        .checked_add(*size)
                        .filter(|n| *n <= MAX_TREE_BYTES)
                        .ok_or("Resource read budget exceeded")?;
                    let (bytes, before) = ordinary_file(&path, MAX_FILE_BYTES)?;
                    if digest(&bytes) != *sha256 || macho(&bytes) != signature.is_some() {
                        return Err(
                            "Resource digest or Mach-O/signature declaration mismatch".into()
                        );
                    }
                    if signature.is_some() {
                        verify_macho_target(&bytes)?;
                    }
                    evidence.push((path, before));
                }
            }
        }
        evidence.push((directory, before));
    }
    if observed.len() != expected.len() || bytes_total != payload.total_bytes {
        return Err("Missing resource entry".into());
    }
    for (path, before) in evidence {
        if !unchanged(
            &before,
            &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
        ) {
            return Err("Resource changed during complete verification".into());
        }
    }
    check_parents(&parents)
}

pub fn verify_resources(root: &Path, manifest: &Manifest) -> Result<(), String> {
    let parents = chain(root)?;
    for (_, payload) in payloads(manifest) {
        verify_payload(&root.join(&payload.root), payload)?;
    }
    check_parents(&parents)
}
