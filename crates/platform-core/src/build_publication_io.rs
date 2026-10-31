//! Platform publication and persistence, with the original Mac flow intact.
use super::*;

#[cfg(not(windows))]
pub(super) fn publish(
    paths: &RuntimePaths,
    project: &ProjectHandle,
    staging: &Path,
    provenance: &BuildProvenance,
    previous_digest: Option<String>,
) -> Result<(), ApiError> {
    let generation = checked_relative(
        &project.root,
        &format!(".platform/artifacts/{}/dist", provenance.build_id),
        true,
    )?;
    ensure_dir(generation.parent().unwrap())?;
    sync_tree(staging)?;
    fs::rename(staging, &generation)?;
    File::open(generation.parent().unwrap())?.sync_all()?;
    let publication = staging.parent().unwrap().join("publication");
    copy_tree(&generation, &publication)?;
    let journal = Journal {
        project_id: project.manifest.id.clone(),
        provenance: provenance.clone(),
        previous_digest,
    };
    let journal_path = journal_path(paths.workspace_root(), &project.manifest.id);
    atomic_json(&journal_path, &journal)?;
    let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
    replace_directory(&publication, &output)?;
    atomic_json(
        &provenance_path(paths.workspace_root(), &project.manifest.id),
        provenance,
    )?;
    // Provenance is the commit point. Cleanup failure must not turn a committed build into failure.
    let _ = fs::remove_file(&journal_path);
    let _ = File::open(journal_path.parent().unwrap()).and_then(|file| file.sync_all());
    // Previous files are retained as runtime resources, not exposed as user version history.
    Ok(())
}

#[cfg(windows)]
pub(super) fn publish(
    paths: &RuntimePaths,
    project: &ProjectHandle,
    staging: &Path,
    provenance: &BuildProvenance,
    previous_digest: Option<String>,
) -> Result<(), ApiError> {
    let generation = checked_relative(
        &project.root,
        &format!(".platform/artifacts/{}/dist", provenance.build_id),
        true,
    )?;
    ensure_dir(generation.parent().unwrap())?;
    // Copy through fresh writable, private handles and flush each while still
    // open. Reopening read-only files for FlushFileBuffers is not supported on
    // Windows; changing a source's attributes could affect external hard links.
    // Keep the owned staging tree for the existing task cleanup/recovery scope.
    copy_tree(staging, &generation)?;
    platform_native::filesystem::sync_directory(generation.parent().unwrap())?;
    let publication = staging.parent().unwrap().join("publication");
    copy_tree(&generation, &publication)?;
    let journal = Journal {
        project_id: project.manifest.id.clone(),
        provenance: provenance.clone(),
        previous_digest,
    };
    let journal_path = journal_path(paths.workspace_root(), &project.manifest.id);
    atomic_json(&journal_path, &journal)?;
    let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
    replace_directory(&publication, &output)?;
    persist_current_provenance_with(|| {
        atomic_json(
            &provenance_path(paths.workspace_root(), &project.manifest.id),
            provenance,
        )
    })?;
    // Preserve the existing provenance commit point. Retiring its outer journal
    // is best effort after commit; absence is reconciled on the next startup.
    let _ = fs::remove_file(&journal_path);
    let _ = platform_native::filesystem::sync_directory(journal_path.parent().unwrap());
    Ok(())
}

#[cfg(windows)]
fn persist_current_provenance_with(
    write: impl FnOnce() -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    write().map_err(|error| {
        if error.details["operationOutcome"] == "unknown" {
            // The native rename may already have published the commit record.
            // A later missing/replaced name cannot certify a pre-commit failure;
            // retain the outer journal and artifacts for actual recovery.
            error.detail("buildProvenanceCommitPending", true)
        } else {
            error
        }
    })
}

pub(super) fn may_rollback(error: &ApiError) -> bool {
    #[cfg(windows)]
    {
        // A partially moved namespace must be recovered from its exact journal
        // before any outer attempt to restore the previous output.
        error.details["artifactExchangePending"] != true
            && error.details["buildProvenanceCommitPending"] != true
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        true
    }
}

