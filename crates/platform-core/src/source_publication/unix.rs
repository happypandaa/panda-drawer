//! Publish an immutable download as a separate, editable author repository.
//! Every mutation is descriptor-relative; receipts survive a lost reply and
//! unfinished copies remain private evidence, never partially added projects.
use super::*;
use crate::{file_identity::PersistedIdentity, files};
use std::{
    ffi::{CStr, CString, OsString},
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
};

const AUTHOR_ENTRIES: usize = 50_000;
const AUTHOR_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Publication {
    schema_version: u32,
    source_id: String,
    source_task_id: String,
    source_digest: String,
    author_digest: String,
    app_id: String,
    directory: String,
    phase: String,
    payload: String,
    payload_identity: PersistedIdentity,
}

struct Anchor {
    path: PathBuf,
    file: File,
}
impl Anchor {
    fn open(path: PathBuf) -> Result<Self, ApiError> {
        let file = files::root(&path)?;
        Ok(Self { path, file })
    }
    fn child(&self, name: &str, create: bool) -> Result<Self, ApiError> {
        files::parts(name)?;
        if name.contains('/') {
            return Err(invalid("源码发布目录无效。"));
        }
        let name_c = c_name(name.as_bytes())?;
        if create {
            let status = unsafe { libc::mkdirat(self.file.as_raw_fd(), name_c.as_ptr(), 0o700) };
            if status != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(files::error(std::io::Error::last_os_error()));
            }
            self.file.sync_all()?;
        }
        let file = open_child(&self.file, name.as_bytes())?;
        if !file.metadata()?.is_dir() {
            return Err(invalid("源码发布位置不是普通目录。"));
        }
        let child = Self {
            path: self.path.join(name),
            file,
        };
        child.verify()?;
        Ok(child)
    }
    fn verify(&self) -> Result<(), ApiError> {
        let current = files::root(&self.path)?;
        same_object(&self.file, &current)
    }
}
fn same_object(first: &File, second: &File) -> Result<(), ApiError> {
    let first = first.metadata()?;
    let second = second.metadata()?;
    if first.dev() != second.dev() || first.ino() != second.ino() {
        return Err(invalid("源码发布目录身份已变化，请保留现场。"));
    }
    Ok(())
}
fn c_name(bytes: &[u8]) -> Result<CString, ApiError> {
    CString::new(bytes).map_err(|_| invalid("源码文件名无效。"))
}
fn open_child(parent: &File, name: &[u8]) -> Result<File, ApiError> {
    let name = c_name(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(files::error(std::io::Error::last_os_error()));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if !metadata.is_dir() && !(metadata.is_file() && metadata.nlink() == 1) {
        return Err(invalid("源码含不支持的文件或共享硬链接，未发布。"));
    }
    Ok(file)
}
fn exists_at(parent: &File, name: &str) -> Result<bool, ApiError> {
    let name = c_name(name.as_bytes())?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(false)
    } else {
        Err(files::error(error))
    }
}
fn names(directory: &File) -> Result<Vec<OsString>, ApiError> {
    let fd = unsafe { libc::dup(directory.as_raw_fd()) };
    if fd < 0 {
        return Err(files::error(std::io::Error::last_os_error()));
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(files::error(std::io::Error::last_os_error()));
    }
    struct Listing(*mut libc::DIR);
    impl Drop for Listing {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let listing = Listing(stream);
    unsafe {
        libc::rewinddir(listing.0);
    }
    let mut result = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(listing.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                return Err(files::error(error));
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        if result.len() >= AUTHOR_ENTRIES {
            return Err(limit());
        }
        result.push(OsString::from_vec(name.into()));
    }
    result.sort();
    Ok(result)
}
fn metadata_equal(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mode() == after.mode()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
}
fn author_tree(
    directory: &File,
    target: Option<&File>,
    relative: &str,
    depth: usize,
    budget: &mut (usize, u64),
    tree: &mut Sha256,
) -> Result<(), ApiError> {
    if depth > 40 {
        return Err(limit());
    }
    let entries = names(directory)?;
    for name in &entries {
        budget.0 += 1;
        if budget.0 > AUTHOR_ENTRIES {
            return Err(limit());
        }
        let label = name
            .to_str()
            .ok_or_else(|| invalid("源码文件名不是 UTF-8。"))?;
        if label.chars().any(char::is_control) {
            return Err(invalid("源码文件名包含控制字符。"));
        }
        let path = if relative.is_empty() {
            label.to_owned()
        } else {
            format!("{relative}/{label}")
        };
        let mut input = open_child(directory, name.as_bytes())?;
        let before = input.metadata()?;
        tree.update(path.as_bytes());
        tree.update([0]);
        if before.is_dir() {
            tree.update(b"directory");
            if let Some(target) = target {
                let name_c = c_name(name.as_bytes())?;
                if unsafe { libc::mkdirat(target.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
                    return Err(files::error(std::io::Error::last_os_error()));
                }
                let child = open_child(target, name.as_bytes())?;
                author_tree(&input, Some(&child), &path, depth + 1, budget, tree)?;
                child.sync_all()?;
            } else {
                author_tree(&input, None, &path, depth + 1, budget, tree)?;
            }
        } else {
            budget.1 = budget.1.checked_add(before.len()).ok_or_else(limit)?;
            if budget.1 > AUTHOR_BYTES || before.len() > AUTHOR_BYTES {
                return Err(limit());
            }
            let mut output = target
                .map(|target| files::create(target, label))
                .transpose()?;
            let mut digest = Sha256::new();
            let mut size = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                size += read as u64;
                if size > before.len() {
                    return Err(invalid("复制时源码文件增长，未发布。"));
                }
                digest.update(&buffer[..read]);
                if let Some(output) = output.as_mut() {
                    output.write_all(&buffer[..read])?;
                }
            }
            if size != before.len() || !metadata_equal(&before, &input.metadata()?) {
                return Err(invalid("复制时源码内容变化，未发布。"));
            }
            if let Some(output) = output {
                output.set_permissions(fs::Permissions::from_mode(before.mode() & 0o777))?;
                output.sync_all()?;
            }
            tree.update(b"file");
            tree.update(size.to_le_bytes());
            tree.update([u8::from(before.mode() & 0o111 != 0)]);
            tree.update(digest.finalize());
        }
        let current = open_child(directory, name.as_bytes())?;
        if !metadata_equal(&before, &current.metadata()?) {
            return Err(invalid("复制时源码条目身份变化，未发布。"));
        }
    }
    if entries != names(directory)? {
        return Err(invalid("复制时源码目录条目变化，未发布。"));
    }
    if let Some(target) = target {
        target.sync_all()?;
    }
    Ok(())
}
fn author_digest(directory: &File, target: Option<&File>) -> Result<String, ApiError> {
    Ok(author_snapshot(directory, target)?.0)
}
fn author_snapshot(directory: &File, target: Option<&File>) -> Result<(String, u64), ApiError> {
    let mut digest = Sha256::new();
    let mut budget = (0, 0);
    author_tree(directory, target, "", 0, &mut budget, &mut digest)?;
    Ok((format!("{:x}", digest.finalize()), budget.1))
}
fn require_copy_space(directory: &File, bytes: u64) -> Result<(), ApiError> {
    let mut space = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), space.as_mut_ptr()) } != 0 {
        return Err(files::error(std::io::Error::last_os_error()));
    }
    let space = unsafe { space.assume_init() };
    let block = if space.f_frsize == 0 {
        space.f_bsize
    } else {
        space.f_frsize
    };
    if u128::from(space.f_bavail) * u128::from(block) < u128::from(bytes) + u128::from(MIN_FREE) {
        return Err(error(
            "SOURCE_PUBLICATION_SPACE_LOW",
            "剩余空间不足以准备源码并保留运行余量，请释放空间后重试。",
        ));
    }
    Ok(())
}
fn record_bytes(receipt: &File) -> Result<Option<Vec<u8>>, ApiError> {
    if !exists_at(receipt, "publication.json")? {
        return Ok(None);
    }
    let file = files::read(receipt, "publication.json")?;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    Ok(Some(bytes))
}
fn save_publication(
    receipt: &Anchor,
    expected: &mut Option<Vec<u8>>,
    publication: &Publication,
) -> Result<(), ApiError> {
    receipt.verify()?;
    if record_bytes(&receipt.file)? != *expected {
        return Err(invalid("源码发布记录被其他操作修改，请保留现场。"));
    }
    let bytes = serde_json::to_vec(publication)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    let temporary = format!(".publication-{}.json", Uuid::new_v4().simple());
    let mut file = files::create(&receipt.file, &temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if record_bytes(&receipt.file)? != *expected {
        return Err(invalid("源码发布记录发生变化，未替换记录。"));
    }
    let from = c_name(temporary.as_bytes())?;
    let to = c"publication.json";
    if unsafe {
        libc::renameat(
            receipt.file.as_raw_fd(),
            from.as_ptr(),
            receipt.file.as_raw_fd(),
            to.as_ptr(),
        )
    } != 0
    {
        return Err(files::error(std::io::Error::last_os_error()));
    }
    receipt.file.sync_all()?;
    receipt.verify()?;
    *expected = Some(bytes);
    Ok(())
}
fn fresh_payload(receipt: &Anchor) -> Result<(String, Anchor), ApiError> {
    let name = format!("payload_{}", Uuid::new_v4().simple());
    let name_c = c_name(name.as_bytes())?;
    if unsafe { libc::mkdirat(receipt.file.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
        return Err(files::error(std::io::Error::last_os_error()));
    }
    receipt.file.sync_all()?;
    let anchor = receipt.child(&name, false)?;
    Ok((name, anchor))
}
fn available_name(root: &File, base: &str) -> Result<String, ApiError> {
    for number in 1..100_000 {
        let name = if number == 1 {
            base.to_owned()
        } else {
            format!("{base}-{number}")
        };
        if !exists_at(root, &name)? {
            return Ok(name);
        }
    }
    Err(invalid("同名项目目录过多，请先核对项目位置。"))
}
fn publication_identity(publication: &Publication, directory: &File) -> Result<(), ApiError> {
    let observed =
        PersistedIdentity::from_file(directory).map_err(|_| invalid("无法核对源码目录身份。"))?;
    if !publication.payload_identity.matches_captured(&observed) {
        return Err(invalid("源码目录与发布记录不一致，未覆盖文件。"));
    }
    Ok(())
}
fn with_publication_reservation(
    ownership: &crate::node_project_ownership::NodeProjectOwnership,
    app: &str,
    publication: &Publication,
    publish: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if let Err(error) = ownership.reserve_published_source_identity(
        app,
        &publication.directory,
        &publication.payload_identity,
    ) {
        ownership.forget_unpublished_source_identity(
            app,
            &publication.directory,
            &publication.payload_identity,
        )?;
        return Err(error);
    }
    let result = publish();
    if result.is_err() {
        ownership.forget_unpublished_source_identity(
            app,
            &publication.directory,
            &publication.payload_identity,
        )?;
    }
    result
}
fn publish_exclusive(from: &File, payload: &str, to: &File, name: &str) -> Result<(), ApiError> {
    let from_name = c_name(payload.as_bytes())?;
    let to_name = c_name(name.as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            from_name.as_ptr(),
            to.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            from.as_raw_fd(),
            from_name.as_ptr(),
            to.as_raw_fd(),
            to_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = -1;
    if result != 0 {
        return Err(invalid(
            "项目目录已有内容或无法发布；原内容与本次现场均保留。",
        ));
    }
    to.sync_all()
        .map_err(|e| files::error(e).outcome("committed"))?;
    from.sync_all()
        .map_err(|e| files::error(e).outcome("committed"))
}

#[cfg(target_os = "macos")]
pub(super) fn copy_selected_working(original: &Path, receipt: &Path) -> Result<PathBuf, ApiError> {
    let original = Anchor::open(original.into())?;
    original.child(".git", false)?;
    let receipt = Anchor::open(receipt.into())?;
    if exists_at(&receipt.file, "selected-working")? {
        return Err(invalid("源码准备副本尚未核对，请先清理本次临时材料。"));
    }
    let working = receipt.child("selected-working", true)?;
    let (expected, bytes) = author_snapshot(&original.file, None)?;
    require_copy_space(&working.file, bytes)?;
    if author_digest(&original.file, Some(&working.file))? != expected
        || author_digest(&working.file, None)? != expected
    {
        return Err(invalid("源码准备副本核验失败，未写入所选文件夹。"));
    }
    original.verify()?;
    working.verify()?;
    receipt.verify()?;
    Ok(working.path)
}

impl SourceAcquisitionManager {
    pub(crate) fn publish_author_source(
        &self,
        source: &SourceCandidate,
        app_id: &str,
        lease_id: &str,
    ) -> Result<PathBuf, ApiError> {
        if Uuid::parse_str(app_id)
            .map(|id| id.to_string() != app_id)
            .unwrap_or(true)
        {
            return Err(invalid("应用标识无效。"));
        }
        if self.owner_app_id(&source.source_id)? != app_id {
            return Err(invalid("来源只属于原项目，不能发布给另一个实例。"));
        }
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        let digest = source
            .content_digest
            .as_deref()
            .ok_or_else(|| invalid("源码摘要缺失。"))?;
        let source_task = source.task_id.clone();
        let source = self.verify_ready_leased(&source.source_id, digest, lease_id)?;
        if source.task_id != source_task {
            return Err(invalid("源码任务身份已变化，未发布。"));
        }
        let (url, provider, repository) = validate_url(&source.url)?;
        if source.provider != provider || source.url != url {
            return Err(invalid("只有本次 Git 添加成功的原始源码可以发布。"));
        }
        let original = Anchor::open(
            source
                .source_path
                .clone()
                .ok_or_else(|| invalid("源码位置缺失。"))?,
        )?;
        // A downloaded Git source must still contain its own ordinary Git root.
        original.child(".git", false)?;
        let (full_digest, copy_bytes) = author_snapshot(&original.file, None)?;
        original.verify()?;
        let workspace = Anchor::open(self.paths.workspace_root().into())?;
        let root = workspace.child("project-sources", true)?;
        ownership.record_publication(app_id)?;
        let receipt = workspace
            .child(".platform", false)?
            .child("source-publications", true)?
            .child(app_id, true)?;
        ownership.record_publication(app_id)?;
        let mut bytes = record_bytes(&receipt.file)?;
        let base: String = repository.chars().take(80).collect();
        let base = if base.is_empty() { "project" } else { &base };
        let mut publication = if let Some(bytes) = &bytes {
            let publication: Publication = serde_json::from_slice(bytes)
                .map_err(|_| invalid("源码发布记录无法读取，请保留现场。"))?;
            if publication.schema_version != 1
                || publication.app_id != app_id
                || publication.source_id != source.source_id
                || publication.source_task_id != source.task_id
                || publication.source_digest != digest
                || !matches!(
                    publication.phase.as_str(),
                    "copying" | "ready" | "published"
                )
                || files::parts(&publication.directory).map_or(true, |parts| parts.len() != 1)
                || !publication.payload.starts_with("payload_")
                || files::parts(&publication.payload).map_or(true, |parts| parts.len() != 1)
                || publication.author_digest.len() != 64
            {
                return Err(invalid("源码发布记录与本次应用不一致，请保留现场。"));
            }
            publication
        } else {
            let (payload, stage) = fresh_payload(&receipt)?;
            let publication = Publication {
                schema_version: 1,
                source_id: source.source_id.clone(),
                source_task_id: source.task_id.clone(),
                source_digest: digest.into(),
                author_digest: full_digest.clone(),
                app_id: app_id.into(),
                directory: available_name(&root.file, base)?,
                phase: "copying".into(),
                payload,
                payload_identity: PersistedIdentity::from_file(&stage.file)
                    .map_err(|_| invalid("无法核对复制目录身份。"))?,
            };
            save_publication(&receipt, &mut bytes, &publication)?;
            publication
        };
        if exists_at(&root.file, &publication.directory)? {
            if let Ok(destination) = root.child(&publication.directory, false) {
                if publication_identity(&publication, &destination.file).is_ok() {
                    root.verify()?;
                    receipt.verify()?;
                    workspace.verify()?;
                    publication.phase = "published".into();
                    save_publication(&receipt, &mut bytes, &publication)?;
                    ownership.reserve_published_source_identity(
                        app_id,
                        &publication.directory,
                        &publication.payload_identity,
                    )?;
                    return Ok(destination.path);
                }
            }
            if !exists_at(&receipt.file, &publication.payload)? {
                return Err(invalid(
                    "发布后的源码位置被替换，请保留现场，不会再复制一份。",
                ));
            }
            ownership.forget_unpublished_source_identity(
                app_id,
                &publication.directory,
                &publication.payload_identity,
            )?;
            publication.directory = available_name(&root.file, base)?;
            save_publication(&receipt, &mut bytes, &publication)?;
        }
        if publication.author_digest != full_digest {
            return Err(invalid("源码或 Git 元信息已变化，未继续发布。"));
        }
        let stage = receipt.child(&publication.payload, false)?;
        publication_identity(&publication, &stage.file)?;
        if publication.phase == "copying" {
            if author_digest(&stage.file, None)? != publication.author_digest
                && !names(&stage.file)?.is_empty()
            {
                return Err(invalid("源码复制尚未完成，请清理本次临时材料后重新添加。"));
            }
            if names(&stage.file)?.is_empty() {
                require_copy_space(&stage.file, copy_bytes)?;
                let copied = author_digest(&original.file, Some(&stage.file))?;
                if copied != publication.author_digest {
                    return Err(invalid("复制时源码变化，临时现场保留。"));
                }
            }
            if author_digest(&stage.file, None)? != publication.author_digest
                || author_digest(&original.file, None)? != publication.author_digest
            {
                return Err(invalid("源码副本核验不一致，未发布。"));
            }
            stage.verify()?;
            original.verify()?;
            self.verify_ready_leased(&source.source_id, digest, lease_id)?;
            publication.phase = "ready".into();
            save_publication(&receipt, &mut bytes, &publication)?;
        }
        if author_digest(&stage.file, None)? != publication.author_digest {
            return Err(invalid("复制现场已变化，未发布。"));
        }
        workspace.verify()?;
        root.verify()?;
        receipt.verify()?;
        stage.verify()?;
        original.verify()?;
        with_publication_reservation(&ownership, app_id, &publication, || {
            publish_exclusive(
                &receipt.file,
                &publication.payload,
                &root.file,
                &publication.directory,
            )
        })?;
        let destination = root.child(&publication.directory, false)?;
        publication_identity(&publication, &destination.file)?;
        root.verify()?;
        receipt.verify()?;
        workspace.verify()?;
        publication.phase = "published".into();
        save_publication(&receipt, &mut bytes, &publication)?;
        ownership.reserve_published_source_identity(
            app_id,
            &publication.directory,
            &publication.payload_identity,
        )?;
        Ok(destination.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_identity_reservation_never_adopts_a_racing_destination_and_survives_rename() {
        use crate::node_project_ownership::NodeProjectOwnership;
        for mode in 0..4 {
            let temporary = tempfile::tempdir().unwrap();
            let base = temporary.path().canonicalize().unwrap();
            let data = base.join("data");
            let workspace = base.join("workspace");
            fs::create_dir(&data).unwrap();
            fs::create_dir(&workspace).unwrap();
            let owner = Uuid::new_v4().to_string();
            let ownership = NodeProjectOwnership::new(data, workspace.clone());
            ownership.initialize(&owner).unwrap();
            ownership.record_publication(&owner).unwrap();
            let receipt_path = workspace.join(".platform/source-publications").join(&owner);
            fs::create_dir_all(receipt_path.join("payload_disposable")).unwrap();
            ownership.record_publication(&owner).unwrap();
            fs::write(
                receipt_path.join("payload_disposable/payload"),
                "our source",
            )
            .unwrap();
            let root_path = workspace.join("project-sources");
            fs::create_dir(&root_path).unwrap();
            let destination = root_path.join("disposable-author");
            let publication = Publication {
                schema_version: 1,
                source_id: format!("src_{}", Uuid::new_v4().simple()),
                source_task_id: format!("task_{}", Uuid::new_v4().simple()),
                source_digest: "a".repeat(64),
                author_digest: "b".repeat(64),
                app_id: owner.clone(),
                directory: "disposable-author".into(),
                phase: "ready".into(),
                payload: "payload_disposable".into(),
                payload_identity: PersistedIdentity::capture(
                    &receipt_path.join("payload_disposable"),
                )
                .unwrap(),
            };
            if mode == 2 {
                ownership
                    .reserve_published_source_identity(
                        &owner,
                        &publication.directory,
                        &publication.payload_identity,
                    )
                    .unwrap();
            }
            let foreign = || {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("foreign"), "another project's source").unwrap();
            };
            if mode == 0 || mode == 2 {
                foreign();
            }
            let receipt = Anchor::open(receipt_path).unwrap();
            let root = Anchor::open(root_path).unwrap();
            let result = with_publication_reservation(&ownership, &owner, &publication, || {
                if mode == 1 {
                    foreign();
                }
                publish_exclusive(
                    &receipt.file,
                    &publication.payload,
                    &root.file,
                    &publication.directory,
                )
            });
            let paths = ownership.owned_paths(&owner).unwrap();
            if mode == 3 {
                result.unwrap();
                // No post-rename capture is needed: the expected payload inode
                // was persisted before moving its nonempty directory.
                assert!(paths.contains(&destination));
            } else {
                assert!(result.is_err());
                assert!(!paths.contains(&destination));
                for path in paths {
                    fs::remove_dir_all(path).unwrap();
                }
                assert_eq!(
                    fs::read_to_string(destination.join("foreign")).unwrap(),
                    "another project's source"
                );
            }
        }
    }

    fn ready_source(manager: &SourceAcquisitionManager, tasks: &BuildManager) -> SourceCandidate {
        let task = tasks.create_workspace_operation("sourceAcquire").unwrap();
        let id = format!("src_{}", Uuid::new_v4().simple());
        private_dir(&manager.root()).unwrap();
        let slot = manager.root().join(&id);
        new_dir(&slot).unwrap();
        fs::create_dir_all(slot.join("repository/.git/objects")).unwrap();
        fs::write(slot.join("repository/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(slot.join("repository/app.js"), "disposable author source").unwrap();
        let record = SourceRecord {
            schema_version: 1,
            source_id: id.clone(),
            owner_app_id: Uuid::new_v4().to_string(),
            task_id: task.task_id.clone(),
            name: "可丢弃·源码发布".into(),
            status: "ready".into(),
            phase: "completed".into(),
            url: "https://github.com/example/disposable-repo.git".into(),
            branch: None,
            provider: "github".into(),
            created_at: 1,
            completed_at: Some(2),
            repository: "repository".into(),
            shallow: true,
            git: None,
            commit: Some("a".repeat(40)),
            file_count: Some(1),
            total_bytes: Some(24),
            content_digest: Some(directory_digest(&slot.join("repository")).unwrap()),
            inspection: None,
            error: None,
        };
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            manager.paths.data_root().into(),
            manager.paths.workspace_root().into(),
        );
        ownership.initialize(&record.owner_app_id).unwrap();
        ownership
            .record_task(&record.owner_app_id, &record.task_id)
            .unwrap();
        ownership
            .record_source(&record.owner_app_id, &record.source_id)
            .unwrap();
        manager.save_record(&record).unwrap();
        let candidate = manager.view(manager.read_record(&id).unwrap()).unwrap();
        tasks
            .finish(&task.task_id, Ok(serde_json::json!(candidate)))
            .unwrap();
        candidate
    }

    #[tokio::test]
    async fn published_author_git_survives_restart_and_retry_preserves_later_user_edits() {
        let (_temp, paths, store, tasks, manager) = super::super::super::tests::fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let destination = manager
            .publish_author_source(&source, &app, "publication-test")
            .unwrap();
        assert_eq!(
            fs::read(destination.join(".git/HEAD")).unwrap(),
            b"ref: refs/heads/main\n"
        );
        fs::write(destination.join("app.js"), "user's later uncommitted edit").unwrap();
        let restarted = SourceAcquisitionManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            GitEnvironmentManager::new(paths.data_root().into(), None),
        )
        .unwrap();
        assert_eq!(
            restarted
                .publish_author_source(&source, &app, "publication-test")
                .unwrap(),
            destination
        );
        assert_eq!(
            fs::read(destination.join("app.js")).unwrap(),
            b"user's later uncommitted edit"
        );
        assert_eq!(
            fs::read_dir(paths.workspace_root().join("project-sources"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read(source.source_path.unwrap().join("app.js")).unwrap(),
            b"disposable author source"
        );
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }

    #[tokio::test]
    async fn existing_project_names_are_preserved_and_publication_uses_a_suffix() {
        let (_temp, paths, store, tasks, manager) = super::super::super::tests::fixture();
        let source = ready_source(&manager, &tasks);
        let root = paths.workspace_root().join("project-sources");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("disposable-repo"), "unrelated user's file").unwrap();
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let path = manager
            .publish_author_source(
                &source,
                &manager.owner_app_id(&source.source_id).unwrap(),
                "publication-test",
            )
            .unwrap();
        assert_eq!(path.file_name().unwrap(), "disposable-repo-2");
        assert_eq!(
            fs::read(root.join("disposable-repo")).unwrap(),
            b"unrelated user's file"
        );
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }

    #[tokio::test]
    async fn partial_publication_requires_cleanup_without_creating_another_payload() {
        let (_temp, paths, store, tasks, manager) = super::super::super::tests::fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let destination = manager
            .publish_author_source(&source, &app, "publication-test")
            .unwrap();
        let receipt_path = paths
            .workspace_root()
            .join(".platform/source-publications")
            .join(&app);
        let mut publication: Publication =
            serde_json::from_slice(&fs::read(receipt_path.join("publication.json")).unwrap())
                .unwrap();
        // Artificially reproduce a durable copying plan plus a partial payload.
        let partial = receipt_path.join(&publication.payload);
        fs::rename(&destination, &partial).unwrap();
        // The simulated crash happened before publication. Its author path was
        // reserved but had never captured a successfully published directory.
        let owner_path = paths
            .data_root()
            .join("source-import-ownership")
            .join(&app)
            .join("ownership.json");
        let mut ownership: serde_json::Value =
            serde_json::from_slice(&fs::read(&owner_path).unwrap()).unwrap();
        ownership["artifacts"][format!("author:{}", publication.directory)] =
            serde_json::Value::Null;
        atomic_json(&owner_path, &ownership).unwrap();
        fs::remove_file(partial.join("app.js")).unwrap();
        publication.phase = "copying".into();
        atomic_json(&receipt_path.join("publication.json"), &publication).unwrap();
        assert!(manager
            .publish_author_source(&source, &app, "publication-test")
            .is_err());
        assert!(!destination.exists());
        assert!(partial.join(".git/HEAD").is_file());
        let updated: Publication =
            serde_json::from_slice(&fs::read(receipt_path.join("publication.json")).unwrap())
                .unwrap();
        assert_eq!(updated.payload, publication.payload);
        assert_eq!(
            fs::read_dir(&receipt_path)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("payload_"))
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(paths.workspace_root().join("project-sources"))
                .unwrap()
                .count(),
            0
        );
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }

    #[test]
    fn descriptor_copy_cannot_follow_replaced_source_or_destination_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for name in ["source", "target", "outside"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        fs::write(root.join("source/public.js"), "owned fixture").unwrap();
        fs::write(root.join("outside/public.js"), "outside must not be copied").unwrap();
        let source = Anchor::open(root.join("source")).unwrap();
        let target = Anchor::open(root.join("target")).unwrap();
        fs::rename(root.join("source"), root.join("preserved-source")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("source")).unwrap();
        fs::rename(root.join("target"), root.join("preserved-target")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("target")).unwrap();
        author_digest(&source.file, Some(&target.file)).unwrap();
        assert!(source.verify().is_err());
        assert!(target.verify().is_err());
        assert_eq!(
            fs::read(root.join("preserved-target/public.js")).unwrap(),
            b"owned fixture"
        );
        assert_eq!(
            fs::read(root.join("outside/public.js")).unwrap(),
            b"outside must not be copied"
        );
    }

    #[tokio::test]
    async fn git_links_and_unleased_publication_are_rejected_before_formal_source_creation() {
        let (_temp, paths, store, tasks, manager) = super::super::super::tests::fixture();
        let source = ready_source(&manager, &tasks);
        let app = manager.owner_app_id(&source.source_id).unwrap();
        assert_eq!(
            manager
                .publish_author_source(&source, &app, "publication-test")
                .unwrap_err()
                .code,
            "SOURCE_LEASE_REQUIRED"
        );
        let lease = store
            .reserve_task(&format!("source:{}", source.source_id), "publication-test")
            .unwrap();
        let original = source.source_path.as_ref().unwrap();
        std::os::unix::fs::symlink(original.join("app.js"), original.join(".git/linked-object"))
            .unwrap();
        assert!(manager
            .publish_author_source(&source, &app, "publication-test")
            .is_err());
        assert!(!paths.workspace_root().join("project-sources").exists());
        assert!(original.join("app.js").is_file());
        drop(lease);
        tasks.shutdown().await;
        store.shutdown().await;
    }
}
