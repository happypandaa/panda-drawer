//! Fixed official Windows MinGit ZIP. Unix source builds and metadata remain
//! in their existing entry point; no Mac signing/build fields are synthesized.
use super::{
    absolute, digest, invalid, native, payload, read_ordinary, relative, tree, unique, verify_pe,
    Files, Result,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs::File,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub(super) const ARCHIVE_NAME: &str = "MinGit-2.56.0.4-64-bit.zip";
const SHA: &str = "e3bf78351fe777fba47ddee5950082f8a98bb0f02604fdc7ba3ee2cb68ffa877";
const SIZE: u64 = 39_846_926;
const MAX_TREE: usize = 128 * 1024 * 1024;
const RELEASE: &[u8] =
    include_bytes!("../../../../scripts/development-git-release.windows-x64.json");

fn member(root: &Path, name: &str) -> PathBuf {
    root.join(Path::new(name).components().collect::<PathBuf>())
}
fn declared() -> Result<Value> {
    serde_json::from_slice(RELEASE).map_err(invalid)
}
fn extract(bytes: &[u8]) -> Result<Files> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if zip.len() > 2000 {
        return Err(invalid("MinGit archive entry budget exceeded"));
    }
    let mut files = Files::new();
    let mut names = HashSet::new();
    let mut directories = Vec::new();
    let mut total = 0usize;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(invalid)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        relative(&name)?;
        if !names.insert(name.to_lowercase()) {
            return Err(invalid("Duplicate or case-colliding MinGit member"));
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != if entry.is_dir() { 0o040000 } else { 0o100000 } {
                return Err(invalid("MinGit ZIP contains a link or special member"));
            }
        }
        if entry.is_dir() {
            directories.push(name);
            continue;
        }
        if entry.size() > 16 * 1024 * 1024 || files.len() >= 1000 {
            return Err(invalid("MinGit file budget exceeded"));
        }
        let mut data = Vec::new();
        Read::by_ref(&mut entry)
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 != entry.size() {
            return Err(invalid("MinGit ZIP member size differs"));
        }
        total = total
            .checked_add(data.len())
            .filter(|value| *value <= MAX_TREE)
            .ok_or_else(|| invalid("MinGit complete tree exceeds budget"))?;
        files.insert(name, data);
    }
    for name in files.keys() {
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            let parent_name = path.to_string_lossy().replace('\\', "/");
            if files
                .keys()
                .any(|name| name.eq_ignore_ascii_case(&parent_name))
            {
                return Err(invalid("MinGit file/directory collision"));
            }
            parent = path.parent();
        }
    }
    for name in directories {
        if !files
            .keys()
            .any(|file| file.starts_with(&format!("{name}/")))
        {
            return Err(invalid("Unexpected empty MinGit directory"));
        }
    }
    for required in [
        "cmd/git.exe",
        "ucrt64/bin/git.exe",
        "LICENSE.txt",
        "ucrt64/libexec/git-core/git-sh-setup",
        "ucrt64/share/git-core/templates/description",
    ] {
        if !files.contains_key(required) {
            return Err(invalid(format!("Missing fixed MinGit member: {required}")));
        }
    }
    // Upstream also includes a 32-bit getprocaddr helper. The two actual Git
    // entry executables must be AMD64; the full verified tree remains intact.
    verify_pe(&files["cmd/git.exe"])?;
    verify_pe(&files["ucrt64/bin/git.exe"])?;
    Ok(files)
}
fn metadata(files: &Files) -> Result<Value> {
    let mut release = declared()?;
    let license = &files["LICENSE.txt"];
    release["tree"] = tree(files);
    release["license"] =
        json!({"path":"LICENSE.txt","size":license.len(),"sha256":digest(license)});
    release["executables"] = json!(files
        .keys()
        .filter(|name| name.ends_with(".exe"))
        .collect::<Vec<_>>());
    Ok(release)
}
fn archive(path: &Path) -> Result<Files> {
    let bytes = read_ordinary(path, SIZE)?;
    if bytes.len() as u64 != SIZE || digest(&bytes) != SHA {
        return Err(invalid("Official MinGit archive size/SHA-256 mismatch"));
    }
    let files = extract(&bytes)?;
    if metadata(&files)? != declared()? {
        return Err(invalid(
            "MinGit archive differs from compiled Windows release",
        ));
    }
    Ok(files)
}
pub(super) fn inspect(path: &Path) -> Result<Value> {
    metadata(&archive(path)?)
}
fn report(root: &Path) -> Value {
    json!({"root":root,"gitPath":member(root,"cmd/git.exe"),"execPath":member(root,"ucrt64/libexec/git-core"),"templatesPath":member(root,"ucrt64/share/git-core/templates"),"gitVersion":"2.56.0.windows.4"})
}
pub(super) fn verify(root: &Path) -> Result<Value> {
    absolute(root)?;
    let mut files = payload::inspect_tree(root)?;
    if files.remove("development-git.json").as_deref() != Some(RELEASE) {
        return Err(invalid(
            "MinGit metadata differs from compiled Windows source",
        ));
    }
    if tree(&files) != declared()?["tree"] {
        return Err(invalid(
            "MinGit complete tree differs from compiled release",
        ));
    }
    verify_pe(
        files
            .get("cmd/git.exe")
            .ok_or_else(|| invalid("MinGit entry missing"))?,
    )?;
    verify_pe(
        files
            .get("ucrt64/bin/git.exe")
            .ok_or_else(|| invalid("MinGit engine missing"))?,
    )?;
    Ok(report(root))
}