pub(super) fn recover_exchange(project: &ProjectHandle, journal: &Journal) -> Result<(), ApiError> {
    #[cfg(windows)]
    {
        let staged = checked_relative(
            &project.root,
            &format!(
                ".platform/builds/{}/publication",
                journal.provenance.build_id
            ),
            true,
        )?;
        let output = checked_relative(&project.root, &project.manifest.output_dir, true)?;
        match fs::symlink_metadata(staged.parent().unwrap()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // No inner journal can remain in an absent task parent. Check
                // the original project binding/volume again, then let the
                // outer journal reconcile the actual old/new output digest.
                // Do not recreate a task directory or initiate another swap.
                project.check()?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        // False only means no active inner journal. The caller must still
        // reconcile the real output digest and outer provenance; it must never
        // invoke replace_directory again merely because the journal is gone.
        directory_exchange::resume_pending(&staged, &output)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (project, journal);
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_exchange_blocks_outer_rollback_without_changing_ordinary_errors() {
        let ordinary = ApiError::new("DISPOSABLE_BUILD_FAILURE", "fixture");
        assert!(may_rollback(&ordinary));
        assert!(may_rollback(
            &ordinary.clone().detail("artifactExchangePending", false)
        ));
        let pending = ordinary
            .detail("artifactExchangePending", true)
            .outcome("unknown");
        assert!(!may_rollback(&pending));
    }

    fn provenance(id: &str, digest: &str) -> BuildProvenance {
        let tool = crate::environment::ExecutableInfo {
            path: "disposable-not-executed.exe".into(),
            version: "fixture".into(),
        };
        BuildProvenance {
            schema_version: 1,
            build_id: id.into(),
            task_id: id.into(),
            completed_at: 123,
            digest_rule_version: 1,
            input_digest: "a".repeat(64),
            artifact_digest: digest.into(),
            node: tool.clone(),
            npm: tool,
            sdk_version: None,
            entry_relative: "index.html".into(),
        }
    }

    #[test]
    fn provenance_precommit_failures_keep_the_original_error_and_rollback_policy() {
        let ordinary = ApiError::new("DISPOSABLE_PROVENANCE_WRITE_FAILED", "not published")
            .detail("retained", "fixture");
        let error = persist_current_provenance_with(|| Err(ordinary.clone())).unwrap_err();
        assert_eq!(error.code, ordinary.code);
        assert_eq!(error.message, ordinary.message);
        assert_eq!(error.details, ordinary.details);
        assert!(may_rollback(&error));
        assert!(persist_current_provenance_with(|| Ok(())).is_ok());
    }

    #[test]
    fn a_published_but_unconfirmed_provenance_never_authorizes_outer_rollback() {
        use platform_native::windows::files as native;

        for replacement in [false, true] {
            let temporary = tempfile::Builder::new()
                .prefix("panda-disposable-provenance-confirmation-中文 space-")
                .tempdir()
                .unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let old_id = format!("task_{}", Uuid::new_v4().simple());
            let next_id = format!("task_{}", Uuid::new_v4().simple());
            let old = provenance(&old_id, &"b".repeat(64));
            let next = provenance(&next_id, &"c".repeat(64));
            let current = provenance_path(&root, "prj_disposable");
            atomic_json(&current, &old).unwrap();
            let outer = journal_path(&root, "prj_disposable");
            atomic_json(
                &outer,
                &Journal {
                    project_id: "prj_disposable".into(),
                    provenance: next.clone(),
                    previous_digest: Some(old.artifact_digest.clone()),
                },
            )
            .unwrap();
            let outer_bytes = fs::read(&outer).unwrap();
            let output = root.join("disposable-project/dist");
            let previous = root.join("disposable-project/previous");
            ensure_dir(&output).unwrap();
            ensure_dir(&previous).unwrap();
            fs::write(output.join("index.html"), b"new published output").unwrap();
            fs::write(previous.join("index.html"), b"old output").unwrap();
            let retained = root.join("disposable-displaced-new-provenance.json");

            let error = persist_current_provenance_with(|| {
                // Real private bytes are flushed and renamed, then the name is
                // deliberately moved/replaced before a controlled confirmation.
                // This checks the publisher's uncertain-commit receipt policy;
                // it does not inject the original native callback end to end.
                let stage = current.parent().unwrap().join("disposable-stage.json");
                let mut file = native::create_private_file(&stage)?;
                file.write_all(&serde_json::to_vec_pretty(&next)?)?;
                file.sync_all()?;
                let expected = native::identity(&file)?;
                drop(file);
                native::rename_file(&stage, &current, true)?;
                fs::rename(&current, &retained)?;
                if replacement {
                    atomic_json(&current, &old)?;
                }
                let cause = match native::open_no_reparse(&current) {
                    Ok(actual) if !expected.same_object(&actual.identity) => std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Published provenance name now identifies a replacement",
                    ),
                    Ok(_) => panic!("The controlled provenance name was not displaced"),
                    Err(error) => error,
                };
                Err(ApiError::from(cause).outcome("unknown"))
            })
            .unwrap_err();

            assert_eq!(error.details["operationOutcome"], "unknown");
            assert_eq!(error.details["buildProvenanceCommitPending"], true);
            assert!(!may_rollback(&error));
            let observed: Option<BuildProvenance> = read_optional(&current).unwrap();
            assert_eq!(
                observed.map(|record| record.build_id),
                replacement.then_some(old_id)
            );
            let displaced: BuildProvenance = read_optional(&retained).unwrap().unwrap();
            assert_eq!(displaced.build_id, next_id);
            assert_eq!(fs::read(&outer).unwrap(), outer_bytes);
            assert_eq!(
                fs::read(output.join("index.html")).unwrap(),
                b"new published output"
            );
            assert_eq!(
                fs::read(previous.join("index.html")).unwrap(),
                b"old output"
            );
        }
    }
}
