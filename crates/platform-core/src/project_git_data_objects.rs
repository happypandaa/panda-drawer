//! Raw object materialization for data previews. Blob payloads deliberately do
//! not pass through Context's bounded metadata/stdout collector, Git filters,
//! checkout, a shell, or the business SDK.
use super::*;
use crate::files;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blob {
    pub oid: String,
    pub bytes: u64,
}

#[derive(Debug)]
pub(crate) struct DataTree {
    pub files: BTreeMap<String, Blob>,
    pub directories: BTreeSet<String>,
}

pub(crate) async fn target_tree(
    project: &ProjectHandle,
    target: &str,
    tools: &GitToolchain,
    context: &mut Context,
) -> Result<DataTree, ApiError> {
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["cat-file", "-t", target]);
    let kind = context.run(command, vec![]).await?;
    if kind.code != 0 || kind.stdout != b"commit\n" {
        return Err(target_error(
            "目标必须是本机仓库中已有的完整提交；未联网获取对象。",
        ));
    }
    let mut command = git_command(tools, context.root(), Some(project));
    command.args([
        "ls-tree",
        "-r",
        "-t",
        "-l",
        "-z",
        "--full-tree",
        target,
        "--",
        "version-data",
    ]);
    let bytes = context.run(command, vec![]).await?.success()?;
    parse_tree(&bytes, version_data::SnapshotLimits::default())
}

fn parse_tree(bytes: &[u8], limits: version_data::SnapshotLimits) -> Result<DataTree, ApiError> {
    if bytes.is_empty() || bytes.last() != Some(&0) {
        return Err(target_error("目标提交没有完整的 version-data 数据快照。"));
    }
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut total = 0u64;
    for (index, row) in bytes[..bytes.len() - 1]
        .split(|byte| *byte == 0)
        .enumerate()
    {
        // Includes explicit Git trees as well as files. The additional four
        // entries are the data root, attachments root and manifest/database.
        if index >= limits.max_entries + 4 {
            return Err(preview_limit());
        }
        let row = std::str::from_utf8(row).map_err(|_| target_error("目标路径不是有效 UTF-8。"))?;
        let (header, path) = row.split_once('\t').ok_or_else(output_invalid)?;
        let fields = header.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 || fields[2].len() != 40 || !valid_oid(fields[2]) {
            return Err(output_invalid());
        }
        files::parts(path)?;
        if path != version_data::DIRECTORY && !path.starts_with("version-data/") {
            return Err(target_error("目标数据树包含范围外路径。"));
        }
        if fields[0] == "040000" && fields[1] == "tree" && fields[3] == "-" {
            if !directories.insert(path.into()) {
                return Err(output_invalid());
            }
        } else if fields[0] == "100644" && fields[1] == "blob" {
            let size = fields[3].parse::<u64>().map_err(|_| output_invalid())?;
            let allowed = match path {
                "version-data/snapshot.json" => size <= MAX_METADATA,
                "version-data/database.sqlite" => size <= limits.max_file_bytes,
                _ if path.starts_with("version-data/attachments/") => size <= limits.max_file_bytes,
                _ => false,
            };
            if !allowed {
                return Err(target_error("目标数据树含额外文件或超过单文件预算。"));
            }
            total = total.checked_add(size).ok_or_else(preview_limit)?;
            if total > limits.max_total_bytes + MAX_METADATA
                || files
                    .insert(
                        path.into(),
                        Blob {
                            oid: fields[2].into(),
                            bytes: size,
                        },
                    )
                    .is_some()
            {
                return Err(preview_limit());
            }
        } else {
            return Err(target_error(
                "目标数据快照只接受普通文件和目录；不采用链接、子模块或可执行文件。",
            ));
        }
    }
    if !directories.contains(version_data::DIRECTORY)
        || !files.contains_key("version-data/snapshot.json")
        || !files.contains_key("version-data/database.sqlite")
    {
        return Err(target_error("目标提交缺少数据快照清单或数据库。"));
    }
    source_restore::validate_collisions(files.keys().map(String::as_str))?;
    Ok(DataTree { files, directories })
}

