//! Disposable offline fixtures verify orchestration; they are not real HTTP/SSH evidence.
use super::*;

#[test]
fn strict_public_repository_urls_are_rebuilt_without_credentials_or_ambiguity() {
    for (input, expected, provider) in [
        (
            "https://github.com/Owner/repository",
            "https://github.com/Owner/repository.git",
            "github",
        ),
        (
            "https://github.com/Owner/repository.git/",
            "https://github.com/Owner/repository.git",
            "github",
        ),
        (
            "https://gitee.com/team_1/repository-v1.2/",
            "https://gitee.com/team_1/repository-v1.2.git",
            "gitee",
        ),
    ] {
        let remote = validate_url(input).unwrap();
        assert_eq!(remote.url, expected);
        assert_eq!(remote.provider, provider);
    }
    for input in [
        "",
        " https://github.com/a/b",
        "https://github.com/a/b\n",
        "https://github.com/a/b?secret=x",
        "https://github.com/a/b#fragment",
        "https://github.com/a/b/extra",
        "https://github.com/a/b//",
        "https://github.com:443/a/b",
        "https://github.com:8443/a/b",
        "https://user@github.com/a/b",
        "https://user:password@github.com/a/b",
        "https://github.com.evil.invalid/a/b",
        "https://github.com@evil.invalid/a/b",
        "https://GITHUB.com/a/b",
        "HTTPS://github.com/a/b",
        "http://github.com/a/b",
        "ssh://git@github.com/a/b",
        "git@github.com:a/b",
        "file:///a/b",
        "ext::command",
        "--upload-pack=command",
        "https://github.com/a",
        "https://github.com//b",
        "https://github.com/a/../b",
        "https://github.com/./b",
        "https://github.com/a/.git",
        "https://github.com/a/%62",
        "https://github.com/a/b%2fc",
        "https://github.com/a/b%0a",
        "https://github.com/a/b\\c",
        "https://github.com/a/中文",
        "https://github.com/-a/b",
        "https://github.com/a/_b",
        "https://github.com/a/b\0",
    ] {
        assert_eq!(
            validate_url(input).err().unwrap().code,
            "GIT_REMOTE_URL_INVALID",
            "accepted {input:?}"
        );
    }
    assert!(validate_url(&format!("https://github.com/{}/repo", "a".repeat(256))).is_err());
    assert!(serde_json::from_str::<GitRemoteCheckInput>(
        r#"{"url":"https://github.com/a/b","transport":"ssh"}"#
    )
    .is_err());
    assert!(serde_json::from_str::<GitRemoteCheckInput>("{}").is_err());
}

#[test]
fn only_versions_verified_for_this_operation_are_accepted() {
    for version in ["2.54.0", "2.54.0 (Apple Git-160)", "2.56.0"] {
        assert!(supported_version(version));
    }
    for version in [
        "2.53.0",
        "2.54.1",
        "2.56.0.rc1",
        "2.99.0",
        "2.54.0 (other fork)",
        "2.54.0 (Apple Git-)",
        "2.54.0 (Apple Git-160) extra",
    ] {
        assert!(!supported_version(version), "accepted {version}");
    }
}

#[test]
fn head_parser_selects_exact_head_and_preserves_optional_symbolic_reference() {
    let oid = "a".repeat(40);
    let output = format!(
        "ref: refs/heads/功能\tHEAD\n{oid}\tHEAD\n{}\trefs/heads/HEAD\n",
        "b".repeat(40)
    );
    let head = parse_head(output.as_bytes()).unwrap().unwrap();
    assert_eq!(head.oid, oid);
    assert_eq!(head.symbolic_ref.as_deref(), Some("refs/heads/功能"));
    let oid = "B".repeat(64);
    let head = parse_head(format!("{oid}\tHEAD\n").as_bytes())
        .unwrap()
        .unwrap();
    assert_eq!(head.oid, oid.to_ascii_lowercase());
    assert!(head.symbolic_ref.is_none());
    for output in [
        String::new(),
        format!("{}\trefs/heads/HEAD\n", "a".repeat(40)),
        "ref: refs/heads/main\tHEAD\n".into(),
    ] {
        assert!(parse_head(output.as_bytes()).unwrap().is_none());
    }
}

