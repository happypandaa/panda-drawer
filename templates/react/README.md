# 熊猫小抽屉 React / TypeScript / Vite 标准模板

模板版本 `react-ts-vite-0.2.0`；SDK `@local-projects/sdk 0.1.0`；接口 API v1。版本元数据在 `template.json`。资源里的 `app/` 会复制到新项目根目录，生成 `panda.json`、`index.html`、`src/` 和 npm 锁文件。创建时在该目录执行一次构建。平台不复制 skill、交接记录或实例身份到项目中。SDK tarball 暂作为模板源码依赖保留，批次 3 改用平台脚本后删除。

模板使用 React 19.3.1、TypeScript 5.9.3、Vite 8.3.4。构建使用平台的开发 Node/npm，支持 `^22.12.0 || ^24.0.0`，推荐 24 LTS；不包含 Node 二进制。第三方依赖首次构建仍需下载。`app/vendor/` 内 SDK tarball 不属于缓存。

初始页面提供项目连接状态、使用引导和可展开的媒体组件示例。打开页面和构建不创建业务表、不插入样例数据，也不请求设备。只有用户明确点“保存素材示例”，才先上传附件，再用事务创建 `media_samples` 演示表并关联记录。AI 根据业务需要替换演示功能，维护对应数据库文档；项目根 README 和 docs 由平台填入实际 ID、路径和工具入口。

浏览器助手独立位于 `app/src/lib/browser-tools.js`，版本 `0.1.0`，附类型声明；没有修改固定 SDK。`app/BROWSER_TOOLS.md` 说明设备用途声明、精确 HTTPS origin、离线与联网边界、采集的内部时长/字节限制、设备释放、文件与 SQL 两阶段保存、附件保留规则。默认运行声明为空，摄像头和麦克风按钮禁用；文件选择、拖入、下载和显式剪贴板操作可直接使用。实际运行时模式是 `normal`。模拟测试不能替代真机权限、录音/拍照、Safari/Chrome 和原生保存框验收。

无法满足采集要求的 `OverconstrainedError` 显示中文检查设备或选择已有文件提示，不把它等同于权限拒绝，不推断某一具体约束有误，也不自动降低约束或重试采集。

组件在 `app/src/components/ui.tsx`，主题变量在 `app/src/theme.css`；默认黑白灰。基础 Button/Card/Badge 是本项目的小型 React 组件，不冒充已安装 shadcn/ui。扩展时优先采用 shadcn/ui 并保持主题；不强制固定组件，也不引入 antd。

构建脚本为 `tsc --noEmit && vite build`。批次 2 仍保留 Vite `base: './'`，适配当前 `/p/<实例>/` 路径；批次 3 独立服务后移除。平台在 `panda.json` 的 `development.sourceDir` 中执行 `npm ci` 和 `npm run <development.build.script>`，不增加输出参数、不复制源码、不保留产物代次、失败不回滚。AI 自行构建后关窗重开，同样读取这个目录里的 `dist/`。

SDK 在模块加载时固定当前文档代次。从平台打开项目后运行；静态预览或开发服务器缺少平台会话。源码或运行声明更新后，旧页面提示需要明确重载；不自动丢弃页面状态。模板助手监听平台状态事件释放设备，但不能控制任意项目自行创建的媒体流。

平台开发者维护步骤：修改模板后运行 `node --test scripts/m5-browser-tools-test.mjs`，在隔离临时目录 `npm ci` / `npm run build`，再生成 starter。SDK 内容若已分发则必须升版本，不能覆盖同名 tarball。资源复制必须排除 `node_modules`、`dist` 和工具缓存。

原生打包使用 `src-tauri/Info.plist` 的相机/麦克风用途说明和 `Entitlements.plist` 的 camera/audio-input 权利；没有开启 App Sandbox。项目窗口仍需显式权限回调，让已声明设备走系统/浏览器提示，其他请求拒绝。配置方法来自 [Tauri macOS bundle 文档](https://v2.tauri.app/distribute/macos-application-bundle/)，权限依据 [Apple Camera entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.camera) 和 [Apple Audio Input entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input)。[WebKit Safari 18.4 发布说明](https://webkit.org/blog/16574/webkit-features-in-safari-18-4/) 记录了新增 WebM 录制和 ImageCapture 支持，因此支持较早 macOS 15 时仍须运行时探测格式，不能假定这些新 API 可用。
