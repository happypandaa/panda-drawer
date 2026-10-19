# 项目与应用包格式 v1

日期：2026-10-19

状态：目录、清单分工及本文档整理已确认；现有开发项目格式继续有效。外部应用包是待实现的 v1 目标设计，配套 Schema 不代表安装器已经可用。初始数据和更新细则等候选方案仍按第 11 节区分，不因编写 Schema 自动批准实施。

本文是开发者和外部 AI 的格式入口，集中说明一个目录如何描述自己、必需文件、字段与校验层级。本轮只增加文档、JSON Schema 和说明性示例，没有修改运行代码、既有项目或安装包。

## 1 三种交付对象及当前状态

| 对象 | 识别入口 | 内容与用途 | 状态 |
| --- | --- | --- | --- |
| 受管开发项目目录 | 根部 `project.json` | 本机一个项目实例：源码、网页、独立数据库、附件与文档 | 已实现；由平台新建或通过完整 ZIP 导入登记 |
| 完整项目 ZIP | ZIP 根部 `backup.json` 和 `project/` | 同一快照时点的项目和用户数据，用于备份、迁移、恢复或复制 | 已实现；沿用[生命周期](project-lifecycle-and-workspace.md)与[备份契约](backup-and-maintenance.md) |
| 外部网页应用包目录 | 根部 `local-app.json` | 作者发布的网页程序、可选源码资料及明确选择的初始资料 | 待实现；本文与配套 Schema 描述目标结构，不是现有导入接口 |

`package.json` 位于源码目录，只描述 npm 依赖和构建脚本；它不替代以上任何识别入口。React、TypeScript、Vite 是默认模板，不是所有网页必须采用的技术。当前平台提供静态网页、本机 SQLite 与附件接口，不支持每应用自带的 Node/Python 后端、SSR 服务、Docker 或额外数据库服务。

文件管理器复制文件夹不等于完成项目登记。当前公开导入入口只接收平台完整 ZIP；尚未提供任意目录收养、`local-app.json` 安装、应用更新或 GitHub URL 安装命令。

## 2 已实现的受管项目目录

默认布局如下，目录名由清单声明；`data/`、`meta/` 和平台控制范围另有边界约束。

```text
<工作区>/projects/<projectId>/
├── project.json
├── README.md
├── AGENTS.md                         AI 接管指引
├── CLAUDE.md                         指向同一套说明
├── .agents/skills/local-projects-app/ 随项目保存的接管资料
├── docs/
│   ├── database.md
│   └── platform.md
├── meta/
│   └── icon.png                      可选
├── app/                              sourceDir 默认值
│   ├── package.json
│   ├── package-lock.json
│   ├── vendor/                       固定版本本地 SDK
│   ├── src/
│   └── public/
├── dist/                             outputDir 默认值
│   ├── index.html
│   └── assets/
├── data/
│   ├── main.sqlite
│   └── attachments/
└── .platform/                        平台管理的构建、发布代、来源及日志
```

“必须声明”“必须存在”和“必须可运行”是不同要求：

| 路径或内容 | 规范要求 | 当前实际检查 |
| --- | --- | --- |
| `project.json` | 根部 UTF-8 JSON 对象，字段见第 3 节 | 识别、登记与操作时读取并验证 |
| 独立数据库 | 清单指定的 `data/` 内 SQLite 文件 | 正常登记要求文件存在，校验身份；缺失不静默重建空库 |
| 源码及附件目录 | 按清单组织；源码和数据分别保存 | 清单阶段可接受尚不存在的声明路径；完整 ZIP 要求对应目录存在 |
| README、数据库及平台说明 | 声明三类文档；数据库说明含表用途、字段、单位、关系和删除规则，空库也说明无业务表 | 清单检查路径，完整 ZIP 检查实际文件；当前不自动证明内容完整或与业务一致 |
| 网页产物 | 可打开时具备入口和运行资源 | 合法旧项目可以尚未构建；已登记产物意外丢失应显示异常，不删除数据 |
| `package.json`、锁文件 | 重新构建时必需；锁定依赖并提供清单所指脚本 | 不是识别入口；旧 M1 无 npm 工程可保留，但不能宣称可构建 |
| 图标 | 可选，位于 `meta/` 的静态 PNG | 缺失、无效或不可解码可回退默认显示并诊断，不单因此关闭业务 |
| AI 接管资料 | 标准新建流程提供，随项目持久保留 | 不是所有历史项目读取时的新增必填；升级不覆盖旧资料 |

