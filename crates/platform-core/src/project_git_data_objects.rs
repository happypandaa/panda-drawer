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
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(["cat-file", "--batch"]);
    let child = command.spawn().map_err(|_| git_failed())?;
    let mut process = Process {
        group: child.id(),
        child,
    };
    let mut stdin = process.child.stdin.take().ok_or_else(git_failed)?;
    let mut stdout = process.child.stdout.take().ok_or_else(git_failed)?;
    let stderr = process.child.stderr.take().ok_or_else(git_failed)?;
    let result = tokio::select! {
        biased;
        _ = watch_stop(&mut context.stop) => Err(interrupted()),
        _ = watch_stop(&mut context.cancel) => Err(interrupted()),
        _ = tokio::time::sleep_until(context.deadline) => Err(preview_timeout()),
        result = async {
            let (_, ()) = tokio::try_join!(
                read_output(stderr, false, 64 * 1024),
                async {
                    // Keep the bounded streaming buffer on the heap: this
                    // future is nested inside preview/cancellation selections.
                    let mut buffer = vec![0u8; 64 * 1024];
                    for (path, blob) in blobs {
                        let relative = path.strip_prefix("version-data/").ok_or_else(output_invalid)?;
                        stdin.write_all(format!("{}\n", blob.oid).as_bytes()).await.map_err(|_| git_failed())?;
                        stdin.flush().await.map_err(|_| git_failed())?;
                        expect_line(&mut stdout, format!("{} blob {}\n", blob.oid, blob.bytes).as_bytes()).await?;
                        let output = files::create(&root, relative)?;
                        let mut output = tokio::fs::File::from_std(output);
                        let mut remaining = blob.bytes;
                        while remaining != 0 {
                            let length = remaining.min(buffer.len() as u64) as usize;
                            stdout.read_exact(&mut buffer[..length]).await.map_err(|_| output_invalid())?;
                            output.write_all(&buffer[..length]).await?;
                            remaining -= length as u64;
                        }
                        output.flush().await?;
                        output.sync_all().await?;
                        if stdout.read_u8().await.map_err(|_| output_invalid())? != b'\n' {
                            return Err(output_invalid());
                        }
                    }
                    drop(stdin);
                    if !read_output(stdout, true, 1).await?.is_empty()
                        || !process.child.wait().await.map_err(|_| git_failed())?.success()
                    {
                        return Err(output_invalid());
                    }
                    Ok::<(), ApiError>(())
                }
            )?;
            Ok(())
        } => result,
    };
    process.finish().await;
    result
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
}
