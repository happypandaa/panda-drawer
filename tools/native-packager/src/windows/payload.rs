use super::*;

const RUNTIME: &[u8] = include_bytes!("../../../../scripts/node-runtime-release.windows-x64.json");
const DEVELOPMENT: &[u8] =
    include_bytes!("../../../../scripts/development-node-release.windows-x64.json");

/// Staged native creation can fail after a real child exists. Keep every
/// enclosing probe scope before any success/failure cleanup continuation runs.
/// Probe wrappers may preserve only the original reason's text, so recognize
/// both the established probe receipt and the native created-owner receipt.
pub(super) fn preserve_unknown_probe<T>(
    result: Result<T>,
    scope: &Path,
    otherwise: impl FnOnce(Result<T>) -> Result<T>,
) -> Result<T> {
    if let Err(error) = &result {
        let reason = error.to_string();
        if reason.contains("Probe outcome is unknown")
            || reason.contains("resource closure outcome is unknown")
        {
            return Err(invalid(format!("{error}; retained {}", scope.display())));
        }
    }
    otherwise(result)
}

fn release(kind: &std::ffi::OsStr) -> Result<(&'static [u8], bool)> {
    match kind.to_str() {
        Some("runtime") => Ok((RUNTIME, false)),
        Some("development") => Ok((DEVELOPMENT, true)),
        _ => Err(invalid("Expected runtime or development payload kind")),
    }
}
fn runtime_metadata() -> Result<Vec<u8>> {
    let mut source: Value = serde_json::from_slice(RUNTIME).map_err(invalid)?;
    for key in ["archiveName", "archiveRoot"] {
        source.as_object_mut().unwrap().remove(key);
    }
    source["source"]
        .as_object_mut()
        .unwrap()
        .remove("checksumUrls");
    pretty(&source)
}
fn pretty(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(invalid)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn member(root: &Path, name: &str) -> PathBuf {
    root.join(Path::new(name).components().collect::<PathBuf>())
}
fn report(root: &Path, development: bool) -> Value {
    if development {
        json!({"root":root,"nodePath":root.join("node.exe"),"npmPath":member(root,"node_modules/npm/bin/npm-cli.js"),"npxPath":member(root,"node_modules/npm/bin/npx-cli.js"),"binPath":root,"nodeVersion":"24.22.0","npmVersion":"11.19.1"})
    } else {
        json!({"outputDir":root,"nodePath":root.join("node.exe"),"version":"24.22.0","platform":"win32","arch":"x64"})
    }
}

fn probe(root: &Path, arguments: Vec<OsString>) -> Result<String> {
    use platform_native::windows::process::{OwnedProcess, Spawn};
    use std::{thread, time::Duration};
    let mut environment = Vec::new();
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
        "TEMP",
        "TMP",
    ] {
        environment.push((key.into(), root.as_os_str().into()));
    }
    environment.push(("PATH".into(), root.as_os_str().into()));
    // npm enables Node's optional on-disk compile cache. Version probes must
    // remain read-only and must never write the developer's real temp/home.
    environment.push(("NODE_DISABLE_COMPILE_CACHE".into(), "1".into()));
    environment.push((
        "npm_config_cache".into(),
        root.join(".npm-cache").into_os_string(),
    ));
    environment.push(("npm_config_update_notifier".into(), "false".into()));
    environment.push(("npm_config_logs_max".into(), "0".into()));
    let mut process = OwnedProcess::spawn(Spawn {
        program: root.join("node.exe"),
        arguments,
        current_directory: root.into(),
        environment,
    })?;
    drop(process.stdin.take());
    let read = |mut file: File| {
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(invalid("Node probe output exceeds budget"));
        }
        Ok(bytes)
    };
    let stdout = process.stdout.take().unwrap();
    let stderr = process.stderr.take().unwrap();
    let output = thread::spawn(move || read(stdout));
    let errors = thread::spawn(move || read(stderr));
    let status = process.wait(Duration::from_secs(10));
    let closed = process.wait_empty(Duration::from_secs(2));
    if !matches!(status, Ok(Some(0))) || !matches!(closed, Ok(true)) {
        let terminated = process.terminate(137);
        let reaped = process.wait(Duration::from_secs(2));
        let stopped = process.wait_empty(Duration::from_secs(2));
        // Closing the Job also requests termination. Join readers even when a
        // confirmation failed, and retain staging instead of claiming rollback.
        drop(process);
        let _ = output.join();
        let _ = errors.join();
        if terminated.is_err() || !matches!(reaped, Ok(Some(_))) || !matches!(stopped, Ok(true)) {
            return Err(invalid(
                "Probe outcome is unknown: process closure could not be confirmed",
            ));
        }
        return Err(invalid("Node probe failed or timed out"));
    }
    let output_result = output.join();
    let error_result = errors.join();
    let bytes = output_result.map_err(|_| invalid("Node probe output reader panicked"))??;
    error_result.map_err(|_| invalid("Node probe error reader panicked"))??;
    if !matches!(status?, Some(0)) || !closed? {
        return Err(invalid("Node probe failed or timed out"));
    }
    String::from_utf8(bytes)
        .map(|value| value.trim().into())
        .map_err(invalid)
}

