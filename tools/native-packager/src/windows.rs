use platform_native::{tool_files::Facts, windows::files as native};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fs::{self, File},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

mod git;
mod owned;
mod payload;
pub const ARCHIVE_NAME: &str = "node-v24.22.0-win-x64.zip";
pub const SOURCE: &str = "https://nodejs.org/download/release/v24.22.0/node-v24.22.0-win-x64.zip";
const ROOT: &str = "node-v24.22.0-win-x64";
pub const ARCHIVE_SHA: &str = "f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3";
const NODE_SHA: &str = "ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c";
const MAX_ARCHIVE: u64 = 96 * 1024 * 1024;
const MAX_TREE: usize = 180 * 1024 * 1024;
type Result<T> = io::Result<T>;
type Files = BTreeMap<String, Vec<u8>>;

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn unique() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
fn absolute(path: &Path) -> Result<()> {
    if !platform_native::filesystem::is_explicit_non_root_absolute_path(path) {
        return Err(invalid(
            "Expected a non-root absolute path without traversal",
        ));
    }
    Ok(())
}

fn read_ordinary(path: &Path, max: u64) -> Result<Vec<u8>> {
    absolute(path)?;
    let _parent = native::pin_directory(path.parent().ok_or_else(|| invalid("Parent missing"))?)?;
    let mut file = native::open_read_file(path)?;
    let before = Facts::from_file(&file)?;
    if !before.is_file() || before.links() != 1 || before.len() > max {
        return Err(invalid("Expected a bounded independent ordinary file"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.len()
        || !before.unchanged(&Facts::from_file(&file)?)
        || !before.unchanged(&Facts::capture(path)?)
    {
        return Err(invalid("File changed during verification"));
    }
    Ok(bytes)
}

fn relative(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\\') || name.chars().any(char::is_control) {
        return Err(invalid("Unsafe archive path"));
    }
    for part in name.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || part.contains(['<', '>', ':', '"', '|', '?', '*'])
        {
            return Err(invalid("Unsafe Windows archive component"));
        }
        let base = part.split('.').next().unwrap().to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && matches!(base.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid("Reserved Windows archive component"));
        }
    }
    Ok(())
}

fn extract(bytes: &[u8]) -> Result<Files> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid_zip)?;
    if archive.len() > 100_000 {
        return Err(invalid("Archive entry budget exceeded"));
    }
    let mut selected = Files::new();
    let mut names = HashSet::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid_zip)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        relative(&name)?;
        let Some(relative) = name.strip_prefix(&format!("{ROOT}/")) else {
            if name == ROOT && entry.is_dir() {
                continue;
            }
            return Err(invalid("Unexpected archive root"));
        };
        if !["node.exe", "LICENSE"].contains(&relative)
            && !relative.starts_with("node_modules/npm/")
        {
            continue;
        }
        if !names.insert(relative.to_ascii_lowercase()) {
            return Err(invalid(
                "Duplicate or case-colliding required archive member",
            ));
        }
        let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_symlink() || !matches!(kind, 0 | 0o100000 | 0o040000) {
            return Err(invalid("Archive links or special members are forbidden"));
        }
        if entry.is_dir() {
            if entry.size() != 0 {
                return Err(invalid("Nonempty directory member"));
            }
            continue;
        }
        if kind == 0o040000 || entry.size() > 160 * 1024 * 1024 || selected.len() >= 10_000 {
            return Err(invalid("Required member exceeds budget or has wrong type"));
        }
        let mut data = Vec::new();
        Read::by_ref(&mut entry)
            .take(160 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 != entry.size() {
            return Err(invalid("Truncated or oversized archive member"));
        }
        total = total
            .checked_add(data.len())
            .filter(|size| *size <= MAX_TREE)
            .ok_or_else(|| invalid("Selected tree exceeds budget"))?;
        selected.insert(relative.into(), data);
    }
    for required in [
        "node.exe",
        "LICENSE",
        "node_modules/npm/LICENSE",
        "node_modules/npm/package.json",
        "node_modules/npm/bin/npm-cli.js",
        "node_modules/npm/bin/npx-cli.js",
    ] {
        if !selected.contains_key(required) {
            return Err(invalid(format!("Missing required member: {required}")));
        }
    }
    let npm: Value =
        serde_json::from_slice(&selected["node_modules/npm/package.json"]).map_err(invalid)?;
    if npm["version"] != "11.19.1" {
        return Err(invalid("Unexpected npm version"));
    }
    verify_pe(&selected["node.exe"])?;
    Ok(selected)
}
fn invalid_zip(error: zip::result::ZipError) -> io::Error {
    invalid(error.to_string())
}
fn verify_pe(bytes: &[u8]) -> Result<()> {
    if bytes.get(..2) != Some(b"MZ") {
        return Err(invalid("Node must be a PE executable"));
    }
    let offset = bytes
        .get(0x3c..0x40)
        .ok_or_else(|| invalid("Truncated DOS header"))?;
    let offset = u32::from_le_bytes(offset.try_into().unwrap()) as usize;
    if bytes.get(offset..offset + 4) != Some(b"PE\0\0")
        || bytes.get(offset + 4..offset + 6) != Some(&[0x64, 0x86])
    {
        return Err(invalid("Node must be an AMD64 PE executable"));
    }
    Ok(())
}

