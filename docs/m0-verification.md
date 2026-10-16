# M0 实施与验收记录

日期：2026-10-15  
结论：M0 本机验收完成；M1 尚未开始。  
代码与内部包版本：`0.0.0`，开发身份 `dev.localprojects.platform.m0`。当前目录没有 Git 仓库，因此不提供虚构的提交号。

## 1 已交付范围

- npm workspace 与 Rust workspace，明确区分桌面前端、Tauri 宿主、Rust 核心、原生 CLI、SDK 和模板资源。
- React/TypeScript/Vite 黑白灰管理诊断页面，显示真实工作区、开发数据根、实例、端口、SQLite 版本、CLI 路径和包内资源结果。
- Tauri 2 + Axum/Tokio 回环服务、静态链接 SQLite；M0 用内存连接验证数据库引擎，不创建业务表。
- 固定开发目录、环境标记、私有权限、实例锁、端口回退、发现与鉴权；CLI 不创建目录、不启动服务、不回退查找其他身份。
- 原生 CLI 的 `service status --json`、`env doctor --json`、帮助与版本。
- 原生 arm64 `.app` 和 DMG，包内携带 CLI、SDK/模板骨架及开发说明。

SDK 和模板仍是明确标注的资源骨架，不是已实现的 SDK 或可创建项目的模板。M0 没有项目 CRUD、业务 SQL/附件接口、浏览器项目会话、项目构建、备份恢复、迁移或清除用户数据功能。

## 2 验证环境

| 项目 | 本次实际版本 |
| --- | --- |
| 系统 | macOS 27.0.1，build 26A434，Apple Silicon / arm64 |
| Xcode | 27.0，build 27A266a |
| Node / npm | 24.14.1 / 11.11.0，系统已有环境 |
| Rust | rustc 1.99.0，工程 `.tools/` 下私有工具链 |
| Tauri | CLI 2.12.3、Rust / JS API 2.12.4 |
| 前端 | React 19.3.1、Vite 8.3.4；具体版本以锁文件为准 |
| SQLite | 3.51.1，由 rusqlite bundled 引擎实测返回 |

没有更改全局 Rust、shell 配置、登录启动项或安装系统守护进程。安装包最低系统与两个 Mach-O 可执行文件的最低系统均为 macOS 15.0；**尚未在 macOS 15 实机测试**。

## 3 隔离边界

实际开发数据根为：

```text
~/Library/Application Support/dev.localprojects.platform.m0/
├── .environment.json
├── settings.json
├── instance.lock
├── runtime/
│   ├── connection.json   # 仅运行时存在
│   └── control-token     # 仅运行时存在，0600
└── workspace/
    ├── projects/
    ├── backups/
    └── .platform/
```

源码版和内部包使用同一开发身份，不能同时占用该工作区。当前没有生产构建，也没有任意数据根覆盖或自选工作区入口。遇到不同身份标记、不明非空目录、符号链接或 settings 工作区重定向时拒绝打开。正式品牌和正式目录留待发行阶段确定，不自动从开发目录迁移。

单元测试在临时目录创建自己的夹具。实际开发工作区的 `projects/` 保持为空；测试没有写入任何正式项目数据。详细规则及 WebKit 缓存、工程工具链的位置见[开发说明](development.md)。

## 4 自动校验

以下检查实际通过：

