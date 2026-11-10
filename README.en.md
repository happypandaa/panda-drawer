# Panda Drawer

[中文](README.md) · [English](README.en.md)

Panda Drawer is a local platform for small personal apps. Find and use a tool, or hand its project folder to an external AI tool to make it work the way you want. The platform manages app windows, runtime environments, configuration and local data APIs; authors and your AI tools maintain the application code.

The platform uses Tauri 2, React, Rust and SQLite. It currently provides an internal development build, version `0.0.0`. Public installers, signed releases and automatic runtime updates are still in preparation.

Source: [GitHub](https://github.com/happypandaa/panda-drawer) · [Gitee](https://gitee.com/mycolorfuldays/panda-drawer). Platform source, the recommended app catalog and test releases are maintained on both services.

## Current features

- **Add apps:** choose from the catalog, use a public GitHub or Gitee repository, add a local folder, or generate a starter project and ask your AI to build it.
- **Run apps:** each app has its own native window and fixed local address. When an app needs a backend, the platform manages its Node service, status, logs and shutdown.
- **Configure apps:** authors declare settings, their purposes and how to obtain them. Users fill out platform forms; secrets are passed only to the app's own backend.
- **Store data:** an optional web SDK provides SQLite, attachments and events. Apps can also use browser storage.
- **Customize further:** copy instructions containing the actual project path and platform rules to an external AI tool. macOS also supports detecting and opening installed AI tools.
- **Manage storage:** inspect usage, clean supported dependencies and caches, and delete apps according to folder ownership. Workspace relocation is available on macOS.

## Getting started

Open the platform and choose Add App in My Apps, or use the entry on the empty app list:

1. **Choose a recommended app** and complete its download and any required configuration.
2. **Paste a GitHub or Gitee URL** to download and inspect the source. Confirm before any required web build runs.
3. **Choose a local folder** to inspect and register the original folder. An empty folder can become a starter app.
4. **Ask your AI to make a new app:** name it, then give its project folder and your requirements to an AI tool that can edit local files.

To change an existing app, prepare AI handoff instructions from its details. macOS app windows also have a Change it your way action. Apps must follow the manifest and runtime rules below; external AI tools need access to the local project folder.

Current recommendations, with their sources in the [catalog](catalog/apps.json):

| App | Purpose | Requirements |
| --- | --- | --- |
| Gleaner (拾闻) | Topic-based news, bookmarks and feedback | An external AI tool that can read and write local files and search the web |
| Clack | Chinese and English typing practice and speed tracking | No API key required |
| Readio | English listening and reading, translation, word lookup, saved sentences and vocabulary | DeepSeek and Doubao speech API keys; both services charge by usage |
| Radio Bob | Personal AI radio | DeepSeek API key |

Download Route in Settings supports automatic detection or manual selection of Gitee for mainland China and GitHub for other regions. Recommended apps maintain their source and `panda.json` in their own repositories. See [downloads and distribution](distribution/README.md).

## App model and data

Each project has a root `panda.json`, currently using `manifestVersion: 2`. An app consists of a **required web page, an optional Node 24 backend, and optional agent instructions**. Plain HTML and built frontends both work; React, PWA features and the platform SDK are optional.

A minimal web app declares:

```json
{
  "manifestVersion": 2,
  "app": { "name": "My App" },
  "web": { "root": ".", "entry": "index.html" }
}
```

- **Web and backend:** apps run directly from their registered project folder. Backend runtime files and dependencies must be ready and follow the platform control protocol. Adding a local folder or opening an app does not implicitly install dependencies or build it. Web builds run the npm script declared in the manifest, in the original folder.
- **SDK:** `platform.sdkApiVersion: 1` enables SQL, attachments and other interfaces, with or without a backend. HTML loads `<script src="/_platform/sdk.js"></script>` to obtain `window.panda`; no SDK npm package is needed.
- **Reload:** after changing code, the manifest or configuration, use `app reload --app ACTUAL_APP_UUID`. Adding or removing a backend preserves the app ID, address, file data and browser storage. Old secrets are deleted only after the backend is successfully removed.
- **Agent:** `agent.instructions` declares the rules file. External agents can share the database with an app under the short-transaction rules. External tools manage AI startup, schedules and sessions.

File data lives in the project's `data/` folder. The SDK creates `data/main.sqlite` and `data/attachments/` on demand. localStorage, IndexedDB and cookies live separately in the app window's browser storage. The platform does not provide business-data backups or code-version restoration; users and their AI tools manage versions, backups and recovery. A failed in-place build can leave files changed by the build script.

Before deletion, the platform shows the scope and handles files according to ownership:

| Folder ownership | What happens to project files |
| --- | --- |
| A user-selected local folder, including a starter generated in an empty folder | The entire project folder, code, Git and `data/` are preserved |
| A folder created or downloaded by the platform | The entire project folder and its contents are permanently deleted |

For both kinds of app, platform registration, private configuration, secrets, logs, caches, tasks and browser storage are removed. Completed downloads, saved attachment copies and other deliveries in user folders are preserved. Before deleting an agent app, disable its schedules in the external AI tool.

Secrets are stored as plain files in the platform's private folder, protected by permissions for the current user, and injected only into the app that declares the backend. Node backends and build scripts run as local code: use trusted sources. Web permission policy is not an operating-system sandbox for backend code. Data sent to online services and their costs depend on the app and its configuration.

Authors and AI tools should read the [current project format](docs/panda-project-format.md), [platform skill](skills/local-projects-app/SKILL.md), [JSON Schema](schemas/panda.schema.json) and [web SDK](packages/sdk/README.md). These detailed references are currently in Chinese.

The platform supports Chinese and English. With no saved preference, a Chinese system interface selects Chinese; other system interface languages select English. A manual selection is retained. The platform and apps manage language independently: apps can follow the system or offer their own setting. Optional manifest `localization` translates app names, descriptions, configuration and other text displayed by the platform. It does not control the app's internal language or add an SDK or environment-variable binding.

## System support and limitations

| System | Current requirements |
| --- | --- |
| macOS | macOS 15 or later, Apple Silicon; workspace on an APFS volume on a local or directly attached disk |
| Windows | Windows 10 22H2 or Windows 11, x64, WebView2 Evergreen; workspace on a local internal NTFS disk |

Windows development builds are available, but workspace relocation, AI tool detection and one-click handoff, agent app location selection, and macOS app title-bar actions are not yet provided. App details support copying prompts and opening the project folder; reload is available through the CLI. Supported targets do not imply every device or scenario has been verified. See the [Windows x64 platform comparison](docs/windows-x64-port.md#4-平台适配层mac-与-windows-对照).

Opening an app again focuses its existing window. Closing its window stops its managed backend. On macOS, closing the manager leaves the platform in the Dock; Quit or Cmd+Q closes all apps and services. On Windows, closing the manager exits the platform.

## Local development and verification

Platform development requires Node.js `^22.12.0 || ^24.0.0` (24 LTS recommended), the pnpm version pinned in [package.json](package.json), and a Rust toolchain. macOS also needs the Xcode toolchain; Windows needs MSVC and the Windows SDK. App users do not need Rust or platform development tools.

Install dependencies and start from the repository root:

```sh
pnpm install --frozen-lockfile
pnpm run dev
```

The platform development page uses `127.0.0.1:1420`. Scripts prefer an existing private Rust toolchain in `.tools/cargo` and `.tools/rustup`, otherwise the system environment.

Use pnpm to install dependencies for the platform repository. Managed app dependency preparation and builds still use npm and npm v2/v3 `package-lock.json` files.

Choose checks appropriate to the change and run build and test suites in sequence. Local macOS Rust verification uses these resource limits; release settings remain unchanged:

```sh
export CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0

pnpm run check
pnpm run test:ui
pnpm run test:sdk
pnpm test
```

The full local CI entry is `node scripts/ci.mjs .`. macOS and Windows are verified on their respective machines. Automated tests do not replace real native-window, permission, device or installation acceptance. See [AGENTS.md](AGENTS.md) for collaboration and resource rules.

| Command | Purpose |
| --- | --- |
| `pnpm run build:app` | Build an internal macOS development `.app` |
| `pnpm run build` | Build the current platform's internal package: `.app`/DMG on macOS or an NSIS installer on Windows |
| `pnpm run cli app list --json` | List registered apps |
| `pnpm run cli env doctor --json` | Inspect the Node/npm environment selected by the platform |
| `pnpm run cli env git doctor --json` | Inspect the Git environment selected by the platform |

Quit the current instance normally before packaging or switching between source and packaged builds. macOS output is in `target/release/bundle/`. Internal builds use the development identity `dev.happypandaa.pandadrawer` and ad-hoc signing on macOS. See [development and packaging](docs/development.md#2-运行与构建).

## Repository and documentation

| Path | Purpose |
| --- | --- |
| `apps/desktop` | React manager and Tauri desktop host |
| `crates/platform-core` | Registration, local services, SQLite, attachments, configuration, builds, process lifecycle and workspace management |
| `crates/platform-native` | Platform adapters for files, processes, paths and system capabilities |
| `crates/platform-cli` | Native CLI connected to the local platform service |
| `packages/sdk` | Global web SDK, shared types and optional ESM aliases |
| `templates/react` | React/TypeScript/Vite starter source for new apps |
| `skills/local-projects-app` | Installed instructions for developing, modifying and adding apps |
| `schemas` | App manifest JSON Schema |
| `catalog` | Recommended apps |
| `distribution` | Download routes and distribution configuration |
| `docs` | Platform contracts, development instructions and verification records |

Useful references:

- [App format and data rules](docs/panda-project-format.md)
- [App development and AI handoff instructions](skills/local-projects-app/SKILL.md)
- [Web SDK API 1](packages/sdk/README.md)
- [Platform source development](docs/development.md#2-运行与构建)
- [Windows x64 platform comparison](docs/windows-x64-port.md#4-平台适配层mac-与-windows-对照)
- [Downloads and distribution](distribution/README.md)

Platform-owned source is licensed under [Apache-2.0](LICENSE). Third-party code and assets retain their accompanying licenses.
