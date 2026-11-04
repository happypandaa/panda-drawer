---
title: panda.json 完整说明
description: panda.json 的字段、路径、配置项、生效时机和三个完整应用示例。
---

# panda.json 完整说明

更新：2026-11-04。来源：[panda-json.md](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/panda-json.md)、[JSON Schema](https://github.com/happypandaa/panda-drawer/blob/develop/schemas/panda.schema.json)、[配置字段校验](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/panda_manifest/configuration.rs)。

这份文件告诉熊猫小抽屉：应用叫什么、从哪里打开、需要用户填写什么。AI 修改应用时要保留正确格式。

## 位置和基本规则

- 文件位于项目根目录，名称固定为 `panda.json`；使用 UTF-8 JSON，不能有重复键或未知字段。
- `manifestVersion` 固定为 `1`，`runtime` 为 `web` 或 `node`。
- 文件跟着应用代码走。不要写本机实例 ID、真实配置值、密钥、数据库位置或平台私有目录。
- `package.json` 描述依赖和脚本，不能代替 `panda.json`。
- 不要修改已登记应用的 `runtime`；换类型要删除原登记后重新添加，并先核对[删除范围](/wiki/data-and-config/)。
- 完成修改后用[共享 Schema](https://github.com/happypandaa/panda-drawer/blob/develop/schemas/panda.schema.json)校验。文件存在、路径边界、配置间引用等规则还要通过平台实际检查。

## 字段表

| 字段 | 网页 `web` | Node `node` | 含义 |
| --- | --- | --- | --- |
| `manifestVersion` | 必填 | 必填 | 固定 `1` |
| `$schema` | 选填 | 选填 | 编辑器校验入口，平台忽略该值 |
| `runtime` | 必填 | 必填 | `web` 或 `node` |
| `app.name` | 必填 | 必填 | 卡片和窗口中的应用名称 |
| `app.version` | 选填 | 选填 | 作者版本号 |
| `app.description` | 选填 | 选填 | 应用简介 |
| `app.icon` | 选填 | 选填 | 项目内 PNG 图标路径 |
| `web.root`、`web.entry` | 必填 | 必填 | 网页根目录及相对该根目录的首页；根目录可为 `.` |
| `backend` | 不允许 | 必填 | `root`、`entry`、`nodeMajor: 24`、`protocolVersion: 1` |
| `backend.apiPrefixes` | 不允许 | 选填 | 后端业务接口前缀数组；省略为 `/api`、`/tts`；`[]` 不代理业务接口 |
| `platform.sdkApiVersion` | 选填 | 不允许 | 固定写 `1` 才启用 SDK 的数据库和附件能力 |
| `development` | 选填 | 不允许 | `sourceDir` 默认 `.`；`build.kind` 为 `npm`，`build.script` 为已有 npm 脚本名 |
| `configuration` | 选填 | 选填 | `version: 1` 与 `fields`；网页禁止秘密字段 |
| `documentation` | 选填 | 选填 | `readme`、`development`；网页另可写 `database`；声明的文件必须存在 |
| `window` | 选填 | 选填 | `width`、`height` 都要写；`rememberSize` 默认 `true` |
| `runtimeRequirements` | 选填 | 不允许 | 网页外部连接和设备用途声明，见[运行环境](/wiki/app-types/) |

`backend.apiPrefixes` 按完整路径段匹配。不要声明根 `/`、`/_platform` 或带转义、越界段的路径；接口前缀必须与实际后端路由一致。

## 路径规则

- 路径使用 `/`，相对项目根；`web.entry` 相对 `web.root`，`backend.entry` 相对 `backend.root`。
- 不要使用绝对路径、`..` 或经符号链接指向项目外的路径。
- `web.root` 不能等于或位于 `data/`、`node_modules/`、以 `.` 开头的目录。
- 网页缺少入口但有构建声明时，状态为 `needsBuild`；没有构建声明则为 `entryMissing`。Node 的网页和后端入口都必须提前存在。
- `development.build.script` 必须存在于对应 `package.json`，并有匹配的 npm v2/v3 `package-lock.json`。

## 用户配置 configuration

`configuration.version` 固定为 `1`，`fields` 最多 32 项。每项的字段如下：

| 字段 | 要求 | 内容 |
| --- | --- | --- |
| `name` | 必填 | 唯一的环境变量名；大写英文字母开头，只含大写字母、数字、下划线，不使用平台或系统保留名 |
| `label` | 必填 | 表单中的名称 |
| `type` | 必填 | `string`、`url`、`integer`、`boolean`、`select` 五种之一 |
| `required` | 必填 | 是否要求用户填写 |
| `secret` | 必填 | 是否为秘密值；秘密值只支持 `string`，网页必须为 `false` |
| `description` | 必填 | 设置用途 |
| `howToGet` | 必填 | 1–12 个非空字符串组成的获取步骤 |
| `placeholder` | 选填 | 输入提示 |
| `default` | 选填 | 与类型匹配的默认值；`secret: true` 禁止声明默认值 |
| `helpUrl` | 选填 | 不含账号、查询和片段的 HTTPS 说明地址 |
| `options` | `select` 必填 | 1–32 个选项，每项包含非空 `value`、`label`，`value` 不重复；其他类型不允许 |
| `endpointField` | 选填 | 只用于秘密字段，引用同一声明中的非秘密 `url` 字段；用于关联密钥对应的服务地址 |

- `url` 的默认值要是有效 HTTPS 地址；`integer` 的默认值要是整数；`boolean` 要是布尔值；`select` 的默认值要来自 `options`。
- 要在字段说明中写清用途、获取方法及相关服务；不要把真实密钥写进声明、代码或示例。
- Node 启动时，配置值以各项 `name` 对应的环境变量注入后端；不要把秘密值交给网页。
- 网页通过 `GET /_platform/config.json` 或 `window.panda.config()` 读取普通配置，得到裸对象：整数为 number，开关为 boolean，其他值为 string；选填未填项省略。配置读取不依赖数据库 SDK 开关。

## 修改后什么时候生效

| 修改内容 | 生效时机 |
| --- | --- |
| 名称、简介、图标、版本、窗口、入口、SDK 和外部地址声明 | 下次真正打开应用 |
| 配置项声明 | 下次打开；已保存的值按变量名保留，新增必填项先要求用户填写 |
| 用户保存的配置值 | 网页刷新或重新打开读取；Node 重新启动后注入 |
| `development` | 下次构建 |
| `runtime` | 不允许原地修改，换类型要删除后重新添加 |

重复打开已存在的窗口只会聚焦，不会自动重读声明或丢弃草稿。

## 示例一：最简网页

项目根已存在 `index.html`，不需要 SDK 或构建声明：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "番茄钟" },
  "web": { "root": ".", "entry": "index.html" }
}
```

## 示例二：需要准备的网页

项目根的 `package.json` 提供 `build` 脚本，并有匹配的 npm 锁文件。该脚本生成 `dist/index.html`；HTML 另行引入平台 SDK：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "我的便笺" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": {
    "sourceDir": ".",
    "build": { "kind": "npm", "script": "build" }
  }
}
```

