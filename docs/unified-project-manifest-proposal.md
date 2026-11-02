# 项目配置文档 panda.json 与网页项目接入：落地方案

日期：2026-11-01。

状态：已与用户逐项确认，替代本文件之前的"项目统一清单修改方案"。项目尚未发布，**不做任何兼容**。

实施要求：决策已全部确定，按批次直接实施，不需要先提交计划等待确认。每批完成后提交简短报告：改动和删除了什么、测试结果、与本文不一致或需要用户决定的地方。只有遇到本文没有覆盖、且会影响用户可见行为的问题时，才停下来询问。

---

## 一、原则

1. **不做兼容。** 不识别旧的 `project.json`、`local-app.json`，不迁移，不提示"旧格式需要适配"，不清理旧残留。旧解析代码和相关文案直接删除。开发机上已有的项目，由用户删除后重新添加，或让 AI 改写成新格式。
2. **尽量兼容用户已有的项目。** 普通网页项目（Vite、React、纯 HTML 等）只需加一个 `panda.json` 就能接入。不要求特定的目录结构、构建参数或平台 SDK。
3. **一个项目只有一个文件夹。** 用户看到的文件夹就是正在运行的那一份。网页项目和 Node 一样，直接运行文件夹里的文件；平台不另存副本、构建代次或版本历史。
4. **平台和 AI 的分工。**
   - 平台负责：启动和停止、配置（含 Node 密钥）、运行日志、按需构建。
   - 平台不负责用户数据：数据放在项目文件夹里，平台不备份、不迁移、不校验。
   - AI 负责：改代码、构建、版本管理和回退，用 Git 完成。
5. **配置文档与本机实例分开。**
   - `panda.json` 跟着代码走，AI 可以随时修改。
   - 本机实例 ID、文件夹位置、配置值和密钥、日志、窗口存储，只记在平台登记里，不写进项目文件夹。

---

## 二、配置文档 panda.json

### 2.1 位置和识别

- 固定为项目根目录下的 `panda.json`。不搜索子目录，不支持别的文件名。
- 文件为 UTF-8 编码的 JSON，出现重复键视为无效。
- 文件夹里没有 `panda.json` 时，沿用现有流程：空文件夹生成起始项目；非空文件夹走"复制说明给 AI"评估。

### 2.2 字段

| 字段 | 网页 `web` | Node `node` | 说明 |
| --- | --- | --- | --- |
| `manifestVersion` | 必填 | 必填 | 配置文档格式版本，固定为 `1` |
| `$schema` | 选填 | 选填 | JSON Schema 地址，供编辑器和 AI 校验，平台忽略 |
| `runtime` | 必填 | 必填 | `web` 或 `node` |
| `app.name` | 必填 | 必填 | 应用名称，显示在卡片和窗口上 |
| `app.version` | 选填 | 选填 | 作者自己的版本号，仅用于显示 |
| `app.description` | 选填 | 选填 | 一段简介 |
| `app.icon` | 选填 | 选填 | 项目内 PNG 图标的相对路径；无效时使用默认图标 |
| `web.root` | 必填 | 必填 | 网页文件目录，相对项目根。网页项目常为 `dist`，Node 常为 `web`，纯 HTML 项目可以写 `.` |
| `web.entry` | 必填 | 必填 | 首页文件，相对 `web.root`，通常为 `index.html` |
| `backend.root`、`backend.entry` | 禁止 | 必填 | 后端目录和启动文件，例如 `backend`、`server.mjs` |
| `backend.nodeMajor` | 禁止 | 必填 | 固定为 `24` |
| `backend.protocolVersion` | 禁止 | 必填 | 平台控制协议版本，固定为 `1` |
| `platform.sdkApiVersion` | 选填 | 禁止 | 使用平台 SDK 时写 `1`；不写 `platform` 表示不使用 SDK，没有数据库和附件 |
| `development.sourceDir` | 选填 | 禁止 | 构建时在哪个目录执行命令，相对项目根，默认 `.` |
| `development.build` | 写了 `development` 时必填 | 禁止 | `{"kind": "npm", "script": "<脚本名>"}`，表示执行 `npm run <脚本名>`，脚本名必须是 `package.json` 里已有的脚本，例如 `build`、`bundle`、`build:web` |
| `configuration` | 选填 | 选填 | 用户配置项。结构沿用现有配置声明 v1；网页不允许 `secret: true` |
| `documentation.readme` | 选填 | 选填 | 给用户看的使用说明 |
| `documentation.development` | 选填 | 选填 | 给 AI 和开发者看的代码说明 |
| `documentation.database` | 选填 | 禁止 | 数据表和字段说明 |
| `window` | 选填 | 选填 | `width`、`height`、`rememberSize`，沿用现有规则 |
| `runtimeRequirements` | 选填 | 禁止 | 网页主动请求的外部地址和使用的设备，沿用现有结构 |

