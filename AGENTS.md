# Project collaboration

- Preserve existing development projects and their source, SDK, databases, attachments, maintenance state, backups, trash and recovery evidence. Use clearly named disposable projects for validation.
- Keep the fixed development application identity and data root. Workspace changes must use the public migration workflow.
- Follow the confirmed milestone and companion contracts in `docs/`; proposals remain proposals until the user confirms them.
- Continue Windows compatibility on `codex/windows-compatibility`, based on `main` at `81bd817`. Treat `main` as the integration baseline; keep existing shared history and do not rewrite it. Follow the branch, synchronization and integration rules in `docs/windows-support-plan.md` section 5.1.
- After each coherent implementation step is complete and its relevant checks pass, commit that step and push it to the configured remote. This is the user's standing request. Do not commit unfinished parallel work, secrets, generated applications, DMGs, or local test evidence.
- Record actual verification and limitations accurately. Mocked media tests do not prove physical camera/microphone, browser compatibility, or system permission behavior.
