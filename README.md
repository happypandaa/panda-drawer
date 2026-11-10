# 熊猫小抽屉

熊猫小抽屉是一个本地小应用平台，帮助你获取和使用个人工具，也可以把应用交给外部 AI，做成自己想要的样子。平台负责应用窗口、运行环境、配置和本地数据接口，应用代码由作者或你的 AI 工具维护。

平台使用 Tauri 2、React、Rust 和 SQLite。当前提供内部开发测试版，版本为 `0.0.0`；正式安装包、签名发行和运行时自动更新仍在准备中。

源码：[GitHub](https://github.com/happypandaa/panda-drawer) · [Gitee](https://gitee.com/mycolorfuldays/panda-drawer)。两端同步维护平台源码、推荐列表和测试 Release。

## 当前功能

- **添加应用**：从推荐列表、公开 GitHub／Gitee 仓库或本机文件夹添加，也可以生成起始项目，再交给 AI 制作。
- **运行应用**：每个应用使用独立原生窗口和固定本机地址；需要后台时，由平台管理 Node 服务的启动、状态、日志和停止。
- **填写配置**：应用作者声明配置项、用途和获取方法，用户在平台表单中填写；密钥只提供给本应用后台。
- **保存数据**：可选网页 SDK 提供 SQLite、附件和事件接口，应用也可以使用浏览器存储。
- **继续定制**：复制包含实际项目路径和平台规则的说明给外部 AI；macOS 还支持检测和打开已安装的 AI 工具。
- **管理空间**：查看空间占用、清理受支持的依赖与缓存、按目录归属删除应用；macOS 支持工作区迁移。

## 开始使用

启动平台后，在“我的应用”中选择“添加应用”，或使用空列表页面的入口：

1. **选一个推荐应用**，按提示完成下载和必要配置。
2. **粘贴 GitHub／Gitee 链接**，获取源码并检查是否符合平台要求；需要构建时先确认执行。
3. **选择本机文件夹**，检查并添加原目录；选择空文件夹时可生成起始应用。
4. **让 AI 新做一个**，为应用命名，再把项目目录和需求交给能修改本机文件的 AI 工具。

想修改已有应用，可以从详情准备 AI 交接说明；macOS 应用窗口还提供“按你的想法改”入口。应用需要符合下方的清单和运行约定，外部 AI 工具需能访问本机项目目录。

当前推荐应用如下，完整来源见 [推荐列表](catalog/apps.json)：

| 应用 | 用途 | 使用前提 |
| --- | --- | --- |
| 拾闻 | 按主题整理资讯、收藏和反馈 | 能读写本机文件、能联网搜索的外部 AI 工具 |
| Clack | 中英文打字练习和速度统计 | 无需 API Key |
| Readio | 英语听读、翻译查词、句子和生词收藏 | DeepSeek、豆包语音 API Key；两个服务均按量付费 |
| Radio Bob | 个人 AI 电台 | DeepSeek API Key |

设置中的“下载线路”支持自动检测和手动选择国内 Gitee／国外 GitHub。推荐应用各自在自己的仓库维护源码和 `panda.json`；下载及发布规则见 [下载与发布](distribution/README.md)。

## 应用模型与数据

项目根目录使用一份 `panda.json`，当前格式为 `manifestVersion: 2`。应用由**网页（必有）、Node 24 后台（可选）和 agent 规则（可选）**组成。普通 HTML 或构建后的前端均可使用，不强制 React、PWA 或平台 SDK。

最小网页应用只需声明：

```json
{
  "manifestVersion": 2,
  "app": { "name": "我的应用" },
  "web": { "root": ".", "entry": "index.html" }
}
```

- **网页与后台**：所有应用直接运行登记的项目文件夹。后台须准备好运行文件及依赖，并遵守平台控制协议；添加本机目录和启动应用不会隐式安装依赖或构建。网页构建按清单在原目录执行 npm 脚本。
- **SDK**：声明 `platform.sdkApiVersion: 1` 后可使用 SQL 和附件等接口，有无后台都一样。HTML 通过 `<script src="/_platform/sdk.js"></script>` 获得 `window.panda`，无需安装 SDK npm 包。
- **重新加载**：修改代码、清单或配置后使用 `app reload --app 实际应用UUID`。增删后台保留应用 ID、地址、文件数据和浏览器存储；成功移除后台后才删除旧密钥。
- **agent**：规则文件由 `agent.instructions` 声明。外部 agent 可按短事务规则与应用共用数据库；AI 的启动、定时任务和会话由外部工具负责。

文件数据放在项目的 `data/` 中。SDK 按需创建 `data/main.sqlite` 和 `data/attachments/`；localStorage、IndexedDB、Cookie 则保存在平台的应用窗口存储中，与项目文件分开。平台不提供业务数据备份或代码版本取回；代码版本、数据备份和回退由用户与 AI 自行管理。原目录构建失败可能留下脚本修改，不会自动恢复原文件。

删除应用前会展示范围，并按目录归属处理：

| 目录归属 | 删除后的文件处理 |
| --- | --- |
| 用户选择的本机目录，包括在空目录生成的起始项目 | 保留整个项目文件夹、代码、Git 和 `data/` |
| 平台新建或默认下载的目录 | 永久删除整个项目文件夹及其内容 |

两类应用的平台登记、私有配置、密钥、日志、缓存、任务和浏览器数据都会清除。保存到用户文件夹的最终下载、附件副本等交付文件保留。删除 agent 应用前，应先关闭外部 AI 工具中针对该应用的定时任务。

密钥存放在平台私有目录，以当前用户文件权限保护明文文件，仅注入声明了后台的本应用。Node 后台和构建脚本按本机代码运行，应使用可信来源；网页权限策略不构成后台代码的系统沙箱。联网服务的数据发送和费用取决于应用及其配置。

作者和 AI 接入应用时，阅读 [当前项目格式](docs/panda-project-format.md)、[平台说明 skill](skills/local-projects-app/SKILL.md)、[JSON Schema](schemas/panda.schema.json) 和 [网页 SDK](packages/sdk/README.md)。

## 系统支持与限制

| 系统 | 当前要求 |
| --- | --- |
| macOS | macOS 15 及以上，Apple Silicon；工作区位于本机或直连磁盘的 APFS 卷 |
| Windows | Windows 10 22H2／Windows 11，x64，WebView2 Evergreen；工作区限本机内置 NTFS 磁盘 |

Windows 已开放开发版运行入口，暂不提供工作区迁移、AI 工具检测与一键交接、agent 应用位置选择，以及 macOS 应用窗口的标题栏操作入口。可从详情复制提示词、打开项目文件夹，重新加载可通过 CLI 执行。系统目标不代表所有设备和使用场景均已验收，具体差异见 [Windows x64 平台对照](docs/windows-x64-port.md#4-平台适配层mac-与-windows-对照)。

每个应用重复打开只聚焦已有窗口；关闭业务窗口会停止本应用的受管后台。macOS 关闭管理窗口后平台仍保留在 Dock，菜单“退出”或 Cmd+Q 才会关闭全部应用和服务；Windows 关闭管理窗口会退出平台。

## 本地开发与验证

开发平台源码需要 Node.js `^22.12.0 || ^24.0.0`（推荐 24 LTS）、[package.json](package.json) 固定版本的 pnpm 和 Rust 工具链。macOS 还需要 Xcode 工具链，Windows 需要 MSVC 与 Windows SDK。普通应用用户无需安装 Rust 或平台开发工具。

在仓库根目录安装依赖并启动：

```sh
pnpm install --frozen-lockfile
pnpm run dev
```

平台开发页面使用 `127.0.0.1:1420`。脚本优先使用工程 `.tools/cargo` 和 `.tools/rustup` 中已有的私有 Rust 工具链，否则使用系统环境。

平台仓库的依赖安装使用 pnpm；平台内受管应用的依赖准备和构建仍使用 npm 与 npm v2/v3 格式的 `package-lock.json`。

按改动范围选择检查，构建和测试套件按顺序执行。macOS 本地 Rust 检查使用以下资源限制，release 配置保持不变：

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

完整本地 CI 入口为 `node scripts/ci.mjs .`；macOS 和 Windows 分别在各自机器验证。自动测试不替代真实原生窗口、系统权限、设备和安装验收，开发协作与资源约束见 [AGENTS.md](AGENTS.md)。

| 命令 | 用途 |
| --- | --- |
| `pnpm run build:app` | 生成 macOS 内部开发 `.app` |
| `pnpm run build` | 生成当前平台内部开发包；macOS 为 `.app`／DMG，Windows 为 NSIS 安装包 |
| `pnpm run cli app list --json` | 查看已登记应用 |
| `pnpm run cli env doctor --json` | 查看平台实际选用的 Node/npm 环境 |
| `pnpm run cli env git doctor --json` | 查看平台实际选用的 Git 环境 |

打包或切换源码版、打包版前，先正常退出当前实例。macOS 构建产物位于 `target/release/bundle/`；内部包使用开发身份 `dev.happypandaa.pandadrawer`，macOS 采用 ad-hoc 签名。更多工具和打包说明见 [开发说明](docs/development.md#2-运行与构建)。

## 工程结构与文档

| 路径 | 用途 |
| --- | --- |
| `apps/desktop` | React 管理界面与 Tauri 桌面宿主 |
| `crates/platform-core` | 应用登记、本地服务、SQLite、附件、配置、构建、进程生命周期和工作区管理 |
| `crates/platform-native` | 文件、进程、路径和系统能力的平台适配 |
| `crates/platform-cli` | 连接本机平台服务的原生 CLI |
| `packages/sdk` | 全局网页 SDK、共享类型和可选 ESM 别名封装 |
| `templates/react` | 新建应用的 React／TypeScript／Vite 起始源码 |
| `skills/local-projects-app` | 随平台安装的应用开发、修改和接入说明 |
| `schemas` | 应用清单 JSON Schema |
| `catalog` | 推荐应用列表 |
| `distribution` | 下载线路与发布配置 |
| `docs` | 平台约定、开发说明和验证记录 |

常用文档：

- [应用格式与数据规则](docs/panda-project-format.md)
- [应用开发和 AI 接手说明](skills/local-projects-app/SKILL.md)
- [网页 SDK API 1](packages/sdk/README.md)
- [平台源码开发](docs/development.md#2-运行与构建)
- [Windows x64 平台对照](docs/windows-x64-port.md#4-平台适配层mac-与-windows-对照)
- [下载与发布](distribution/README.md)

平台自有源码采用 [Apache-2.0](LICENSE)，第三方代码及资源遵循各自随附的许可证。