文档路径一旦声明，对应文件必须存在。不写文档不影响添加；交接提示词会请 AI 补写。

**从旧格式中删除、不再出现的字段：** `app.id`、`packageFormat`、`formatVersion`、`web.routing`、`platform.database`、`platform.attachmentsDir`、`development.outputDir`、`dataContract`、`documentation.platform`，以及旧网页清单的 `id`、`outputDir`、根相对的 `entry`、`docs`。

### 2.3 路径规则

- 所有路径使用 `/` 分隔。不允许绝对路径、`..`，也不允许经由符号链接指向项目文件夹以外。
- `web.entry` 相对 `web.root`，`backend.entry` 相对 `backend.root`。例如：`web.root` 为 `dist` 时，`entry` 写 `index.html`，不写 `dist/index.html`。

### 2.4 校验

- 检查从严：未知字段（`$schema` 除外）、类型错误、不支持的版本、路径越界，一律判为无效。
- 报错写明字段和原因，例如"`web.entry`：文件 `dist/index.html` 不存在"，并附「复制说明给 AI」。
- 检查只读，从不修改项目文件夹。
- 入口文件缺失时的状态：
  - 网页项目声明了 `development.build`：显示"需要构建"。
  - 其他情况：显示"入口文件缺失"。
  - Node 的网页入口和后端入口都必须存在。
- 发布 JSON Schema：放在仓库 `schemas/panda.schema.json`，随应用安装，交接提示词给出实际路径。结构检查由 Schema 和 Rust 共用同一份定义，前端类型由同一来源生成。"文件是否存在"这类检查由平台完成。

### 2.5 示例

最小 Node 应用：

```json
{
  "manifestVersion": 1,
  "runtime": "node",
  "app": { "name": "示例笔记" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": { "root": "backend", "entry": "server.mjs", "nodeMajor": 24, "protocolVersion": 1 }
}
```

普通 Vite / React 项目：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "小游戏" },
  "web": { "root": "dist", "entry": "index.html" },
  "development": { "build": { "kind": "npm", "script": "build" } }
}
```

单个 HTML 文件的项目（不构建、不用 SDK）：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "番茄钟" },
  "web": { "root": ".", "entry": "index.html" }
}
```

使用平台 SDK 和用户配置项的网页项目：

```json
{
  "$schema": "<平台安装目录>/schemas/panda.schema.json",
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "读书笔记", "version": "0.2.0", "icon": "meta/icon.png" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": { "sourceDir": ".", "build": { "kind": "npm", "script": "build" } },
  "configuration": {
    "version": 1,
    "fields": [
      {
        "name": "BOOK_API_BASE",
        "label": "图书查询地址",
        "type": "url",
        "required": false,
        "secret": false,
        "description": "查询书籍信息的接口地址",
        "howToGet": ["不填则使用内置地址"]
      }
    ]
  },
  "documentation": { "development": "README.md", "database": "docs/database.md" }
}
```

### 2.6 修改后什么时候生效

平台在每次打开应用前重新读取 `panda.json`。