工作区 `.platform/project-state/`、任务、维护、回收站、恢复现场、备份及固定数据根中的设置均由平台管理。它们不是开发者用清单手工生成的登记材料；项目内 `.platform/` 也包含发布代和来源证据，不可手动删除。两级控制目录均不能整体当作缓存，清理只能通过已定义的受保护范围规则执行。

## 3 `project.json` 字段与最小清单

下表是当前 project v1。未新增 `kind`、`app.id`、安装版本等必填项，旧项目不需要因本文补写新字段。

| 字段 | 必需 | 类型与规则 |
| --- | --- | --- |
| `manifestVersion` | 是 | 整数 `1`，清单主版本 |
| `id` | 是 | 1–64 字符，小写字母开头，其余小写字母、数字、`_`、`-`；与受管目录名一致，由平台创建/复制流程分配 |
| `name` | 是 | 非空纯文本名称；同名可以，不作为身份 |
| `description` | 否 | 字符串，省略默认空 |
| `icon` | 否 | `meta/` 内静态 PNG 的相对路径；标准为 `meta/icon.png` |
| `sdkApiVersion` | 是 | 当前为整数 `1`，表示接口主版本，不是 SDK npm 包版本 |
| `sourceDir` | 是 | 源码目录，默认 `app` |
| `outputDir` | 是 | 网页产物目录，默认 `dist` |
| `entry` | 是 | 相对项目根的入口文件，必须位于 `outputDir` 内，默认 `dist/index.html` |
| `database` | 是 | `data/` 内的数据库文件，默认 `data/main.sqlite` |
| `attachmentsDir` | 是 | `data/` 内的附件目录，默认 `data/attachments`；不能包含数据库 |
| `build` | 是 | 对象，包含非空字符串 `script`；填写 npm 脚本名，标准为 `build` |
| `docs` | 是 | 对象，必须有字符串路径 `readme`、`database`、`platform` |
| `runtimeRequirements` | 否 | 第 7 节的网络/设备需求对象，省略表示本地默认策略 |

顶层未知字段会由现有解析器保留；`build`、`docs` 也接受额外键。它们不自动获得执行语义，避免将拼错字段误认为新能力。`runtimeRequirements` 及其内部对象则拒绝未知字段。

当前各入口的文本限制不同：新建名称最多 200 个字符、简介最多 2000 个字符；元数据编辑名称最多 512 UTF-8 字节、简介最多 16384 字节、图标引用最多 1024 字节；普通清单读取只要求名称非空。这是现状，本文不伪造已经统一的上限。可移植编写时采用简洁名称并核对目标入口限制。

最小完整清单见[可下载 JSON 示例](project-format-v1/examples/minimal-project/project.json)：

```json
{
  "manifestVersion": 1,
  "id": "prj_format_v1_example",
  "name": "项目格式 v1 清单示例",
  "sdkApiVersion": 1,
  "sourceDir": "app",
  "outputDir": "dist",
  "entry": "dist/index.html",
  "database": "data/main.sqlite",
  "attachmentsDir": "data/attachments",
  "build": { "script": "build" },
  "docs": {
    "readme": "README.md",
    "database": "docs/database.md",
    "platform": "docs/platform.md"
  }
}
```

这是最小清单，不是完整可运行目录或导入 ZIP；示例 ID 为虚构值。实际项目通过平台创建，或使用平台完整导出归档导入。不能把这个 JSON 放进 `projects/` 就冒充登记完成。

## 4 `package.json` 与构建材料

重新构建时，`sourceDir` 下需要 `package.json` 和 `package-lock.json`；`package.json.scripts` 包含 `project.json.build.script` 指向的脚本，平台按锁文件准备依赖，再执行该脚本并传入平台暂存产物目录。脚本须遵守此目录参数，不能写入 `data/` 或修改 `meta/`。

