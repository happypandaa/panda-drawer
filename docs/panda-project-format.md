# 项目格式：panda.json

更新：2026-11-10。本文描述统一应用清单 v2；统一模型范围与验收要求见[统一应用模型任务](tasks/2026-11-09-unified-app-model/task.md)，AI 操作以随安装提供的 [skill](../skills/local-projects-app/SKILL.md) 为准。

## 一个项目文件夹，一份清单

项目根必须有 UTF-8 `panda.json`，使用 `manifestVersion: 2`，不含 `runtime`。平台只有一种应用：网页必有，Node 24 后台和 agent 规则可选；是否启动后台只看 `backend`。`app.name` 必填，`app.version`、`app.description`、`app.icon` 可选。图标为项目内 PNG；无效图标使用默认展示，不影响有效项目运行。没有作者应用 ID、本机实例 ID、数据库路径或平台私有目录字段。`package.json` 仅描述依赖及脚本，不能代替应用清单。

平台读取清单并生成唯一 UUID 的实例登记，位于平台数据根的 `apps/<uuid>/`，不把登记、配置、密钥、日志、构建缓存或任务控制文件写入项目。源码、Git、网页和文件数据共用项目这一个文件夹；不保存程序副本或网页产物代次。v1 清单不添加、不打开，提示让 AI 按平台说明更新；不自动转换旧清单或迁移旧登记。

完整字段及禁止未知字段的校验以共享 [JSON Schema](../schemas/panda.schema.json) 和解析器为准。开发交接提供实际 Schema 和 CLI 路径。

## 平台与应用的语言

平台与应用分别管理语言。平台首次使用操作系统首选界面语言：`zh` 开头使用 `zh-CN`，其他使用 `en`；手动选择后以保存的选择为准。应用内部可跟随系统或提供自己的语言设置，平台不通过 SDK、环境变量或事件绑定应用语言。

清单可选的 `localization` 为平台展示文字提供翻译，例如：

```json
"localization": {
  "en": {
    "app": {"name": "Reading Notes", "description": "Keep your reading notes"},
    "configuration": {
      "API_BASE": {
        "label": "API address",
        "description": "The service endpoint",
        "placeholder": "https://api.example.com",
        "howToGet": ["Copy the address from the service console."]
      },
      "QUALITY": {"options": {"high": "High"}}
    },
    "runtimeRequirements": {
      "network": {"connect": {
        "https://api.example.com": {"purpose": "Look up information", "dataSent": "Your search text"}
      }},
      "devices": {"microphone": {"purpose": "Record a voice note"}}
    }
  }
}
```

语言键只允许 `zh-CN`、`en`。`app` 可覆盖 `name/description`；`configuration` 按已有字段 name 引用，可覆盖 `label/description/placeholder/howToGet`，`options` 按已有选项 value 覆盖 label；网络按 `connect/images/media` 分组和已有 origin 引用，可覆盖 `purpose/dataSent`；设备可覆盖已声明的 `camera/microphone.purpose`。

当前语言缺哪个字段就回退哪个原字段；howToGet 整数组替换，options 按 value 逐项覆盖。无效引用、未知键、错误类型、显式 null、空字符串或空步骤数组报错，长度约束与原字段一致。翻译不修改配置值、默认值、必填性、类型、权限、路径或 origin。

登记和检查结果保留原始文字及翻译数据，平台卡片、添加预览、详情、配置表单和原生窗口标题在渲染时按平台语言取值，切换语言立即生效。修改清单文件本身仍按重新加载流程生效。此字段与 SDK 开关无关，不影响应用界面、业务内容、自带 AI 提示词或 SDK 返回的原始应用信息。推荐列表独立使用 catalog 的可选 `i18n` 文案，已添加应用使用自身清单。

## 网页与 SDK

无需平台能力的普通网页最小声明：

```json
{
  "manifestVersion": 2,
  "app": { "name": "我的网页" },
  "web": { "root": ".", "entry": "index.html" }
}
```

`web.entry` 相对 `web.root`。可采用普通 HTML、现成网页或构建后的前端；不强制 React、PWA、构建声明或平台 SDK。`platform: {"sdkApiVersion": 1}` 启用当前 SQL／附件 API，有无后台均可使用；省略后不能调用这些接口，也不会因此创建数据库。