## 示例三：带配置项的 Node 应用

前后端文件已准备好，后端遵守平台控制协议。用户在平台填写自己的服务密钥；示例不包含真实凭据：

```json
{
  "manifestVersion": 1,
  "runtime": "node",
  "app": { "name": "朗读助手", "version": "1.0.0" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": {
    "root": "backend",
    "entry": "server.mjs",
    "nodeMajor": 24,
    "protocolVersion": 1,
    "apiPrefixes": ["/v1"]
  },
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
        "howToGet": ["在服务商控制台创建自己的 API Key。"],
        "endpointField": "SERVICE_URL"
      },
      {
        "name": "MAX_ITEMS",
        "label": "每次条目数",
        "type": "integer",
        "required": false,
        "secret": false,
        "description": "一次最多处理的条目数量。",
        "howToGet": ["按自己的使用需要填写整数。"],
        "default": 10
      },
      {
        "name": "SPEAK",
        "label": "自动朗读",
        "type": "boolean",
        "required": false,
        "secret": false,
        "description": "完成处理后是否自动朗读。",
        "howToGet": ["需要自动朗读时开启。"],
        "default": false
      },
      {
        "name": "VOICE",
        "label": "朗读风格",
        "type": "select",
        "required": false,
        "secret": false,
        "description": "应用提供的朗读风格。",
        "howToGet": ["选择喜欢的风格。"],
        "default": "calm",
        "options": [
          { "value": "calm", "label": "平静" },
          { "value": "bright", "label": "明快" }
        ]
      }
    ]
  },
  "window": { "width": 960, "height": 640, "rememberSize": true }
}
```

[平台规则总览](/wiki/platform-rules/) · [应用类型与运行方式](/wiki/app-types/) · [数据与配置](/wiki/data-and-config/) · [添加要求](/wiki/adding-apps/)