fn archive(path: &Path) -> Result<Files> {
    let bytes = read_ordinary(path, MAX_ARCHIVE)?;
    if digest(&bytes) != ARCHIVE_SHA {
        return Err(invalid("Official Node archive SHA-256 mismatch"));
    }
    let files = extract(&bytes)?;
    if digest(&files["node.exe"]) != NODE_SHA {
        return Err(invalid("Official node.exe SHA-256 mismatch"));
    }
    Ok(files)
}
fn tree(files: &Files) -> Value {
    let mut hash = Sha256::new();
    let mut total = 0usize;
    for (name, bytes) in files {
        hash.update(format!("{name}\0{}\0{}\n", bytes.len(), digest(bytes)));
        total += bytes.len();
    }
    json!({"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1","fileCount":files.len(),"totalBytes":total,"sha256":format!("{:x}",hash.finalize())})
}
fn metadata(files: &Files) -> Value {
    json!({
        "runtimeRelease":{"schemaVersion":1,"version":"24.22.0","platform":"win32","arch":"x64","archiveName":ARCHIVE_NAME,"archiveRoot":ROOT,
            "source":{"url":SOURCE,"sha256":ARCHIVE_SHA,"checksumUrls":["https://nodejs.org/download/release/v24.22.0/SHASUMS256.txt","https://nodejs.org/dist/v24.22.0/SHASUMS256.txt"]},
            "binary":{"path":"node.exe","sha256":digest(&files["node.exe"]),"size":files["node.exe"].len()},"license":{"path":"LICENSE","sha256":digest(&files["LICENSE"])}}
        ,"developmentRelease":{"schemaVersion":1,"nodeVersion":"24.22.0","npmVersion":"11.19.1","platform":"win32","arch":"x64","archiveName":ARCHIVE_NAME,"archiveSha256":ARCHIVE_SHA,"tree":tree(files),"executables":["node.exe"]}
    })
}

fn receive(cache: &Path) -> Result<Value> {
    absolute(cache)?;
    let _parent = native::pin_directory(
        cache
            .parent()
            .ok_or_else(|| invalid("Cache parent missing"))?,
    )?;
    match native::create_private_directory(cache) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let _cache = native::pin_directory(cache)?;
    let temporary = cache.join(format!(".node-download-{}", unique()));
    let mut file = native::create_private_file(&temporary)?;
    let identity = native::identity(&file)?;
    let result = (|| {
        let mut source = io::stdin().lock().take(MAX_ARCHIVE + 1);
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = source.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_ARCHIVE {
                return Err(invalid("Download budget exceeded"));
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n])?;
        }
        file.sync_all()?;
        if format!("{:x}", hash.finalize()) != ARCHIVE_SHA {
            return Err(invalid("Downloaded archive SHA-256 mismatch"));
        }
        native::rename_file(&temporary, &cache.join(ARCHIVE_NAME), false)?;
        Ok(json!({"archive":cache.join(ARCHIVE_NAME),"sha256":ARCHIVE_SHA,"size":total}))
    })();
    drop(file);
    if result.as_ref().is_err_and(|error| {
        error.kind() != io::ErrorKind::InvalidData
            || !error.to_string().contains("Publication outcome is unknown")
    }) {
        let _ = native::remove_file(&temporary, &identity);
    }
    result
}