fn run_probe(root: &Path, home: &Path, temporary: &Path, args: Vec<OsString>) -> Result<String> {
    use platform_native::windows::process::{OwnedProcess, Spawn};
    let mut environment: Vec<(OsString, OsString)> = Vec::new();
    for key in ["SystemRoot", "WINDIR", "ComSpec"] {
        if let Some(value) = std::env::var_os(key) {
            environment.push((key.into(), value));
        }
    }
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
    ] {
        environment.push((key.into(), home.as_os_str().into()));
    }
    for key in ["TEMP", "TMP"] {
        environment.push((key.into(), temporary.as_os_str().into()));
    }
    let mut search = vec![
        root.join("cmd"),
        root.join("ucrt64/bin"),
        root.join("usr/bin"),
    ];
    if let Some(system) = std::env::var_os("SystemRoot") {
        search.push(PathBuf::from(system).join("System32"));
    }
    environment.push((
        "PATH".into(),
        std::env::join_paths(search).map_err(invalid)?,
    ));
    for (key, value) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_SYSTEM", "NUL"),
        ("GIT_CONFIG_GLOBAL", "NUL"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "Never"),
    ] {
        environment.push((key.into(), value.into()));
    }
    environment.push(("GIT_CEILING_DIRECTORIES".into(), home.as_os_str().into()));
    for key in ["GIT_ASKPASS", "SSH_ASKPASS", "GIT_SSH_COMMAND"] {
        environment.push((
            key.into(),
            member(root, "usr/bin/false.exe").into_os_string(),
        ));
    }
    let mut arguments = vec![
        "-c".into(),
        "core.longpaths=true".into(),
        "-c".into(),
        "credential.helper=".into(),
        "-c".into(),
        "core.hooksPath=NUL".into(),
    ];
    arguments.extend(args);
    let mut process = OwnedProcess::spawn(Spawn {
        program: member(root, "cmd/git.exe"),
        arguments,
        current_directory: home.into(),
        environment,
    })?;
    drop(process.stdin.take());
    let read = |mut file: File| -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 {
            return Err(invalid("MinGit probe output exceeded budget"));
        }
        Ok(bytes)
    };
    let stdout = process.stdout.take().unwrap();
    let stderr = process.stderr.take().unwrap();
    let output = thread::spawn(move || read(stdout));
    let errors = thread::spawn(move || read(stderr));
    let status = process.wait(Duration::from_secs(5));
    let closed = process.wait_empty(Duration::from_secs(2));
    if !matches!(status, Ok(Some(0))) || !matches!(closed, Ok(true)) {
        let terminated = process.terminate(137);
        let reaped = process.wait(Duration::from_secs(2));
        let stopped = process.wait_empty(Duration::from_secs(2));
        drop(process);
        let _ = output.join();
        let _ = errors.join();
        if terminated.is_err() || !matches!(reaped, Ok(Some(_))) || !matches!(stopped, Ok(true)) {
            return Err(invalid(
                "Probe outcome is unknown: MinGit process closure could not be confirmed",
            ));
        }
        return Err(invalid("MinGit probe failed or timed out"));
    }
    let output_result = output.join();
    let error_result = errors.join();
    let bytes = output_result.map_err(|_| invalid("MinGit output reader panicked"))??;
    error_result.map_err(|_| invalid("MinGit error reader panicked"))??;
    String::from_utf8(bytes)
        .map(|value| value.trim().into())
        .map_err(invalid)
}
fn probe_release(root: &Path) -> Result<()> {
    let scratch = root
        .parent()
        .unwrap()
        .join(format!(".windows-git-probe-{}", unique()));
    let mut created = payload::Created {
        files: Vec::new(),
        directories: Vec::new(),
    };
    created.directory(&scratch)?;
    let result = (|| {
        let home = scratch.join("home");
        let temporary = scratch.join("tmp");
        created.directory(&home)?;
        created.directory(&temporary)?;
        if run_probe(root, &home, &temporary, vec!["--version".into()])?
            != "git version 2.56.0.windows.4"
        {
            return Err(invalid("MinGit version differs from fixed release"));
        }
        let exec = run_probe(root, &home, &temporary, vec!["--exec-path".into()])?;
        let expected = native::open_no_reparse(&member(root, "ucrt64/libexec/git-core"))?.identity;
        if !expected.same_object(&native::open_no_reparse(Path::new(&exec))?.identity) {
            return Err(invalid("MinGit exec path escaped the verified resource"));
        }
        Ok(())
    })();
    if result
        .as_ref()
        .is_err_and(|error| error.to_string().contains("Probe outcome is unknown"))
    {
        return Err(invalid(format!(
            "{}; retained {}",
            result.unwrap_err(),
            scratch.display()
        )));
    }
    created.cleanup().map_err(|error| {
        invalid(format!(
            "MinGit probe scope retained at {}: {error}",
            scratch.display()
        ))
    })?;
    result
}