| 修改内容 | 生效时机 |
| --- | --- |
| 名称、简介、图标、版本、窗口设置 | 下次打开 |
| `web.root`、`web.entry`、`backend` 入口 | 下次打开 |
| `platform` 增加或删除、`runtimeRequirements` | 下次打开 |
| 配置项声明 | 下次打开。已保存的值按变量名保留；删掉的字段不再提供；新增必填项还没填值时显示"需要配置"，并跳到配置页，不打开应用 |
| `development` | 下次构建 |
| `runtime` 改变 | 不打开，提示"应用类型变了，请删除后重新添加" |
| 配置文档无效 | 不打开，显示原因，附「复制说明给 AI」 |

- 名称和图标只来自 `panda.json`。删除界面上的改名、改图标功能，包括 `metadata.rs` 中把修改写回清单的逻辑。

---

## 三、本机实例和数据

### 3.1 实例登记

- 平台登记保存以下内容：实例 ID、项目文件夹路径与文件身份、归属（平台所有或用户所有）、应用类型、配置值和密钥、日志位置、窗口存储、固定访问地址（见第六节）。
- 网页项目登记删除 `database_path`、`database_identity`。删除"文件夹 ID 与清单 ID 必须一致"的检查。运行时使用的实例 ID 只从登记中取得。
- 同一个文件夹不能重复添加（现有规则）。从 Git 多次添加同一个仓库，会得到多个独立的文件夹和实例。

### 3.2 数据

应用数据分两类，说明、删除确认和交接提示词里都要分开写清：

- **文件数据：** 放在项目文件夹的 `data/` 下，两类应用相同。
- **浏览器数据：** 页面自己写入的 localStorage、IndexedDB、Cookie 等，保存在平台的窗口存储里，不在项目文件夹中，与实例绑定。删除项目时一定清除（即使项目文件夹保留）；同一个文件夹删除后重新添加，是新的实例，也读不到之前的浏览器数据。需要长期保存或希望随文件夹保留的数据，应使用 SDK 数据库或 `data/` 下的文件。

具体规则：

- **网页：**
  - 使用 SDK 时，数据库为 `data/main.sqlite`，附件放在 `data/attachments/`，首次用到时创建。
  - 数据库文件不存在时直接新建空库，平台不判断它是不是被误删的。
  - 不使用 SDK 的项目，平台不创建 `data/`。
- **Node：**
  - `APP_DATA_DIR` 指向 `<项目文件夹>/data/`，启动前自动创建。
  - `APP_CACHE_DIR`、`APP_TEMP_DIR` 和运行日志仍放在平台控制目录。
- 平台不备份、不迁移数据，也不核对数据文件的身份；数据文件发生变化不会导致拒绝启动。
- 平台生成的起始项目带 `.gitignore`，内容至少包括 `node_modules/` 和 `data/`，并在注释中说明：想提交数据，删掉 `data/` 这一行即可。
- 网页服务不对外提供 `data/` 目录（见 6.1）。
- **删除项目时：**
  - 平台所有的项目（新建、从 Git 添加）：整个文件夹连同 `data/` 一起删除。
  - 用户所有的项目（添加本地项目，包括在用户选的空文件夹里生成的项目）：只删除平台的登记、配置、日志和窗口存储，文件夹和 `data/` 保留。
  - 删除确认框按归属分别写明：项目文件夹和 `data/` 删除还是保留；浏览器数据一律清除。
- 详情页底部文案改为："配置和密钥由平台保存；应用数据在项目文件夹的 data/ 里，页面自己保存的浏览器数据随应用删除而清除。"

### 3.3 项目文件夹里不再有平台控制文件

- 网页项目现在会在项目文件夹的 `.platform/` 下保存任务、构建、项目状态、操作记录、交接说明、上传暂存等。本次全部移出：
  - 仍然需要的，移到平台控制目录，按实例存放，与 Node 的 `node-apps/<实例>/` 同类。
  - 只为第四、五节删除的功能服务的，直接删除。