| 检查 | 结果 |
| --- | --- |
| `npm ci` | 锁文件安装成功，npm 审计无已报告漏洞 |
| `npm run check` | TypeScript、Vite 生产构建及 Rust 全 workspace / all-targets 检查成功 |
| `npm test`；修复后再次执行 `cargo test --locked --workspace` | 11 项测试通过，0 失败 |
| `node scripts/rust.mjs clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `node scripts/rust.mjs fmt --all --check` | 通过 |

8 项核心测试覆盖：拒绝收养未知非空目录、拒绝工作区重定向、拒绝符号链接、锁互斥与释放、0600 权限和跨实例清理保护、严格回环 URL、实际 HTTP 鉴权/Host/Origin 检查及退出释放、端口占用回退。

3 项 CLI 测试覆盖：其他实例/环境/工作区拒绝、身份字段缺失或类型错误拒绝、HTTP 重定向不跟随。另实际执行离线诊断，确认不会创建开发目录；在线时读取真实运行信息；退出后返回 `SERVICE_NOT_RUNNING` 和非零退出码。

## 5 运行与打包验收

| 场景 | 实际结果 |
| --- | --- |
| `npm run dev` | Tauri 源码进程启动，CLI 可连接到 Rust 服务，Vite 使用 `127.0.0.1:1420` |
| 前端修改 | 修改 SQLite 标签后 Vite 输出 HMR 更新，已打开的浏览器开发预览无需刷新即可显示新标签 |
| Rust 修改 | Tauri 重新编译并启动，新实例 ID 生效，开发工作区不变 |
| 普通浏览器打开管理开发页 | 明确提示缺少 Tauri 宿主，状态为未读取，不模拟数据库或服务连接 |
| 开发进程异常终止 | CLI 不因旧 discovery 文件仍在就误报在线；下次启动使用新实例和凭据 |
| 从打包 `.app` 启动 | 原生窗口加载 `tauri://localhost`，显示真实服务、SQLite、包内 CLI 和全部资源状态；刷新按钮可用 |
| 无 Node 的受控 PATH | 完整退出旧实例后，以系统 `open --env PATH=/usr/bin:/bin -a <绝对 .app 路径>` 启动，新实例的真实管理页面、Rust/SQLite 和包内资源全部正常 |
| 关闭窗口 | 使用 Cmd+W 后 CLI 仍连接同一实例；关闭窗口不等于退出服务 |
| 重复启动 | 第二次进程退出，原实例 ID 与 PID 不变，复用已有管理窗口 |
| 明确退出 | 修复后 Cmd+Q 关闭服务、清除 discovery/token，保留 workspace；在 `.app` 和 DMG 实例分别复测 |
| DMG | 完整生成、校验并只读挂载；从挂载位置启动应用，显示包内实际资源路径；包内 CLI 查询成功；退出后已卸载测试卷 |
| 包完整性 | `codesign --verify --deep --strict` 通过；桌面和 CLI 均为 arm64、最低 macOS 15.0 |

本机原生窗口和普通浏览器预览均已查看实际界面。浏览器预览的成功不代表已有业务项目浏览器接口，后者仍属于 M1。

验收中修复了 macOS 默认 Quit 跳过 `ExitRequested` 的实际问题：应用菜单使用显式退出，`ExitRequested` 和 `Exit` 都执行同一同步收尾；等待前释放状态锁，HTTP 收尾上限 3 秒。当前只有只读状态请求，后续引入写入和任务时必须扩展退出协调。Dock/系统 AppleEvent 独立退出入口已有收尾兜底，但本次未单独完成该入口的 UI 自动验收。

**启动方式的已知限制：**直接从工具进程执行 `.app/Contents/MacOS/` 内的桌面二进制时，本机出现过白屏，尽管 Rust 服务在线且系统日志显示页面加载完成。绝对路径也曾复现；系统日志显示这类进程未由 LaunchServices 启动，WebKit 记录窗口被遮挡并冻结渲染，尚未证明最终根因。通过系统打开完整 `.app`、DMG 启动及上述受限 PATH 的标准应用启动均正常。本次不为该差异添加猜测性的渲染修改，也不把服务在线当作 GUI 通过。日常打开完整 `.app`，不直接运行包内桌面二进制；后续若标准启动也复现，应按启动缺陷优先处理。

## 6 产物与复现

从仓库根目录依次执行，不并行运行这些准备/构建命令：

```sh
npm ci
npm run check
npm test
npm run dev
# 明确退出开发版后再打包
npm run build
```

本次产物位于：

- `target/release/bundle/macos/Local Projects Dev.app`，约 11.00 MiB。
- `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg`，5,136,216 字节，约 4.90 MiB。

本次 DMG SHA-256：

```text
09a295d789d7d064ff835936a351bd2c597a3194babb83a7af2f2ee0183e59c1
```

DMG 重新生成后容器元数据可能改变，因此哈希仅标识本次验收产物。前端产物嵌入桌面二进制，CLI 位于 `Contents/MacOS/`，资源骨架位于 `Contents/Resources/resources/`；Node、Rust 工具链与 node_modules 不进入 `.app`。

包采用 **ad-hoc 内部测试签名**。本机检测到的 Developer ID Application 身份数量为 0；未公证，未验证互联网下载首启、正式覆盖升级或 macOS 15 真机。以上属于后续发行条件，不将该包视为对外发行版本。