fn probe_release(root: &Path, development: bool) -> Result<()> {
    if probe(root, vec!["--version".into()])? != "v24.22.0" {
        return Err(invalid("Node version differs from fixed release"));
    }
    let identity: Value = serde_json::from_str(&probe(root, vec!["--input-type=commonjs".into(),"--eval".into(),"process.stdout.write(JSON.stringify({version:process.versions.node,platform:process.platform,arch:process.arch}))".into()])?).map_err(invalid)?;
    if identity != json!({"version":"24.22.0","platform":"win32","arch":"x64"}) {
        return Err(invalid("Node execution target differs from fixed release"));
    }
    if development {
        for name in ["npm", "npx"] {
            if probe(
                root,
                vec![
                    member(root, &format!("node_modules/npm/bin/{name}-cli.js")).into_os_string(),
                    "--version".into(),
                ],
            )? != "11.19.1"
            {
                return Err(invalid("npm/npx version differs from fixed release"));
            }
        }
    }
    Ok(())
}
pub(super) fn inspect_tree(root: &Path) -> Result<Files> {
    let mut files = Files::new();
    let mut total = 0usize;
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Files,
        total: &mut usize,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err(invalid("Resource tree depth exceeded"));
        }
        let pin = native::pin_directory(directory)?;
        let before = Facts::from_file(&pin.file)?;
        let mut nonempty = false;
        for entry in fs::read_dir(directory)? {
            nonempty = true;
            let path = entry?.path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|part| {
                    part.as_os_str()
                        .to_str()
                        .ok_or_else(|| invalid("Non UTF-8 resource member"))
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            relative(&name)?;
            let info = Facts::capture(&path)?;
            if info.is_dir() {
                visit(root, &path, files, total, depth + 1)?;
            } else {
                let bytes = read_ordinary(&path, 160 * 1024 * 1024)?;
                *total = total
                    .checked_add(bytes.len())
                    .filter(|size| *size <= MAX_TREE + 65536)
                    .ok_or_else(|| invalid("Resource tree byte budget exceeded"))?;
                if files.len() >= 10_001 {
                    return Err(invalid("Resource file budget exceeded"));
                }
                files.insert(name, bytes);
            }
        }
        if !nonempty || !before.unchanged(&Facts::from_file(&pin.file)?) {
            return Err(invalid(
                "Resource directory empty or changed during verification",
            ));
        }
        Ok(())
    }
    visit(root, root, &mut files, &mut total, 0)?;
    Ok(files)
}

pub(super) fn verify(kind: &std::ffi::OsStr, root: &Path) -> Result<Value> {
    absolute(root)?;
    let (_, development) = release(kind)?;
    let mut files = inspect_tree(root)?;
    if development {
        if files.remove("development-node.json").as_deref() != Some(DEVELOPMENT) {
            return Err(invalid("Development metadata differs from compiled source"));
        }
        let expected: Value = serde_json::from_slice(DEVELOPMENT).map_err(invalid)?;
        if tree(&files) != expected["tree"] {
            return Err(invalid(
                "Development Node/npm tree differs from compiled release",
            ));
        }
    } else {
        let metadata = runtime_metadata()?;
        if files.remove("runtime.json").as_deref() != Some(metadata.as_slice()) || files.len() != 2
        {
            return Err(invalid("Runtime metadata or member set differs"));
        }
        let source: Value = serde_json::from_slice(RUNTIME).map_err(invalid)?;
        for name in ["node.exe", "LICENSE"] {
            let data = files
                .get(name)
                .ok_or_else(|| invalid("Runtime member missing"))?;
            let field = if name == "node.exe" {
                "binary"
            } else {
                "license"
            };
            if digest(data) != source[field]["sha256"].as_str().unwrap() {
                return Err(invalid("Runtime member differs from compiled release"));
            }
        }
    }
    verify_pe(
        files
            .get("node.exe")
            .ok_or_else(|| invalid("Node missing"))?,
    )?;
    Ok(report(root, development))
}

