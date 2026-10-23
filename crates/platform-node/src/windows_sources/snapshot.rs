//! Stable, handle-retained source snapshots and create-only private copies.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SourceFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
    pub identity: WindowsObjectIdentity,
}
pub(super) struct PinnedFile {
    pub directory: WindowsSourceDirectory,
    pub metadata: platform_host::WindowsSourceEntry,
    pub record: SourceFile,
}
pub(super) struct Snapshot {
    pub root: WindowsSourceDirectory,
    pub files: BTreeMap<String, PinnedFile>,
    pub excluded: Vec<String>,
    entries: BTreeMap<String, platform_host::WindowsSourceEntry>,
    pub guards: Vec<WindowsSourceDirectory>,
}
impl Snapshot {
    pub fn digest(&self) -> String {
        digest_files(self.files.values().map(|file| &file.record))
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let file = self.files.get(path).ok_or_else(changed)?;
        let actual = file
            .directory
            .read_bounded(&file.metadata.name, MAX_BLOB as usize)
            .map_err(fs_error)?;
        if actual.identity != file.metadata.identity
            || actual.bytes.len() as u64 != file.record.size
            || sha(&actual.bytes) != file.record.sha256
        {
            return Err(changed());
        }
        Ok(actual.bytes)
    }
    pub fn verify(&self, control: &Control, exclude: bool) -> Result<()> {
        let current = scan(self.root.clone(), control, exclude)?;
        if self.entries != current.entries || self.excluded != current.excluded {
            return Err(changed());
        }
        for (path, file) in &self.files {
            let Some(actual) = current.files.get(path) else {
                return Err(changed());
            };
            // Executability comes from the immutable source proof on Windows,
            // while this rescan verifies the physical identity and bytes.
            if actual.record.path != file.record.path
                || actual.record.size != file.record.size
                || actual.record.sha256 != file.record.sha256
                || actual.record.identity != file.record.identity
            {
                return Err(changed());
            }
        }
        for guard in &self.guards {
            guard.verify_binding().map_err(fs_error)?;
        }
        Ok(())
    }
}
pub(super) fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn digest_files<'a>(files: impl Iterator<Item = &'a SourceFile>) -> String {
    let mut tree = Sha256::new();
    for file in files {
        tree.update(file.path.as_bytes());
        tree.update([0]);
        tree.update(file.size.to_le_bytes());
        tree.update([u8::from(file.executable)]);
        let digest: Vec<u8> = (0..file.sha256.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&file.sha256[index..index + 2], 16).unwrap())
            .collect();
        tree.update(digest);
    }
    format!("{:x}", tree.finalize())
}
pub(super) fn changed() -> ApiError {
    error(
        "SOURCE_LOCAL_CHANGED",
        "源码目录、文件身份或内容在检查期间变化；副本未提交，原目录未修改。",
    )
}
pub(super) fn scan(
    root: WindowsSourceDirectory,
    control: &Control,
    exclude: bool,
) -> Result<Snapshot> {
    let root = if exclude
        && root
            .source_entries(MAX_ENTRIES)
            .map_err(fs_error)?
            .iter()
            .any(|entry| entry.name == "project.json")
    {
        root.pin_file("project.json").map_err(fs_error)?
    } else {
        root
    };
    let output = if exclude {
        optional_json(&root, "project.json")
            .ok()
            .flatten()
            .and_then(|value| value["outputDir"].as_str().map(str::to_owned))
            .filter(|path| policy::validate_tree_path(path).is_ok())
    } else {
        None
    };
    let mut result = Snapshot {
        guards: vec![root.clone()],
        root,
        files: BTreeMap::new(),
        entries: BTreeMap::new(),
        excluded: Vec::new(),
    };
    struct ScanScope<'a> {
        output: Option<&'a str>,
        exclude: bool,
        count: usize,
        total: u64,
    }
    fn visit(
        directory: &WindowsSourceDirectory,
        prefix: &str,
        scope: &mut ScanScope<'_>,
        result: &mut Snapshot,
        control: &Control,
    ) -> Result<()> {
        control.check()?;
        let mut entries = directory.source_entries(MAX_ENTRIES).map_err(fs_error)?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            control.check()?;
            scope.count += 1;
            if scope.count > MAX_ENTRIES {
                return Err(limit());
            }
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            // Only the name is observed for an excluded file or link. No leaf
            // handle, contents, metadata query or recursion follows this branch.
            if scope.exclude && policy::local_excluded(&path, scope.output) {
                if path.len() > 1024
                    || result.excluded.len() >= 1000
                    || result.excluded.iter().map(String::len).sum::<usize>() + path.len() > 65536
                {
                    return Err(limit());
                }
                result.excluded.push(path);
                continue;
            }
            policy::validate_tree_path(&path)?;
            if entry.reparse {
                return Err(invalid("源码含未排除的链接；未跟随或复制。"));
            }
            let (pin, metadata) = directory.pin_source_entry(&entry).map_err(fs_error)?;
            result.entries.insert(path.clone(), metadata.clone());
            result.guards.push(pin.clone());
            if entry.is_directory {
                visit(&pin, &path, scope, result, control)?;
            } else {
                if result.files.len() >= MAX_FILES || entry.length > MAX_BLOB {
                    return Err(limit());
                }
                scope.total = scope.total.checked_add(entry.length).ok_or_else(limit)?;
                if scope.total > MAX_TREE {
                    return Err(limit());
                }
                let file = pin
                    .read_bounded(&entry.name, MAX_BLOB as usize)
                    .map_err(fs_error)?;
                if file.identity != entry.identity || file.bytes.len() as u64 != entry.length {
                    return Err(changed());
                }
                result.files.insert(
                    path.clone(),
                    PinnedFile {
                        directory: pin,
                        metadata,
                        record: SourceFile {
                            path,
                            size: file.bytes.len() as u64,
                            sha256: sha(&file.bytes),
                            executable: false,
                            identity: file.identity,
                        },
                    },
                );
            }
        }
        directory.verify_binding().map_err(fs_error)
    }
    visit(
        &result.root.clone(),
        "",
        &mut ScanScope {
            output: output.as_deref(),
            exclude,
            count: 0,
            total: 0,
        },
        &mut result,
        control,
    )?;
    let paths: Vec<u8> = result
        .files
        .values()
        .flat_map(|file| {
            format!(
                "100644 blob {} {}\t{}\0",
                "1".repeat(40),
                file.record.size,
                file.record.path
            )
            .into_bytes()
        })
        .collect();
    policy::parse_tree(&paths)?;
    Ok(result)
}