实际构建阶段检查脚本名只含 ASCII 字母、数字、`_`、`:`、`-`，且不能以 `-` 开头；检查脚本存在、锁文件版本为 2 或 3，以及根包与锁文件中的 dependencies/devDependencies/optionalDependencies 一致。相对 `file:` SDK 包及其他本地依赖应随源码保留。旧 SDK 不随平台更新自动替换。

完整 ZIP 的 npm 材料检查较窄：若存在 `package.json`，检查锁文件可解析及声明的本地文件依赖存在；不等于已经通过锁版本、依赖一致或实际构建。本文不对任意 npm 字段重复定义 Schema，也不宣称只有 React/Vite 工程才能符合网页运行约定。

`node_modules`、工具缓存不是可移植依赖声明，源码和锁文件才是重新构建的依据。带有效产物的项目日常运行无需 Node/npm；修改源码后重建才要求对应工具环境。构建失败保留上次产物与数据，不能清空数据库来“重建项目”。

## 5 外部应用包目录：v1 目标，待实现

本节描述将来“安装应用”的输入，不可交给当前完整项目 ZIP 导入入口。包根必须有专用 `local-app.json`，目录由用户明确选择；不递归猜测 `dist`、不将任意 GitHub 仓库当成发布包。

```text
example-app-1.0.0/
├── local-app.json                    固定清单名
├── dist/                             web.root
│   ├── index.html                    web.entry，相对 web.root
│   └── assets/
├── meta/icon.png                     可选
├── docs/                             可选使用/开发说明
├── source/                           可选源码资料，不在安装时构建
└── initial-data/                     seed 候选方案才使用，见第 6 节
```

网页入口及其本地运行资源必须齐备；源码可以没有。只复制清单及声明范围，原包只读，其他内容展示为未纳入安装。源码资料不证明与产物匹配，也不自动开放受管构建。安装不执行 npm、包内 SQL/JS/shell 或迁移脚本，不克隆 GitHub。

网页仅支持已构建静态 HTML/JS/CSS；平台提供本地服务。`hash`、`files` 是目标路由类型，依赖通用服务器 history 回退的包不在目标 v1 中。片段导航与平台发布代基址的兼容仍需实现和实测，声明 `hash` 不表示已解决兼容。

分享包不默认包含开发者的当前 `data/`、项目控制目录、管理凭据、会话、秘密 key、开发 `.env`、数据库伴随文件、`node_modules` 或现有实例的 `project.json`。初始资料必须单独声明；检测不等于证明已脱敏或应用可信。支持范围仍是用户信任的应用，当前共享 origin 架构不是恶意第三方应用沙箱。

## 6 `local-app.json` 字段：目标结构与候选细则

所有字段均为待实现的外部包设计；其中数据初始化规则继续作为明确标注的候选。具体机器路径、端口、平台实例 ID、登记身份和凭据不得由包指定。

| 字段 | 必需 | 目标结构与限制 |
| --- | --- | --- |
| `packageFormat` | 是 | 固定 `local-web-app` |
| `formatVersion` | 是 | 整数 `1`，包格式版本；不是项目清单版本 |
| `runtime` | 是 | 固定 `static-web` |
| `app` | 是 | 对象，必需 `id`、`name`、`version`，可选 `description`、`icon` |
| `app.id` | 是 | 3–128 ASCII 字符；点分段以小写字母开头，其余为小写字母、数字、`-`，推荐反向域名；作者跨版本稳定标识 |
| `app.name` / `description` | 是 / 否 | 非空名称最多 200 字符，简介最多 2000 字符 |
| `app.version` | 是 | 三段非负整数、无前导零，每段最多 2147483647；不带 `v`、范围或预发行后缀 |
| `app.icon` | 否 | 相对包根的 PNG；声明后缺失/无效阻断安装检测，与旧项目坏图标降级规则不同 |
| `web` | 是 | `root` 为包根相对目录；`entry` 为网页根相对 `.html`/`.htm` 文件；`routing` 为 `hash` 或 `files` |
| `requires` | 是 | 必需唯一字符串数组 `capabilities`；非空数组要求正整数 `apiVersion`；可选 `minPlatformVersion`，版本格式同上 |
| `runtimeRequirements` | 是 | 网络/设备对象，无需求写 `{}`，不接受 `null` |
| `data` | 是，设计候选 | `formatVersion` 为 0–2147483647，`initialization` 按下表描述；Schema 编码候选结构，不代表已具备安装初始化能力 |
| `documentation` | 否 / 条件必需 | 可含包根相对文件路径 `readme`、`data`、`development`；候选 seed 要求 `data` |
| `source` | 否 | 仅有 `root`，包根相对源码资料目录；不自动认定产物来源匹配 |
| `distribution` | 否 | `repository`、`releaseUrl` 为无凭据 HTTPS URL，`releaseTag` 为非空资料；不触发联网获取 |
| `extensions` | 否 | 自定义展示元数据对象，不得改变权限、路径、初始化或更新语义 |

