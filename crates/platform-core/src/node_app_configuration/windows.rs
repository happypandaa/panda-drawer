// Windows equivalent of the existing plaintext 0700/0600 backend. Only the
// admitted configuration leaf is repaired; credentials with invalid protection
// are rejected. No parent ACL or system credential provider is involved.
use platform_native::windows::files as configuration_native;

fn checked_directory(path: &Path) -> Result<()> {
    let _scope = configuration_native::pin_directory(path)
        .map_err(|_| invalid("应用配置目录必须为绝对普通目录，且不能经过链接。"))?;
    configuration_native::make_directory_private(path)
        .map_err(|_| invalid("无法保护当前用户的应用配置目录。"))
}

fn checked_file(file: &File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("应用配置文件不可读取。"))?;
    let identity =
        configuration_native::identity(file).map_err(|_| invalid("无法检查应用配置文件身份。"))?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || identity.directory
        || identity.links != 1
    {
        return Err(invalid("应用配置不得使用链接或特殊文件。"));
    }
    configuration_native::verify_private(file)
        .map_err(|_| invalid("应用配置文件必须由当前用户私有访问。"))
}

fn open_configuration_file(path: &Path, write: bool, create: bool) -> Result<File> {
    let _parent = configuration_native::pin_directory(
        path.parent()
            .ok_or_else(|| invalid("应用配置文件位置无效。"))?,
    )
    .map_err(|_| invalid("应用配置目录不可读取。"))?;
    // Create with a private DACL atomically. Hold this handle until the same
    // object has been reopened without delete sharing; an intervening same-name
    // replacement never authorizes changing that replacement's permissions.
    let created = if create {
        match configuration_native::create_private_file(path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
            Err(_) => return Err(invalid("无法准备应用配置文件。")),
        }
    } else {
        None
    };
    let expected = created
        .as_ref()
        .map(configuration_native::identity)
        .transpose()
        .map_err(|_| invalid("无法检查本次应用配置文件身份。"))?;
    let file = configuration_native::open_owned_file_private(path, write, expected.as_ref())
        .map_err(|_| invalid("无法打开应用配置文件。"))?;
    let metadata = file
        .metadata()
        .map_err(|_| invalid("应用配置文件不可读取。"))?;
    let actual =
        configuration_native::identity(&file).map_err(|_| invalid("无法检查应用配置文件身份。"))?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || actual.directory
        || actual.links != 1
        || expected
            .as_ref()
            .is_some_and(|expected| !expected.same_object(&actual))
    {
        return Err(invalid("应用配置文件已变化，或使用了链接及特殊文件。"));
    }
    // Native opening repairs only this bound, current-user file's DACL.
    checked_file(&file)?;
    Ok(file)
}

// LockFileEx does not promise immediate unlock when a HANDLE is closed. In
// particular, save is immediately followed by a view through a new HANDLE.
// Release the byte-range lock explicitly before closing its owning HANDLE.
// Closing remains the fallback if unlock fails; an already committed save must
// never be reported as failed (or discard its referenced keys) during Drop.
struct ConfigurationLock(File);
impl Drop for ConfigurationLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

impl NodeConfigurationStore {
    fn lock(&self) -> Result<ConfigurationLock> {
        checked_directory(&self.directory)?;
        let file = open_configuration_file(&self.directory.join("configuration.lock"), true, true)?;
        file.lock_exclusive()
            .map_err(|_| invalid("无法锁定应用配置。"))?;
        Ok(ConfigurationLock(file))
    }

    fn read(&self) -> Result<StoredConfiguration> {
        let path = self.directory.join("configuration.json");
        let _scope = configuration_native::pin_directory(&self.directory)
            .map_err(|_| invalid("应用配置目录不可读取。"))?;
        match fs::symlink_metadata(&path) {
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
            Ok(_) => (),
        }
        let file = open_configuration_file(&path, false, false)?;
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
        let parent = configuration_native::pin_directory(&self.directory)
            .map_err(|_| invalid("应用配置目录不可读取。"))?;
        let temporary = self
            .directory
            .join(format!(".configuration-{}.tmp", Uuid::new_v4()));
        let mut file = configuration_native::create_private_file_for_publication(&temporary)
            .map_err(|_| invalid("无法准备应用配置保存。"))?;
        let expected = configuration_native::identity(&file)
            .map_err(|_| invalid("无法检查应用配置文件身份。"))?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("无法写入应用配置，原配置已保留。"))?;
            configuration_native::rename_private_file_handle(
                &file,
                &self.directory.join("configuration.json"),
                true,
            )
            .map_err(|_| invalid("无法提交应用配置，原配置已保留。"))?;
            // As on Mac, successful native rename is the commit point. No
            // fallible post-commit observation may cause save to discard keys.
            let _ = configuration_native::sync_directory(&parent.file);
            Ok(())
        })();
        drop(file);
        if result.is_err() {
            let _ = configuration_native::remove_file(&temporary, &expected);
        }
        result
    }
}

#[cfg(test)]
mod windows_lock_tests {
    include!("lock_tests.rs");
}

