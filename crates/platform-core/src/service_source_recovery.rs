//! Tracks only attempts explicitly started by the unified native add flow.
use super::*;
use std::fs;

fn invalid() -> ApiError {
    ApiError::new("SOURCE_IMPORT_INVALID", "添加记录身份或目录无法核对。")
}
fn task_name(task: &str) -> Result<std::ffi::CString, ApiError> {
    if !crate::projects::valid_id(task) || !task.starts_with("task_") {
        return Err(invalid());
    }
    std::ffi::CString::new(format!("{task}.json")).map_err(|_| invalid())
}
fn imports_dir(state: &HttpState, create: bool) -> Result<Option<fs::File>, ApiError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut dir = crate::files::root(state.paths.data_root())?;
    for name in [c"onboarding", c"imports"] {
        if create {
            let result = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(std::io::Error::last_os_error().into());
            }
            dir.sync_all()?;
        }
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            if !create && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(invalid());
        }
        dir = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(Some(dir))
}
fn read_record(dir: &fs::File, task: &str) -> Result<Value, ApiError> {
    let name = task_name(task)?;
    let file = crate::files::read(dir, name.to_str().map_err(|_| invalid())?)?;
    let mut bytes = vec![];
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    let record: Value = serde_json::from_slice(&bytes)?;
    if record["version"] != 1 || record["taskId"] != task || record["identifier"] != APP_IDENTIFIER
    {
        return Err(invalid());
    }
    Ok(record)
}
fn recorded_tasks(dir: &fs::File) -> Result<Vec<String>, ApiError> {
    use std::os::fd::AsRawFd;
    let fd = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(invalid());
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(invalid());
    }
    struct Stream(*mut libc::DIR);
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Stream(stream);
    let mut names = vec![];
    for _ in 0..102 {
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_string_lossy();
        if let Some(task) = name.strip_suffix(".json") {
            task_name(task)?;
            names.push(task.into());
        }
    }
    Ok(names)
}
impl RuntimeHandle {
    pub fn remember_source_import(&self, task_id: &str, _url: &str) -> Result<(), ApiError> {
        use std::{
            io::Write,
            os::fd::{AsRawFd, FromRawFd},
        };
        let state = self.current();
        if state.builds.task(task_id)?.task_type != "sourceAcquire" {
            return Err(invalid());
        }
        let source = state
            .source_acquisition
            .as_ref()
            .map_err(Clone::clone)?
            .list()?
            .into_iter()
            .find(|s| s.task_id == task_id)
            .ok_or_else(invalid)?;
        let dir = imports_dir(&state, true)?.ok_or_else(invalid)?;
        let name = task_name(task_id)?;
        let value = json!({"version":1,"taskId":task_id,"sourceId":source.source_id,"url":source.url,"identifier":APP_IDENTIFIER});
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST)
                && read_record(&dir, task_id)? == value
            {
                return Ok(());
            }
            return Err(invalid());
        }
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        file.write_all(&serde_json::to_vec(&value)?)?;
        file.sync_all()?;
        dir.sync_all()?;
        Ok(())
    }
    pub fn complete_source_import(&self, task_id: &str) -> Result<(), ApiError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let state = self.current();
        let Some(dir) = imports_dir(&state, false)? else {
            return Ok(());
        };
        let name = task_name(task_id)?;
        match read_record(&dir, task_id) {
            Ok(_) => (),
            Err(error) if error.code == "FILE_NOT_FOUND" => return Ok(()),
            Err(error) => return Err(error),
        }
        let made = unsafe { libc::mkdirat(dir.as_raw_fd(), c"completed".as_ptr(), 0o700) };
        if made != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(invalid());
        }
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                c"completed".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(invalid());
        }
        let completed = unsafe { fs::File::from_raw_fd(fd) };
        #[cfg(target_os = "macos")]
        let moved = unsafe {
            libc::renameatx_np(
                dir.as_raw_fd(),
                name.as_ptr(),
                completed.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let moved = unsafe {
            libc::renameat2(
                dir.as_raw_fd(),
                name.as_ptr(),
                completed.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let moved = -1;
        if moved != 0 {
            return Err(invalid());
        }
        dir.sync_all()?;
        completed.sync_all()?;
        Ok(())
    }
    pub async fn recover_source_imports(
        &self,
        apps: Arc<crate::node_apps::NodeApps>,
    ) -> Result<Value, ApiError> {
        let state = self.current();
        let Some(dir) = imports_dir(&state, false)? else {
            return Ok(json!([]));
        };
        let mut attempts = vec![];
        let registered = apps.list().await?;
        for task_id in recorded_tasks(&dir)? {
            let record = read_record(&dir, &task_id)?;
            let task = state.builds.task(&task_id)?;
            if task.task_type != "sourceAcquire" {
                return Err(invalid());
            }
            let task_id = task_id.as_str();
            let source_id = record["sourceId"].as_str().ok_or_else(invalid)?;
            if task
                .result
                .as_ref()
                .and_then(|r| r["sourceId"].as_str())
                .is_some_and(|id| id != source_id)
            {
                return Err(invalid());
            }
            let source_id = Some(source_id);
            let installed = registered
                .iter()
                .find(|item| source_id.is_some_and(|id| item["installation"]["sourceId"] == id));
            if !task.status.is_final() {
                attempts.push(json!({"taskId":task_id,"url":record["url"],"status":"running","message":"上次添加的任务还在处理，请先核对任务。"}));
                continue;
            }
            if let Some(app_id) = installed.and_then(|app| app["id"].as_str()) {
                if self
                    .source_import_publish(apps.clone(), task_id.into(), app_id.into())
                    .await
                    .is_err()
                {
                    attempts.push(json!({"taskId":task_id,"url":record["url"],"appId":app_id,"status":"blocked","message":"应用已经登记，源码位置仍需核对；程序和本次材料已保留，不会重复添加。"}));
                    continue;
                }
            }
            match self.source_import_cleanup(task_id.into()).await {
                Ok(result) if result["completed"] == true => {
                    self.complete_source_import(task_id)?;
                    attempts.push(json!({"taskId":task_id,"url":record["url"],"status":if installed.is_some(){"installed"}else{"cleaned"},"appId":installed.map(|app|app["id"].clone()),"message":if installed.is_some(){"上次添加的应用已登记，请查看原应用，勿重复添加。"}else{"上次添加已中断，本次临时文件已清理，可以重新下载。"}}));
                }
                Ok(_) | Err(_) => attempts.push(json!({"taskId":task_id,"url":record["url"],"status":"blocked","message":"上次添加的停止状态、安装结果或临时文件仍需核对，尚未重新添加。"})),
            }
        }
        Ok(json!(attempts))
    }
}

#[cfg(test)]
mod tests {
    // Only synthetic task/source records and clearly disposable directories are
    // used. No Git remote, npm, application process or author data is opened.
    use super::*;
    use std::{
        os::unix::fs::{symlink, FileTypeExt},
        path::Path,
    };

    struct SyntheticSource {
        task: String,
        id: String,
        slot: PathBuf,
        url: String,
    }
    fn source(
        handle: &RuntimeHandle,
        kind: &str,
        finished: bool,
        external: &Path,
    ) -> SyntheticSource {
        let state = handle.current();
        let task = state.builds.create_workspace_operation(kind).unwrap();
        let id = format!("src_{}", Uuid::new_v4().simple());
        let slot = state
            .paths
            .workspace_root()
            .join(".platform/sources")
            .join(&id);
        let url = format!("https://github.com/example/disposable-{}.git", &id[4..]);
        fs::create_dir_all(slot.join("repository/.git")).unwrap();
        fs::create_dir_all(slot.join(".acquiring/home")).unwrap();
        fs::write(slot.join("repository/app.js"), "disposable source payload").unwrap();
        fs::write(slot.join("repository/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            slot.join(".acquiring/home/download.tmp"),
            "disposable download evidence",
        )
        .unwrap();
        crate::builds::atomic_json(&slot.join("source.json"), &json!({
            "schemaVersion":1,"sourceId":id,"taskId":task.task_id,"name":"disposable recovery source",
            "status":if finished {"ready"}else{"acquiring"},"phase":if finished {"completed"}else{"downloading"},
            "url":url,"provider":"github","branch":null,"localSourcePath":external,"capturePolicy":null,
            "buildSourceDigest":null,"excludedPaths":[],"createdAt":1,"completedAt":if finished {Some(2)}else{None},
            "repository":"repository","shallow":true,"git":null,"commit":null,"fileCount":1,"totalBytes":25,
            "contentDigest":"a".repeat(64),"inspection":null,"error":null
        })).unwrap();
        if finished {
            state
                .builds
                .finish(
                    &task.task_id,
                    Ok(json!({"sourceId":id,"contentDigest":"a".repeat(64)})),
                )
                .unwrap();
        }
        SyntheticSource {
            task: task.task_id,
            id,
            slot,
            url,
        }
    }
    fn nodes(paths: &RuntimePaths) -> Arc<crate::node_apps::NodeApps> {
        Arc::new(
            crate::node_apps::NodeApps::new(
                paths.data_root().into(),
                paths.data_root().join("must-not-run-disposable-node"),
            )
            .unwrap(),
        )
    }
    fn marker_path(paths: &RuntimePaths, task: &str) -> PathBuf {
        paths
            .data_root()
            .join("onboarding/imports")
            .join(format!("{task}.json"))
    }
    fn external(root: &Path) -> PathBuf {
        let path = root
            .canonicalize()
            .unwrap()
            .join("disposable-external-author");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep.js"), "external author source must remain").unwrap();
        path
    }

    #[tokio::test]
    async fn recovery_requires_an_explicit_source_acquire_marker_and_completion_archives_exact_bytes(
    ) {
        let (temporary, paths, service, _client) = super::super::tests::start().await;
        let handle = service.handle();
        let apps = nodes(&paths);
        let author = external(temporary.path());
        let acquired = source(&handle, "sourceAcquire", true, &author);
        let captured = source(&handle, "sourceCapture", true, &author);
        assert_eq!(
            handle.recover_source_imports(apps.clone()).await.unwrap(),
            json!([])
        );
        assert!(acquired.slot.join("repository/app.js").is_file());
        assert_eq!(
            handle
                .remember_source_import(&captured.task, &captured.url)
                .unwrap_err()
                .code,
            "SOURCE_IMPORT_INVALID"
        );
        assert!(!marker_path(&paths, &captured.task).exists());

        handle
            .remember_source_import(&acquired.task, "https://example.invalid/untrusted-argument")
            .unwrap();
        let path = marker_path(&paths, &acquired.task);
        let original = fs::read(&path).unwrap();
        let record: Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(record["sourceId"], acquired.id);
        assert_eq!(record["url"], acquired.url);
        handle
            .remember_source_import(&acquired.task, &acquired.url)
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        handle.complete_source_import(&acquired.task).unwrap();
        let archived = path
            .parent()
            .unwrap()
            .join("completed")
            .join(path.file_name().unwrap());
        assert!(!path.exists());
        assert_eq!(fs::read(&archived).unwrap(), original);
        handle.complete_source_import(&acquired.task).unwrap();
        assert_eq!(fs::read(&archived).unwrap(), original);
        assert_eq!(
            handle.recover_source_imports(apps.clone()).await.unwrap(),
            json!([])
        );
        assert!(acquired.slot.join("repository/app.js").is_file());
        assert!(captured.slot.join("repository/app.js").is_file());
        assert_eq!(
            fs::read(author.join("keep.js")).unwrap(),
            b"external author source must remain"
        );
        apps.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_marker_ancestors_and_marker_files_never_follow_symlinks() {
        let (temporary, paths, service, _client) = super::super::tests::start().await;
        let handle = service.handle();
        let apps = nodes(&paths);
        let author = external(temporary.path());
        let acquired = source(&handle, "sourceAcquire", true, &author);
        let outside = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-marker-outside");
        fs::create_dir_all(outside.join("imports")).unwrap();
        let private = outside
            .join("imports")
            .join(format!("{}.json", acquired.task));
        fs::write(&private, b"outside marker content must remain").unwrap();
        let onboarding = paths.data_root().join("onboarding");
        symlink(&outside, &onboarding).unwrap();
        assert!(handle
            .remember_source_import(&acquired.task, &acquired.url)
            .is_err());
        assert!(handle.complete_source_import(&acquired.task).is_err());
        assert!(handle.recover_source_imports(apps.clone()).await.is_err());
        assert_eq!(
            fs::read(&private).unwrap(),
            b"outside marker content must remain"
        );
        assert!(!outside.join("imports/completed").exists());
        assert!(acquired.slot.join("repository/app.js").is_file());

        fs::remove_file(&onboarding).unwrap();
        let directory = onboarding.join("imports");
        fs::create_dir_all(&directory).unwrap();
        symlink(&private, marker_path(&paths, &acquired.task)).unwrap();
        assert!(handle
            .remember_source_import(&acquired.task, &acquired.url)
            .is_err());
        assert!(handle.complete_source_import(&acquired.task).is_err());
        assert!(handle.recover_source_imports(apps.clone()).await.is_err());
        assert_eq!(
            fs::read(&private).unwrap(),
            b"outside marker content must remain"
        );
        assert!(acquired.slot.join("repository/app.js").is_file());
        assert!(!directory.join("completed").exists());
        apps.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_keeps_running_attempts_and_rejects_wrong_source_before_reading_or_cleaning_it(
    ) {
        let (temporary, paths, service, _client) = super::super::tests::start().await;
        let handle = service.handle();
        let apps = nodes(&paths);
        let author = external(temporary.path());
        let running = source(&handle, "sourceAcquire", false, &author);
        let other = source(&handle, "sourceAcquire", true, &author);
        handle
            .remember_source_import(&running.task, &running.url)
            .unwrap();
        let result = handle.recover_source_imports(apps.clone()).await.unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
        assert_eq!(result[0]["status"], "running");
        assert!(running.slot.join(".acquiring/home/download.tmp").is_file());
        assert!(!paths
            .data_root()
            .join("cleanup/imports")
            .join(&running.task)
            .exists());
        let state = handle.current();
        state
            .builds
            .finish(&running.task, Ok(json!({"sourceId":running.id})))
            .unwrap();
        let path = marker_path(&paths, &running.task);
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["sourceId"] = json!(other.id);
        crate::builds::atomic_json(&path, &record).unwrap();
        let other_record = other.slot.join("source.json");
        let preserved = fs::read(&other_record).unwrap();
        fs::remove_file(&other_record).unwrap();
        let name = std::ffi::CString::new(other_record.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            handle
                .recover_source_imports(apps.clone())
                .await
                .unwrap_err()
                .code,
            "SOURCE_IMPORT_INVALID"
        );
        assert!(fs::symlink_metadata(&other_record)
            .unwrap()
            .file_type()
            .is_fifo());
        assert!(running.slot.join("repository/app.js").is_file());
        assert!(other.slot.join("repository/app.js").is_file());
        assert!(path.is_file());
        assert!(!path.parent().unwrap().join("completed").exists());
        assert!(!paths
            .data_root()
            .join("cleanup/imports")
            .join(&running.task)
            .exists());
        fs::remove_file(&other_record).unwrap();
        fs::write(&other_record, preserved).unwrap();
        assert_eq!(
            fs::read(author.join("keep.js")).unwrap(),
            b"external author source must remain"
        );
        apps.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn final_recovery_cleans_only_the_marked_source_and_retains_other_sources_and_author_files(
    ) {
        let (temporary, paths, service, _client) = super::super::tests::start().await;
        let handle = service.handle();
        let apps = nodes(&paths);
        let author = external(temporary.path());
        let acquired = source(&handle, "sourceAcquire", true, &author);
        let other = source(&handle, "sourceAcquire", true, &author);
        let other_record = fs::read(other.slot.join("source.json")).unwrap();
        handle
            .remember_source_import(&acquired.task, &acquired.url)
            .unwrap();
        let path = marker_path(&paths, &acquired.task);
        let marker = fs::read(&path).unwrap();
        let result = handle.recover_source_imports(apps.clone()).await.unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
        assert_eq!(result[0]["status"], "cleaned", "{result}");
        assert_eq!(result[0]["taskId"], acquired.task);
        assert!(!acquired.slot.exists());
        assert!(!path.exists());
        assert_eq!(
            fs::read(
                path.parent()
                    .unwrap()
                    .join("completed")
                    .join(path.file_name().unwrap())
            )
            .unwrap(),
            marker
        );
        assert_eq!(
            fs::read(other.slot.join("source.json")).unwrap(),
            other_record
        );
        assert_eq!(
            fs::read(other.slot.join("repository/app.js")).unwrap(),
            b"disposable source payload"
        );
        assert!(other.slot.join(".acquiring/home/download.tmp").is_file());
        assert_eq!(
            fs::read(author.join("keep.js")).unwrap(),
            b"external author source must remain"
        );
        assert_eq!(
            handle.recover_source_imports(apps.clone()).await.unwrap(),
            json!([])
        );
        assert_eq!(
            fs::read(other.slot.join("source.json")).unwrap(),
            other_record
        );
        apps.stop_all().await.unwrap();
        service.shutdown().await.unwrap();
    }
}