/// Each result file is created exclusively relative to an already opened,
/// private candidate root. Both protocol framing and the tree-declared object
/// identity/length are checked before accepting the stream.
pub(super) async fn materialize(
    project: &ProjectHandle,
    tools: &GitToolchain,
    context: &mut Context,
    destination: &Path,
    blobs: &BTreeMap<String, Blob>,
) -> Result<(), ApiError> {
    context.checkpoint()?;
    let root = files::root(destination)?;
    let blobs = blobs.clone();
    let stop = context.stop.clone();
    let cancel = context.cancel.clone();
    let deadline = context.deadline;
    #[cfg(test)]
    let mut writer_pause = context.writer_pause.take();
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["cat-file", "--batch"]);
    let run = context.start(command, move |pipes| {
        let mut stdin = BlockingPipe::new(pipes.stdin);
        let mut stdout = BlockingPipe::new(pipes.stdout);
        let internal_stop = pipes.stop;
        crate::owned_process::IoPlan::new(async move {
            // The registered session keeps this actual JoinHandle until the
            // blocking protocol/file writer completes. Caller stop never aborts
            // it or drops a tokio::fs wrapper over unfinished filesystem I/O.
            tokio::task::spawn_blocking(move || {
                let check = || {
                    process_io::checkpoint(&stop, &cancel, deadline)?;
                    if *internal_stop.borrow() {
                        return Err(interrupted());
                    }
                    Ok::<_, ApiError>(())
                };
                let mut buffer = vec![0u8; 64 * 1024];
                for (path, blob) in blobs {
                    check()?;
                    let relative = path
                        .strip_prefix("version-data/")
                        .ok_or_else(output_invalid)?;
                    stdin
                        .write_all(format!("{}\n", blob.oid).as_bytes())
                        .map_err(|_| git_failed())?;
                    stdin.flush().map_err(|_| git_failed())?;
                    process_io::expect_blocking_line(
                        &mut stdout,
                        format!("{} blob {}\n", blob.oid, blob.bytes).as_bytes(),
                    )?;
                    let mut output = files::create(&root, relative)?;
                    #[cfg(test)]
                    if let Some((entered, release)) = writer_pause.take() {
                        let _ = entered.send(());
                        let _ = release.recv();
                    }
                    let mut remaining = blob.bytes;
                    while remaining != 0 {
                        check()?;
                        let length = remaining.min(buffer.len() as u64) as usize;
                        stdout
                            .read_exact(&mut buffer[..length])
                            .map_err(|_| output_invalid())?;
                        output.write_all(&buffer[..length])?;
                        remaining -= length as u64;
                    }
                    output.sync_all()?;
                    let mut delimiter = [0u8; 1];
                    stdout
                        .read_exact(&mut delimiter)
                        .map_err(|_| output_invalid())?;
                    if delimiter[0] != b'\n' {
                        return Err(output_invalid());
                    }
                }
                drop(stdin);
                let mut extra = [0u8; 1];
                if stdout.read(&mut extra).map_err(|_| output_invalid())? != 0 {
                    return Err(output_invalid());
                }
                Ok::<_, ApiError>(())
            })
            .await
            .map_err(|_| super::preview_worker_failed())?
        })
        .drain("stderr", async move {
            read_output(pipes.stderr, false, 64 * 1024)
                .await
                .map(|_| ())
        })
    })?;
    let output = context.wait(run).await?;
    if !output.status.success() {
        return Err(git_failed());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_tree_rejects_missing_special_and_oversize_payloads() {
        let oid = "1234567890123456789012345678901234567890";
        let base = format!("040000 tree {oid}       -\tversion-data\0");
        let manifest = format!("100644 blob {oid}      10\tversion-data/snapshot.json\0");
        let database = format!("100644 blob {oid}      12\tversion-data/database.sqlite\0");
        let valid = format!("{base}{manifest}{database}");
        assert!(parse_tree(valid.as_bytes(), version_data::SnapshotLimits::default()).is_ok());
        for invalid in [
            format!("{base}{manifest}"),
            valid.replace("100644", "120000"),
            format!("{valid}100644 blob {oid} 256000001\tversion-data/attachments/huge\0"),
            format!("{valid}100644 blob {oid} 1\tversion-data/unknown\0"),
            format!("{valid}100644 blob {oid} 1\tversion-data/attachments/../escape\0"),
        ] {
            assert!(
                parse_tree(invalid.as_bytes(), version_data::SnapshotLimits::default()).is_err()
            );
        }
    }

    #[tokio::test]
    async fn cancellation_retains_real_blocking_writer_lease_gate_and_candidate_until_join() {
        let (_temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let created = store
            .create(crate::projects::CreateProjectInput {
                name: "Disposable raw writer".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&created.id).unwrap();
        let operation = Arc::new(
            store
                .reserve_transient_operation(&created.id, "raw-writer-fixture")
                .unwrap(),
        );
        let phase = Arc::new(project.operation_gate.clone().read_owned().await);
        let (stop, receiver) = watch::channel(false);
        let (_cancel, cancellation) = watch::channel(false);
        let mut context = Context::new(receiver, cancellation).unwrap();
        context.bind(store.processes(), operation);
        context.phase(phase);
        let candidate = Arc::new(tempfile::tempdir().unwrap());
        let destination = candidate.path().canonicalize().unwrap();
        context.retain(candidate);
        let private = context.root().to_path_buf();
        let script = project.root.join("synthetic-cat-file.py");
        fs::write(&script, "#!/usr/bin/python3\nimport sys\nfor line in sys.stdin.buffer:\n oid=line.strip()\n payload=b'x'*(9*1024*1024)\n sys.stdout.buffer.write(oid+b' blob '+str(len(payload)).encode()+b'\\n'+payload+b'\\n');sys.stdout.buffer.flush()\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let tools = GitToolchain {
            git: ExecutableInfo {
                path: script,
                version: "2.56.0".into(),
            },
            path: "/usr/bin:/bin".into(),
            exec_path: None,
            template_path: None,
        };
        let blobs = BTreeMap::from([(
            "version-data/attachments/payload.bin".into(),
            Blob {
                oid: "a".repeat(40),
                bytes: 9 * 1024 * 1024,
            },
        )]);
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        struct ReleaseWriter(Option<std::sync::mpsc::Sender<()>>);
        impl Drop for ReleaseWriter {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let release = ReleaseWriter(Some(release));
        context.writer_pause = Some((entered, blocked));
        let writer_project = project.clone();
        let writer_destination = destination.clone();
        let request = tokio::spawn(async move {
            materialize(
                &writer_project,
                &tools,
                &mut context,
                &writer_destination,
                &blobs,
            )
            .await
        });
        tokio::task::spawn_blocking(move || observed.recv_timeout(Duration::from_secs(3)))
            .await
            .unwrap()
            .unwrap();
        assert!(destination.join("attachments/payload.bin").is_file());
        stop.send(true).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(4), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
        assert!(store.has_active_tasks());
        assert!(project.operation_gate.try_write().is_err());
        assert!(destination.is_dir());
        assert!(private.is_dir());
        assert!(store
            .processes()
            .shutdown()
            .await
            .iter()
            .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
        drop(release);
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let registry = store.processes();
                if registry.shutdown().await.is_empty()
                    && registry
                        .snapshot()
                        .iter()
                        .all(|state| state.resource_closed())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!store.has_active_tasks());
        assert!(project.operation_gate.try_write().is_ok());
        assert!(!destination.exists());
        assert!(!private.exists());
        assert!(store.shutdown().await.is_empty());
    }
}