pub struct LocalFileVault {
    directory: PathBuf,
    instance: Uuid,
    directory_identity: configuration_native::FileIdentity,
    created: std::sync::Mutex<BTreeMap<Uuid, configuration_native::FileIdentity>>,
}
impl LocalFileVault {
    pub fn new(directory: PathBuf, instance: Uuid) -> Result<Self> {
        if instance.is_nil() || !directory.is_absolute() {
            return Err(invalid("本地密钥目录或应用实例无效。"));
        }
        let parent = directory
            .parent()
            .ok_or_else(|| invalid("本地密钥目录无效。"))?;
        checked_directory(parent)?;
        let _parent = configuration_native::pin_directory(parent)
            .map_err(|_| invalid("本地密钥父目录不可读取。"))?;
        match configuration_native::create_private_directory(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(invalid("无法创建本地密钥目录。")),
        }
        let scope = configuration_native::pin_directory(&directory)
            .map_err(|_| invalid("本地密钥目录不能经过链接或特殊文件。"))?;
        configuration_native::verify_private(&scope.file)
            .map_err(|_| invalid("本地密钥目录必须属于当前用户且保持私有访问。"))?;
        let directory_identity = configuration_native::identity(&scope.file)
            .map_err(|_| invalid("无法检查本地密钥目录身份。"))?;
        Ok(Self {
            directory,
            instance,
            directory_identity,
            created: std::sync::Mutex::new(BTreeMap::new()),
        })
    }
    fn scope(&self, instance: Uuid, entry: Uuid) -> Result<configuration_native::PinnedDirectory> {
        if instance != self.instance || entry.is_nil() {
            return Err(invalid("本地密钥条目不属于此应用实例。"));
        }
        let scope = configuration_native::pin_directory(&self.directory)
            .map_err(|_| invalid("本地密钥目录不能经过链接或特殊文件。"))?;
        let identity = configuration_native::identity(&scope.file)
            .map_err(|_| invalid("无法检查本地密钥目录身份。"))?;
        if !self.directory_identity.same_object(&identity) {
            return Err(invalid("本地密钥目录已变化，已保留现场。"));
        }
        configuration_native::verify_private(&scope.file)
            .map_err(|_| invalid("本地密钥目录必须属于当前用户且保持私有访问。"))?;
        Ok(scope)
    }
    fn open_entry(
        &self,
        instance: Uuid,
        entry: Uuid,
    ) -> Result<Option<(File, configuration_native::PinnedDirectory)>> {
        let scope = self.scope(instance, entry)?;
        let file =
            match configuration_native::open_read_file(&self.directory.join(entry.to_string())) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(invalid("无法读取本地密钥文件，请重新填写密钥。")),
            };
        checked_credential_file(&file)?;
        Ok(Some((file, scope)))
    }
}
fn checked_credential_file(file: &File) -> Result<configuration_native::FileIdentity> {
    let metadata = file
        .metadata()
        .map_err(|_| invalid("无法检查本地密钥文件。"))?;
    let identity =
        configuration_native::identity(file).map_err(|_| invalid("无法检查本地密钥文件身份。"))?;
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || metadata.len() == 0
        || metadata.len() > MAX_SECRET_BYTES as u64
        || identity.directory
        || identity.links != 1
    {
        return Err(invalid(
            "本地密钥文件类型、链接或大小无效，请重新填写密钥。",
        ));
    }
    configuration_native::verify_private(file)
        .map_err(|_| invalid("本地密钥文件必须属于当前用户且保持私有访问。"))?;
    Ok(identity)
}
impl SecretVault for LocalFileVault {
    fn insert(&self, instance: Uuid, entry: Uuid, value: &str) -> Result<()> {
        validate_secret(value)?;
        let scope = self.scope(instance, entry)?;
        let path = self.directory.join(entry.to_string());
        let mut file = configuration_native::create_private_file(&path)
            .map_err(|_| invalid("无法新建本地密钥文件，原配置已保留。"))?;
        let expected = configuration_native::identity(&file)
            .map_err(|_| invalid("无法检查本地密钥文件身份。"))?;
        let result = (|| {
            file.write_all(value.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("无法写入本地密钥文件，原配置已保留。"))?;
            checked_credential_file(&file)?;
            configuration_native::sync_directory(&scope.file)
                .map_err(|_| invalid("无法完成本地密钥文件保存，原配置已保留。"))?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = result {
            let _ = configuration_native::remove_file(&path, &expected);
            return Err(error);
        }
        self.created
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(entry, expected);
        Ok(())
    }
    fn get(&self, instance: Uuid, entry: Uuid) -> Result<String> {
        let (file, _scope) = self
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
        let _scope = self.scope(instance, entry)?;
        if let Some((file, _entry_scope)) = self.open_entry(instance, entry)? {
            if !expected.same_object(&checked_credential_file(&file)?) {
                return Err(invalid("密钥文件已变化，已保留现场。"));
            }
            configuration_native::remove_file(&self.directory.join(entry.to_string()), expected)
                .map_err(|_| invalid("无法清理本次未提交的密钥文件。"))?;
        }
        created.remove(&entry);
        Ok(())
    }
    fn retire(&self, instance: Uuid, entry: Uuid) -> Result<()> {
        if let Some((file, scope)) = self.open_entry(instance, entry)? {
            let identity = checked_credential_file(&file)?;
            self.created
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(entry, identity);
            drop((file, scope));
            self.discard(instance, entry)?;
        }
        Ok(())
    }
    fn retire_instance(&self, instance: Uuid) -> Result<()> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(|_| invalid("无法枚举本地密钥文件。"))?
        {
            let entry = entry.map_err(|_| invalid("无法枚举本地密钥文件。"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("本地密钥文件名无效。"))?;
            let id = Uuid::parse_str(&name)
                .ok()
                .filter(|value| value.to_string() == name)
                .ok_or_else(|| invalid("本地密钥目录中有未知文件，已保留现场。"))?;
            entries.push(id);
        }
        for entry in entries {
            self.retire(instance, entry)?;
        }
        Ok(())
    }
    fn contains(&self, instance: Uuid, entry: Uuid) -> Result<bool> {
        self.open_entry(instance, entry)
            .map(|entry| entry.is_some())
    }
}