#[test]
fn head_parser_rejects_ambiguous_or_malformed_remote_output() {
    let oid = "a".repeat(40);
    for output in [
        "unstructured secret output".into(),
        "\n".into(),
        "short\tHEAD\n".into(),
        format!("{oid}\tHEAD\n{oid}\tHEAD\n"),
        format!("{}\tHEAD\n", "0".repeat(40)),
        "ref: refs/heads/a.lock\tHEAD\n".into(),
        "ref: refs/heads/../other\tHEAD\n".into(),
        "ref: refs/heads/a\tHEAD\nref: refs/heads/b\tHEAD\n".into(),
        format!("{oid}\tHEAD\tunexpected\n"),
        format!("{oid}\tHEAD\r\n"),
    ] {
        assert_eq!(
            parse_head(output.as_bytes()).err().unwrap().code,
            "GIT_REMOTE_OUTPUT_INVALID"
        );
    }
    assert!(parse_head(b"\xff\tHEAD\n").is_err());
}

#[cfg(target_os = "macos")]
mod processes {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
    use tempfile::{Builder, TempDir};

    struct Sample {
        _temp: TempDir,
        root: PathBuf,
        git: PathBuf,
        environment: GitEnvironmentManager,
    }
    fn quote(path: &Path) -> String {
        format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
    }
    fn sample() -> Sample {
        let temp = Builder::new()
            .prefix("disposable-git-remote-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let git = root.join("synthetic Git 工具");
        fs::write(
            root.join("git-toolchain.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1, "gitPath": git,
            }))
            .unwrap(),
        )
        .unwrap();
        let environment =
            GitEnvironmentManager::new(root.clone(), Some(root.join("missing-managed-resource")));
        Sample {
            _temp: temp,
            root,
            git,
            environment,
        }
    }
    fn install(sample: &Sample, version: &str, body: &str) {
        fs::write(&sample.git, format!(
            "#!/bin/sh\nif test \"${{1-}}\" = '--version'; then printf '%s\\n' 'git version {version}'; exit 0; fi\n{body}\n"
        )).unwrap();
        fs::set_permissions(&sample.git, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn input() -> GitRemoteCheckInput {
        GitRemoteCheckInput {
            url: "https://github.com/disposable-owner/disposable-repo/".into(),
        }
    }
    async fn await_file(path: &Path) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("synthetic process did not start");
    }

    #[tokio::test]
    async fn public_check_preserves_settings_and_uses_isolated_arguments_environment_and_cwd() {
        let sample = sample();
        let observations = sample.root.join("observations");
        let cwd_record = sample.root.join("cwd");
        fs::write(
            sample.root.join("toolchain.json"),
            "preserved Node settings",
        )
        .unwrap();
        let settings_before = fs::read(sample.root.join("git-toolchain.json")).unwrap();
        install(&sample, "2.54.0 (Apple Git-160)", &format!(
            "{{ printf 'arg=%s\\n' \"$@\"; /usr/bin/env; }} > {}\npwd > {}\nprintf 'ref: refs/heads/main\\tHEAD\\n{}\\tHEAD\\n'",
            quote(&observations), quote(&cwd_record), "a".repeat(40)
        ));
        let (_sender, stop) = watch::channel(false);
        let result = GitRemoteChecker::new()
            .check(&sample.environment, input(), stop)
            .await
            .unwrap();
        assert_eq!(result.state, "readable");
        assert_eq!(result.provider, "github");
        assert_eq!(
            result.url,
            "https://github.com/disposable-owner/disposable-repo.git"
        );
        assert_eq!(result.transport, "public-https");
        assert_eq!(result.git.version, "2.54.0 (Apple Git-160)");
        assert_eq!(
            result.head.unwrap().symbolic_ref.as_deref(),
            Some("refs/heads/main")
        );
        assert!(result.checked_at.ends_with('Z') && result.checked_at.contains('T'));
        let cwd = fs::read_to_string(cwd_record).unwrap();
        assert!(
            !Path::new(cwd.trim()).exists(),
            "temporary check directory was not removed"
        );
        assert!(!cwd.starts_with(sample.root.to_str().unwrap()));
        let observed = fs::read_to_string(observations).unwrap();
        for required in [
            "arg=ls-remote\narg=--symref\narg=--exit-code\narg=--\narg=https://github.com/disposable-owner/disposable-repo.git\narg=HEAD\n",
            "GIT_CONFIG_GLOBAL=/dev/null\n", "GIT_CONFIG_SYSTEM=/dev/null\n", "GIT_CONFIG_NOSYSTEM=1\n",
            "GIT_CONFIG_COUNT=0\n", "GIT_TERMINAL_PROMPT=0\n", "GIT_ALLOW_PROTOCOL=https\n", "GIT_PROTOCOL_FROM_USER=0\n",
            "GIT_ASKPASS=/usr/bin/false\n", "GIT_SSH=/usr/bin/false\n", "GIT_SSH_COMMAND=/usr/bin/false\n",
            "arg=credential.helper=\n", "arg=credential.interactive=false\n", "arg=http.emptyAuth=false\n",
            "arg=http.proactiveAuth=none\n", "arg=http.delegation=none\n", "arg=http.followRedirects=false\n",
            "arg=http.extraHeader=\n", "arg=http.proxy=\n", "arg=http.sslVerify=true\n", "arg=http.saveCookies=false\n",
        ] { assert!(observed.contains(required), "missing {required}"); }
        for forbidden in [
            "SSH_AUTH_SOCK=",
            "http_proxy=",
            "https_proxy=",
            "ALL_PROXY=",
            "GIT_DIR=",
            "GIT_WORK_TREE=",
            "GIT_TRACE=",
            "GIT_CURL_VERBOSE=",
            "arg=http.cookieFile=",
        ] {
            assert!(!observed.contains(forbidden), "unexpected {forbidden}");
        }
        for variable in ["HOME", "XDG_CONFIG_HOME"] {
            assert!(observed.contains(&format!("{variable}={}\n", cwd.trim())));
        }
        assert!(observed.contains(&format!(
            "GIT_CEILING_DIRECTORIES={}\n",
            Path::new(cwd.trim()).parent().unwrap().display()
        )));
        assert_eq!(
            fs::read(sample.root.join("git-toolchain.json")).unwrap(),
            settings_before
        );
        assert_eq!(
            fs::read_to_string(sample.root.join("toolchain.json")).unwrap(),
            "preserved Node settings"
        );
    }

    #[tokio::test]
    async fn exit_two_and_success_without_exact_head_are_readable_without_head() {
        for body in [
            "exit 2".to_owned(),
            String::new(),
            format!("printf '{}\\trefs/heads/HEAD\\n'", "b".repeat(40)),
        ] {
            let sample = sample();
            install(&sample, "2.56.0", &body);
            let (_sender, stop) = watch::channel(false);
            let result = GitRemoteChecker::new()
                .check(&sample.environment, input(), stop)
                .await
                .unwrap();
            assert_eq!(result.state, "readable-no-head");
            assert!(result.head.is_none());
        }
    }

    #[tokio::test]
    async fn invalid_url_and_initial_stop_do_not_execute_even_the_version_probe() {
        let sample = sample();
        let marker = sample.root.join("unexpected-execution");
        fs::write(
            &sample.git,
            format!("#!/bin/sh\nprintf unexpected > {}\n", quote(&marker)),
        )
        .unwrap();
        fs::set_permissions(&sample.git, fs::Permissions::from_mode(0o755)).unwrap();
        let checker = GitRemoteChecker::new();
        let (_sender, stop) = watch::channel(false);
        let error = checker
            .check(
                &sample.environment,
                GitRemoteCheckInput {
                    url: "https://user:fixture-token@github.com/a/b".into(),
                },
                stop,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "GIT_REMOTE_URL_INVALID");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("fixture-token"));
        let (_sender, stop) = watch::channel(true);
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop)
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_CHECK_CANCELLED"
        );
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn unsupported_or_missing_explicit_git_does_not_probe_remote_or_fall_back() {
        let sample = sample();
        let marker = sample.root.join("unexpected-remote");
        install(
            &sample,
            "2.53.0",
            &format!("printf unexpected > {}", quote(&marker)),
        );
        let checker = GitRemoteChecker::new();
        let (_sender, stop) = watch::channel(false);
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop.clone())
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_VERSION_UNSUPPORTED"
        );
        assert!(!marker.exists());
        fs::remove_file(&sample.git).unwrap();
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop)
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_ENVIRONMENT_UNAVAILABLE"
        );
    }

    #[tokio::test]
    async fn errors_discard_private_stderr_and_bound_each_output_stream() {
        let sample = sample();
        let checker = GitRemoteChecker::new();
        let (_sender, stop) = watch::channel(false);
        install(
            &sample,
            "2.56.0",
            "printf 'synthetic-sensitive-stderr' >&2\nexit 128",
        );
        let error = checker
            .check(&sample.environment, input(), stop.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code, "GIT_REMOTE_CHECK_FAILED");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("synthetic-sensitive-stderr"));
        for body in ["printf '%65537s' x", "printf '%65537s' x >&2"] {
            install(&sample, "2.56.0", body);
            assert_eq!(
                checker
                    .check(&sample.environment, input(), stop.clone())
                    .await
                    .unwrap_err()
                    .code,
                "GIT_REMOTE_OUTPUT_LIMIT"
            );
        }
        install(&sample, "2.56.0", "printf 'synthetic-sensitive-stdout'");
        let error = checker
            .check(&sample.environment, input(), stop)
            .await
            .unwrap_err();
        assert_eq!(error.code, "GIT_REMOTE_OUTPUT_INVALID");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("synthetic-sensitive-stdout"));
    }

    #[tokio::test]
    async fn no_matching_refs_exit_cannot_claim_success_with_nonempty_output() {
        let sample = sample();
        install(
            &sample,
            "2.56.0",
            &format!("printf '{}\\tHEAD\\n'\nexit 2", "a".repeat(40)),
        );
        let (_sender, stop) = watch::channel(false);
        assert_eq!(
            GitRemoteChecker::new()
                .check(&sample.environment, input(), stop)
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_OUTPUT_INVALID"
        );
    }

    #[tokio::test]
    #[ignore = "Opt-in: PLATFORM_TEST_REMOTE_GIT must name an official disposable Git under .test-artifacts; no network is used"]
    async fn official_git_does_not_read_repository_configuration_above_the_check_directory() {
        let git = PathBuf::from(
            std::env::var_os("PLATFORM_TEST_REMOTE_GIT")
                .expect("official disposable Git path required"),
        )
        .canonicalize()
        .unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        assert!(git.starts_with(workspace.join(".test-artifacts")));
        assert!(git.components().any(|part| part
            .as_os_str()
            .to_string_lossy()
            .starts_with("disposable-")));
        let sample = sample();
        let parent = sample.root.join("synthetic-parent-repository");
        let cwd = parent.join("empty-child");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(parent.join(".git/objects")).unwrap();
        fs::create_dir_all(parent.join(".git/refs/heads")).unwrap();
        fs::write(parent.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(parent.join(".git/config"), "[core]\nrepositoryformatversion=0\nbare=false\n[remotecheck]\nisolation=synthetic-parent-config-should-not-be-read\n").unwrap();
        let before = fs::read(parent.join(".git/config")).unwrap();
        let toolchain = GitToolchain {
            git: ExecutableInfo {
                path: git.clone(),
                version: "2.56.0".into(),
            },
            path: format!(
                "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                git.parent().unwrap().display()
            ),
            exec_path: None,
            template_path: None,
        };
        let (_sender, mut stop) = watch::channel(false);
        let processes = OwnedProcessRegistry::default();
        let keep_alive = Arc::new(sample._temp);
        // A control command proves this disposable fixture really is discoverable.
        let mut control = isolated_command(&toolchain, &cwd).unwrap();
        control.env_remove("GIT_CEILING_DIRECTORIES").args([
            "config",
            "--local",
            "--get",
            "remotecheck.isolation",
        ]);
        let output = execute(
            control,
            &processes,
            keep_alive.clone(),
            &mut stop,
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(
            output.stdout,
            b"synthetic-parent-config-should-not-be-read\n"
        );
        // Reuse the production environment unchanged, with only a non-network Git command.
        let mut isolated = isolated_command(&toolchain, &cwd).unwrap();
        isolated.args(["config", "--local", "--get", "remotecheck.isolation"]);
        assert_eq!(
            execute(
                isolated,
                &processes,
                keep_alive.clone(),
                &mut stop,
                Instant::now() + Duration::from_secs(5)
            )
            .await
            .err()
            .unwrap()
            .code,
            "GIT_REMOTE_CHECK_FAILED"
        );
        assert_eq!(fs::read(parent.join(".git/config")).unwrap(), before);
        assert_eq!(fs::read_dir(&cwd).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn concurrent_clones_share_one_slot_and_service_stop_releases_it() {
        let sample = sample();
        let started = sample.root.join("started");
        install(
            &sample,
            "2.56.0",
            &format!("printf started > {}\n/bin/sleep 60 & wait", quote(&started)),
        );
        let checker = GitRemoteChecker::new();
        let other = checker.clone();
        let environment = sample.environment.clone();
        let (sender, stop) = watch::channel(false);
        let first_stop = stop.clone();
        let running =
            tokio::spawn(async move { other.check(&environment, input(), first_stop).await });
        await_file(&started).await;
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop.clone())
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_CHECK_BUSY"
        );
        sender.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.unwrap_err().code, "GIT_REMOTE_CHECK_CANCELLED");
        install(&sample, "2.56.0", "exit 2");
        let (_sender, stop) = watch::channel(false);
        assert!(checker
            .check(&sample.environment, input(), stop)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn cancellation_and_deadline_await_the_actual_managed_verification_worker() {
        for cancel in [false, true] {
            let sample = sample();
            fs::remove_file(sample.root.join("git-toolchain.json")).unwrap();
            let root = sample.root.join("missing-managed-resource");
            let (pause, entered) =
                crate::managed_git::verification_pause::Guard::install(root.clone());
            let processes = OwnedProcessRegistry::default();
            let environment = GitEnvironmentManager::with_processes(
                sample.root.clone(),
                Some(root),
                processes.clone(),
            );
            let checker = GitRemoteChecker::with_processes(processes.clone());
            let other = checker.clone();
            let (sender, stop) = watch::channel(false);
            let mut running = tokio::spawn(async move {
                other
                    .check_with_timeout(&environment, input(), stop, Duration::from_millis(40))
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), entered)
                .await
                .unwrap()
                .unwrap();
            if cancel {
                sender.send(true).unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(80), &mut running)
                    .await
                    .is_err(),
                "request lost its still-running blocking verifier"
            );
            assert_eq!(checker.available.available_permits(), 0);
            assert!(
                processes.snapshot().is_empty(),
                "verification must finish before any tool probe starts"
            );
            drop(pause);
            let error = tokio::time::timeout(Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(
                error.code,
                if cancel {
                    "GIT_REMOTE_CHECK_CANCELLED"
                } else {
                    "GIT_REMOTE_CHECK_TIMEOUT"
                }
            );
            assert_eq!(checker.available.available_permits(), 1);
            assert!(processes.snapshot().is_empty());
        }
    }

    #[tokio::test]
    async fn timeout_budget_also_covers_environment_resolution() {
        let sample = sample();
        let marker = sample.root.join("unexpected-after-version");
        fs::write(&sample.git, format!("#!/bin/sh\n/bin/sleep 0.4\nprintf unexpected > {}\nprintf 'git version 2.56.0\\n'\n", quote(&marker))).unwrap();
        fs::set_permissions(&sample.git, fs::Permissions::from_mode(0o755)).unwrap();
        let (_sender, stop) = watch::channel(false);
        let checker = GitRemoteChecker::new();
        let error = checker
            .check_with_timeout(
                &sample.environment,
                input(),
                stop,
                Duration::from_millis(40),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "GIT_REMOTE_CHECK_TIMEOUT");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn timeout_and_dropped_future_stop_the_complete_remote_process_group() {
        for abort in [false, true] {
            let sample = sample();
            let started = sample.root.join("started");
            let child_record = sample.root.join("child-pid");
            let cwd_record = sample.root.join("cwd");
            install(
                &sample,
                "2.56.0",
                &format!(
                    "pwd > {}\n/bin/sleep 60 &\nprintf '%s' \"$!\" > {}\nprintf started > {}\nwait",
                    quote(&cwd_record),
                    quote(&child_record),
                    quote(&started)
                ),
            );
            let checker = GitRemoteChecker::new();
            let other = checker.clone();
            let environment = sample.environment.clone();
            let (_sender, stop) = watch::channel(false);
            let running = tokio::spawn(async move {
                other
                    .check_with_timeout(&environment, input(), stop, Duration::from_secs(2))
                    .await
            });
            await_file(&started).await;
            let pid: i32 = fs::read_to_string(child_record).unwrap().parse().unwrap();
            if abort {
                running.abort();
                assert!(running.await.unwrap_err().is_cancelled());
            } else {
                assert_eq!(
                    running.await.unwrap().unwrap_err().code,
                    "GIT_REMOTE_CHECK_TIMEOUT"
                );
            }
            // Dropping the normal waiter only requests stop. The owned registry
            // must actually finish before its directory/admission can be released.
            assert!(checker.processes.shutdown().await.is_empty());
            let cwd = fs::read_to_string(cwd_record).unwrap();
            assert!(!Path::new(cwd.trim()).exists());
            tokio::time::timeout(Duration::from_secs(2), async {
                while unsafe { libc::kill(pid, 0) } == 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("remote process descendant survived cleanup");
            assert_eq!(checker.available.available_permits(), 1);
        }
    }

    #[tokio::test]
    async fn success_also_stops_children_that_closed_output_pipes() {
        let sample = sample();
        let marker = sample.root.join("unexpected-child-survived");
        install(&sample, "2.56.0", &format!(
            "(/bin/sleep 0.3; printf survived > {}) </dev/null >/dev/null 2>&1 &\nprintf '{}\\tHEAD\\n'",
            quote(&marker), "a".repeat(40)
        ));
        let (_sender, stop) = watch::channel(false);
        assert_eq!(
            GitRemoteChecker::new()
                .check(&sample.environment, input(), stop)
                .await
                .unwrap()
                .state,
            "readable"
        );
        tokio::time::sleep(Duration::from_millis(450)).await;
        assert!(!marker.exists());
    }
    // This child deliberately leaves the leader's process group but keeps the
    // inherited OS output pipe open. The registry cannot claim that reader is
    // joined merely because the leader/group has exited. It self-expires too.
    fn hold_output(marker: &Path, release: &Path) -> String {
        format!(
            r#"/usr/bin/perl -MPOSIX -e 'POSIX::setsid() >= 0 or die; open(my $f, ">", $ARGV[0]) or die; print $f $$; close($f); my $end=time()+12; while (!-e $ARGV[1] && time()<$end) {{ select(undef,undef,undef,0.01); }}' {} {} &
wait"#,
            quote(marker),
            quote(release)
        )
    }
    struct ReleaseOutput(PathBuf);
    impl Drop for ReleaseOutput {
        fn drop(&mut self) {
            let _ = fs::write(&self.0, b"release inherited pipe");
        }
    }
    async fn assert_closed(processes: &OwnedProcessRegistry) {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let issues = processes.shutdown().await;
                if issues.is_empty()
                    && processes
                        .snapshot()
                        .iter()
                        .all(|item| item.resource_closed())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual process readers did not close");
    }

    #[tokio::test]
    async fn pending_remote_cleanup_retains_real_directory_and_admission_until_reader_join() {
        let mut sample = sample();
        let processes = OwnedProcessRegistry::default();
        sample.environment =
            GitEnvironmentManager::with_processes(sample.root.clone(), None, processes.clone());
        let marker = sample.root.join("escaped-reader-pid");
        let release = sample.root.join("release-reader");
        let _release = ReleaseOutput(release.clone());
        let cwd_record = sample.root.join("cwd");
        install(
            &sample,
            "2.56.0",
            &format!(
                "pwd > {}\n{}",
                quote(&cwd_record),
                hold_output(&marker, &release)
            ),
        );
        let checker = GitRemoteChecker::with_processes(processes.clone());
        let (sender, stop) = watch::channel(false);
        let environment = sample.environment.clone();
        let other = checker.clone();
        let first_stop = stop.clone();
        let running =
            tokio::spawn(async move { other.check(&environment, input(), first_stop).await });
        // Interrupt only after the remote helper has really inherited its pipe;
        // a short wall deadline can expire during earlier version resolution.
        await_file(&marker).await;
        sender.send(true).unwrap();
        let error = running.await.unwrap().unwrap_err();
        assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
        let cwd = fs::read_to_string(cwd_record).unwrap();
        assert!(
            Path::new(cwd.trim()).is_dir(),
            "live reader lost its real cwd owner"
        );
        assert_eq!(checker.available.available_permits(), 0);
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop)
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_CHECK_BUSY"
        );
        assert!(processes
            .shutdown()
            .await
            .iter()
            .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
        assert!(processes.snapshot().iter().any(|item| !item.io_joined));
        fs::write(&release, b"actual output may close").unwrap();
        assert_closed(&processes).await;
        assert!(!Path::new(cwd.trim()).exists());
        assert_eq!(checker.available.available_permits(), 1);
    }

    #[tokio::test]
    async fn dropped_check_during_environment_probe_retains_admission_until_real_probe_drain() {
        let mut sample = sample();
        let processes = OwnedProcessRegistry::default();
        sample.environment =
            GitEnvironmentManager::with_processes(sample.root.clone(), None, processes.clone());
        let marker = sample.root.join("version-reader-pid");
        let release = sample.root.join("release-version-reader");
        let _release = ReleaseOutput(release.clone());
        let unexpected_remote = sample.root.join("unexpected-remote-process");
        fs::write(&sample.git, format!(
            "#!/bin/sh\nif test \"${{1-}}\" = '--version'; then\n{}\nexit 0\nfi\nprintf unexpected > {}\n",
            hold_output(&marker, &release), quote(&unexpected_remote)
        )).unwrap();
        fs::set_permissions(&sample.git, fs::Permissions::from_mode(0o755)).unwrap();
        let checker = GitRemoteChecker::with_processes(processes.clone());
        let environment = sample.environment.clone();
        let other = checker.clone();
        let (_sender, stop) = watch::channel(false);
        let first_stop = stop.clone();
        let request =
            tokio::spawn(async move { other.check(&environment, input(), first_stop).await });
        await_file(&marker).await;
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(checker.available.available_permits(), 0);
        assert_eq!(
            checker
                .check(&sample.environment, input(), stop)
                .await
                .unwrap_err()
                .code,
            "GIT_REMOTE_CHECK_BUSY"
        );
        assert!(!unexpected_remote.exists());
        let issues = processes.shutdown().await;
        assert!(issues
            .iter()
            .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
        assert_eq!(checker.available.available_permits(), 0);
        assert!(processes
            .snapshot()
            .iter()
            .all(|item| item.component == "git-environment"));
        fs::write(&release, b"real probe may finish").unwrap();
        assert_closed(&processes).await;
        assert_eq!(checker.available.available_permits(), 1);
        assert!(!unexpected_remote.exists());
    }
}
