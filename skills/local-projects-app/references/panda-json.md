# 应用清单 panda.json v2

项目根目录的 `panda.json` 跟着代码走，描述网页和可选后台。实例 ID、归属、保存的配置、密钥和日志由平台私下登记，不写入清单。结构以 `../../../schemas/panda.schema.json` 为准。

## 字段

| 字段 | 要求 | 说明 |
| --- | --- | --- |
| `manifestVersion` | 必填 | 固定为 `2`；旧版本必须按说明修改，不自动转换 |
| `$schema` | 选填 | 编辑器提示，平台不联网读取 |
| `app.name` | 必填 | 卡片和窗口名称 |
| `app.version`、`description`、`icon` | 选填 | 版本、简介、项目内 PNG 图标路径 |
| `web.root`、`web.entry` | 必填 | 网页目录（可为 `.`）与相对该目录的入口 |
| `backend` | 选填 | 存在即启动后台：`root`、`entry`、`nodeMajor: 24`、`protocolVersion: 1` |
| `backend.apiPrefixes` | 选填 | 业务接口完整段前缀，默认 `/api`、`/tts`；`[]` 不代理业务接口，禁止根路径、`/_platform` 开头、编码或点路径 |
| `platform.sdkApiVersion` | 选填 | `1` 启用 SDK；有无后台均可 |
| `development` | 选填 | `sourceDir`（默认 `.`）和 `build: {"kind":"npm","script":"<脚本名>"}`；只构建网页 |
| `configuration` | 选填 | 用户配置；只有声明后台时可含密钥 |
| `documentation` | 选填 | `readme`、`development`、`database`；声明的文件必须存在 |
| `agent` | 选填 | `{"instructions":"AGENTS.md"}`；规则文件必须存在 |
| `window` | 选填 | `width`、`height` 都必填，`rememberSize` 默认 true |
| `runtimeRequirements` | 选填 | 网页主动请求的外部地址与设备用途；不约束后台 |
| `localization` | 选填 | 按 `zh-CN`、`en` 覆盖供平台展示的文字；不决定应用内部语言 |

清单不包含 `runtime`，未知字段、重复键、非法类型和显式 `null` 均拒绝。

## 路径和运行文件

UTF-8 JSON；路径用 `/`，相对项目根。不能是绝对路径或越过项目边界的 `..`，不能经链接指向外部。网页目录不能指向 data/、node_modules/ 或隐藏内容。

缺网页产物但声明构建时显示需要构建；后台运行文件须预先准备好。网页构建使用声明的 npm 脚本和 npm v2/v3 package-lock.json，不负责编译后台。

## 配置

```json
"configuration": {
  "version": 1,
  "fields": [{
    "name": "API_BASE",
    "label": "接口地址",
    "type": "url",
    "required": false,
    "secret": false,
    "description": "这个设置用来做什么",
    "howToGet": ["第一步怎么获取", "第二步"]
  }]
}
```

每项必填 name、label、type、required、secret、description、howToGet（非空字符串数组）。类型为 string/url/integer/boolean/select；select 必填 options（value/label 对）。可选 placeholder、default、helpUrl；密钥禁止默认值。

公开值由 `/_platform/config.json` 或 `panda.config()` 返回，未填写的可选项不出现。有后台时配置作为环境变量注入，密钥仅给后台，不能返回页面。去掉 backend 时须同时去掉密钥声明；成功切换后平台删除已保存的密钥并提示。

## 多语言展示文字

平台与应用各自管理语言。`localization` 只翻译卡片、添加检查、详情、配置表单和原生窗口标题中由平台展示的文字，不指定应用内部语言。应用可跟随 `navigator.language` 或提供自己的设置；平台不提供 SDK locale、语言环境变量或语言同步机制。

```json
"localization": {
  "en": {
    "app": {"name": "My App", "description": "A personal tool"},
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
      "network": {
        "connect": {
          "https://api.example.com": {
            "purpose": "Look up information",
            "dataSent": "Your search text"
          }
        }
      },
      "devices": {"microphone": {"purpose": "Record a voice note"}}
    }
  }
}
```

只接受 `zh-CN`、`en` 两个语言键。示例中的配置字段、选项、网络地址和设备必须已经在原声明中存在。`app` 只允许 `name/description`；配置按字段 `name` 引用，只允许 `label/description/placeholder/howToGet/options`，其中 options 按原选项 `value` 引用 label。网络按 `connect/images/media` 分组和原 `origin` 引用，只允许 `purpose/dataSent`；设备只允许已声明的 `camera/microphone.purpose`。

逐字段覆盖，缺译回原字段；`howToGet` 是整个非空字符串数组的替换，选项 label 逐个覆盖。未知键、无效引用、错误类型、显式 `null`、空文字或空步骤数组拒绝，沿用原字段长度约束。译文不修改配置 name/value、默认值、required、secret、类型、路径、origin 或权限。

平台保留原始声明和译文，在渲染时按当前平台语言选择；切换语言可立即更新已经读取的展示文字。修改清单文件本身仍须按应用重新加载流程读取新声明。SDK 返回的原始应用信息不因平台语言改变。

## agent 标记

`agent` 与后台互相独立，不自行启用 SDK、启动 AI 或调度任务。`instructions` 须指向项目内存在的非隐藏普通文件；拒绝空路径、隐藏目录、绝对路径、`..`、反斜杠、冒号和空路径段。平台的 `isAgent` 只用于显示，不能授予路径权限。日常任务与修改应用的分工见 `agent-apps.md`。

## 重新加载

改清单后运行 `app reload --app <id>`：应用没打开时打开，开着时按新清单切换。新增必填配置未填时打开配置表单，旧实例继续运行；清单无效时也保留旧实例。启动失败进入失败状态，不自动回退代码；修好后再 reload。每次切换失效旧会话，应用 ID、地址、文件夹、data/、公开配置和浏览器数据保留。

增加后台：加 backend、准备运行文件、reload，然后用 `app logs` 排查。移除后台：去掉 backend 和所有密钥声明，再 reload。不通过删除重加转换应用。

## 示例

启用 SDK 的静态网页：

```json
{
  "manifestVersion": 2,
  "app": {"name": "我的应用"},
  "web": {"root": ".", "entry": "index.html"},
  "platform": {"sdkApiVersion": 1}
}
```

带后台、SDK 和网页构建的应用：

```json
{
  "manifestVersion": 2,
  "app": {"name": "示例笔记"},
  "web": {"root": "dist", "entry": "index.html"},
  "backend": {
    "root": "backend", "entry": "server.mjs",
    "nodeMajor": 24, "protocolVersion": 1,
    "apiPrefixes": ["/api"]
  },
  "platform": {"sdkApiVersion": 1},
  "development": {"build": {"kind": "npm", "script": "build"}}
}
```