需要构建的项目可声明：

```json
{
  "manifestVersion": 2,
  "app": { "name": "构建网页" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": {
    "sourceDir": ".",
    "build": { "kind": "npm", "script": "build" }
  }
}
```

构建在原项目的 `development.sourceDir`（省略为 `.`）中执行受管 `npm ci`，再执行 `npm run` 指定的 `development.build.script`。包声明和有效 npm v2/v3 锁文件必须匹配。脚本负责生成所声明入口，平台不追加 `--outDir`，不复制工程、不计算新旧匹配状态、不保存旧产物或回滚脚本改动。失败可能留下脚本已修改的文件。源码和数据版本由作者自己的 Git 和备份方式管理。

构建前关闭应用窗口；窗口打开时构建被拒绝。构建结束后重新打开，直接服务原目录文件。缺入口但有构建声明时显示 `needsBuild`；无构建声明则显示 `entryMissing`，均可完成本地登记。添加、只读检查和打开不隐式构建。

每个实例使用固定端口，网页从根路径运行；地址被占用时明确报错，不静默换端口。HTML 通过普通 `<script src="/_platform/sdk.js"></script>` 获得 `window.panda`，API 固定 `/_platform/api/v1/`，不解析 URL 中的 ID。类型声明随资源，交接给出实际 `sdkTypesPath`；模板已有 `src/panda.d.ts`，不再有 vendor／tgz／SDK 依赖或 Vite base。可选 `runtimeRequirements` 只约束网页的网络和设备使用，有后台时同样可声明，不约束后台联网。

`panda.context.get().projectRoot` 返回当前应用根目录的绝对路径，采用系统原生格式，可用于生成交给 AI 的工作目录提示；不返回工作区或其他应用的路径。context 仍按现有网页 SDK 开关和会话规则访问。

## agent 应用

```json
"agent": { "instructions": "AGENTS.md" }
```

`agent` 是可选的工作方式标记，与有无后台无关，不自动启用 SDK。`agent` 存在时只接受必填的字符串 `instructions`，不接受 `null`、未知字段或空对象。规则文件相对项目根，须存在且为项目内非隐藏普通文件；禁止隐藏路径、绝对路径、`..`、空段、反斜杠和冒号，链接不得指向项目外或隐藏目标。缺失时检查报 `agent.instructions` 错误并提示补齐文件。

应用根目录也是 agent 工作目录，通常在根目录放 `AGENTS.md`。日常任务只按规则读写业务数据；修改界面、功能或表结构时，另按应用修改流程同步修改程序和 `docs/database.md`，删除或清空数据前须取得用户同意。详情和列表的 `isAgent` 是标识及提醒用的提示元数据，无法读取项目时可保留上次有效结果；它不证明清单有效，也不授予路径访问或操作权限。

agent、SDK 与后台可共用 `data/main.sqlite`，每个连接先设置 5000 ms 锁等待。开始状态单独提交；搜索、下载、缩图期间不持有写事务，成果和最终状态在一个短 `BEGIN IMMEDIATE ... COMMIT` 事务提交，失败状态单独记录。长写事务会导致界面保存失败。agent 不自行建库建表，不执行 `PRAGMA journal_mode`、`VACUUM`，不替换数据库或 WAL／SHM，不使用 `INSERT OR REPLACE`，并遵守列的写入分工。详细流程以随安装提供的 [agent 说明](../skills/local-projects-app/references/agent-apps.md)为准。

外部写入 `data/attachments/` 的普通附件可直接通过 SDK 的 `list`、`get`、`url`、`remove` 使用，安全检查不变。外部数据库和附件写入没有 `data.changed` 通知，应用须重新查询；平台空间统计也要重新扫描，缓存的 `stale: false` 不保证包含外部变化。数据库并发和外部附件已用 macOS 一次性真实应用窗口验证，Windows agent 运行未验收。

