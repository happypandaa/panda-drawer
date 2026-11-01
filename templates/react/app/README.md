# Web application

This project folder is the running application. `panda.json` declares the web entry, optional platform SDK and build script. Source is in `src/`; Vite writes `dist/`.

Close the application window before building. Use the platform Build action, or follow `development.build.script` in `panda.json`. Reopen the window to read the files from this same folder. Failed builds are not rolled back; use Git to save and revert code. See `docs/development.md`.

File data lives in `data/`. Ask the user before deleting it. Browser data is stored separately by the platform and is cleared when the application is removed, even if this folder remains.

## 平台 SDK 和配置

index.html 的普通脚本 /_platform/sdk.js 提供 window.panda，类型在 src/panda.d.ts；无需安装或内置 SDK 包。SQL／附件 API 为 /_platform/api/v1/，不要从 URL 解析实例 ID。公开配置用 window.panda.config() 或 fetch('/_platform/config.json') 读取，配置变化刷新或重新打开即可，不需要构建。
