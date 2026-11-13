---
title: App structure and runtime
description: Web files, optional Node 24 backends, AI agent workflows, the SDK, and reloading.
lang: en
---

# App structure and runtime

Updated 2026-11-13. Sources include the [platform skill](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md), [app window and web runtime](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md), [backend protocol](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md), [agent apps](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/agent-apps.md), and [SDK](https://github.com/happypandaa/panda-drawer/blob/develop/packages/sdk/README.md).

Every app has a web interface. Apps that use private service keys can include a backend. Apps can also provide instructions for the user's AI tool to carry out routine tasks.

## Web interface and SDK

- The platform serves static web files from the registered app folder. `web.entry` is relative to `web.root`. The entry can be a standalone HTML file or output from a build using a tool such as Vite or React.
- Load `/_platform/sdk.js` before the app's module to access the SDK through `window.panda`. Any app can enable SQL, attachments, and AI handoff through `platform.sdkApiVersion: 1`. The platform handles `/_platform/*` even when the app has a backend.
- The web interface reads public values from `/_platform/config.json` or `panda.config()`, independently of the SDK data setting. The platform injects secrets into the backend only.
- `panda.context.get().projectRoot` returns the app folder's absolute path in the operating system's native format. The SDK must be enabled.
- A user click can call `panda.ai.open(text)` if `capabilities` includes `ai.open`. Text must be nonempty and at most 2,000 characters. The platform opens the selected AI tool in the current app folder and fills in the text for the user to send. If direct opening is unavailable, the platform copies the text and asks the user to paste it. The app can also offer a copy button.

## Building and reloading

- `development.build` prepares the web interface with or without a backend. The platform runs managed `npm ci` and the declared `npm run <script>` in `development.sourceDir`, which defaults to `.`. The script must produce the entry under `web.root`. The platform does not append an output directory argument.
- Close the app window before using **Prepare app** or `app build --app <id>`. For a CLI build, use `task wait` to wait for the result. Builds write to the app folder. If a build fails, check the logs, Git status, and actual output.
- When the runtime files are ready, confirm that the user has saved pending edits, then click **Reload** in the title bar or run `app reload --app <id>`. The platform reads the manifest and configuration again, restarts the backend if present, and opens the app if closed.
- Each app has one native window. Opening the app again brings the existing window to the front. If the manifest is invalid or a required setting is missing, reload preflight preserves the current instance. A failed startup switch reports an error. Fix it before reloading again.
- Declare both `window.width` and `window.height` as integers from 1 through 32768. `rememberSize` defaults to `true`. Without a window declaration, the default is 1080 × 780, resizing is remembered, and the minimum content area is 640 × 480.
- `alert`, `confirm`, and `prompt` use native dialogs. External links and `window.open` open in the system browser.

## Optional Node 24 backend

Apps that declare `backend` must ship ready-to-run backend files and dependencies alongside the web interface. The web build does not compile the backend. Starting the backend does not install dependencies. Opening the app starts the service, and closing the app stops it.

| Stage | Protocol |
| --- | --- |
| Startup | Exchange newline-delimited JSON over stdin and stdout. On `initialize`, check `protocolVersion: 1`, the `instanceId`, and the token. Once listening, send `ready` with the actual host and port. |
| HTTP | Requests include `Authorization: Bearer <token>` for the current instance. `GET /_platform/health` returns the same `instanceId`, `protocolVersion: 1`, and `status: "ok"`. |
| Heartbeat | Reply to `ping` with a `pong` containing the same `instanceId` and `requestId`. |
| Shutdown | On `shutdown`, send `stopping`, stop accepting requests, save data, close listeners and child processes, and exit. Follow the same cleanup steps when stdin closes. |

Reserve stdout for control messages and write normal logs to stderr. Backend startup and the web proxy share a 30-second budget. Each health check has up to 10 seconds, a ping has 3 seconds, and a graceful shutdown has 5 seconds.

| Environment variable | Meaning |
| --- | --- |
| `APP_HOST` | Backend listen address, `127.0.0.1` |
| `APP_PORT` | Backend listen port. `0` requests a system-assigned port. |
| `APP_DATA_DIR` | The app's `data/` directory, created before startup |
| `APP_CACHE_DIR` | Private platform cache directory |
| `APP_TEMP_DIR` | Private platform temporary directory |
| Each configuration field's `name` | The user-supplied value injected into the target backend |

## Web paths and network access

- An app runs at the root of a fixed `http://127.0.0.1:<port>` address and can refer to assets as `/assets/...`. A missing page path without an extension falls back to the home page. Asset files and platform endpoints do not use the fallback.
- The platform serves `/_platform/sdk.js`, `/_platform/config.json`, and `/_platform/api/v1/`. The web root does not expose database or dependency directories, hidden files, `panda.json`, links that leave the app boundary, or hard links that fail validation.
- Declare `origin`, `purpose`, and `dataSent` under `runtimeRequirements.network.connect` for external requests from the web interface. The service must allow cross-origin access. Camera and microphone use requires a purpose under `runtimeRequirements.devices` and operating-system permission.
- The web interface reaches the backend on same-origin paths listed in `backend.apiPrefixes`. Omission defaults to `/api` and `/tts`. Use `[]` to disable proxying. External web requests follow the web declaration. Backend network requests are independent of that policy.
- Validate application request sizes. Keep the backend port and token out of the web interface. The proxy supports streaming requests. Application redirects and WebSockets are outside the proxy contract.

## Agent apps

- `agent.instructions` points to instructions for routine tasks. The user's AI follows them to gather or process information. The app shows the results, stores data, and accepts feedback. Gleaner is one example.
- The app root is the AI tool's working directory. Routine tasks write only to tables and columns allowed by the instructions. The app creates and upgrades tables on startup. If a database or table is missing, ask the user to open the app first.
- Users can continue using the app while their AI tool is offline. The database retains their feedback for the next run. When they want new results, they start a task in their own AI conversation.
- Use **Change it your way** to modify the app itself. Explain proposed changes before editing its instructions. Schema upgrades only add tables or columns. Update affected instructions and documentation, then reload before running routine tasks.

[Platform rules](/en/wiki/platform-rules/) · [Manifest format](/en/wiki/panda-json/) · [Shared data rules](/en/wiki/data-and-config/) · [Adding apps](/en/wiki/adding-apps/)