外部包目标采用 UTF-8 严格 JSON：不接受注释、尾逗号、重复键、非有限数字、未知字段和未知枚举；唯一开放对象为 `extensions`。可选字段出现时也要有效。重复键与原始编码需在解析阶段检查，不能在解析丢失信息后仅靠 Schema 发现。现有项目读取尚未统一实施此规则，不因此收紧旧项目。

环境兼容另行判定：当前 API 主版本为 1，当前能力名为 `db.schema`、`db.execute`、`db.transaction`、`files`、`events`。Schema 不把这五项永久封为所有未来平台的全集，检测须与目标平台实际能力核对。使用 SDK/API 就必须声明 API 版本；仅靠源码静态扫描不能证明是否使用全部接口。

| 数据初始化候选 | 包中材料 | 拟议结果和边界 |
| --- | --- | --- |
| `empty` | `data.formatVersion=0`，无 database/attachments 字段 | 新实例空库、无业务表、空附件；不能冒充需要业务表的应用已初始化 |
| `seed` | 版本正整数，`initialization.database` 必需、`attachments` 可选，另有数据说明 | 一致性生成的独立 SQLite DELETE 快照、可选干净附件；`user_version` 等于声明版本，完整性/外键检查通过 |

需要表但无初始记录也属于 seed 候选。如何初始化、数据版本绑定、更新仅接受相同数据版本、更新后维护结束等细则仍待单独确认；不执行安装 SQL/脚本的设计边界保留。不能将候选种子要求反向用于拒绝保全已有坏库和 WAL/SHM 现场。完整方案及未决项见[原设计](web-app-package-design.md)。

最小外部包见[示例目录说明](project-format-v1/examples/minimal-app/README.md)、[清单](project-format-v1/examples/minimal-app/local-app.json)和[静态 HTML](project-format-v1/examples/minimal-app/dist/index.html)。该示例不使用 SDK、设备、网络或业务表，仅展示文件格式；不代表当前平台能安装它，也不提供持久便签等业务功能。

## 7 路径、运行需求与身份

### 7.1 路径与内容

1. 清单路径相对所属根、使用 `/`；拒绝空路径、绝对路径、`.`/`..` 段、反斜杠、空段及控制字符。外部包只有 `web.entry` 相对网页根，其余相对包根。
2. 项目源码和产物互不包含，均不能与 `data/`、`meta/`、`.platform/` 重叠。入口在产物内；数据库和附件分别位于 `data/`，数据库不在附件树内。`meta/` 独立保存展示资料。
3. 外部包声明目录互不包含，独立声明文件不在另一声明目录内，网页入口在网页根内除外；种子库和附件可共用父目录但不能相互包含。只提供网页根内的静态资源，不公开源码、文档或数据库。
4. 真实目标与祖先不能通过软链接越界；目标文件系统的大小写/Unicode 别名、Windows 保留名及非法字符等必须另查。外部包目标还需拒绝特殊文件、嵌套挂载和多硬链接文件，且复核复制期间内容变化。

这些是编写与目标检测规则。当前受管清单主要检查相对路径组件、反斜杠、软链接和范围关系，尚未统一实施所有原始字符串、名称碰撞、硬链接检查；完整 ZIP 有独立且更严格的条目及碰撞检查。不能用 Schema 通过代替真实路径检查。HTML/CSS 中合法 `../` 可以指向网页根内资源，清单禁止点段不等于禁止所有网页相对引用。

### 7.2 网络与设备对象

两类清单复用以下对象结构：