- 完成后，平台不在项目文件夹里写任何控制文件。例外只有两类：用户触发的构建产物和依赖（`node_modules`、构建输出），以及 SDK 的 `data/`。平台也不向项目的 `.git` 写任何东西。
- 实施计划中逐项列出现有 `.platform/` 下每类文件的去向。

---

## 四、网页项目的构建

- 平台直接运行 `web.root` 里的文件。AI 自己执行构建脚本（例如 `npm run build`）后，关闭窗口再打开即可生效。平台不检查产物是否为最新。
- **「构建」按钮和 CLI `project build`：**
  - 在 `development.sourceDir` 下依次执行 `npm ci` 和 `npm run <script>`，不加 `--outDir`。
  - 使用平台自带的 Node 和 npm，沿用现有的 npm 配置隔离、超时、重试、空间预算和错误详情。
- **构建前提：**
  - `sourceDir` 下必须有 `package.json`，以及 npm v2/v3 格式的 `package-lock.json`。缺少锁文件时构建失败，提示让 AI 生成 `package-lock.json`。
  - 只支持 npm。
- 构建结束后检查 `web.root/web.entry` 是否存在。
- 构建前必须关闭这个应用的窗口。窗口开着时提示"先关闭应用窗口"。
- 构建失败时不回滚任何文件（项目文件夹由 AI 负责），显示错误，附「复制说明给 AI」。
- **删除的机制：**
  - 构建副本（`.platform/builds/`）、`--outDir` 暂存、"构建输出违约"检查及回滚。
  - 不可变产物 `.platform/artifacts/`、构建代次保留、窗口占用跟踪、`artifact-views`、发布日志恢复。
  - 输入摘要，以及"需要重新构建"状态。
  - 涉及 `builds.rs` 的大部分，以及 `build_candidate*`、`build_artifacts*`、`build_window_views`、`build_publication_io`、`build_directory_exchange`、`build_copy_io`、`build_source_io`、`project_git_build_guard` 等。具体清单由实施方核对后在计划中列出。
- Node 不使用平台构建（规则不变）。

---

## 五、删除的功能

### 5.1 源码与数据版本
- **前端：** `ProjectGitPanel`、`ProjectGitVersions`、`ProjectGitSourceRestore`、`ProjectGitDataRestore`、`ProjectGitDataRecovery`，以及只服务于这些功能的 `SourceRecoveryNotice`、`ProjectOperationRecovery` 等（实施方核对）。
- **后端：** `project_git*`、`project_version_data`、`data_restore_*`、`source_restore_guard`、`project_data_restore_registration` 等模块，以及对应的 Tauri 命令和 CLI 命令。
- 项目自己的 `.git` 和提交记录不受影响。

### 5.2 网页项目的 ZIP 导出、导入和复制
- `ProjectExportDialog`、`ArchiveImportPanel`、`LifecyclePanels` 中的相关部分。
- `backups.rs`、`lifecycle.rs` 中的导出、导入和复制，以及相关 CLI 命令。

### 5.3 之前计划留到下期的 Node 后端命令
- `node_development_git`、源码取回、`node_development_open_codex` 等，本次一并删除。

### 5.4 只服务于以上功能的清理和恢复
- 「设置 › 存储与清理」以及启动时的恢复、清理流程中，只服务于已删功能的部分一并删除；其余保留。

---

## 六、独立地址和网页服务

### 6.1 每个应用一个独立服务
- 网页应用改用与 Node 代理相同的模式：打开窗口时，为这个应用启动一个独立的本机服务；所有窗口关闭后停止。它是平台进程内的轻量监听，不是 Node 进程。
- 根路径直接提供 `web.root` 下的文件。以下内容不提供：以 `.` 开头的文件和目录、`data/`、`node_modules/`、`panda.json`。
- **按真实文件路径检查，不只看 URL：** 每次读取都先解析符号链接，得到实际文件路径，再判断：实际文件必须位于 `web.root` 之内，并且不在项目的 `data/`、`node_modules/`、以 `.` 开头的目录里，也不是 `panda.json`。任何一条不满足就拒绝。另外在配置文档校验阶段：`web.root` 等于或位于 `data/`、`node_modules/`、以 `.` 开头的目录中时，判为无效。`web.root` 为 `.` 时同样适用以上排除。硬链接无法从路径识别，处理办法由实施方在计划中给出（例如拒绝提供链接数大于 1 的文件）。
- `/_platform/` 是平台保留路径，下面有：
  - `sdk.js`：平台提供的 SDK，见 6.4。
  - `config.json`：用户配置项，见 6.5。
  - `api/v1/…`：SDK 接口。
  - 现有的 `bootstrap`。
