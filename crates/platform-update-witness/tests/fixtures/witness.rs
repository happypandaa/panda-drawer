// Feature-gated artificial processes for integration tests only. Never bundled.
#[cfg(target_os = "macos")]
fn main() {
    use platform_update_witness::{
        ParentExitWitness, ProcessIdentity, SessionNonce, WaitOutcome, WitnessErrorKind,
    };
    use std::io::{BufRead, Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn say(value: &str) {
        println!("{value}");
        std::io::stdout().flush().unwrap();
    }
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(2).expect("test fixture mode");
    let nonce = "a".repeat(64); // Artificial format bytes; no key/randomness generation.
    if args[1] == "parent" {
        let parent = ProcessIdentity::current().unwrap();
        let mut helper = Command::new(std::env::current_exe().unwrap())
            .args([
                "helper",
                mode,
                &parent.pid().to_string(),
                &parent.start_seconds().to_string(),
                &parent.start_microseconds().to_string(),
                &parent.effective_uid().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        say(&format!("PARENT {} HELPER {}", parent.pid(), helper.id()));
        for command in std::io::stdin().lock().lines() {
            match command.unwrap().trim() {
                "eof" => {
                    if let Some(mut pipe) = helper.stdin.take() {
                        pipe.write_all(
                            b"Desktop ReadyForNativeExit: artificial text is not proof\n",
                        )
                        .unwrap();
                    }
                    say("PIPE_CLOSED");
                }
                "exit" => std::process::exit(0),
                _ => panic!("unknown artificial parent command"),
            }
        }
        // Test driver disappearance also ends this artificial parent, without
        // signaling numeric PIDs or relying on a kill-on-drop fallback.
        std::process::exit(0);
    }
    assert_eq!(args[1], "helper");
    let expected = ProcessIdentity::from_parts(
        args[3].parse().unwrap(),
        args[4].parse().unwrap(),
        args[5].parse().unwrap(),
        args[6].parse().unwrap(),
    )
    .unwrap();
    let identity = match mode.as_str() {
        "mismatch" => ProcessIdentity::from_parts(
            expected.pid(),
            expected.start_seconds() + 1,
            expected.start_microseconds(),
            expected.effective_uid(),
        )
        .unwrap(),
        "nonparent" => ProcessIdentity::from_parts(
            expected.pid() + 1,
            expected.start_seconds(),
            expected.start_microseconds(),
            expected.effective_uid(),
        )
        .unwrap(),
        _ => expected,
    };
    if mode == "late" {
        say("UNREGISTERED");
        let mut ignored = String::new();
        std::io::stdin().read_to_string(&mut ignored).unwrap();
    }
    let registration = ParentExitWitness::register(identity, SessionNonce::parse(&nonce).unwrap());
    if mode != "normal" {
        let error = registration.unwrap_err();
        if mode == "mismatch" {
            assert_eq!(error.kind, WitnessErrorKind::ProcessIdentityChanged);
        }
        if mode == "nonparent" {
            assert_eq!(error.kind, WitnessErrorKind::NotActualParent);
        }
        if mode == "late" {
            assert!(matches!(
                error.kind,
                WitnessErrorKind::NotActualParent
                    | WitnessErrorKind::ProcessUnavailable
                    | WitnessErrorKind::ProcessAlreadyExited
                    | WitnessErrorKind::ProcessIdentityChanged
            ));
        }
        say(&format!("REJECTED {:?}", error.kind));
        assert_eq!(
            ParentExitWitness::register(identity, SessionNonce::parse(&nonce).unwrap())
                .unwrap_err()
                .kind,
            WitnessErrorKind::NonceAlreadyUsed
        );
        say("FAILED_NONCE_REPLAY_REJECTED");
        return;
    }
    let mut witness = registration.unwrap();
    assert_eq!(witness.registration().unwrap().parent(), expected);
    assert_eq!(witness.registration().unwrap().nonce(), nonce);
    say(&format!("READY {} {}", nonce, expected.pid()));
    assert_eq!(
        witness.wait(Duration::from_secs(61)).unwrap_err().kind,
        WitnessErrorKind::InvalidWaitBudget
    );
    assert!(matches!(
        witness.wait(Duration::from_millis(100)).unwrap(),
        WaitOutcome::Pending
    ));
    say("ALIVE_PENDING");
    let mut ignored = String::new();
    std::io::stdin().read_to_string(&mut ignored).unwrap();
    assert!(ignored.contains("ReadyForNativeExit"));
    assert_eq!(unsafe { libc::getppid() }, expected.pid());
    assert!(matches!(
        witness.wait(Duration::from_millis(100)).unwrap(),
        WaitOutcome::Pending
    ));
    say("EOF_AND_READY_TEXT_PENDING");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let WaitOutcome::ParentExited(evidence) =
            witness.wait(Duration::from_millis(100)).unwrap()
        {
            assert_eq!(evidence.parent(), expected);
            assert_eq!(evidence.nonce(), nonce);
            say(&format!(
                "EXITED {} {}",
                evidence.nonce(),
                evidence.parent().pid()
            ));
            assert_eq!(
                witness.wait(Duration::ZERO).unwrap_err().kind,
                WitnessErrorKind::EvidenceAlreadyConsumed
            );
            assert_eq!(
                witness.registration().unwrap_err().kind,
                WitnessErrorKind::EvidenceAlreadyConsumed
            );
            assert_eq!(
                ParentExitWitness::register(expected, SessionNonce::parse(&nonce).unwrap())
                    .unwrap_err()
                    .kind,
                WitnessErrorKind::NonceAlreadyUsed
            );
            say("EXIT_EVIDENCE_AND_NONCE_REPLAY_REJECTED");
            return;
        }
    }
    panic!("owned artificial parent did not produce its registered kernel exit event");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This artificial process fixture supports macOS only.");
    std::process::exit(1);
}
