# 默认网页项目模板

版本 `react-ts-vite-0.3.0`，SDK API 1。源码资源位于 `app/`，新建项目时复制到项目根，生成 `panda.json` v2（不写 runtime，backend 可选） 并在原根执行声明的 npm 构建。平台只打包作者源码，不预构建 starter、复制技能、写实例身份或控制文件。

模板使用 React 19.3.1、TypeScript 5.9.3、Vite 8.3.4，脚本 `tsc --noEmit && vite build`，支持构建 Node `^22.12.0 || ^24.0.0`。Vite 使用默认根地址，不再设置 base。`index.html` 先加载普通 `/_platform/sdk.js`，页面使用 `window.panda`；`src/panda.d.ts` 与平台的 canonical SDK 类型相同。没有 SDK npm 依赖、vendor 或 tgz。

每个实例使用自己的固定端口；SDK 的 SQL／附件／事件访问 `/_platform/api/v1/`，不解析 URL 中的 ID。默认 `platform.sdkApiVersion:1`，文件数据按需创建于 `data/`。公开配置使用 `window.panda.config()` 或 `fetch('/_platform/config.json')`，与是否启用 SQL SDK 无关；配置变化无需构建。

构建执行原目录 npm ci 后运行 `development.build.script`，不追加 outDir，不保存旧代或回滚脚本写入。修改完成后使用 app reload 或标题栏「重新加载」；带后台时也会重启后台。`.gitignore` 默认忽略依赖、dist 和 data；需要把文件数据加入 Git 时由用户明确删除 data 行。

浏览器助手独立位于 `app/src/lib/browser-tools.js`，附类型；`app/src/components/MediaToolsPanel.tsx` 演示显式采集和保存，`app/BROWSER_TOOLS.md` 说明权限、释放、上传与 SQL 两阶段边界。默认未声明相机／麦克风用途，按钮不启用；普通文件选择、拖入、下载和显式剪贴板可用。模拟测试不证明真实设备、权限或原生保存框。

基础黑白灰 UI 位于 `src/components/ui.tsx` 和 `src/theme.css`，是普通本地组件。扩展优先保持现有主题，不强制特定 UI 库。平台更新不会改写已存在的作者源码。