- **前端路由回退：** 同时满足以下条件时返回 `web.entry`：请求的路径没有对应文件；路径没有文件扩展名；是页面请求。这样 React Router 等前端路由在子路径刷新时不会白屏。
- 访问认证沿用 Node 代理的机制，只有平台窗口能访问。
- **关闭即释放数据：** 应用的所有窗口关闭后，平台停止服务，并排空进行中的 SQL、上传等 SDK 操作，然后真正关闭数据库连接（完成 WAL 写回），不再持有 `data/` 下任何文件。只停止网页监听不够。"应用已关闭"的状态只在连接关闭后才显示，交接提示词所说的"应用关闭后可以直接操作数据库"以此为准。CLI 的 `db` 命令在应用关闭时也必须用完即关，不常驻连接。
- 删除共用服务上的 `/p/<id>/` 路由和 `/_builds/` 版本地址。

### 6.2 地址固定
- 同一个实例每次打开时，地址（协议、主机、端口）必须相同，保证 localStorage、IndexedDB、Cookie 在下次打开时仍然可用。
- Node 应用的网页地址同样改为固定。Node 后端自己监听的端口可以继续由系统分配。
- 实现方式由实施方评估后，在计划中给出选择：
  - A. 每个实例固定一个端口，记在登记里。
  - B. 所有实例共用一个固定端口，用 `<实例短名>.localhost` 区分。需要验证 macOS 的 WKWebView 和 Windows 的 WebView2 都能正确解析，存储也能隔离。选 B 时，6.1 的"每个应用一个独立服务"相应改为"一个共享监听，按主机名分发到各实例"，各实例的启停、认证、关窗释放数据等规则不变。
- A/B 在批次 3 开始时选型。
- 地址无法使用时（例如端口被其他程序占用），不允许悄悄换成另一个地址，以免应用"数据不见了"。必须给出明确提示和处理办法，具体在计划中说明。

### 6.3 安全策略（两类应用的网页部分统一）

适度放宽，参考如下（实施方可微调，但不得收紧下列已放开的项）：

```
default-src 'self';
script-src 'self' 'unsafe-inline' 'unsafe-eval' https:;
style-src 'self' 'unsafe-inline' https:;
font-src 'self' data: https:;
img-src 'self' data: blob: https:;
media-src 'self' blob: https:;
worker-src 'self' blob:;
connect-src 'self' <runtimeRequirements 声明的地址>;
object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'
```

- `connect-src`：网页项目为 `'self'` 加上 `runtimeRequirements` 中声明的地址；Node 为 `'self'`。
- 已知代价：放宽后，混入页面的脚本可以通过图片地址向外带出少量数据。用户已接受这一点。

### 6.4 SDK 由平台提供
- `/_platform/sdk.js` 是普通脚本（不是 ES 模块），加载后在 `window` 上提供接口（名称由实施方定，例如 `window.panda`），接口版本按 `sdkApiVersion`。纯 HTML 项目用一行 `<script src="/_platform/sdk.js"></script>` 即可；打包项目同样在 `index.html` 里引入。
- SDK 固定访问 `/_platform/api/v1/`，不再从地址中解析项目 ID。
- 类型声明文件（`.d.ts`）随应用安装，交接提示词给出路径。npm 包 `@local-projects/sdk` 改为可选（只提供类型或一层薄封装），模板不再内置 `.tgz`。
- 没有声明 `platform` 的应用，`/_platform/api/` 返回明确错误"这个应用没有启用平台 SDK"，平台不创建数据库。

