# Project Git process ownership

This step moves ProjectGit metadata commands, reference transactions, and raw source/data object reads to the owned-process registry. It follows the Git probe step and the prepared-resource/probe split. It does not establish permission to install an update, restart the application, or run the production profile.

## Actual owners and phase changes

Each Git command owns the operation's actual `ProjectTaskLease`, current phase gate, private `TempDir`, and retained candidate/lock owners through an immutable per-command `KeepAlive` snapshot. ProjectGit gets the runtime registry through `ProjectStore::processes()`. Existing independent fixture constructors remain available. Caller cancellation or a disappearing reply only requests stop; it cannot drop a process record's actual owners.

The read gate is an owned, shared guard. Data snapshot and apply flows explicitly remove that read phase before acquiring their exclusive snapshot lease, and attach the new phase to later Git commands. The snapshot lease API itself is unchanged. A completed Git command releases its per-command scope before an exclusive upgrade; a still-pending command prevents the upgrade and retains its real operation lease and gate.

`BuildGitGuard::capture_with_keep_alive` and `recheck_with_keep_alive` accept a registry and a fresh actual phase owner for each call. The returned guard stores the checked repository state and tool selection, not the gate. Product build integration must pass its real build protection/candidate scope to both phases. `Context::bind_keep_alive` is also the narrow hook for the separate Node Git adapter work. This step does not wire or claim completion of that Node adapter or source acquisition.

Executable preparation awaits actual managed-resource verification before observing cancellation. The prepared probe phase alone may be selected against cancellation/deadline because its processes, I/O, and owner are already registered. A raw crate caller force-dropping a preparation future is not proof of a registered verification worker; product callers retain their owned request until preparation completes.

## Streams and file writers

The registry owns the original OS pipes and their pumps. Metadata commands use separate stdout, stdin, and stderr sessions/drains, so a failed reader cannot drop its sibling. Metadata stdout remains capped at 8 MiB; stderr is drained with a 64 KiB capture limit.

Raw `cat-file --batch` payloads bypass that metadata collector. The source/data reader uses an owned blocking protocol and filesystem writer whose actual blocking `JoinHandle` is awaited by its registered session. Ordinary `File::write_all` and `sync_all` finish or unwind before that join completes; no cancellable `tokio::fs` future hides a continuing write. Chunk checks observe stop and the operation deadline, while a genuinely blocked filesystem operation retains its owner until it actually completes.

Object requests and responses are interleaved. This avoids deadlocking a request list larger than the registry's 64 KiB facade while the child is blocked writing a large first response. Exact OID/type/length framing and trailing delimiter/EOF checks remain in place. Target candidates remain owned through subsequent hash verification. Raw materialization writes only candidates; it does not run filters, check out source, replace the live database, or publish Git refs itself.

A joined blocking-writer panic is reported as a worker failure after its thread has actually joined and its files have unwound. That is distinct from a missing/panicking registry session or driver whose resource ownership cannot be proven: the latter remains unconfirmed under the registry contract. This step does not claim that a failed candidate is a successful restore or that a joined worker failure grants installer permission.

## Publication and resource closure

The reference transaction has independent publication and closure receipts. The session marks `commitSent` before attempting the first commit byte, and marks `commitAcknowledged` only after the exact Git acknowledgement. A known acknowledgement remains `operationOutcome: committed` even if a held pipe makes process cleanup pending. A possibly sent commit without acknowledgement is `unknown`. `safeAbort` requires both an unsent commit and proven process/I/O closure.

`processResourceClosed` is the build-facing closure field; `processClosed` is retained with the same value. A pending or unconfirmed process, unknown process receipt, or cancellation while a prepared probe reply is being dropped reports both as false. Error translation through `BuildGitGuard` preserves those fields. False also carries `operationOutcome: unknown` unless a separate commit acknowledgement already proves `committed` (or admission proves `not_started`). False means the caller must retain its candidate/protection and avoid rollback while real I/O can still be using it. An error result alone never proves resource closure. Registry shutdown still independently requires the driver join as well as child/group and I/O completion.

An acquired index lock retains a cloned real FD in the command scope. The stack lock records whether cleanup is allowed, but only the last verified actual owner can remove its own identity-matching path. Branch operations similarly retain their old-ref lock. Commit-sent/unknown or acknowledged-but-unfinished paths preserve locks and the existing journal. This does not authorize deletion of foreign lock files.

## Verification

All checks use a named disposable source export, its own Cargo target, synthetic temporary projects, and a copied official Git resource. No actual development project, user database, remote repository, or installed application is used. The export's root HEAD, dirty foundation file hashes, frozen probe patch, and prepared-resolution file hash are recorded in the local verification ledger.

The pending-I/O fixtures deliberately start a bounded child in a different session to hold an inherited pipe. Private release files/drop guards end that artificial hold; the tests do not signal an unowned numeric PID or group. These checks prove retained pipe/writer ownership, not tracking or control of escaped descendants.

Verification includes real official-Git initialization/source save, branch creation, a source HTTP restore round trip, data apply with SQLite/attachments/empty directories/epoch change, and two data saves with temporary-copy cleanup. Raw previews cover a database/attachment above the 8 MiB metadata cap and a 9 MiB source blob plus 1,700 other objects (request list above 64 KiB). The pending/cancel checks assert actual operation leases, phase gates, candidates, and index FDs stay held until joined I/O; commit cases separately cover unsent abort, sent without ACK, and acknowledged commit. Final default-test counts, strict Clippy/formatting results, and retained initial failures are recorded in the frozen step manifest. This step does not rerun the entire data-restore fault matrix: its immutable journal/slot transaction engine is unchanged. Integration must still verify common-registry injection, build rollback behavior, and the separate source-acquisition and Node adapters before claiming all helpers are drained.

Root integration on `9e43626` passed 67 ProjectGit-filter tests (40 opt-ins excluded), workspace all-target strict Clippy, formatting and diff checks. Four exact official-Git checks also passed on the disposable verified tool resource: large source/1,700 requests, large SQLite/attachment preview, full data apply, and two data saves with temporary-copy cleanup. Their wall times were 10.20, 23.37, 21.97 and 40.89 seconds respectively, including command overhead. These root checks do not replace the independent export's seven distinct official/HTTP opt-ins or the unchanged transaction fault-matrix evidence. Product webpage build scope wiring and Node Git wiring remain separate increments.
