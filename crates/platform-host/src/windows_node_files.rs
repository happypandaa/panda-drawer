//! Handle-rooted Node source reads and private slot files. Source trees are
//! trusted executable inputs, not private platform storage or a sandbox.
use super::*;
use windows_sys::Win32::Storage::FileSystem::{
    FileBasicInfo, FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo, FILE_BASIC_INFO,
    FILE_ID_EXTD_DIR_INFO,
};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSourceEntry {
    pub name: String,
    pub is_directory: bool,
    pub identity: WindowsObjectIdentity,
    /// Directory size and timestamps are normalized to zero: NTFS directory
    /// enumeration caches can disagree with current handle metadata. Directory
    /// identity/type/attributes bind traversal; full snapshots bind membership.
    pub length: u64,
    pub last_write_time: i64,
    pub change_time: i64,
    pub attributes: u32,
    pub reparse: bool,
}

#[derive(Clone)]
pub struct WindowsSourceDirectory {
    ancestry: Arc<Chain>,
    lineage: Vec<Arc<Object>>,
    pins: Vec<Arc<Object>>,
    path: PathBuf,
    private: bool,
    operation: Arc<Mutex<()>>,
    #[cfg(feature = "test-fixtures")]
    fail_publication_verification: Arc<std::sync::atomic::AtomicBool>,
}