```json
{
  "network": {
    "connect": [{
      "origin": "https://api.example.com",
      "purpose": "查询公开参考资料",
      "dataSent": "用户主动提交的查询文字"
    }],
    "images": [],
    "media": []
  },
  "devices": {
    "microphone": { "purpose": "用户点击后录制短音频，仅保存到本项目" }
  }
}
```

分组及成员可省略，网络未声明项为空，设备未声明项不启用。条目对象拒绝未知键，用途/发送说明去除首尾空白后非空且不含控制字符。同组 origin 不得重复，即使用途不同。origin 必须为规范精确 HTTPS origin，无路径、尾斜杠、凭据、查询、片段或通配；使用公共 DNS 名称而非 IP、本机或保留后缀。语义解析以当前 `RuntimePolicy` 为准；名称检查不保证 DNS 实际解析地址、可达性或 CORS。

现有 `project.json` 缺失或显式 `null` 会兼容为空策略；新的编写 Schema 建议省略或写对象，拒绝显式 `null`。外部包目标则必须有对象。声明与系统授权分开；CSP、CORS、设备硬件、浏览器兼容及实际用户授权均需运行验证。秘密 key 不放进清单、前端、数据库或包资料，首版无秘密代理。详见[能力契约](project-capabilities-and-secrets.md)。

### 7.3 不能混用的身份与版本

| 标识 | 所属与含义 |
| --- | --- |
| `project.json.id` | 平台一个本机项目实例；复制生成新 ID，恢复/迁移保留原 ID |
| `local-app.json.app.id` / `app.version` | 作者的应用族及发布版本；同应用可以安装多个独立实例，不证明作者身份 |
| `manifestVersion` / `formatVersion` | 两种文件格式分别版本化；数值相同不表示可以互相解析 |
| SDK npm 版本 / `sdkApiVersion` | 固定依赖包版本 / 接口主版本，各自兼容；不是数据库 schema 版本 |
| `data.formatVersion` | 外部包候选业务存储版本，不是平台数据库引擎版本 |
| 平台 bundle ID / 工作区根 | 平台的固定本机环境身份和位置，不由应用包决定 |
| 业务主键、业务 UUID | 项目业务数据；复制项目不全库替换这些值 |
| documentEpoch、维护 ID、卷 UUID/inode、identity binding | 平台本机控制和保护证据，不是作者清单字段，不随应用包指定或复制授权 |

卷身份修复不改变项目格式。`identity-binding.json` 和固定根修复收据不是应用发行材料；公开工作区迁移按[身份规则](workspace-identity.md)处理，不按手工拷贝控制记录认领新根。

### 7.4 是否要求 PWA：当前不设必填，兼容仍待评估

