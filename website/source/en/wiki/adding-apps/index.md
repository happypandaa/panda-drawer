---
title: Ways to add apps
description: Add an app from a link or local folder, start a new app, and understand agent folder ownership and download routes.
lang: en
---

# Ways to add apps

Updated 2026-11-13. Sources include the [platform skill](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md), [add app dialog](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/AddApplicationDialog.tsx), [link acquisition flow](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/SourceAcquisitionPanel.tsx), [recommended catalog](https://github.com/happypandaa/panda-drawer/blob/develop/catalog/apps.json), and [download route configuration](https://github.com/happypandaa/panda-drawer/blob/develop/distribution/download-sources.json).

Add an app from a link, choose an existing folder, or generate a starter project. An AI tool can first check whether an existing app meets the current requirements.

## Add from an app link

In **Add application**, enter the link in **App link**. Panda Drawer supports public HTTPS repositories on GitHub and Gitee. The repository root must contain a version 2 `panda.json`.

- If the web entry already exists, the platform uses those runtime files. When output is missing and `development.build` is declared, the platform asks before preparing the web files. If both the web entry and the build declaration are missing, fix the app first.
- Apps with `backend` must have their backend runtime files committed. Backends with dependencies also need `package.json` and a matching npm v2 or v3 `package-lock.json`. When adding by link, the platform installs production dependencies with `npm ci --omit=dev --ignore-scripts`. Web preparation still runs the script the user approved.
- If the result reports missing required settings, enter them in the platform form before opening. If the result is unclear, inspect the original task first.

## Local folders and new apps

- Under **Add application**, choose **Local folder** and select the entire app folder. The platform registers that original folder once the manifest and runtime files pass its checks. The user owns the folder.
- **New** generates a Vite web starter with the SDK enabled. This also works in an empty folder the user selects. Starter folders created in the managed workspace belong to the platform. User-selected folders belong to the user.
- When the platform is running, `app create --name "App name" --description "One-line description" --json` creates the same starter in the workspace and returns the app ID and folder. Clarify what the user wants before creating it.
- Local folders need ready-to-run backend files and dependencies before adding. If the web output is missing but the app declares a build, use **Prepare app** after adding it.

## Read-only inspection and repair

- State which folder or repository revision you could read. Read source files, the README, the manifest, and build configuration only. Leave real `.env` files, secrets, business databases, and private attachments outside the inspection.
- Run `app check --dir <folder> --json` to inspect a local folder. Identify the actual app root. Explain whether it can be added as-is, needs changes, is currently unsuitable, or needs more information. State the evidence and the size of any change. Review migration of old accounts and business data separately.
- During read-only assessment, leave dependencies, builds, app execution, and registration alone. If changes are needed, explain the plan and wait for the user to agree. After adaptation, use `app add --dir <folder> --json` to add the user-owned folder.
- If adding by link fails, use **Hand to AI** to repair the app and leave it under platform management. Run `app reserve --json` to obtain an empty managed folder and its reservation ID. Use the platform's Git to download into that folder, adapt the app, then run `app add --reservation <id> --json`. The platform verifies folder identity. After 24 hours, expiration only cleans up an empty folder with a matching identity. It retains and reports nonempty folders.

## Agent app folder ownership

When adding an app that declares an agent from recommendations or a link on macOS, use the current workspace or choose or create an empty folder. The default workspace location is platform-managed. The selected folder belongs to the user. The download goes straight into the selected folder. Files such as `.git` and `.env` count as existing content, while the empty-folder check ignores allowed system metadata. Windows currently uses the default location for this flow.

The app stays in its original folder after being added. Deletion follows the ownership recorded at registration. [Data and deletion rules](/en/wiki/data-and-config/)

## Recommended apps and download routes

- The recommendations include Gleaner, Clack, Readio, and Radio Bob. `catalog/apps.json` records the app URL, branch, required configuration, and summary. Gleaner's catalog `agent` flag is for display. Adding the app still checks its own `panda.json`.
- The catalog's `i18n` supports `zh-CN` and `en` for the card's `name`, `summary`, `needs`, and `note`. This is independent of the installed manifest's `localization` and the app interface language.
- In Settings, **Download source** defaults to **Automatic (recommended)**. Select **China (Gitee)** or **Global (GitHub)** to use a particular source. The selection applies to the catalog and recommended apps. Automatic mode can try the other source after a failed download. A manually entered link uses the address the user supplied.

[Platform rules](/en/wiki/platform-rules/) · [Manifest format](/en/wiki/panda-json/) · [App runtime](/en/wiki/app-types/) · [Data and configuration](/en/wiki/data-and-config/) · [Getting started](/en/wiki/getting-started/)