pub(super) struct Created {
    pub(super) files: Vec<(PathBuf, native::FileIdentity)>,
    pub(super) directories: Vec<(PathBuf, native::FileIdentity)>,
}
impl Created {
    pub(super) fn directory(&mut self, path: &Path) -> Result<()> {
        native::create_private_directory(path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Creating {}: {error}", path.display()),
            )
        })?;
        self.directories
            .push((path.into(), native::open_no_reparse(path)?.identity));
        Ok(())
    }
    pub(super) fn write(&mut self, root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
        // Extended Win32 paths require native separators; Path::join preserves
        // literal '/' in a string even though components() recognizes it.
        let relative: PathBuf = Path::new(name).components().collect();
        let path = root.join(relative);
        let mut parent = root.to_path_buf();
        for component in Path::new(name).parent().unwrap().components() {
            parent.push(component);
            if !self.directories.iter().any(|(known, _)| known == &parent) {
                self.directory(&parent)?;
            }
        }
        let mut file = native::create_private_file(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Creating {}: {error}", path.display()),
            )
        })?;
        self.files.push((path, native::identity(&file)?));
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    pub(super) fn cleanup(&self) -> Result<()> {
        // Establish the complete original directory scope before removing any
        // child. Matching a moved file's ID alone cannot authorize its removal
        // from an unrelated same-name replacement directory.
        let mut pins = Vec::new();
        for (path, expected) in &self.directories {
            let pin = native::pin_directory(path)?;
            if !expected.same_object(&native::identity(&pin.file)?) {
                return Err(invalid(
                    "Owned staging directory changed; retaining all material",
                ));
            }
            pins.push(pin);
        }
        for (path, expected) in self.files.iter().rev() {
            native::remove_file(path, expected)?;
        }
        for (path, expected) in self.directories.iter().rev() {
            drop(pins.pop());
            owned::remove_directory(path, expected)?;
        }
        Ok(())
    }
}

pub(super) fn output_guard(output: &Path) -> Result<Value> {
    absolute(output)?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("Output parent missing"))?;
    let _pin = native::pin_directory(parent)?;
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Output already exists",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut chain = Vec::new();
    for component in platform_native::filesystem::path_components(parent) {
        let captured = native::open_no_reparse(&component)?.identity;
        if !captured.directory {
            return Err(invalid("Output ancestor is not a directory"));
        }
        chain.push(json!({"volumeSerial":format!("{:016x}",captured.volume_serial),"fileId":captured.file_id}));
    }
    Ok(Value::Array(chain))
}