PWA（Progressive Web App）通过网页技术提供接近应用的体验。Web App Manifest 描述浏览器安装后的名称、图标、启动地址和显示模式；Service Worker 可以另行实现请求处理与离线缓存。浏览器的可安装性与离线能力需要分别判断，Service Worker 并非当前所有安装方式的前提。参见 [W3C Web App Manifest](https://www.w3.org/TR/appmanifest/)、[MDN 可安装性](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable)与[Service Worker](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API/Using_Service_Workers)。

当前 project v1 与外部包目标均没有 PWA 必填条件。建议继续以可在平台服务下运行的静态网页为准入要求，把 PWA 作为后续可选增强；这是本次问题的取舍建议，不表示用户已经确认完整 PWA 协议或当前平台已经验收 PWA 兼容。普通网页没有 Web App Manifest 或 Service Worker，不应仅因此被当作格式不合格。

| 方面 | PWA 的潜在收益 | 本平台的成本与边界 |
| --- | --- | --- |
| 浏览器安装 | 支持的浏览器可提供图标与独立窗口 | 桌面宿主已有项目窗口；浏览器安装入口和系统支持仍需分别验收 |
| 离线与加载 | 可缓存网页资源、减少重复加载、展示离线页面 | 当前资源已经位于本机；Service Worker 的缓存更新、清退和旧文档协调增加复杂度 |
| 数据 | 可以设计离线交互 | 缓存页面不提供 Rust/SQLite 服务；平台关闭后不能宣称数据库写入已保存；浏览器私有数据不自动进入项目备份 |
| 更新 | 可以设计分代缓存和更新提示 | 要与平台已有发布代、documentEpoch、维护、恢复和用户草稿协调，不能让旧缓存长期冒充当前程序 |
| 入口与作用域 | 固定网站地址适合浏览器安装 | 本平台端口可变化、会话需重新建立；安装地址、origin、Service Worker scope、缓存归属都必须另设适配 |

四类清单各有对象：`project.json` 管受管实例，`local-app.json` 管分发包，源码 `package.json` 管依赖/构建，PWA 的 `manifest.webmanifest`（文件名可另选）供浏览器读取。它们不能互相替代。PWA manifest 可以位于网页产物中，但本轮不新增 PWA 字段、Service Worker 模板或安装检测实现。

源码核对还表明：平台优先复用旧端口，冲突时另选端口；网页位于项目路径而注入基址指向不可变发布代，相对 manifest、`sw.js`、图标和启动路径可能需要适配。当前响应为 `no-store`，会话在服务内存中，HTML 带当前文档代次和策略信息；这些事实不构成对任意项目 Service Worker 的注册/缓存兼容承诺。上述结论来自[本地服务](../crates/platform-core/src/service.rs)与[发布代实现](../crates/platform-core/src/builds.rs)核对，未注册 Service Worker 或运行 PWA 测试。

后续若支持项目自带 Service Worker，需要至少验收：每项目及发布代的缓存命名与作用域；禁止跨项目拦截或清掉其他项目缓存；不缓存入口 ticket、会话、管理接口及敏感 API 响应；不自动排队重放 SQL/附件写入；旧缓存与维护/退出/恢复状态一致；动态端口、更新和卸载后的重新连接。服务器仍是数据保存结果和代次校验的权威，离线页面不能伪造保存成功。这里是基于当前架构的兼容要求分析，不声称已经通过这些检查；完整兼容与是否提供平台标准 PWA 模式另行设计。

## 8 JSON Schema 的使用与边界

配套文件采用 [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/json-schema-core)，编写工具需要支持该方言：

| 文件 | 角色 |
| --- | --- |
| [project.schema.json](project-format-v1/project.schema.json) | 当前开发项目的编写检查：保留已有必填字段和扩展字段，并明确兼容读取差异 |
| [local-app.schema.json](project-format-v1/local-app.schema.json) | 待实现外部包的目标结构；包含明确标注的初始化候选，不作为已批准安装行为 |
| [最小项目清单](project-format-v1/examples/minimal-project/project.json) | 可以通过 project Schema；不是完整运行目录 |
| [最小应用包](project-format-v1/examples/minimal-app/local-app.json) | 可以通过目标 Schema；只有静态 HTML，不假装已完成安装 |

Schema 检查必填、类型、已知枚举、部分路径/文本/版本结构和条件字段；它不会创建文件、运行脚本、安装依赖或初始化数据库。本轮没有把 Schema 接入 Rust/CLI/GUI。`$id` 是模式标识，不表示平台自动联网；可下载文件后离线验证。不要将 Schema 的 `$schema` 属性直接插入严格应用清单；可在编辑器配置中关联对应文件。

project Schema 是面向新编写的规范，不是当前 Rust 解析器的逐行为复制：比兼容读取更严格的项包括显式 null 运行需求、非字符串/坏图标、原始路径歧义，以及构建阶段才严格检查的脚本名。已存在的文件被 Schema 报错，不授权自动修改或删除它，仍按现有恢复与兼容规则保全。

以下事项不能仅靠 Schema 证明：重复 JSON 键/UTF-8 原始字节、文件存在和类型、软/硬链接、路径包含关系和身份、跨平台名称碰撞、PNG 解码、ZIP 摘要、SQLite 独立性与完整性、网络 origin 完整语义、按 origin 去重、能力/版本环境比较、源码与产物对应、网页路由及业务运行。URI 的 `format` 在标准默认词汇中不保证作为断言执行，不能以其取代专用校验，见[验证词汇](https://json-schema.org/draft/2020-12/json-schema-validation)。

## 9 校验顺序与失败处理

| 层级 | 当前开发项目 / 完整 ZIP | 外部包目标（未实现） |
| --- | --- | --- |
| 识别与解析 | 读取 `project.json`；ZIP 读取 `backup.json + project/` | 用户选择包根；读取根部 `local-app.json`，不猜目录 |
| 结构 | 当前 Rust 字段/版本校验；文档 Schema 可独立检查 | 严格 JSON、目标 Schema、格式及运行类型 |
| 文件与身份 | 登记数据库、真实路径、ZIP 条目/摘要及资料校验 | 声明范围、链接/碰撞、预算、暂存复制和复核 |
| 环境和数据 | API、维护/会话、SQLite 与现有来源记录 | API/平台能力、网页策略、候选初始化检查 |
| 发布 | 持久操作日志后登记；构建失败保留旧产物 | 检测通过后用户确认，持久日志和新实例提交 |
| 实际使用 | 网页、SQL、附件和受影响功能分别验证 | 安装完成后主动打开验证；不能用静态通过代替 |

当前网页产物检查是有限扫描，不是完整 HTML/CSS/JS 语法分析器；不能把现有构建检查当作外部包检测器已完成。新检测要与真实静态服务和 CSP 共用规则，区分导航、资源、JSON 数据脚本及可执行脚本；动态引用、外部服务、设备与浏览器保留运行未验证项。

外部检测的目标结果为 `passed`、`blocked`、`incomplete`：分别表示基础检查通过、确定阻断、尚未完成；运行尚未验证单独展示。任务状态仍为 queued/running/succeeded/failed/interrupted。项目维护、未构建、来源是否匹配和检测状态不互相替代；无源码安装类型不得伪造构建成功或来源 matched。

| 示例问题 | Schema 能否发现 | 后续校验 / 当前行为 |
| --- | --- | --- |
| 缺 `entry`、错字段类型、版本值不符 | 能（取决于对应 Schema） | 明确字段位置，不补猜入口 |
| 项目清单未知展示扩展 | 不阻断 | 现有保留，不赋予新执行语义 |
| 外部清单拼错 `web.entyr` 或未知字段 | 能 | 目标检测阻断，当前无安装接口 |
| `../data`、绝对路径或反斜杠声明 | 能识别常见字符串形式 | 还要实际解析边界及目标身份 |
| 入口在另一个目录、文件不存在、源码/数据重叠 | 不完整或不能 | 文件与关系校验，不能以 JSON 合法放行 |
| 两项用途不同但 origin 相同、IP/保留域名 | 不完整 | `RuntimePolicy` 等专用语义校验 |
| 入口只引用 TSX、缺 CSS、CSP 冲突 | 不能 | 目标静态检测；当前有限构建检查不等同此能力 |
| 数据库损坏、WAL 未快照、数据版本不符 | 不能 | 按操作类型诊断和保全；不能直接丢弃伴随文件 |
| 浏览器拒绝摄像头、断网、远端 CORS | 不能 | 实际运行；提示并保留用户已有数据 |

当前错误继续使用 `INVALID_PROJECT_MANIFEST`、`UNSUPPORTED_PROJECT_VERSION`、路径、构建、归档等实际错误；目标 `PACKAGE_*` 错误族见[设计稿第 6 节](web-app-package-design.md#6-错误与提示规范)，尚未成为公开 API。未来诊断应带字段/文件位置、检查阶段、确定性及修复办法，不能只显示“格式错误”，也不能把读取失败显示为零占用或检测通过。

## 10 数据保留、分享与兼容

完整项目归档携带当前用户数据库/附件、源码、网页、文档和快照状态，不是默认公开分享包。备份与恢复按现有一致性快照、受保护现场、维护和代次规则执行；不用“补齐格式”为由重置用户库。依赖/缓存排除按已识别范围，不按名称递归丢掉用户附件。

分享应用默认携带程序与作者明确选择的干净初始资料。目标安装后产生独立项目 ID、数据库和附件；原包只读。复制实例复制当前数据，恢复取归档时点，不重新播种。未来更新也不得把初始库覆盖当前用户数据；更新精确规则仍为候选。

未来无源码安装类型需要独立内部项目格式/类型及备份兼容，不能给当前 `project.json` 虚填 sourceDir/build、伪造来源记录，或要求维护结束时构建不存在的源码。内部路径映射、归档格式和正式 API 在实施前另定；旧平台不理解新类型应明确拒绝，不当作现有开发项目读取。

平台升级继续保留旧项目的源码、固定 SDK、锁文件、文档、数据库、附件、维护状态、备份、回收站和恢复证据。未来 app 包格式不会自动使 Windows 运行时开放、M5/M6 验收完成或旧项目需要批量升级。

## 11 已实现、待实现和仍待确认

| 范围 | 状态 |
| --- | --- |
| current project v1 识别、默认新建、元数据、构建、数据库/附件、完整 ZIP 与生命周期 | 已实现，各阶段实际验收和限制仍以原记录为准 |
| 两类目录角色、专用清单分工、集中规范及 Schema/最小示例 | 本轮文档交付；Schema 未接入程序 |
| local-app 包解析/检测、暂存范围、确认安装、新实例登记、无源码类型、来源、备份/维护兼容 | 待实现；不能调用或声称已经安装通过 |
| 静态网页路由/完整资源检测、无源码无 Node 运行、跨机器 GitHub 分享与真实验收 | 待实现与验证；源码可移植不等于所有系统已支持 |
| PWA 安装、manifest、Service Worker 与平台会话/发布代兼容 | 当前不作为格式必填；可选增强为本次建议，完整兼容与真实浏览器安装未验收 |
| empty/seed、`user_version` 绑定、同数据版本更新、更新后显式结束维护 | 仍为具体设计候选，需确认再实现；Schema 只记录候选结构 |
| 安装清单、来源及路径映射的内部格式、归档版本与公开 CLI/API | 待定稿，不新增现有项目必填或伪路由 |
| 检测预算、计划 TTL、包大小及文件数、更新失败提交点和恢复交互 | 仍待确定/落实；不把建议数值变成正式限额 |

本轮不扩展 M0–M6 或 Windows W1–W4 的完成状态。实施时另列任务、按依赖与公共迁移/维护流程落实；媒体、系统权限、浏览器、最低系统和发行仍需真实验收。

## 12 本轮文档检查

2026-10-19 完成以下检查：

- 使用隔离环境中的 `jsonschema 4.23.0 / Draft202012Validator`，两份 Schema 均通过 2020-12 元模式检查；两个最小清单及字段/条件/路径/版本等共 164 个有效与无效用例通过。检查覆盖未知字段分工、缺失字段、empty/seed 候选关系、API 条件、点段/驱动器/反斜杠、控制字符和末尾换行、版本上限等。发现的路径控制字符及末尾换行遗漏已修正。
- 用 Node 24.14.1 编译全部 28 个正则，确认 ECMAScript 语法可用；正文最小项目 JSON 与独立文件一致，正文运行需求对象通过两份 Schema 对应定义。
- 最小外部包声明的 HTML 和 README 实际存在；HTML 为 UTF-8 纯静态文字，无脚本、内联事件和远程资源。此项仅检查说明性文件，未安装或用浏览器运行它。
- 10 份相关 Markdown 的 195 个本地链接及 1 个锚点检查通过，`git diff --check` 通过。独立复核已修正项目 `.platform/` 资料描述，并确认已实现/候选/未实现与原数据保护边界一致。

私有验证脚本、隔离依赖和结果保留在忽略的 `.test-artifacts/project-format-v1-validation*` 下，不进入 Git。本轮只提交文档、Schema 和说明性示例；未改运行代码、工程依赖或锁文件，未启动服务、注册 Service Worker、创建项目或访问既有开发资料。没有新 `.app`/DMG、外部安装或 PWA 真实运行验收；原里程碑及设备/系统限制保持。

依据：[主开发契约](platform-development-contract.md)、[生命周期](project-lifecycle-and-workspace.md)、[备份](backup-and-maintenance.md)、[能力声明](project-capabilities-and-secrets.md)、[外部包原设计](web-app-package-design.md)。实现核对位置为 [ProjectManifest/清单](../crates/platform-core/src/projects.rs)、[构建](../crates/platform-core/src/builds.rs)、[归档](../crates/platform-core/src/backups.rs)、[运行声明](../crates/platform-core/src/runtime_policy.rs)及[基本信息](../crates/platform-core/src/metadata.rs)。
