# Panda Drawer

[中文](README.md) · [GitHub](https://github.com/happypandaa/panda-drawer) · [Gitee](https://gitee.com/mycolorfuldays/panda-drawer)

Panda Drawer is an open-source desktop app for managing and launching small apps, each in its own window.

- **Add an app.** Pick one from the catalog, import a GitHub or Gitee repository, or add a local folder.
- **Build and customize apps.** Use your preferred AI tool to build from a template or edit an existing app. Reload the app to try your changes.
- **See your AI's work in an app.** For example, ask it to collect news on topics you follow, then read the briefings in Gleaner.

## Run from source

Panda Drawer is in development. To try it, run it from source.

- macOS 15 or later on Apple Silicon.
- Windows 10 22H2 or Windows 11 on x64 with WebView2 Evergreen.

You need Node.js 22.12 or later in the 22.x series, or 24.x, the [pinned pnpm version](https://github.com/happypandaa/panda-drawer/blob/develop/package.json), and Rust. macOS also needs the Xcode toolchain. Windows needs MSVC and the Windows SDK.

```sh
git clone --branch develop https://github.com/happypandaa/panda-drawer.git
cd panda-drawer
pnpm install --frozen-lockfile
pnpm run dev
```

Open **My Apps → Add App** and try typing practice with Clack, news with Gleaner, English listening and reading with Readio, or personal radio with Radio Bob. [Browse the app catalog](https://github.com/happypandaa/panda-drawer/blob/develop/catalog/apps.json)

App build scripts and Node.js backends run locally, so use sources you trust.

## Build an app for Panda Drawer

Start with an HTML page and a `panda.json` manifest. Use the web SDK when your app needs a local database or attachments. [App format](https://github.com/happypandaa/panda-drawer/blob/develop/docs/panda-project-format.md) · [Building apps with AI](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md) · [Web SDK](https://github.com/happypandaa/panda-drawer/blob/develop/packages/sdk/README.md)

Report bugs or discuss improvements in [GitHub Issues](https://github.com/happypandaa/panda-drawer/issues).

Panda Drawer uses the [Apache-2.0 license](LICENSE). Third-party code and assets retain their own licenses.
