// Existing Unix private-file behavior, preserved in the platform adapter.
impl NodeConfigurationStore {
    fn lock(&self) -> Result<File> {
        checked_directory(&self.directory)?;
        let mut options = safe_options();
        options.read(true).write(true).create(true);
        let file = options
            .open(self.directory.join("configuration.lock"))
            .map_err(|_| invalid("无法锁定应用配置。"))?;
        checked_file(&file)?;
        file.lock_exclusive()
            .map_err(|_| invalid("无法锁定应用配置。"))?;
        Ok(file)
    }

    fn read(&self) -> Result<StoredConfiguration> {
        let mut options = safe_options();
        options.read(true);
        let file = match options.open(self.directory.join("configuration.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredConfiguration {
                    version: 1,
                    instance_id: self.instance.to_string(),
                    revision: 0,
                    values: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                })
            }
            Err(_) => return Err(invalid("无法读取应用配置。")),
        };
        checked_file(&file)?;
        if file
            .metadata()
            .map_err(|_| invalid("无法读取应用配置。"))?
            .len()
            > MAX_RECORD_BYTES
        {
            return Err(invalid("应用配置文件超过大小限制。"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("无法读取应用配置。"))?;
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(invalid("应用配置文件超过大小限制。"));
        }
        let record: StoredConfiguration =
            serde_json::from_slice(&bytes).map_err(|_| invalid("应用配置记录格式无效。"))?;
        if record.version != 1 || record.instance_id != self.instance.to_string() {
            return Err(invalid("应用配置与当前实例不匹配。"));
        }
        Ok(record)
    }

    fn commit(&self, record: &StoredConfiguration) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(record).map_err(|_| invalid("无法编码应用配置。"))?;
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(invalid("应用配置超过大小限制。"));
        }
        let temporary = self
            .directory
            .join(format!(".configuration-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = safe_options();
            options.write(true).create_new(true);
            let mut file = options
                .open(&temporary)
                .map_err(|_| invalid("无法准备应用配置保存。"))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("无法写入应用配置，原配置已保留。"))?;
            fs::rename(&temporary, self.directory.join("configuration.json"))
                .map_err(|_| invalid("无法提交应用配置，原配置已保留。"))?;
            // The successful rename is the commit point. A directory fsync is a
            // best-effort power-loss durability improvement, not a second commit.
            if let Ok(directory) = File::open(&self.directory) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn checked_directory(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid("应用配置目录必须为绝对普通目录。"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| invalid("应用配置目录不可读取。"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("应用配置目录不得经过链接或特殊文件。"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(path).map_err(|_| invalid("应用配置目录不可读取。"))?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("应用配置目录不属于当前用户。"));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| invalid("无法保护应用配置目录。"))?;
    }
    Ok(())
}

fn safe_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}

fn checked_file(file: &File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("应用配置文件不可读取。"))?;
    if !metadata.is_file() {
        return Err(invalid("应用配置不得使用特殊文件。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("应用配置不得使用硬链接或其他用户的文件。"));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| invalid("无法保护应用配置文件。"))?;
    }
    Ok(())
}

/// Local plaintext credential storage, selected explicitly by the user. This
/// implementation never calls an OS credential service and never falls back to
/// a different provider. Each instance gets its own private directory.
pub struct LocalFileVault {
    directory: PathBuf,
    instance: Uuid,
    created: std::sync::Mutex<BTreeMap<Uuid, (u64, u64)>>,
}

impl LocalFileVault {
    pub fn new(directory: PathBuf, instance: Uuid) -> Result<Self> {
        if !cfg!(unix) {
            return Err(invalid("当前平台尚未验证本地密钥文件权限。"));
        }
        if instance.is_nil() || !directory.is_absolute() {
            return Err(invalid("本地密钥目录或应用实例无效。"));
        }
        let parent = directory
            .parent()
            .ok_or_else(|| invalid("本地密钥目录无效。"))?;
        checked_directory(parent)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(invalid("无法创建本地密钥目录。")),
        }
        checked_credential_directory(&directory)?;
        Ok(Self {
            directory,
            instance,
            created: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    fn entry_path(&self, instance: Uuid, entry: Uuid) -> Result<PathBuf> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("本地密钥条目不属于此应用实例。"));
        }
        checked_credential_directory(&self.directory)?;
        Ok(self.directory.join(entry.to_string()))
    }

    fn open_entry(&self, instance: Uuid, entry: Uuid) -> Result<Option<File>> {
        let path = self.entry_path(instance, entry)?;
        let mut options = safe_options();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Inspecting a malicious special file must not block on a FIFO.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid("无法读取本地密钥文件，请重新填写密钥。")),
        };
        checked_credential_file(&file)?;
        Ok(Some(file))
    }
}