pub(super) struct Destination {
    pub root: WorkspaceDirectory,
    directories: BTreeMap<String, WorkspaceDirectory>,
    pub files: BTreeMap<String, SourceFile>,
}
impl Destination {
    pub fn new(root: WorkspaceDirectory) -> Self {
        Self {
            root,
            directories: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }
    pub fn write(&mut self, path: &str, bytes: &[u8], executable: bool) -> Result<()> {
        policy::validate_tree_path(path)?;
        if bytes.len() as u64 > MAX_BLOB {
            return Err(limit());
        }
        let mut parts: Vec<_> = path.split('/').collect();
        let file = parts.pop().unwrap();
        let mut directory = self.root.clone();
        let mut prefix = String::new();
        for part in parts {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            directory = if let Some(created) = self.directories.get(&prefix) {
                created.clone()
            } else {
                let created = directory.create_directory(part).map_err(fs_error)?;
                self.directories.insert(prefix.clone(), created.clone());
                created
            };
        }
        let identity = directory.write_new(file, bytes).map_err(fs_error)?;
        self.files.insert(
            path.into(),
            SourceFile {
                path: path.into(),
                size: bytes.len() as u64,
                sha256: sha(bytes),
                executable,
                identity,
            },
        );
        Ok(())
    }
    pub fn digest(&self) -> String {
        digest_files(self.files.values())
    }
}

pub(super) struct EvidenceSnapshot {
    root: WindowsSourceDirectory,
    entries: BTreeMap<String, platform_host::WindowsSourceEntry>,
    _guards: Vec<WindowsSourceDirectory>,
}
impl EvidenceSnapshot {
    pub fn verify(&self, control: &Control) -> Result<()> {
        let current = pin_evidence(&self.root, control)?;
        if self.entries != current.entries {
            return Err(invalid("Git对象证据在读取期间变化；未提交候选。"));
        }
        self.root.verify_binding().map_err(fs_error)
    }
}
pub(super) fn pin_evidence(
    root: &WindowsSourceDirectory,
    control: &Control,
) -> Result<EvidenceSnapshot> {
    fn visit(
        root: &WindowsSourceDirectory,
        control: &Control,
        guards: &mut Vec<WindowsSourceDirectory>,
        total: &mut u64,
        count: &mut usize,
        prefix: &str,
        entries: &mut BTreeMap<String, platform_host::WindowsSourceEntry>,
    ) -> Result<()> {
        control.check()?;
        if prefix.split('/').count() > 40 {
            return Err(limit());
        }
        for entry in root.source_entries(MAX_ENTRIES).map_err(fs_error)? {
            *count += 1;
            *total = total.checked_add(entry.length).ok_or_else(limit)?;
            if *count > MAX_ENTRIES || *total > MAX_DISK || entry.reparse {
                return Err(limit());
            }
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            let (pin, metadata) = root
                .pin_source_entry(&entry)
                .map_err(|issue| fs_error(issue).detail("evidenceEntry", path.clone()))?;
            if metadata.is_directory {
                visit(&pin, control, guards, total, count, &path, entries)?;
            }
            entries.insert(path, metadata);
            guards.push(pin);
        }
        Ok(())
    }
    let mut guards = vec![root.clone()];
    let mut entries = BTreeMap::new();
    visit(root, control, &mut guards, &mut 0, &mut 0, "", &mut entries)?;
    Ok(EvidenceSnapshot {
        root: root.clone(),
        entries,
        _guards: guards,
    })
}
