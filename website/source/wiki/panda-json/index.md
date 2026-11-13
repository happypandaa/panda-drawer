---
title: panda.json 完整说明
description: panda.json v2 的字段、配置、本地化、agent 标记和三个完整示例。
---

# panda.json 完整说明

更新于 2026-11-13。依据 [平台清单说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/panda-json.md)、[JSON Schema](https://github.com/happypandaa/panda-drawer/blob/develop/schemas/panda.schema.json)、[清单校验代码](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/panda_manifest.rs)和[配置字段校验](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/panda_manifest/configuration.rs)。

`panda.json` 告诉平台应用叫什么、从哪个网页打开，以及是否有后台或 agent 规则。把文件放在应用根目录，与代码一起保存。

## 位置与字段

- 使用 UTF-8 JSON。`manifestVersion` 固定为 `2`，旧清单需要按当前格式修改。平台拒绝 `runtime`、未知字段、重复键、非法类型和显式 `null`。
- 清单声明应用的结构和配置项。实例 ID、归属、已填写的配置值、密钥和日志由平台登记。
- 项目路径用 `/` 分隔。入口相对各自的根目录，其他路径相对应用根目录。不得使用绝对路径、越界路径或指向外部的链接。网页根不能指向 `data/`、`node_modules/` 或隐藏内容。
- 修改后用共享 Schema 校验。文件存在、路径边界和字段间引用还须通过平台检查。

| 字段 | 要求 | 内容 |
| --- | --- | --- |
| `manifestVersion` | 必填 | `2` |
| `$schema` | 选填 | 编辑器提示，平台不联网读取 |
| `app.name` | 必填 | 平台卡片和窗口名称 |
| `app.version`、`app.description`、`app.icon` | 选填 | 版本、简介、应用目录中的 PNG 图标路径 |
| `web.root`、`web.entry` | 必填 | 网页目录可为 `.`，首页相对该目录 |
| `backend` | 选填 | 声明即有后台。填写 `root`、`entry`、`nodeMajor: 24`、`protocolVersion: 1` |
| `backend.apiPrefixes` | 选填 | 业务接口完整段前缀。省略时为 `/api`、`/tts`，`[]` 不代理业务接口。禁止根路径、`/_platform` 开头的前缀、编码或点路径 |
| `platform.sdkApiVersion` | 选填 | 固定为 `1`。有无后台均可启用 SDK |
| `development` | 选填 | `sourceDir` 默认 `.`，`build.kind` 为 `npm`，`build.script` 为已有的 npm 脚本。只构建网页 |
| `configuration` | 选填 | 用户配置字段。声明 `backend` 后才允许 `secret: true` |
| `documentation` | 选填 | `readme`、`development`、`database`，声明的文件须存在 |
| `agent.instructions` | 选填 | 指向项目内存在的非隐藏普通规则文件，例如 `AGENTS.md` |
| `window` | 选填 | `width`、`height` 都必填。`rememberSize` 默认 `true` |
| `runtimeRequirements` | 选填 | 网页请求的外部地址和设备用途。后台网络请求独立于网页策略 |
| `localization` | 选填 | 按 `zh-CN`、`en` 覆盖平台展示文字 |

缺网页产物时，平台根据构建声明判断是否需要准备。后台入口与依赖须预先准备。`agent.instructions` 还须避免空路径段、`.`、`..`、反斜杠、冒号和隐藏目录。声明 agent 与声明后台相互独立。平台用 `isAgent` 展示标识，实际文件访问仍按清单和路径边界检查。

## 用户配置

`configuration.version` 固定为 `1`，`fields` 最多 32 项。

| 字段 | 要求 | 内容 |
| --- | --- | --- |
| `name` | 必填 | 唯一的环境变量名，以大写英文字母开头，只含大写字母、数字和下划线，避开平台与系统保留名 |
| `label` | 必填 | 表单中的名称 |
| `type` | 必填 | `string`、`url`、`integer`、`boolean` 或 `select` |
| `required` | 必填 | 用户是否必须填写 |
| `secret` | 必填 | 秘密配置只允许 `string`，并要求声明 `backend` |
| `description` | 必填 | 配置用途 |
| `howToGet` | 必填 | 1–12 个非空字符串组成的获取步骤 |
| `placeholder` | 选填 | 输入提示 |
| `default` | 选填 | 与类型匹配，密钥禁止默认值。`url` 默认值须为有效 HTTPS 地址，`select` 默认值须来自选项 |
| `helpUrl` | 选填 | 不含账号、查询和片段的 HTTPS 地址 |
| `options` | `select` 必填 | 1–32 个 `value`、`label` 对，`value` 不重复。其他类型禁止声明 |
| `endpointField` | 选填 | 秘密字段可引用同一声明中的非秘密 `url` 字段，关联服务地址与密钥 |

用户通过平台表单填写。网页从 `/_platform/config.json` 或 `panda.config()` 获取公开值，字符串、数字和布尔值保留类型，未填写的可选项省略。后台启动时按字段 `name` 接收配置环境变量，密钥只注入对应后台。

## 平台展示文字与应用语言

| 可翻译对象 | 引用方式与允许字段 |
| --- | --- |
| `app` | `name`、`description` |
| `configuration` | 按原字段 `name`，可写 `label`、`description`、`placeholder`、`howToGet`、`options`。选项按原 `value` 替换标签 |
| `runtimeRequirements.network` | 按 `connect`、`images`、`media` 和原 `origin`，可写 `purpose`、`dataSent` |
| `runtimeRequirements.devices` | 已声明的 `camera`、`microphone` 可翻译 `purpose` |

- `localization` 按 `zh-CN`、`en` 分组，逐字段覆盖，缺译回原字段。`howToGet` 替换整个非空步骤数组。无效引用、空文字、显式 `null` 和未知键会被拒绝。
- 覆盖仅用于平台管理界面及原生窗口标题。配置值、默认值、权限、路径和 SDK 返回的原始应用信息都以原声明为准。
- 平台语言即时更新已读取的展示文字。应用内部可跟随 `navigator.language` 或提供自己的设置。平台不通过 SDK、环境变量或通知传递语言设置。改动清单文件后仍须重新加载。

## 修改后重新加载

- 准备好文件，保存未提交的输入，再点标题栏「重新加载」或运行 `app reload --app <id>`。应用尚未打开时，这条命令会打开它。
- 清单无效或缺少必填配置时，平台保留正在运行的实例。启动切换失败会显示错误，修好再重新加载。
- 添加后台时填写 `backend` 并准备运行文件。移除后台时同时去掉所有秘密配置声明，切换成功后平台删除旧密钥，失败时保留。
- 增减后台使用同一应用 ID、地址、文件夹、`data/`、公开配置和浏览器数据。成功切换使旧会话失效。

## 示例一　最简网页

`index.html` 位于应用根目录。

```json
{
  "manifestVersion": 2,
  "app": { "name": "番茄钟" },
  "web": { "root": ".", "entry": "index.html" }
}
```

## 示例二　使用 SDK 和网页构建的 agent 应用

根目录包含 `AGENTS.md`。`package.json` 的 `build` 脚本生成 `dist/index.html`，项目带匹配的 npm 锁文件。

```json
{
  "manifestVersion": 2,
  "app": { "name": "读书便笺" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": {
    "sourceDir": ".",
    "build": { "kind": "npm", "script": "build" }
  },
  "agent": { "instructions": "AGENTS.md" }
}
```

## 示例三　有后台、密钥配置和英文展示文字

后台运行文件位于 `backend/server.mjs`，网页位于 `web/index.html`。用户在平台中填写自己的密钥。

```json
{
  "manifestVersion": 2,
  "app": { "name": "朗读助手", "version": "1.0.0" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": {
    "root": "backend",
    "entry": "server.mjs",
    "nodeMajor": 24,
    "protocolVersion": 1,
    "apiPrefixes": ["/v1"]
  },
  "platform": { "sdkApiVersion": 1 },
  "configuration": {
    "version": 1,
    "fields": [
      {
        "name": "SERVICE_URL",
        "label": "服务地址",
        "type": "url",
        "required": true,
        "secret": false,
        "description": "接收朗读请求的服务地址。",
        "howToGet": ["按服务商说明填写 HTTPS 接口地址。"],
        "default": "https://api.example.com"
      },
      {
        "name": "SERVICE_API_KEY",
        "label": "服务 API Key",
        "type": "string",
        "required": true,
        "secret": true,
        "description": "调用朗读服务的个人密钥。",
        "howToGet": ["在服务商控制台创建 API Key。"],
        "endpointField": "SERVICE_URL"
      },
      {
        "name": "VOICE",
        "label": "朗读风格",
        "type": "select",
        "required": false,
        "secret": false,
        "description": "选择应用提供的朗读风格。",
        "howToGet": ["选择喜欢的风格。"],
        "default": "calm",
        "options": [
          { "value": "calm", "label": "平静" },
          { "value": "bright", "label": "明快" }
        ]
      }
    ]
  },
  "localization": {
    "en": {
      "app": { "name": "Read Aloud" },
      "configuration": {
        "SERVICE_URL": { "label": "Service URL" },
        "VOICE": { "label": "Voice", "options": { "calm": "Calm", "bright": "Bright" } }
      }
    }
  }
}
```

[平台规则总览](/wiki/platform-rules/) · [应用运行方式](/wiki/app-types/) · [数据与配置](/wiki/data-and-config/) · [添加要求](/wiki/adding-apps/)