fn checked_credential_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(invalid("本地密钥目录必须为绝对路径。"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if !matches!(
            component,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        ) {
            return Err(invalid("本地密钥目录必须为规范路径。"));
        }
        current.push(component);
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| invalid("无法检查本地密钥目录。"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid("本地密钥目录不能经过链接或特殊文件。"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(|_| invalid("无法检查本地密钥目录。"))?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
            return Err(invalid("本地密钥目录必须属于当前用户且权限为 0700。"));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(invalid("当前平台尚未验证本地密钥目录权限。"))
    }
}

fn checked_credential_file(file: &File) -> Result<(u64, u64)> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("无法检查本地密钥文件。"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SECRET_BYTES as u64 {
        return Err(invalid("本地密钥文件类型或大小无效，请重新填写密钥。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(invalid(
                "本地密钥文件必须属于当前用户、权限为 0600，且不能使用链接。",
            ));
        }
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Err(invalid("当前平台尚未验证本地密钥文件权限。"))
    }
}

impl SecretVault for LocalFileVault {
    fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
        validate_secret(value)?;
        let path = self.entry_path(instance, entry)?;
        let mut options = safe_options();
        options.write(true).create_new(true);
        let mut file = options
            .open(&path)
            .map_err(|_| invalid("无法新建本地密钥文件，原配置已保留。"))?;
        let written = file
            .write_all(value.as_bytes())
            .and_then(|_| file.sync_all());
        if written.is_err() {
            drop(file);
            // create_new succeeded: this path was created by this operation.
            let _ = fs::remove_file(&path);
            return Err(invalid("无法写入本地密钥文件，原配置已保留。"));
        }
        let identity = match checked_credential_file(&file) {
            Ok(identity) => identity,
            Err(error) => {
                drop(file);
                let _ = fs::remove_file(&path);
                return Err(error);
            }
        };
        if File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(invalid("无法完成本地密钥文件保存，原配置已保留。"));
        }
        self.created
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(entry, identity);
        Ok(())
    }

    fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
        let file = self
            .open_entry(instance, entry)?
            .ok_or_else(|| invalid("本地密钥文件缺失，请重新填写并保存密钥。"))?;
        let mut bytes = Vec::new();
        file.take((MAX_SECRET_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("无法读取本地密钥文件，请重新填写密钥。"))?;
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(invalid("本地密钥文件超过大小限制。"));
        }
        let value = String::from_utf8(bytes)
            .map_err(|_| invalid("本地密钥文件格式无效，请重新填写密钥。"))?;
        validate_secret(&value)?;
        Ok(value)
    }

    fn discard(&self, instance: Uuid, entry: Uuid) -> Result<()> {
        let mut created = self.created.lock().unwrap_or_else(|p| p.into_inner());
        let expected = created
            .get(&entry)
            .ok_or_else(|| invalid("只能清理本次新建的密钥文件。"))?;
        if let Some(file) = self.open_entry(instance, entry)? {
            if checked_credential_file(&file)? != *expected {
                return Err(invalid("密钥文件已变化，已保留现场。"));
            }
            fs::remove_file(self.entry_path(instance, entry)?)
                .map_err(|_| invalid("无法清理本次未提交的密钥文件。"))?;
        }
        created.remove(&entry);
        if let Ok(directory) = File::open(&self.directory) {
            let _ = directory.sync_all();
        }
        Ok(())
    }

    fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
        self.open_entry(instance, entry).map(|file| file.is_some())
    }
}