pub(super) fn prepare(
    source: &Path,
    output: &Path,
    expected_guard: Option<&Value>,
) -> Result<Value> {
    absolute(output)?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("Output parent missing"))?;
    let _parent = native::pin_directory(parent)?;
    let guard = payload::output_guard(output)?;
    if expected_guard.is_some_and(|expected| expected != &guard) {
        return Err(invalid(
            "Output parent identity changed during MinGit preparation",
        ));
    }
    let mut files = archive(source)?;
    files.insert("development-git.json".into(), RELEASE.into());
    let stage = parent.join(format!(".windows-git-stage-{}", unique()));
    let mut created = payload::Created {
        files: Vec::new(),
        directories: Vec::new(),
    };
    created.directory(&stage)?;
    let expected = created.directories[0].1.clone();
    let mut moved = false;
    let result = (|| {
        {
            let _stage = native::pin_directory(&stage)?;
            for (name, bytes) in files {
                created.write(&stage, &name, &bytes)?;
            }
            verify(&stage)?;
            probe_release(&stage)?;
            verify(&stage)?;
        }
        native::rename_directory_noreplace_expected(&stage, output, &expected)?;
        moved = true;
        verify(output)
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) if error.to_string().contains("Probe outcome is unknown") => {
            Err(invalid(format!("{error}; retained {}", stage.display())))
        }
        Err(error) if moved || error.to_string().contains("Publication outcome is unknown") => {
            Err(invalid(format!(
                "Publication outcome is unknown after the move: {error}; retained {}",
                output.display()
            )))
        }
        Err(error) => match created.cleanup() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(invalid(format!(
                "{error}; owned MinGit staging retained at {}: {cleanup}",
                stage.display()
            ))),
        },
    }
}

pub(super) fn receive(cache: &Path) -> Result<Value> {
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
    let temporary = cache.join(format!(".mingit-download-{}", unique()));
    let mut file = native::create_private_file(&temporary)?;
    let expected = native::identity(&file)?;
    let result = (|| {
        let mut source = io::stdin().lock().take(SIZE + 1);
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > SIZE {
                return Err(invalid("MinGit archive download budget exceeded"));
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
        }
        file.sync_all()?;
        if total != SIZE || format!("{:x}", hash.finalize()) != SHA {
            return Err(invalid("Downloaded MinGit size/SHA-256 mismatch"));
        }
        native::rename_file_noreplace_expected(&temporary, &cache.join(ARCHIVE_NAME), &expected)?;
        Ok(json!({"archive":cache.join(ARCHIVE_NAME),"sha256":SHA,"size":total}))
    })();
    drop(file);
    match result {
        Ok(value) => Ok(value),
        Err(error) if error.to_string().contains("Publication outcome is unknown") => Err(error),
        Err(error) => match native::remove_file(&temporary, &expected) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(invalid(format!(
                "{error}; MinGit download cleanup unconfirmed at {}: {cleanup}",
                temporary.display()
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn archive_digest_gate_preserves_untrusted_input_without_zip_parse_or_execution() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("caller.zip");
        fs::write(&path, b"caller bytes are not a ZIP").unwrap();
        assert!(archive(&path).unwrap_err().to_string().contains("SHA-256"));
        assert_eq!(fs::read(path).unwrap(), b"caller bytes are not a ZIP");
    }
    #[test]
    fn replaced_output_parent_stops_mingit_before_archive_access() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let output = parent.join("output");
        let expected = payload::output_guard(&output).unwrap();
        fs::rename(&parent, root.path().join("retained-parent")).unwrap();
        fs::create_dir(&parent).unwrap();
        let error = prepare(&root.path().join("absent.zip"), &output, Some(&expected)).unwrap_err();
        assert!(error.to_string().contains("parent identity changed"));
        assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
    }
}