pub(super) fn prepare(
    kind: &std::ffi::OsStr,
    source: &Path,
    output: &Path,
    expected_guard: Option<&Value>,
) -> Result<Value> {
    absolute(output)?;
    let (_, development) = release(kind)?;
    let _parent = native::pin_directory(
        output
            .parent()
            .ok_or_else(|| invalid("Output parent missing"))?,
    )?;
    let current_guard = output_guard(output)?;
    if expected_guard.is_some_and(|expected| expected != &current_guard) {
        return Err(invalid("Output parent identity changed during preparation"));
    }
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Output already exists",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let mut files = archive(source)?;
    let declarations = metadata(&files);
    if declarations["runtimeRelease"]
        != serde_json::from_slice::<Value>(RUNTIME).map_err(invalid)?
        || declarations["developmentRelease"]
            != serde_json::from_slice::<Value>(DEVELOPMENT).map_err(invalid)?
    {
        return Err(invalid(
            "Official archive differs from compiled release declarations",
        ));
    }
    if development {
        files.insert("development-node.json".into(), DEVELOPMENT.into());
    } else {
        files.retain(|name, _| ["node.exe", "LICENSE"].contains(&name.as_str()));
        files.insert("runtime.json".into(), runtime_metadata()?);
    }
    let stage = output
        .parent()
        .unwrap()
        .join(format!(".windows-node-stage-{}", unique()));
    let mut created = Created {
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
            verify(kind, &stage)?;
            probe_release(&stage, development).map_err(|error| {
                io::Error::new(error.kind(), format!("Probing verified Node: {error}"))
            })?;
            verify(kind, &stage)?;
        }
        native::rename_directory_noreplace_expected(&stage, output, &expected)?;
        moved = true;
        if !expected.same_object(&native::open_no_reparse(output)?.identity) {
            return Err(invalid("Published directory identity differs"));
        }
        verify(kind, output)
    })();
    preserve_unknown_probe(result, &stage, |result| match result {
        Ok(value) => Ok(value),
        Err(error) if moved || error.to_string().contains("Publication outcome is unknown") => {
            Err(invalid(format!(
                "Publication outcome is unknown after the move: {error}; retained {}",
                output.display()
            )))
        }
        Err(error) => match created.cleanup() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(invalid(format!(
                "{error}; owned staging retained at {}: {cleanup}",
                stage.display()
            ))),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_created_process_retains_each_enclosing_real_probe_scope() {
        use std::cell::Cell;
        let root = tempfile::Builder::new()
            .prefix("panda-unknown-probe-中文 space-")
            .tempdir()
            .unwrap();
        // These are the exact shared boundary used by Node staging, MinGit's
        // nested probe scratch and MinGit's outer staging, not real resources.
        for (index, label) in ["node-stage", "git-probe-scratch", "git-stage"]
            .into_iter()
            .enumerate()
        {
            let scope = root.path().join(label);
            let mut created = Created {
                files: Vec::new(),
                directories: Vec::new(),
            };
            created.directory(&scope).unwrap();
            created
                .write(&scope, "owned-evidence", b"preserve actual scope")
                .unwrap();
            let expected = native::open_no_reparse(&scope).unwrap().identity;
            let reason = if index == 0 {
                "Probing verified Node: Created process setup failed; resource closure outcome is unknown: controlled setup failure"
            } else if index == 1 {
                "Created process setup failed; resource closure outcome is unknown: controlled setup failure"
            } else {
                "Probe outcome is unknown: MinGit process closure could not be confirmed"
            };
            let entered_cleanup = Cell::new(false);
            let result: Result<()> =
                preserve_unknown_probe(Err(invalid(reason)), &scope, |result| {
                    entered_cleanup.set(true);
                    created.cleanup()?;
                    result
                });
            assert!(result.unwrap_err().to_string().contains(reason));
            assert!(!entered_cleanup.get());
            assert!(expected.same_object(&native::open_no_reparse(&scope).unwrap().identity));
            assert_eq!(
                fs::read(scope.join("owned-evidence")).unwrap(),
                b"preserve actual scope"
            );
        }
    }

    #[test]
    fn ordinary_probe_failure_still_runs_its_original_owned_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let scope = root.path().join("ordinary-probe-failure");
        let mut created = Created {
            files: Vec::new(),
            directories: Vec::new(),
        };
        created.directory(&scope).unwrap();
        created.write(&scope, "owned-only", b"disposable").unwrap();
        let result: Result<()> = preserve_unknown_probe(
            Err(invalid("Node probe failed or timed out")),
            &scope,
            |result| {
                created.cleanup()?;
                result
            },
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "Node probe failed or timed out"
        );
        assert!(!scope.exists());
    }
    #[test]
    fn private_writer_and_cleanup_use_actual_native_ids_and_protected_files() {
        let root = tempfile::tempdir().unwrap();
        let before = fs::metadata(root.path()).unwrap().permissions();
        let stage = root.path().join("owned 中文 stage");
        let mut made = Created {
            files: Vec::new(),
            directories: Vec::new(),
        };
        made.directory(&stage).unwrap();
        made.write(&stage, "nested/payload", b"owned bytes")
            .unwrap();
        native::verify_private(&native::open_no_reparse(&stage).unwrap().file).unwrap();
        native::verify_private(
            &native::open_read_file(&stage.join("nested").join("payload")).unwrap(),
        )
        .unwrap();
        made.cleanup().unwrap();
        assert!(!stage.exists());
        assert_eq!(
            before.readonly(),
            fs::metadata(root.path()).unwrap().permissions().readonly()
        );
    }

    #[test]
    fn cleanup_never_removes_a_matching_file_from_a_replaced_root_or_parent() {
        for nested in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let stage = root.path().join("owned-stage");
            let mut made = Created {
                files: Vec::new(),
                directories: Vec::new(),
            };
            made.directory(&stage).unwrap();
            made.write(&stage, "nested/payload", b"retain original file")
                .unwrap();
            let replaced = if nested {
                stage.join("nested")
            } else {
                stage.clone()
            };
            let held = root.path().join("retained-original");
            fs::rename(&replaced, &held).unwrap();
            native::create_private_directory(&replaced).unwrap();
            if !nested {
                native::create_private_directory(&stage.join("nested")).unwrap();
            }
            let original = if nested {
                held.join("payload")
            } else {
                held.join("nested").join("payload")
            };
            fs::rename(original, stage.join("nested").join("payload")).unwrap();
            assert!(made.cleanup().is_err());
            assert_eq!(
                fs::read(stage.join("nested").join("payload")).unwrap(),
                b"retain original file"
            );
            assert!(held.exists());
        }
    }
    #[test]
    fn preparation_refuses_a_replaced_output_parent_before_reading_the_archive() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("selected-parent");
        native::create_private_directory(&parent).unwrap();
        let output = parent.join("output");
        let expected = output_guard(&output).unwrap();
        fs::rename(&parent, root.path().join("retained-original-parent")).unwrap();
        native::create_private_directory(&parent).unwrap();
        let error = prepare(
            std::ffi::OsStr::new("runtime"),
            &root.path().join("nonexistent.zip"),
            &output,
            Some(&expected),
        )
        .unwrap_err();
        assert!(error.to_string().contains("parent identity changed"));
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        assert!(root.path().join("retained-original-parent").exists());
    }
}
