---
title: 应用构成与运行方式
description: 必需的网页、可选 Node 24 后台、agent 工作方式、SDK 和重新加载规则。
---

# 应用构成与运行方式

更新于 2026-11-13。依据 [平台说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[窗口与网页环境](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md)、[后台协议](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md)、[agent 应用说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/agent-apps.md)和 [SDK](https://github.com/happypandaa/panda-drawer/blob/develop/packages/sdk/README.md)。

每个应用都有网页。需要私密服务密钥时可以加后台，也可以用应用规则告诉自己的 AI 如何完成日常任务。

## 网页与 SDK

- 平台从登记的应用文件夹提供普通静态网页。入口是 `web.root` 下的 `web.entry`，可以是单个 HTML 或 Vite、React 等工具的构建结果。
- HTML 在应用模块前引入 `/_platform/sdk.js`，通过 `window.panda` 使用 SDK。所有应用都可以用 `platform.sdkApiVersion: 1` 启用 SQL、附件与 AI 交接能力。有后台时 `/_platform/*` 仍由平台处理。
- `/_platform/config.json` 与 `panda.config()` 读取公开配置，独立于 SDK 数据能力开关。秘密值只注入后台。
- `panda.context.get().projectRoot` 返回当前应用根目录的绝对路径，使用系统原生格式。调用需要启用 SDK。
- 用户点击按钮时，可在 `capabilities` 包含 `ai.open` 的情况下调用 `panda.ai.open(text)`。文字需非空且不超过 2000 字。平台按当前应用目录打开用户选定的 AI 工具并预填，用户负责发送。无法直接打开时，平台复制文字并提示用户粘贴。应用可以提供复制入口作为后备。

## 网页构建与重新加载

- `development.build` 对有无后台的应用均可用，只准备网页。按 `development.sourceDir`（默认 `.`）执行受管 `npm ci` 和声明的 `npm run <script>`。脚本要在 `web.root` 中生成入口，平台不追加输出目录参数。
- 使用平台「准备应用」或 `app build --app <id>` 时，先关闭应用窗口。命令行构建可用 `task wait` 等待结果。构建直接修改应用目录，失败时查看日志、Git 状态和实际产物。
- 已经准备好运行文件时，确认用户保存编辑，再点标题栏「重新加载」或运行 `app reload --app <id>`。平台重读清单和配置，有后台时重启后台，应用未打开时会打开。
- 同一应用只有一个原生窗口。再次点击打开只聚焦原窗口。清单无效或缺必填配置时，重新加载的预检会保留旧实例。启动切换失败会明确报错，修好再重新加载。
- `window.width`、`height` 都要填写，取 1–32768 的整数，`rememberSize` 默认 `true`。没有窗口声明时默认 1080×780，并记住调整后的尺寸，实际最小内容区为 640×480。
- `alert`、`confirm`、`prompt` 使用原生对话框。外部链接和 `window.open` 交给系统浏览器。

## 可选 Node 24 后台

声明 `backend` 后，前后端入口和所需依赖要准备好。网页构建不会编译后台，后台启动时不会安装依赖。打开应用时平台启动服务，关闭应用时停止服务。

| 环节 | 协议要求 |
| --- | --- |
| 启动 | 通过 stdin/stdout 交换逐行 JSON。收到 `initialize` 后核对 `protocolVersion: 1`、`instanceId` 和 token。成功监听后发出 `ready`，报告实际 host 和 port。 |
| HTTP | 请求带 `Authorization: Bearer <本次 token>`。`GET /_platform/health` 返回同一 `instanceId`、`protocolVersion: 1` 与 `status: "ok"`。 |
| 心跳 | 收到 `ping` 后回复带同一 `instanceId`、`requestId` 的 `pong`。 |
| 关闭 | 收到 `shutdown` 后先回复 `stopping`，随后停止接收请求、保存数据、关闭监听及子进程并退出。stdin 关闭时同样收尾。 |

stdout 只输出控制协议消息，普通日志写 stderr。后端启动和页面代理共用 30 秒预算，健康检查每次最多 10 秒，ping 为 3 秒，正常停止宽限为 5 秒。

| 环境变量 | 含义 |
| --- | --- |
| `APP_HOST` | 后台监听地址 `127.0.0.1` |
| `APP_PORT` | 后台监听端口，`0` 表示由系统分配 |
| `APP_DATA_DIR` | 应用的 `data/`，启动前已创建 |
| `APP_CACHE_DIR` | 平台私有缓存目录 |
| `APP_TEMP_DIR` | 平台私有临时目录 |
| 配置项的 `name` | 用户保存的对应值，注入目标后台 |

## 网页路径与网络

- 应用在固定的 `http://127.0.0.1:<端口>` 根路径运行，可以用 `/assets/...` 引用资源。不存在且没有扩展名的页面路径回退到首页。资源文件与平台接口不参与回退。
- `/_platform/sdk.js`、`/_platform/config.json`、`/_platform/api/v1/` 由平台提供。网页根不会提供数据库目录、依赖目录、隐藏文件、`panda.json`、越界链接或不符合要求的硬链接文件。
- 网页主动请求外部服务时，在 `runtimeRequirements.network.connect` 声明 `origin`、`purpose`、`dataSent`，服务还要允许跨域。摄像头、麦克风需在 `runtimeRequirements.devices` 声明用途并取得系统授权。
- 网页通过同源的 `backend.apiPrefixes` 业务路径访问后台。省略前缀时为 `/api`、`/tts`，`[]` 表示不代理。网页外部请求仍按网页声明约束，后台的网络请求不受该网页策略约束。
- 应用自行校验业务请求大小，网页不要写死后台端口或接触后台 token。平台代理支持流式业务请求，业务重定向与 WebSocket 超出代理契约。

## agent 应用

- `agent.instructions` 指向应用日常规则。用户的 AI 按规则收集或处理内容，应用展示结果、保存数据和接收反馈。拾闻就是这种应用。
- 应用根目录是 AI 的工作目录。日常任务只写规则允许的表和列。应用启动时负责建表与升级，日常 AI 发现数据库或表缺失时请用户先打开应用，再按规则处理。
- AI 不在线时应用照常使用，反馈留在数据库供下次读取。用户需要新结果时，在自己的 AI 对话里发起任务。
- 用「按你的想法改」进入应用修改流程。改规则前先说明改动，表结构升级只增加表和列，更新受影响的规则与说明。完成后先重新加载应用，再开始日常任务。

[平台规则总览](/wiki/platform-rules/) · [清单格式](/wiki/panda-json/) · [共享数据规则](/wiki/data-and-config/) · [添加要求](/wiki/adding-apps/)