pub fn run(args: Vec<OsString>) -> Result<Value> {
    match args.as_slice() {
        [command, target] if command == "git-receive" => git::receive(Path::new(target)),
        [command, target] if command == "git-inspect" => git::inspect(Path::new(target)),
        [command, root] if command == "git-verify" => git::verify(Path::new(root)),
        [command, source, output] if command == "git-prepare" => {
            git::prepare(Path::new(source), Path::new(output), None)
        }
        [command, source, output, guard] if command == "git-prepare" => {
            let expected: Value = serde_json::from_str(
                guard
                    .to_str()
                    .ok_or_else(|| invalid("Invalid guard encoding"))?,
            )
            .map_err(invalid)?;
            git::prepare(Path::new(source), Path::new(output), Some(&expected))
        }
        [command, target] if command == "receive" => receive(Path::new(target)),
        [command, target] if command == "inspect" => Ok(metadata(&archive(Path::new(target))?)),
        [command, kind, source, output] if command == "prepare" => {
            payload::prepare(kind, Path::new(source), Path::new(output), None)
        }
        [command, kind, source, output, guard] if command == "prepare" => {
            let expected: Value = serde_json::from_str(
                guard
                    .to_str()
                    .ok_or_else(|| invalid("Invalid guard encoding"))?,
            )
            .map_err(invalid)?;
            payload::prepare(kind, Path::new(source), Path::new(output), Some(&expected))
        }
        [command, output] if command == "check-output" => payload::output_guard(Path::new(output)),
        [command, kind, root] if command == "verify" => payload::verify(kind, Path::new(root)),
        _ => Err(invalid(
            "Usage: native-packager receive <absolute-cache-dir> | inspect <absolute-archive>",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn fixture(extra: Option<&str>) -> Vec<u8> {
        let mut pe = vec![0u8; 128];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        pe[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in [
            ("node.exe", pe.as_slice()),
            ("LICENSE", b"artificial license"),
            ("node_modules/npm/LICENSE", b"npm license"),
            (
                "node_modules/npm/package.json",
                b"{\"version\":\"11.19.1\"}",
            ),
            ("node_modules/npm/bin/npm-cli.js", b"never executed"),
            ("node_modules/npm/bin/npx-cli.js", b"never executed"),
        ] {
            writer
                .start_file(format!("{ROOT}/{name}"), SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        if let Some(name) = extra {
            writer
                .start_file(format!("{ROOT}/{name}"), SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"never executed").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn parser_keeps_complete_npm_and_rejects_unsafe_or_duplicate_selected_paths() {
        let files = extract(&fixture(None)).unwrap();
        assert_eq!(files.len(), 6);
        for name in [
            "node_modules/npm/../escape",
            "node_modules/npm/CON.txt",
            "node_modules/npm/license",
            "node_modules/npm/LICENsE",
        ] {
            assert!(extract(&fixture(Some(name))).is_err(), "{name}");
        }
        let bytes = fixture(None);
        assert!(extract(&bytes[..bytes.len() - 10]).is_err());
    }
    #[test]
    fn file_digest_gate_precedes_zip_parsing_and_preserves_the_caller_input() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("corrupt.zip");
        fs::write(&file, b"not a ZIP").unwrap();
        assert!(archive(&file).unwrap_err().to_string().contains("SHA-256"));
        assert_eq!(fs::read(file).unwrap(), b"not a ZIP");
    }
    #[test]
    fn directory_cleanup_retains_a_same_name_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("owned");
        native::create_private_directory(&path).unwrap();
        let expected = native::open_no_reparse(&path).unwrap().identity;
        fs::rename(&path, root.path().join("retained-original")).unwrap();
        native::create_private_directory(&path).unwrap();
        assert!(owned::remove_directory(&path, &expected).is_err());
        assert!(path.exists());
        assert!(root.path().join("retained-original").exists());
    }
}
