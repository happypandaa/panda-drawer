# Web application

This project folder is the running application. `panda.json` v2 declares the web entry, optional backend, platform SDK and build script; it has no runtime field. Source is in `src/`; Vite writes `dist/`.

Use the platform Build action, or follow `development.build.script` in `panda.json`. Run app reload or click Reload to read the files from this same folder. A declared backend is restarted; identity and browser data are preserved. Failed builds are not rolled back; use Git to save and revert code. See `docs/development.md`.

File data lives in `data/`. Ask the user before deleting it. Browser data is stored separately by the platform and is cleared when the application is removed, even if this folder remains.

## Language

The platform and this app manage language independently. The app can follow `navigator.language` or offer its own language setting. The platform does not pass its language through the SDK or an environment variable.

To translate only the app name and description shown by the platform, add this optional member to the generated `panda.json`:

```json
"localization": {
  "en": {
    "app": {
      "name": "My App",
      "description": "A personal tool"
    }
  }
}
```

The platform accepts `zh-CN` and `en`, uses the current platform language, and falls back to each original field when a translation is absent. This metadata does not translate the app's own page or change its language setting. See the installed platform skill for configuration, network and device text translations.

## 平台 SDK 和配置

index.html 的普通脚本 /_platform/sdk.js 提供 window.panda，类型在 src/panda.d.ts；无需安装或内置 SDK 包。SQL／附件 API 为 /_platform/api/v1/，不要从 URL 解析实例 ID。公开配置用 window.panda.config() 或 fetch('/_platform/config.json') 读取，配置变化刷新或重新打开即可，不需要构建。
