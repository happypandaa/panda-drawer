---
title: Platform rules
description: App structure, AI handoff, editing boundaries, and command-line rules for Panda Drawer.
lang: en
copy_code: true
---

# Platform rules

Updated 2026-11-13. Sources include the [platform skill](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md), [handoff prompts](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/project-ai-requirements.ts), and [handoff generation code](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/resources.rs).

This guide explains how an AI tool can inspect, modify, and reload an app. Before making changes, read the installed platform instructions at the path provided in the handoff.

## What the app and the platform do

- Every app has a web interface. It can also have a Node 24 backend and instructions for an AI agent. The root `panda.json` uses manifest version 2 and declares these through `backend` and `agent.instructions`.
- The platform uses the registered app folder directly. It handles adding apps, starting and stopping them, building the web interface, storing configuration, logging, and deletion. The AI tool edits code, prepares the files declared in the manifest, and uses Git to save or revert code.
- The user chooses their AI tool and starts an agent app's regular tasks in that tool. The platform does not run agents, read conversations, or schedule tasks.
- The web interface uses static files or build output. A backend runs as one Node 24 service that follows the platform protocol. The platform uses npm and a v2 or v3 `package-lock.json` for managed dependency installation and builds. Server-side rendering, multiple backend services, Python, Java, and Docker are outside the current runtime contract.

## Taking over an app

1. Read the installed platform instructions, `panda.json`, the README, and the app documentation declared in the manifest. If the app has a backend, inspect it with `app logs`. Briefly tell the user what the app does, then ask what they want to change.
2. Explain small copy, color, or layout edits before making them. For data changes, schema changes, or larger edits, explain the plan and wait for the user's agreement.
3. Save the current code with Git and preserve existing uncommitted work. Make changes inside the app folder the user handed over. The platform manages registrations, private configuration, secrets, and logs. Leave real `.env` files and secrets alone. The user enters actual values in the platform form.
4. Explain the effect and obtain explicit consent before deleting or clearing `data/`, a database, or attachments. Arrange data backups separately from Git history.
5. Once the changes and runtime files are ready, confirm that the user has handled unsaved input. Run `app reload --app <id>` or ask the user to click **Reload** in the app window. Reload restarts any backend and opens the app if it was closed. Close the app window before a platform-managed build.
6. Describe the changes, data impact, checks completed, and anything still unverified. Update the app documentation. If the CLI cannot connect, keep the code changes and tell the user which step remains.

## Handoff prompts

| Handoff | What the AI tool does |
| --- | --- |
| Modify an app | Read the app folder, platform instructions, and CLI location. Understand the app before editing it. A backend handoff also includes the log location and inspection command. |
| Modify an agent app | Read its declared instructions too. If the user requests a routine task, follow those instructions. Before changing the instructions, explain the proposed change. Schema upgrades only add tables or columns. Update the database documentation and affected routine steps, and complete the upgrade by reloading the app. |
| Create an app | Read the generated starter project. Ask who the app is for and what it should do first, then propose a first version. |
| Inspect and add a folder | Use the platform check result to assess the folder without writing to it. Once the user agrees to the changes, save the current code, adapt the app, and add it. |
| Run a routine agent task | Use the app folder and its `AGENTS.md` or other declared instructions. The app can use the SDK to hand over task text, which the user reviews and sends. |

The platform ships its own `SKILL.md` and `references/`. If the installed path from the handoff is unreadable, tell the user first. The public version also documents the formats. [Public platform instructions](https://github.com/happypandaa/panda-drawer/tree/develop/skills/local-projects-app)

## Command line

Use the CLI path in the current handoff. Run `app list` to confirm the app ID. Replace `<cli>`, `<id>`, `<folder>`, and `TASK_ID` with the actual values.

```sh
<cli> app list --json
<cli> app info --app <id> --json
<cli> app check --dir <folder> --json
<cli> app reload --app <id> --json
<cli> app logs --app <id> --json
<cli> app build --app <id> --json
<cli> db schema --app <id> --json
<cli> db execute --app <id> --input request.json --json
<cli> task wait --task TASK_ID --json
<cli> env doctor --json
<cli> env git doctor --json
```

- `env doctor --json` reports Node and npm paths in `buildEnvironment`. `env git doctor --json` reports the Git path.
- Explain the impact before running `app delete`, `storage cleanup`, or `env node/git set/clear`. The platform asks for confirmation in its main window. If the user declines or two minutes elapse, the operation does not run.
- When the result of a write is unclear, inspect the actual state before deciding what to do next.

[Manifest format](/en/wiki/panda-json/) · [App runtime](/en/wiki/app-types/) · [Data and configuration](/en/wiki/data-and-config/) · [Adding apps](/en/wiki/adding-apps/) · [Work with your AI](/en/wiki/ai-workspace/)