### 6.5 用户配置项（网页）
- 声明结构与 Node 相同，但不允许 `secret: true`。
- 配置值由平台保存在实例控制目录，表单复用 `NodeConfigurationForm`。
- **页面读取：** `GET /_platform/config.json`，返回 `{"变量名": 值}`。
  - 文本和网址为字符串，整数为数字，开关为布尔值，下拉选项为字符串。
  - 未填写的选填项不出现在结果里。
  - 读取不依赖 SDK；SDK 另外提供一层封装，例如 `panda.config()`。
- 保存后，下次打开应用（或刷新页面）即生效，不需要重新构建。
- 新增的必填项还没填值时，显示"需要配置"，不打开应用。
- Node 不变：配置通过环境变量传入，密钥只交给后端。

---

## 七、添加入口

### 7.1 新建项目（网页）
- 生成根目录布局的起始项目：
  - `panda.json`、`package.json`、`package-lock.json`。
  - `index.html`、`src/`，Vite 默认配置（批次 3 之后不再需要 `base: './'`）。
  - `.gitignore`（`node_modules/`、`data/`）、`README.md`、`docs/development.md`。
- 默认启用 SDK（`platform.sdkApiVersion: 1`），通过 `/_platform/sdk.js` 使用，起点仍然带数据库。
- 创建时平台执行一次构建。模板是平台自己的，不需要用户确认。
- 存放位置与现有平台所有项目一致，归属为平台所有。

### 7.2 添加本地项目
- `panda.json` 中 `runtime` 为 `node`：走现有 Node 流程，改为读取 `panda.json`。
- `runtime` 为 `web`：登记为用户所有，直接运行这个文件夹。
  - 入口缺失、但声明了构建：卡片显示"需要构建"和「构建」按钮。文件夹是用户自己的，不需要额外的风险提示。
  - 入口缺失、也没有声明构建：显示"入口文件缺失"。
- 空文件夹：生成 Node 起始项目。文件夹是用户自己选的，**归属改为用户所有**：删除应用时只删平台记录，文件夹（连同生成的代码和 `data/`）保留。这一点与现有流程不同，需要一并修改。
- 非空且没有 `panda.json`：走 AI 评估（现有）。

### 7.3 从 Git 添加
- **Node：** 规则不变，只改为读取 `panda.json`。依赖用 `npm ci --omit=dev --ignore-scripts` 安装；需要构建却没有提交运行文件的，按"需要修改"失败。
- **网页：**
  1. 入口已存在（仓库里提交了构建产物）：直接发布并登记，不安装依赖，不执行任何脚本。
  2. 入口不存在，但声明了 `development.build`：任务停在"等待确认"，首页卡片和任务入口显示"需要确认"。确认框文案：
     > 这个项目还没有构建好的文件，需要平台构建。构建会运行该仓库自带的脚本（包括依赖安装脚本），可能需要几分钟。
     >
     > 仓库：<地址>
     >
     > 「构建并添加」 「取消」

     选「构建并添加」：在下载好的文件夹中按第四节执行 `npm ci` 和 `npm run <development.build.script>`，检查入口，然后发布并登记。选「取消」：全部回滚。
  3. 入口不存在，也没有声明构建：按"需要修改"失败。
  4. 构建失败：全部回滚，包括文件夹、临时材料和归属记录。显示错误，附「复制说明给 AI」。用户修好后，用「添加本地项目」添加。
  5. 等待确认期间退出平台：下次启动时按"登记前中断"回滚。
- 沿用现有的提交点规则：登记之前失败就回滚；登记之后保留项目，只清理临时材料。

---

## 八、界面

- **详情：**
  - 网页项目的主操作为「打开」；声明了 `development.build` 时，次要位置显示「构建」。
  - 信息列表：应用配置（两类应用都有，没有配置项时隐藏）、项目文件夹、占用空间、运行日志（仅 Node）。
  - 底部文案按 3.2 修改。