平台不调起 agent、不读取会话、不定时、不监听外部文件变化、不转发消息。macOS 从推荐列表添加 agent 应用时可沿用当前工作区的 `project-sources/` 默认布局，或选择一个空文件夹作为应用根目录本身。默认下载归平台所有，选定文件夹归用户所有；失败只清理本次可核对的写入，保留用户根目录。普通推荐、粘贴链接及 Windows 保持默认下载流程；已下载的目录仍从“本机文件夹”添加。删除前用户须关闭外部 AI 工具中的定时任务。

## 可选后台

```json
{
  "manifestVersion": 2,
  "app": { "name": "我的服务" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": { "root": "backend", "entry": "server.mjs", "protocolVersion": 1, "nodeMajor": 24 }
}
```

前后端入口和运行依赖须提前准备；后台是固定 Node 24 可直接执行的 JavaScript，遵守 initialize／ready、健康检查、ping／pong、shutdown／stopping 协议。stdout 只输出协议消息，普通日志写 stderr。`backend.apiPrefixes` 声明网页转发到后台的业务前缀，省略时为 `/api`、`/tts`；任何以 `/_platform` 开头的前缀均禁止，平台保留这些 SDK、配置和会话接口。

`development`、`platform.sdkApiVersion`、`documentation.database`、`runtimeRequirements` 有无后台均可声明。`development.build` 只构建网页，不准备后台运行文件。本地添加不安装依赖，启动不构建。协议和约束见 [skill](../skills/local-projects-app/SKILL.md)。

## 重新加载

修改清单加上或去掉 `backend` 后，运行 `app reload --app <id>` 或点窗口上的「重新加载」。reload 保留 UUID、地址、窗口、浏览器存储、文件夹、`data/` 和公开配置；应用未打开时按新清单打开。清单无效或新增必填配置未填时，旧实例继续运行；缺配置时打开主窗口配置表单，填好后再次 reload。

预检通过后停止旧服务并启动新实例。启动失败明确显示原因和日志，保留原登记与密钥，不回退代码。新实例成功后才提交新状态；去掉后台时仅在切换成功后删除旧密钥并提示一次。删除、维护或构建期间拒绝 reload。不要通过删除重加切换后台。

## 配置、文档及数据

`configuration` 使用 `version: 1` 与 `fields`，声明变量用途及获取方法；`howToGet` 是非空字符串数组。真实值由平台表单保存；只有声明 `backend` 的应用才允许 `secret: true`，秘密仅注入本应用后台，不写入清单或网页。网页通过 `GET /_platform/config.json` 或 `window.panda.config()` 获取公开配置的裸类型化对象；整数为 number，开关为 boolean，其他值为 string，选填未填项省略。配置读取不依赖 SDK 开关，保存后用 reload 按最新配置运行，不需构建。`documentation.readme`、`documentation.development`、`documentation.database` 可选，声明后文件必须存在。可选 `window` 控制初始窗口尺寸及记忆，详见 Schema。

文件数据在项目 `data/`：启用 SDK 的应用按需创建 `data/main.sqlite` 及 `data/attachments`，后台通过 `APP_DATA_DIR` 写入。检查、登记和无数据操作的普通网页不会提前生成数据库。localStorage、IndexedDB、Cookie 是平台窗口的浏览器数据，与项目文件分开。

删除按平台登记的 `ownership` 处理：用户目录保留代码、Git 和 `data/`，平台所有目录整根删除。用户选择的本地目录，包括在空目录中生成的起始项目，归用户；当前平台新建和默认下载的目录归平台，不单凭是否来自 Git 判断归属。两类实例的私有配置、密钥、缓存、日志、任务和浏览器数据均删除。文件数据和浏览器数据在删除提示中分别说明。

## 路径和接入

路径必须在项目内。网页根按解析符号链接后的真实位置检查，不能等于或位于 `data/`、`node_modules/`、隐藏目录中；不能提供 `panda.json`、隐藏文件或共享硬链接。静态请求逐级核对边界和真实文件身份。清单通过不证明网页所有脚本或业务安全。

[本地目录添加](local-directory-onboarding.md)只读检查后登记原目录；[Git 添加](git-app-import.md)准备平台独占源码，网页入口缺失且有构建声明时先获得明确构建确认。平台不提供项目 ZIP 导入导出、复制、源码／数据版本及取回。
