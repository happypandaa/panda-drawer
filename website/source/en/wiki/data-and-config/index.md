---
title: Data, configuration, logs, and deletion
description: App files, browser storage, backend secrets, shared databases, and what happens when an app is deleted.
lang: en
---

# Data, configuration, logs, and deletion

Updated 2026-11-13. Sources include the [platform skill](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md), [windows and data](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md), [backend logs](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md), [agent data rules](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/agent-apps.md), [workspace paths](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/paths.rs), and [cleanup interface](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/LocalDataCleanupPanel.tsx).

App files, browser data, and API keys live in different places. Confirm the data type and folder ownership before deleting an app or modifying its database.

## Where data is stored

| Data | Location and behavior |
| --- | --- |
| App code and runtime files | The registered app folder. The platform and AI use the same files. |
| Platform-created or downloaded apps | The current app workspace. A fresh development installation defaults to `~/PandaDrawer-Dev`, and the standard installation defaults to `~/PandaDrawer`. If the location is already occupied, the platform tries a numeric suffix. The app displays the actual path. |
| SDK database and attachments | `data/main.sqlite` and `data/attachments/`, created on first use. A missing database is created empty. A corrupt database produces an error. |
| Backend file data | `APP_DATA_DIR` points to the app's `data/`. Use `APP_CACHE_DIR` and `APP_TEMP_DIR` for cache and temporary files. |
| Browser data | The platform isolates window storage by app ID, including localStorage, IndexedDB, and cookies. |
| Public configuration and secrets | Platform-private storage. The web interface reads public configuration. Only the current app's backend receives secrets. |
| Backend logs | `app logs --app <id> --json` returns the log paths and recent entries. `current.log` and `previous.log` each retain approximately 256 KB. Secret configuration values are redacted. |

Adding or removing a backend and reloading preserves the app ID, address, folder, `data/`, public configuration, and browser data. Remove secret declarations when removing a backend. The platform deletes the old secrets after a successful switch and retains them if the switch fails.

Users enter configuration values in the platform form. Secrets live in platform-private files protected by the current user's file permissions. Keep secrets out of the web interface, logs, and code. Reload after changing runtime configuration.

## Reading and writing the database and attachments

- While a regular app is running, the AI tool writes to the database through the platform CLI's `db` commands and the web interface uses the SDK. Once the app is fully closed, SQLite tools can open the database directly. Read the actual schema before changing it and update the documentation afterward.
- The SDK, backend, and agent can share `data/main.sqlite`. Set a 5000 ms busy timeout for external connections and preserve WAL mode, foreign keys, and existing settings. Finish time-consuming reads, network calls, and downloads outside the transaction, then commit with a short `BEGIN IMMEDIATE` transaction. A long-held lock can cause `SQL_BUSY` when the interface saves.
- Leave a running database and its `-wal` and `-shm` files in place. After a failure, confirm rollback. If the outcome is unclear, query the data before deciding whether to retry. Commit SQL writes and attachment writes separately.
- Ordinary files written under `data/attachments/` can be accessed through `panda.files.list`, `get`, `url`, and `remove`. Attachment path and link validation still applies.
- External database and attachment writes do not emit `data.changed`. The app can query again when the window gains focus, the page becomes visible, or the user clicks refresh. Disk usage is a snapshot and requires a new scan after external writes.

## Data boundaries for routine agent tasks

- Write only to tables and columns allowed by the app instructions. Inspect current database state and resume repeated tasks from the work already completed. The app creates and upgrades databases and tables. If the database or table is missing, ask the user to open the app first.
- Use parameter binding or reliable value encoding for SQL inputs. Use `INSERT` or `INSERT OR IGNORE` for new items and preserve user-entered favorites, notes, and other fields. Avoid `INSERT OR REPLACE` and `REPLACE`, which can overwrite existing rows.
- Record the starting state, do time-consuming work separately, and commit in a short transaction. Confirm rollback before recording a failed write. Routine tasks leave `PRAGMA journal_mode`, `VACUUM`, and file-level cleanup to the app's maintenance process.
- Put schema upgrades in the app. Add tables or columns only. Update database documentation and affected agent steps. Reload to finish the upgrade before resuming routine tasks.

## Backups and deletion

Use Git for code history. The starter project's `.gitignore` excludes `data/` by default. Put persistent app data in `data/` or the SDK database. The app author and user arrange backups and exports.

| Folder ownership | What deletion does |
| --- | --- |
| User-selected local folder, including starter projects or agent apps downloaded there | Keeps the folder, code, Git history, and `data/`. Clears platform configuration, secrets, logs, caches, and browser data. |
| Platform-created, downloaded to the default location, or registered through a valid reservation | Deletes the managed folder and its contents, and clears platform configuration, secrets, logs, caches, and browser data. The platform preserves the folder if its identity changes or ownership is unclear. |

Adding the same user folder again cannot recover the original app instance's browser data. Completed exports, downloads, and attachment copies saved to an external folder are outside app deletion. Explain the impact and obtain explicit consent before deleting or clearing the database, files, or attachments.

**Clear all local project content** in Settings lists the scope and asks for two confirmations. It permanently removes managed apps, data, private configuration, and web storage. User-owned original folders and the Panda Drawer application remain. The interface shows the applicable scope and actual result.

[Platform rules](/en/wiki/platform-rules/) · [Manifest format](/en/wiki/panda-json/) · [App runtime](/en/wiki/app-types/) · [Adding apps](/en/wiki/adding-apps/)