- **卡片状态：** 需要配置、需要构建、入口文件缺失、配置文档有误、需要确认（Git 添加时）。
- **删除确认框：** 按归属说明数据保留还是删除。
- **删除的入口：** 改名和改图标、源码与数据版本、导出、复制。布局方案中"更多"折叠区里的版本类入口随之删除。
- 「设置 › 存储与清理」：删除只服务于已删功能的清理项。

---

## 九、提示词、skill 和文档

- **平台要求文档**（`docs/project-compatibility-review-prompt.md`）：改写为 `panda.json` 规范（含 Schema 路径），以及两类应用的要求、构建与数据规则、安全策略、`/_platform/` 路径。改完运行 `node scripts/generate-project-ai-requirements.mjs`。
- **Node 交接提示词：** `local-app.json` 改为 `panda.json`；说明数据在 `data/`，删除前先问；删除"数据单独保存"的说法。
- **网页交接提示词：** 重写，要点如下：
  - 项目文件夹就是正在运行的那一份。
  - 改完可以自己执行 `development.build.script` 指定的脚本，也可以请用户点「构建」；构建前提醒用户关闭窗口，关窗再打开即生效。
  - SDK 通过 `/_platform/sdk.js` 使用，配置从 `/_platform/config.json` 读取。
  - 数据在 `data/`，删除前先问。应用打开时，数据库只通过 CLI 的 `db` 命令修改；应用关闭后可以直接操作。
  - 用 Git 保存和回退。
  - 不再提 `.platform`、发布目录、版本面板。
- **平台介绍提示词：** 说明两类应用都可以从本地和 Git 接入，给出 `panda.json` 最小示例和 Schema 路径。
- **求助提示词**同步修改。
- **skill：** `skills/local-projects-app/SKILL.md`、`skills/local-folder-review/` 同步修改。（2026-11-02：两者已合并为 `skills/local-projects-app`。）
- **文档：**
  - 标注退役：`project-and-app-format-v1.md`、`web-build-generation-retention.md`、`project-data-git.md`、`project-data-restore*.md`。
  - 按新规则修改：`node-single-directory.md`、`local-directory-onboarding.md`、`git-app-import.md`、`project-complete-deletion.md`（数据随文件夹处理）、`app-window-runtime-contract.md`（安全策略）、`node-app-runtime.md`（`APP_DATA_DIR`）、`development.md`。
- **提示词测试的断言：**
  - 不出现 `project.json`、`local-app.json`、`.platform`、"发布目录"、"版本面板"。
  - 网页提示词包含 `/_platform/sdk.js`、`/_platform/config.json`、`data/`、Git 保存和回退。
  - 平台介绍提示词不含任何项目路径或 ID。

---

## 十、实施批次

### 批次 1：配置文档与 Node
- 统一解析器（Rust）、JSON Schema、前端类型。Node 改为读取 `panda.json`，删除 `local-app.json` 解析和 `dataContract`、`development` 等旧字段。
- Node 的 `APP_DATA_DIR` 移到项目的 `data/`，删除流程和文案相应调整。
- Node 起始项目生成 `panda.json` 和 `.gitignore`。
- 完成第五节的全部删除。
- Node 相关的提示词和文档。
- 本批不改网页运行，网页暂时仍读旧清单。

测试：
- `panda.json` 的必填、选填、禁止字段和错误提示。
- 修改名称、版本、配置项后重开生效。
- 数据出现在项目的 `data/`；本地项目删除后文件夹和数据保留，Git 项目删除后全部清除。
- 已删除组件和命令在代码中搜索不到引用。

### 批次 2：网页项目单目录
- 网页改为读取 `panda.json`；实例身份移出配置文档；删除数据库身份绑定；`.platform/` 移出项目文件夹。
- 构建改为在原文件夹执行 `npm run <development.build.script>`，删除副本、代次、占用跟踪和 `artifact-views`。
- 新建模板改为根目录布局（本批仍在 `/p/<id>/` 下运行，`base: './'` 保留到批次 3）。
- 「添加本地项目」和「从 Git 添加」支持网页项目，包括等待确认流程。
- 删除界面上的改名和改图标；详情增加「构建」。
- 网页提示词、平台介绍提示词和文档。