impl WindowsSourceDirectory {
    pub fn open(path: &Path) -> Result<Self> {
        let normalized;
        let path = if matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)))
        {
            let mut parts = path.components();
            let Some(Component::Prefix(prefix)) = parts.next() else {
                unreachable!()
            };
            let Prefix::VerbatimDisk(drive) = prefix.kind() else {
                unreachable!()
            };
            if !matches!(parts.next(), Some(Component::RootDir)) {
                return Err(error("NODE_SOURCE_PATH_INVALID", "目录不是绝对盘符路径。"));
            }
            let mut ordinary = PathBuf::from(format!("{}:\\", drive as char));
            for part in parts {
                let Component::Normal(name) = part else {
                    return Err(error("NODE_SOURCE_PATH_INVALID", "目录含非普通片段。"));
                };
                ordinary.push(name);
            }
            normalized = ordinary;
            &normalized
        } else {
            path
        };
        let ancestry = Arc::new(open_parent_mode(path, true)?);
        let mut canonical = vec![0u16; 32768];
        let length = unsafe {
            GetFinalPathNameByHandleW(
                ancestry.objects.last().unwrap().raw(),
                canonical.as_mut_ptr(),
                canonical.len() as u32,
                0,
            )
        };
        if length == 0 || length as usize >= canonical.len() {
            return Err(os_error("NODE_SOURCE_CANONICAL_UNAVAILABLE"));
        }
        let actual = PathBuf::from(String::from_utf16(&canonical[..length as usize]).map_err(
            |_| {
                error(
                    "NODE_SOURCE_CANONICAL_UNAVAILABLE",
                    "实际应用目录不是有效Unicode。",
                )
            },
        )?);
        let mut parts = actual.components();
        let Some(Component::Prefix(prefix)) = parts.next() else {
            return Err(error(
                "NODE_SOURCE_CANONICAL_UNAVAILABLE",
                "实际应用目录没有盘符。",
            ));
        };
        let drive = match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive.to_ascii_uppercase(),
            _ => {
                return Err(error(
                    "NODE_SOURCE_CANONICAL_UNAVAILABLE",
                    "实际应用目录不是本机盘符路径。",
                ))
            }
        };
        if drive != ancestry.drive || !matches!(parts.next(), Some(Component::RootDir)) {
            return Err(error(
                "NODE_SOURCE_CANONICAL_UNAVAILABLE",
                "实际应用目录盘符发生变化。",
            ));
        }
        let mut actual_path = PathBuf::from(format!("{}:\\", drive as char));
        for part in parts {
            let Component::Normal(name) = part else {
                return Err(error(
                    "NODE_SOURCE_CANONICAL_UNAVAILABLE",
                    "实际应用目录含非普通片段。",
                ));
            };
            actual_path.push(name);
        }
        let result = Self {
            ancestry,
            lineage: Vec::new(),
            pins: Vec::new(),
            path: actual_path,
            private: false,
            operation: Arc::new(Mutex::new(())),
            #[cfg(feature = "test-fixtures")]
            fail_publication_verification: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        result.verify()?;
        Ok(result)
    }
    pub fn open_private(path: &Path) -> Result<Self> {
        let mut result = Self::open(path)?;
        result.private = true;
        result.verify()?;
        Ok(result)
    }
    pub fn from_workspace(directory: &WorkspaceDirectory) -> Result<Self> {
        directory.verify_binding()?;
        Self::open_private(directory.path())
    }
    /// Creates one new private child beneath a retained ordinary parent. Only
    /// the new child receives the private ACL; the parent is never repaired.
    pub fn create_private_in(parent: &Path, name: &str) -> Result<Self> {
        let parent = Self::open(parent)?;
        let security = descriptor(&user_sid()?, true)?;
        let created = relative(
            parent.object().raw(),
            name,
            true,
            true,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            DIRECTORY_SHARING,
            Some(&security),
        )?;
        owner_and_acl(created.raw(), &user_sid()?, true)?;
        parent.verify()?;
        let mut result = Self::open_private(&parent.path.join(name))?;
        if result.identity() != &created.identity {
            return Err(error(
                "NODE_PRIVATE_DIRECTORY_CHANGED",
                "新建私有目录身份发生变化，现场已保留。",
            ));
        }
        result.pins.push(created);
        result.verify()?;
        Ok(result)
    }
    fn object(&self) -> &Arc<Object> {
        self.lineage
            .last()
            .unwrap_or_else(|| self.ancestry.objects.last().unwrap())
    }
    fn verify(&self) -> Result<()> {
        if drive_mapping(self.ancestry.drive)? != self.ancestry.mapping {
            return Err(error(
                "NODE_SOURCE_DRIVE_CHANGED",
                "应用目录盘符映射已变化。",
            ));
        }
        for object in self
            .ancestry
            .objects
            .iter()
            .chain(self.lineage.iter())
            .chain(self.pins.iter())
        {
            object.verify()?;
            if object.identity.volume_serial != self.ancestry.objects[0].identity.volume_serial {
                return Err(error(
                    "NODE_SOURCE_VOLUME_CHANGED",
                    "应用目录跨越了其他卷。",
                ));
            }
        }
        if self.private {
            let sid = user_sid()?;
            owner_and_acl(self.ancestry.objects.last().unwrap().raw(), &sid, true)?;
            for object in self.lineage.iter().chain(self.pins.iter()) {
                owner_and_acl(object.raw(), &sid, true)?;
            }
        }
        Ok(())
    }
    pub fn verify_binding(&self) -> Result<()> {
        self.verify()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn identity(&self) -> &WindowsObjectIdentity {
        &self.object().identity
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self.ancestry
            .objects
            .iter()
            .chain(self.lineage.iter())
            .any(|item| &item.identity == other.identity())
            || other
                .ancestry
                .objects
                .iter()
                .chain(other.lineage.iter())
                .any(|item| &item.identity == self.identity())
    }
    /// Pins metadata only, without reading arbitrary source or credential bytes.
    /// Files deny new write/delete handles; already occupied files are refused.
    pub fn pin_entry(&self, name: &str) -> Result<(Self, WindowsSourceEntry)> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录检查异常，已保留现场。"))?;
        self.verify()?;
        let listed = self.list_unlocked()?;
        let entry = listed
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| error("NODE_SOURCE_CHANGED", "目录条目已变化，请重新检查。"))?;
        let object = relative(
            self.object().raw(),
            name,
            entry.is_directory,
            false,
            FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | SYNCHRONIZE
                | if entry.is_directory {
                    FILE_TRAVERSE
                } else {
                    FILE_READ_DATA
                },
            if entry.is_directory {
                DIRECTORY_SHARING
            } else {
                FILE_SHARE_READ
            },
            None,
        )?;
        let standard = object.verify()?;
        let basic: FILE_BASIC_INFO = info(object.raw(), FileBasicInfo)?;
        if standard.EndOfFile < 0 {
            return Err(error("NODE_SOURCE_METADATA_INVALID", "目录条目长度无效。"));
        }
        let metadata = WindowsSourceEntry {
            name: name.into(),
            is_directory: entry.is_directory,
            identity: object.identity.clone(),
            length: standard.EndOfFile as u64,
            last_write_time: basic.LastWriteTime,
            change_time: basic.ChangeTime,
            attributes: basic.FileAttributes,
            reparse: false,
        };
        let mut pinned = self.clone();
        pinned.pins.push(object);
        pinned.verify()?;
        Ok((pinned, metadata))
    }
    /// Enumerates source metadata beneath the existing directory HANDLE only.
    /// Reparse entries are reported, never opened: callers can exclude .git,
    /// credentials and dependency trees before deciding which objects to pin.
    pub fn source_entries(&self, limit: usize) -> Result<Vec<WindowsSourceEntry>> {
        if limit == 0 || limit > 40_000 {
            return Err(error(
                "NODE_SOURCE_LIMIT_INVALID",
                "来源目录枚举上限须为1至40000。",
            ));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "来源枚举异常。"))?;
        self.verify()?;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut restart = true;
        loop {
            let mut buffer = vec![0u64; 65536 / 8];
            let class = if restart {
                FileIdExtdDirectoryRestartInfo
            } else {
                FileIdExtdDirectoryInfo
            };
            restart = false;
            if unsafe {
                GetFileInformationByHandleEx(
                    self.object().raw(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    65536,
                )
            } == 0
            {
                if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(os_error("NODE_SOURCE_ENUMERATION_FAILED"));
            }
            let mut offset = 0;
            loop {
                let name_offset = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
                if offset + size_of::<FILE_ID_EXTD_DIR_INFO>() > 65536 {
                    return Err(error(
                        "NODE_SOURCE_ENUMERATION_INVALID",
                        "来源目录数据不完整。",
                    ));
                }
                let item = unsafe {
                    &*buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_EXTD_DIR_INFO>()
                };
                let length = item.FileNameLength as usize;
                if length == 0
                    || length > 510
                    || !length.is_multiple_of(2)
                    || offset + name_offset + length > 65536
                {
                    return Err(error(
                        "NODE_SOURCE_ENUMERATION_INVALID",
                        "来源目录名称无效。",
                    ));
                }
                let units = unsafe {
                    std::slice::from_raw_parts(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset + name_offset)
                            .cast::<u16>(),
                        length / 2,
                    )
                };
                let name = String::from_utf16(units)
                    .map_err(|_| error("NODE_SOURCE_NAME_INVALID", "来源名称不是有效Unicode。"))?;
                if name != "." && name != ".." {
                    component(&name)?;
                    if entries.len() == limit {
                        return Err(error(
                            "NODE_SOURCE_ENUMERATION_LIMIT",
                            "来源目录超过本次有界枚举上限。",
                        ));
                    }
                    if !names.insert(name.to_lowercase()) {
                        return Err(error(
                            "NODE_SOURCE_NAME_COLLISION",
                            "来源目录含大小写名称冲突。",
                        ));
                    }
                    if item.EndOfFile < 0 {
                        return Err(error("NODE_SOURCE_METADATA_INVALID", "来源条目长度无效。"));
                    }
                    let directory = item.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                    entries.push(WindowsSourceEntry {
                        name,
                        is_directory: directory,
                        identity: WindowsObjectIdentity {
                            volume_serial: self.identity().volume_serial,
                            file_id128: item.FileId.Identifier,
                        },
                        length: if directory { 0 } else { item.EndOfFile as u64 },
                        last_write_time: if directory { 0 } else { item.LastWriteTime },
                        change_time: if directory { 0 } else { item.ChangeTime },
                        attributes: item.FileAttributes,
                        reparse: item.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                    });
                }
                if item.NextEntryOffset == 0 {
                    break;
                }
                let next = item.NextEntryOffset as usize;
                if next < name_offset + length || !next.is_multiple_of(8) || offset + next >= 65536
                {
                    return Err(error(
                        "NODE_SOURCE_ENUMERATION_INVALID",
                        "来源目录偏移无效。",
                    ));
                }
                offset += next;
            }
        }
        self.verify()?;
        entries.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }
    /// Opens the already-enumerated direct child without enumerating again,
    /// and binds its identity and metadata before it can be read or copied.
    pub fn pin_source_entry(
        &self,
        expected: &WindowsSourceEntry,
    ) -> Result<(Self, WindowsSourceEntry)> {
        if expected.reparse || expected.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(error(
                "NODE_SOURCE_REPARSE_REJECTED",
                "被选中的来源条目是重解析点。",
            ));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "来源读取异常。"))?;
        self.verify()?;
        let object = relative(
            self.object().raw(),
            &expected.name,
            expected.is_directory,
            false,
            FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | SYNCHRONIZE
                | if expected.is_directory {
                    FILE_LIST_DIRECTORY | FILE_TRAVERSE
                } else {
                    FILE_READ_DATA
                },
            if expected.is_directory {
                DIRECTORY_SHARING
            } else {
                FILE_SHARE_READ
            },
            None,
        )?;
        let standard = object.verify()?;
        let basic: FILE_BASIC_INFO = info(object.raw(), FileBasicInfo)?;
        if standard.EndOfFile < 0 {
            return Err(error("NODE_SOURCE_METADATA_INVALID", "来源条目长度无效。"));
        }
        let observed = WindowsSourceEntry {
            name: expected.name.clone(),
            is_directory: expected.is_directory,
            identity: object.identity.clone(),
            length: if expected.is_directory {
                0
            } else {
                standard.EndOfFile as u64
            },
            last_write_time: if expected.is_directory {
                0
            } else {
                basic.LastWriteTime
            },
            change_time: if expected.is_directory {
                0
            } else {
                basic.ChangeTime
            },
            attributes: basic.FileAttributes,
            reparse: false,
        };
        if &observed != expected {
            return Err(error(
                "NODE_SOURCE_CHANGED",
                "来源条目身份或内容元数据已变化，请重新检查。",
            ));
        }
        let mut guard = self.clone();
        if expected.is_directory {
            guard.lineage.push(object);
            guard.path.push(&expected.name);
        } else {
            guard.pins.push(object);
        }
        guard.verify()?;
        Ok((guard, observed))
    }
    /// Reopens the same external directory with creation rights. No ACL is
    /// supplied or repaired; the existing directory's access checks apply.
    fn source_writer(&self) -> Result<Arc<Object>> {
        if self.private {
            return Err(error(
                "NODE_EXTERNAL_DIRECTORY_REQUIRED",
                "此入口只用于普通外部目录。",
            ));
        }
        self.verify()?;
        let parent = if self.lineage.len() > 1 {
            &self.lineage[self.lineage.len() - 2]
        } else if self.lineage.len() == 1 {
            self.ancestry.objects.last().unwrap()
        } else {
            self.ancestry
                .objects
                .get(self.ancestry.objects.len().saturating_sub(2))
                .ok_or_else(|| error("NODE_SOURCE_PATH_INVALID", "不能初始化卷根目录。"))?
        };
        let name = self
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| error("NODE_SOURCE_PATH_INVALID", "外部目录名称无效。"))?;
        let writer = relative(
            parent.raw(),
            name,
            true,
            false,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            DIRECTORY_SHARING,
            None,
        )?;
        if writer.identity != *self.identity() {
            return Err(error("NODE_SOURCE_CHANGED", "外部目录身份已变化。"));
        }
        self.verify()?;
        Ok(writer)
    }
    pub fn create_source_directory(&self, name: &str) -> Result<Self> {
        self.create_source_directory_notified(name, || {})
    }
    pub fn create_source_directory_notified(
        &self,
        name: &str,
        on_created: impl FnOnce(),
    ) -> Result<Self> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let parent = self.source_writer()?;
        let child = relative_mode_with_hook(
            parent.raw(),
            name,
            true,
            true,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            DIRECTORY_SHARING,
            None,
            false,
            on_created,
        )?;
        let mut result = self.clone();
        result.lineage.push(child);
        result.path.push(name);
        result.verify()?;
        Ok(result)
    }
    /// Returns only a newly created file, with no write/delete sharing. On any
    /// later write failure the caller must retain the file and report its path.
    pub fn create_source_file(&self, name: &str) -> Result<(File, WindowsObjectIdentity)> {
        self.create_source_file_notified(name, || {})
    }
    pub fn create_source_file_notified(
        &self,
        name: &str,
        on_created: impl FnOnce(),
    ) -> Result<(File, WindowsObjectIdentity)> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let parent = self.source_writer()?;
        let object = relative_mode_with_hook(
            parent.raw(),
            name,
            false,
            true,
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ,
            None,
            false,
            on_created,
        )?;
        let handle = object
            .handle
            .0
            .try_clone()
            .map_err(|e| error("NODE_FILE_HANDLE_FAILED", e.to_string()))?;
        self.verify()?;
        Ok((File::from(handle), object.identity.clone()))
    }
    pub fn open_directory(&self, name: &str) -> Result<Self> {
        self.directory(name, false)
    }
    pub fn create_private_directory(&self, name: &str) -> Result<Self> {
        if !self.private {
            return Err(error(
                "NODE_PRIVATE_PARENT_REQUIRED",
                "受管目录必须从已验证的私有父目录创建。",
            ));
        }
        self.directory(name, true)
    }
    fn directory(&self, name: &str, create: bool) -> Result<Self> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常，已保留现场。"))?;
        self.verify()?;
        let sid = user_sid()?;
        let security = descriptor(&sid, true)?;
        let child = relative(
            self.object().raw(),
            name,
            true,
            create,
            DIRECTORY_ACCESS
                | if self.private {
                    FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
                } else {
                    0
                },
            DIRECTORY_SHARING,
            if create { Some(&security) } else { None },
        )?;
        if self.private {
            owner_and_acl(child.raw(), &sid, true)?;
        }
        let mut result = self.clone();
        result.lineage.push(child);
        result.path.push(name);
        result.verify()?;
        Ok(result)
    }
    fn file(&self, name: &str, access: u32, sharing: u32, create: bool) -> Result<Arc<Object>> {
        self.verify()?;
        let security = descriptor(&user_sid()?, false)?;
        let file = relative(
            self.object().raw(),
            name,
            false,
            create,
            access | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            sharing,
            if create { Some(&security) } else { None },
        )?;
        if self.private {
            owner_and_acl(file.raw(), &user_sid()?, true)?;
        }
        self.verify()?;
        Ok(file)
    }
    /// Retains the actual leaf handle as well as the complete parent chain.
    pub fn pin_file(&self, name: &str) -> Result<Self> {
        let file = self.file(name, FILE_READ_DATA, FILE_SHARE_READ, false)?;
        let mut result = self.clone();
        result.pins.push(file);
        result.verify()?;
        Ok(result)
    }
    pub fn open_file(&self, name: &str) -> Result<File> {
        let file = self.file(name, FILE_READ_DATA, FILE_SHARE_READ, false)?;
        let handle = file
            .handle
            .0
            .try_clone()
            .map_err(|e| error("NODE_FILE_HANDLE_FAILED", e.to_string()))?;
        file.verify()?;
        Ok(File::from(handle))
    }
    pub fn read_bounded(&self, name: &str, limit: usize) -> Result<WorkspaceFile> {
        if limit > MAX_BYTES {
            return Err(error(
                "NODE_FILE_LIMIT_INVALID",
                "读取上限超出受管文件范围。",
            ));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let object = self.file(name, FILE_READ_DATA, FILE_SHARE_READ, false)?;
        let size = object.verify()?.EndOfFile;
        if size < 0 || size as u64 > limit as u64 {
            return Err(error("WORKSPACE_FILE_TOO_LARGE", "文件超过读取上限。"));
        }
        let handle = object
            .handle
            .0
            .try_clone()
            .map_err(|e| error("NODE_FILE_HANDLE_FAILED", e.to_string()))?;
        let mut bytes = Vec::new();
        File::from(handle)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| error("NODE_FILE_READ_FAILED", e.to_string()))?;
        if bytes.len() as i64 != size || object.verify()?.EndOfFile != size {
            return Err(error("NODE_FILE_CHANGED", "读取期间文件发生变化。"));
        }
        self.verify()?;
        Ok(WorkspaceFile {
            bytes,
            identity: object.identity.clone(),
        })
    }
    pub fn write_new(&self, name: &str, bytes: &[u8]) -> Result<WindowsObjectIdentity> {
        if !self.private || bytes.len() > MAX_BYTES {
            return Err(error(
                "NODE_PRIVATE_WRITE_REJECTED",
                "仅允许在私有受管目录有界创建文件。",
            ));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let object = self.file(
            name,
            FILE_READ_DATA | FILE_WRITE_DATA,
            FILE_SHARE_READ,
            true,
        )?;
        let handle = object
            .handle
            .0
            .try_clone()
            .map_err(|e| error("NODE_FILE_HANDLE_FAILED", e.to_string()))?;
        let mut file = File::from(handle);
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|e| error("NODE_FILE_WRITE_FAILED", e.to_string()))?;
        if object.verify()?.EndOfFile != bytes.len() as i64 {
            return Err(error(
                "NODE_FILE_WRITE_INCOMPLETE",
                "写入未完成，现场已保留。",
            ));
        }
        self.verify()?;
        Ok(object.identity.clone())
    }
    /// A share-exclusive ordinary file provides an interprocess configuration
    /// lock. Existing nonempty or nonprivate files are never adopted/repaired.
    pub fn lock_private(&self, name: &str) -> Result<File> {
        if !self.private {
            return Err(error("NODE_PRIVATE_LOCK_REJECTED", "配置锁需要私有目录。"));
        }
        let object = match self.file(name, FILE_READ_DATA | FILE_WRITE_DATA, 0, true) {
            Ok(file) => file,
            Err(e) if e.code == "WORKSPACE_ALREADY_EXISTS" => {
                self.file(name, FILE_READ_DATA | FILE_WRITE_DATA, 0, false)?
            }
            Err(e) => return Err(e),
        };
        if object.verify()?.EndOfFile != 0 {
            return Err(error(
                "NODE_LOCK_CONTENT_INVALID",
                "配置锁含未知内容，已保留。",
            ));
        }
        let handle = object
            .handle
            .0
            .try_clone()
            .map_err(|e| error("NODE_FILE_HANDLE_FAILED", e.to_string()))?;
        Ok(File::from(handle))
    }
    pub fn publish_noreplace_checked(
        &self,
        source: &str,
        target: &str,
        expected: &WindowsObjectIdentity,
    ) -> Result<()> {
        if !self.private || source.eq_ignore_ascii_case(target) {
            return Err(error(
                "NODE_PUBLICATION_REJECTED",
                "发布名称或私有目录无效。",
            ));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let source = self.file(source, DELETE | FILE_READ_DATA, FILE_SHARE_READ, false)?;
        if &source.identity != expected {
            return Err(error(
                "NODE_PUBLICATION_IDENTITY_CHANGED",
                "暂存文件身份已变化，保留现场。",
            ));
        }
        let units = component(target)?;
        let length = offset_of!(FILE_RENAME_INFORMATION, FileName) + units.len() * 2;
        let mut storage = vec![
            0usize;
            length
                .max(size_of::<FILE_RENAME_INFORMATION>())
                .div_ceil(size_of::<usize>())
        ];
        let value = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        unsafe {
            (*value).Anonymous.ReplaceIfExists = false;
            (*value).RootDirectory = self.object().raw();
            (*value).FileNameLength = (units.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                units.as_ptr(),
                storage
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                    .cast::<u16>(),
                units.len(),
            );
        }
        let mut io = IO_STATUS_BLOCK::default();
        nt_result(
            unsafe {
                NtSetInformationFile(
                    source.raw(),
                    &mut io,
                    storage.as_ptr().cast(),
                    length as u32,
                    FileRenameInformation,
                )
            },
            "NODE_PUBLICATION_FAILED",
        )?;
        // Successful native rename is the commit point. A later diagnostic
        // failure must never be reported as a guaranteed uncommitted write.
        #[cfg(feature = "test-fixtures")]
        if self
            .fail_publication_verification
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(error(
                "NODE_PUBLICATION_COMMITTED_UNVERIFIED",
                "发布已到达提交点，后置核对未确认；保留所有修订与凭据。",
            ));
        }
        (|| {
            source.verify()?;
            verify_leaf_name(source.raw(), target)?;
            self.verify()
        })()
        .map_err(|_| {
            error(
                "NODE_PUBLICATION_COMMITTED_UNVERIFIED",
                "发布已到达提交点，后置核对未确认；保留所有修订与凭据。",
            )
        })
    }
    /// Deletes one expected file only. No recursive cleanup, path reopen,
    /// replacement-object deletion or implicit recovery is offered.
    pub fn remove_file_checked(&self, name: &str, expected: &WindowsObjectIdentity) -> Result<()> {
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
        };
        if !self.private {
            return Err(error("NODE_PRIVATE_DELETE_REJECTED", "清理需要私有目录。"));
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        let file = self.file(name, DELETE | FILE_READ_DATA, FILE_SHARE_READ, false)?;
        if &file.identity != expected {
            return Err(error(
                "NODE_DELETE_IDENTITY_CHANGED",
                "文件身份已变化，已保留。",
            ));
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                file.raw(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(os_error("NODE_FILE_DELETE_FAILED"));
        }
        self.verify()
    }
    pub fn list(&self) -> Result<Vec<WorkspaceEntry>> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| error("NODE_DIRECTORY_POISONED", "目录操作异常。"))?;
        self.list_unlocked()
    }
    fn list_unlocked(&self) -> Result<Vec<WorkspaceEntry>> {
        self.verify()?;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut restart = true;
        loop {
            let mut buffer = vec![0u64; 65536 / 8];
            let class = if restart {
                FileIdBothDirectoryRestartInfo
            } else {
                FileIdBothDirectoryInfo
            };
            restart = false;
            if unsafe {
                GetFileInformationByHandleEx(
                    self.object().raw(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    65536,
                )
            } == 0
            {
                if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(os_error("NODE_ENUMERATION_FAILED"));
            }
            let mut offset = 0;
            loop {
                let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > 65536 {
                    return Err(error("NODE_ENUMERATION_INVALID", "目录数据不完整。"));
                }
                let item = unsafe {
                    &*buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>()
                };
                let length = item.FileNameLength as usize;
                if length == 0
                    || length > 510
                    || !length.is_multiple_of(2)
                    || offset + name_offset + length > 65536
                {
                    return Err(error("NODE_ENUMERATION_INVALID", "目录名称无效。"));
                }
                let units = unsafe {
                    std::slice::from_raw_parts(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset + name_offset)
                            .cast::<u16>(),
                        length / 2,
                    )
                };
                let name = String::from_utf16(units)
                    .map_err(|_| error("NODE_NAME_INVALID", "目录名不是有效Unicode。"))?;
                if name != "." && name != ".." {
                    component(&name)?;
                    if entries.len() == MAX_ENTRIES {
                        return Err(error("NODE_ENUMERATION_LIMIT", "目录条目超过受管上限。"));
                    }
                    if !names.insert(name.to_lowercase()) {
                        return Err(error("NODE_NAME_COLLISION", "目录存在名称冲突。"));
                    }
                    if item.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(error("NODE_REPARSE_REJECTED", "目录含重解析点，已拒绝。"));
                    }
                    entries.push(WorkspaceEntry {
                        name,
                        is_directory: item.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    });
                }
                if item.NextEntryOffset == 0 {
                    break;
                }
                let next = item.NextEntryOffset as usize;
                if next < name_offset + length || !next.is_multiple_of(8) || offset + next >= 65536
                {
                    return Err(error("NODE_ENUMERATION_INVALID", "目录偏移无效。"));
                }
                offset += next;
            }
        }
        self.verify()?;
        Ok(entries)
    }
    #[cfg(feature = "test-fixtures")]
    pub fn fail_next_publication_verification_for_test(&self) {
        self.fail_publication_verification
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(feature = "test-fixtures")]
pub fn disposable_node_directory(parent: &Path) -> Result<WindowsSourceDirectory> {
    let name = parent
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| error("NODE_TEST_PATH_INVALID", "夹具名无效。"))?;
    if !name.starts_with("windows-node-disposable-") {
        return Err(error(
            "NODE_TEST_PATH_INVALID",
            "只允许明确命名的新Node可丢弃夹具。",
        ));
    }
    let directory = WindowsSourceDirectory::open(
        parent
            .parent()
            .ok_or_else(|| error("NODE_TEST_PATH_INVALID", "夹具父目录无效。"))?,
    )?;
    let security = descriptor(&user_sid()?, true)?;
    let created = relative(
        directory.object().raw(),
        name,
        true,
        true,
        DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
        DIRECTORY_SHARING,
        Some(&security),
    )?;
    owner_and_acl(created.raw(), &user_sid()?, true)?;
    WindowsSourceDirectory::open_private(parent)
}

#[cfg(test)]
#[path = "windows_node_files_tests.rs"]
mod tests;
