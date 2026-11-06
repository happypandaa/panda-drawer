# 配置文档 panda.json

项目根目录下的 `panda.json` 说明应用的名称、类型和入口。它跟着代码走，AI 可以修改。本机的实例 ID、配置值、密钥、日志位置由平台另外登记，不写进这个文件。结构以 `../../../schemas/panda.schema.json` 为准。

## 字段

| 字段 | 网页 web | Node node | 说明 |
| --- | --- | --- | --- |
| `manifestVersion` | 必填 | 必填 | 固定为 `1` |
| `$schema` | 选填 | 选填 | 供编辑器校验，平台忽略 |
| `runtime` | 必填 | 必填 | `web` 或 `node` |
| `app.name` | 必填 | 必填 | 应用名称，显示在卡片和窗口上 |
| `app.version`、`description`、`icon` | 选填 | 选填 | 版本号、简介、项目内 PNG 图标路径 |
| `web.root`、`web.entry` | 必填 | 必填 | 网页文件目录（可以是 `.`）和首页文件；entry 相对 root |
| `backend` | 不允许 | 必填 | `root`、`entry`、`nodeMajor: 24`、`protocolVersion: 1` |
| `backend.apiPrefixes` | 不允许 | 选填 | 后端接口路径前缀数组，例如 `["/v1"]`；省略默认 `/api`、`/tts`，`[]` 不代理业务接口；按完整段匹配，禁止根 `/`、`/_platform` 和转义路径 |
| `platform.sdkApiVersion` | 选填 | 不允许 | 写 `1` 启用 SDK 的数据库和附件 |
| `development` | 选填 | 不允许 | `sourceDir`（默认 `.`）和 `build: {"kind": "npm", "script": "<脚本名>"}` |
| `configuration` | 选填 | 选填 | 用户配置项，见下文；网页不允许 `secret: true` |
| `documentation` | 选填 | 选填 | `readme`、`development`，网页另可写 `database`；声明的文件必须存在 |
| `agent` | 选填 | 选填 | `{"instructions":"AGENTS.md"}`；标记 agent 工作方式，规则文件必须存在，见下文 |
| `window` | 选填 | 选填 | `width`、`height`（两项都要写）、`rememberSize`（默认 true） |
| `runtimeRequirements` | 选填 | 不允许 | 网页主动请求的外部地址和使用的设备，见 `app-window-runtime.md` |

不要写本表以外的字段，未知字段会导致应用打不开。

## 规则

- 文件为 UTF-8 编码的 JSON，不能有重复的键。
- 路径用 `/` 分隔、相对项目根；不能用绝对路径或 `..`，也不能经符号链接指向项目外。
- `web.root` 不能是或位于 `data/`、`node_modules/`、以 `.` 开头的目录。
- 入口文件缺失时：网页声明了构建就显示"需要构建"，否则显示"入口文件缺失"；Node 的网页和后端入口都必须存在。
- `development.build.script` 必须是 package.json 里已有的脚本，并且需要 npm v2/v3 格式的 package-lock.json。

## agent 标记

```json
"agent": { "instructions": "AGENTS.md" }
```

`agent` 可省略；存在时只允许必填的字符串 `instructions`，不接受 `null`、空对象或其他字段。路径相对应用根，用 `/` 分隔，须指向存在的项目内普通文件；不得为隐藏文件、位于隐藏目录，或通过链接指向项目外及隐藏目标。路径不能含绝对路径、`..`、空段、反斜杠或冒号。文件缺失时检查报 `agent.instructions` 错误，应补齐声明的规则文件。

通常把规则写在根目录的 `AGENTS.md`。`agent` 不增加 runtime、不启用 SDK，也不调起 AI；使用 SDK 数据库和附件的网页仍需声明 `platform.sdkApiVersion: 1`。Node 清单可以带此标记，但不因此获得网页 SDK 能力。日常执行与修改应用的分工见 `agent-apps.md`。

项目列表和详情返回的 `isAgent` 仅用于显示标识和删除提醒；项目无法读取时可保留上次有效结果。它不是清单字段、有效性结论或路径访问授权，不要写进 `panda.json`。

## 用户配置项 configuration

```json
"configuration": {
  "version": 1,
  "fields": [
    {
      "name": "API_BASE",
      "label": "接口地址",
      "type": "url",
      "required": false,
      "secret": false,
      "description": "这个设置用来做什么",
      "howToGet": ["第一步怎么获取", "第二步"]
    }
  ]
}
```

- 每项必填：`name`（变量名）、`label`、`type`（`string`、`url`、`integer`、`boolean`、`select`）、`required`、`secret`、`description`、`howToGet`（非空字符串数组）。
- 选填：`placeholder`、`default`（密钥不能有默认值）、`helpUrl`、`options`（`select` 类型必填，每项为 `{"value": "...", "label": "..."}`）。
- Node：启动时以环境变量注入后端。网页：页面通过 `/_platform/config.json` 读取，不能声明密钥。

## 修改后什么时候生效

| 修改内容 | 生效时机 |
| --- | --- |
| 名称、简介、图标、版本、窗口、入口、SDK 声明、外部地址声明 | 下次打开 |
| 配置项声明 | 下次打开。已保存的值按变量名保留；新增的必填项要用户先填写 |
| `development` | 下次构建 |
| `agent`、规则文件路径 | 下次检查或打开；声明的文件须持续存在 |
| `runtime` | 不允许修改，换类型要删除后重新添加 |

## 示例

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

单个 HTML 页面：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "番茄钟" },
  "web": { "root": ".", "entry": "index.html" }
}
```
