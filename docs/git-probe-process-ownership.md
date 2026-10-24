# Git probe process ownership

This step moves `git_remote` and `git_environment` commands onto the shared owned-process registry. It covers anonymous public HTTPS HEAD checks, explicit/managed Git version checks, Apple developer-directory lookup, and system SSH version checks. The production runtime injects one registry into both components; the existing constructors retain a private default for independent fixtures. This step does not grant installer, restart, or production-runtime permission.

## Ownership and cancellation

The registry owns the actual child, its dedicated ordinary process group, the original stdin/stdout/stderr pumps, and every session/drain until real completion. The Git adapters use one stdout session and a separate stderr drain. A failed reader cannot discard its sibling reader. Closing the adapter stdin facade closes and joins the real stdin pump. The adapters do not create hidden spawned work inside their I/O factories.

A remote check owns its real `OwnedSemaphorePermit` and `TempDir` in one `Arc<RemoteCheckScope>`. That same scope goes through executable resolution and every resulting version probe before it reaches the remote Git process. If the request is dropped during version resolution, the unfinished version probe retains the actual remote-check admission and directory. Ordinary successful sequential probes release their scope normally.

The work budgets remain 20 seconds for a remote request (including resolution) and 5 seconds per offline tool probe. Each remote output stream remains capped at 64 KiB; each tool-version stream remains capped at 16 KiB. Remote diagnostics remain private. Resolution separates `prepare_resolution` from `resolve_prepared_with_keep_alive`: the product request first awaits the actual blocking managed-resource verifier, then checks stop/deadline, and only then starts cancelable, registry-owned version probes. Preparation can exceed the soft request deadline; reporting timeout must not abandon its real filesystem worker. During a version probe, the outer remote-request budget can stop waiting; dropping that probe reply requests stop while the registry retains its real owner and shared scope. The remote command and each probe also have their own owned cancellation/result handling. `PROCESS_CLEANUP_PENDING` and unconfirmed-owner failures from an owned result remain explicit. A request returning any error does not prove resource closure. Only actual child/group exit, joined OS pumps/session/drains, and joined registry driver allow shutdown to treat the resource as closed.

The native/HTTP product task owns preparation through completion. A bare crate caller that forcibly drops the complete resolution/request future while managed verification is running still falls outside this process adapter's ownership guarantee; no process record exists yet. The split API prevents the product's ordinary cancellation/deadline path from doing that. This step does not add a general filesystem-worker registry.

The product integration must use `GitEnvironmentManager::with_processes(data_root, managed_root, registry.clone())` and `GitRemoteChecker::with_processes(registry.clone())` with the runtime's common registry, including rebuilt managers after workspace migration. A temporary registry per request would not satisfy the shutdown contract.

The runtime service and build manager now perform this injection. The native remote-check shutdown regression observes both environment probes and the remote command in that actual store registry, then requires every retained record to be resource-closed after service shutdown.

## Verification

Checks ran from a clean `2bdb127` export with frozen registry foundation `a943bc55d59ab597994dfbc75dfa8df498ade1cbbbfb9a7dd2bc4c96f14fe6ee`, independent Cargo target, and disposable synthetic tools/data only:

- Remote filter: 18 passed, with one official-Git opt-in checked separately. Includes management/service checks, group cleanup, caller drop, work timeout, bounds, and diagnostic privacy.
- Environment filter: 14 passed, with one official-managed-Git opt-in checked separately. The injected-instance test observes all configure/SSH/selected Git probes in one registry and rejects admission after sealing.
- Both official-Git opt-ins passed using a copied disposable official Git resource. The remote test only reads synthetic local configuration; neither opt-in connects to a remote repository.
- Core all-target strict Clippy passed. Workspace formatting was checked separately in the frozen-step ledger.

The new pending-cleanup tests deliberately start a bounded, self-expiring child that calls `setsid` and retains an inherited output pipe. A private release file and a drop guard end that artificial hold; the test never signals an unowned PID/PGID. This child is outside the registry's ordinary-group tracking scope. The test proves that a still-open OS pipe prevents a false I/O-complete receipt and preserves the actual directory/admission until EOF and join. It does not prove tracking or control of escaped descendants.

Early failed attempts are retained in the local ledger: an incorrectly quoted test-only Perl string prevented initial compilation; a 200 ms test deadline expired during version resolution rather than the intended remote phase; and the sealed-admission test initially expected a different error code instead of the foundation's `SERVICE_STOPPING`. The fixtures now synchronize with the semantic remote-start marker and assert the actual frozen API. No runtime completion checks were weakened.

Root integration on `a8e2787` passed 19 remote-filter tests (one opt-in excluded), 14 environment-filter tests (one opt-in excluded), both exact official-Git offline opt-ins, workspace all-target strict Clippy, formatting and diff checks. The added per-disposable-root pause is inside the actual managed verifier's blocking worker. Both stop and deadline leave the real remote admission held until that worker completes; no version or remote process starts afterward. The existing short version-probe deadline still stops its process before a later write. An intermediate direct-await implementation failed that latter regression and was replaced by the phase split; the initial split had a compile-time helper-name typo, corrected before verification. An overly broad opt-in filter also invoked 30 unrelated tests without their required fixture variables and failed; the two intended tests were then run by exact name. These failed logs remain local and are not counted as successful verification.

## Remaining work

`project_git::Context::run`, Git reference transactions, source acquisition, and the raw source/data materialization sessions still require their own owned-process adaptation. Their real project/source leases, candidate directories, phase-specific operation gates, and actual blocking file writers must remain owned through cleanup. This probe step does not establish safe platform exit or updater installation while those helpers remain outside the registry.
