# React / TypeScript / Vite 标准模板

模板版本 `react-ts-vite-0.1.0`；SDK `@local-projects/sdk 0.1.0`；接口 API v1。版本元数据在 `template.json`。平台仅在新建项目时复制 `app/`，保留版本固定的本地 SDK 包与依赖锁文件，不覆盖旧项目。

模板使用 React 19.3.1、TypeScript 5.9.3、Vite 8.3.4。Node 使用系统安装，支持 `^22.12.0 || ^24.0.0`，推荐 24 LTS；不包含 Node 二进制。第三方依赖首次构建仍需下载。`app/vendor/` 内 SDK tarball 不属于缓存。

初始页面只有项目连接状态和使用引导，不创建业务表、不插入样例数据、不在构建时初始化数据库。页面调用 SDK 的 context/schema/subscription 接口。AI 根据具体业务另行设计数据与页面；项目根 README 和 docs 由平台填入实际 ID、路径和工具入口。

组件在 `app/src/components/ui.tsx`，主题变量在 `app/src/theme.css`；默认黑白灰。基础 Button/Card/Badge 是本项目的小型 React 组件，不冒充已经安装 shadcn/ui。扩展时优先采用 shadcn/ui 并保持主题；不强制使用固定组件，也不引入 antd。

构建脚本为 `tsc --noEmit && vite build`，Vite `base: './'` 支持平台的项目路径和版本资源路径。平台从源码目录执行 `npm ci`，再执行 `npm run build -- --outDir <平台暂存目录>`，验证产物后发布。不要把直接 npm build 写出的 `app/dist/` 当作已发布项目，也不要手改项目根 `dist/`。构建不应读取或写入 `data/`。

SDK 在模块加载时固定当前文档代次。从平台打开项目后运行；静态预览或开发服务器缺少平台会话。源码更新构建后，旧页面收到 `project.updated` 提示，用户明确点击重新加载；不自动丢弃页面状态。

平台开发者维护步骤：先修改 SDK/模板，运行 SDK 测试，使用 `scripts/package-sdk.mjs` 校验或生成版本包并复制到模板，再在隔离临时目录 `npm ci` / `npm run build` 验证。SDK 内容若已分发则必须升版本，不能覆盖同名 tarball。资源复制必须排除 `node_modules`、`dist` 和工具缓存。