测试：
- AI 自行构建后关窗再开即生效；窗口开着时构建会提示先关窗。
- Git 添加的三种情况（已有产物、需要构建、无法接入），以及确认、取消、构建失败、等待确认时退出平台。
- 项目文件夹里没有 `.platform`。

### 批次 3：独立地址
- 网页独立服务；地址固定（含 Node）；两类应用统一放宽安全策略；前端路由回退。
- `/_platform/sdk.js`、`config.json`、`api/v1/`；SDK 改造。
- 网页用户配置项：声明、表单、读取。
- 删除 `/p/` 路由。
- 模板去掉 `base: './'`，改用 `/_platform/sdk.js`。
- 提示词更新 SDK 和配置的用法。

测试：
- 用绝对路径引用资源的项目能正常加载。
- 关窗并退出平台后再打开，localStorage 仍在。
- 内联脚本、`onclick`、CDN 上的库都能工作。
- 读取配置；未启用 SDK 时调用接口给出明确错误。
- 前端路由在子路径刷新不白屏。

---

## 十一、总体验收（macOS 和 Windows 开发版）

1. 从 GitHub 添加一个提交了 `dist` 的普通 Vite 项目（只加了 `panda.json`）：不执行任何脚本，直接打开，资源加载正常。
2. 从 GitHub 添加一个没有提交 `dist`、但声明了构建的项目：出现确认框；确认后构建并打开；取消后不留下任何文件。
3. 添加一个本地文件夹，里面是单个 `index.html`，包含内联脚本、`onclick` 和 CDN 上的库：能打开，功能正常。
4. 页面写入 localStorage 后关窗、退出平台，再打开：数据仍在。
5. 网页项目声明一个配置项，用户填写后，页面能通过 `/_platform/config.json` 读到；修改后重新打开生效。
6. 使用 SDK 的网页项目：数据库位于项目的 `data/main.sqlite`；删除本地项目后文件夹和数据保留；删除从 Git 添加的项目后全部清除。
7. Node 应用的 `APP_DATA_DIR` 指向项目的 `data/`；修改 `panda.json` 的名称、版本、配置项后重开生效。
8. AI 在项目文件夹里执行 `npm run build`，关窗再开即生效；平台「构建」按钮效果相同；窗口开着时构建会提示先关窗。
9. 使用 React Router（`BrowserRouter`）的项目在子路径刷新不白屏。
10. 项目文件夹里没有 `.platform`；平台不向项目的 `.git` 写入任何内容。
11. 写错 `panda.json` 的几种情况（未知字段、入口不存在、`runtime` 改变）各有明确提示。
12. 代码中搜索不到 `project.json`、`local-app.json`、`/p/` 路由以及已删除组件的引用；两套 CI 通过；`tsc -b` 通过。

所有验证都使用明确命名、可以丢弃的测试项目。

---

## 十二、本次不做

- 网页密钥，以及由平台代发外部请求（另行设计）。
- pnpm、yarn 等其他包管理器。
- 应用类型转换、数据迁移、旧项目兼容。
- SSR、多服务、其他运行时。
- 前端（浏览器侧）报错收集。

---

## 十三、待确认（小项）

1. **网页窗口约束。** 现有约定禁止应用使用 `alert`、`confirm`、`prompt`、`window.open` 和 `target="_blank"`（`app-window-runtime-contract.md`）。普通网页常用这些。建议：`alert`、`confirm`、`prompt` 由平台显示原生对话框；外部链接和 `window.open` 改用系统浏览器打开。这样普通网页不需要修改。
2. 数据库文件不存在时直接新建空库（本文默认采用）。
3. 新建网页项目默认启用 SDK（本文默认采用）。
