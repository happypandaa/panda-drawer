---
title: 应用类型与运行方式
description: 网页构建、Node 24 控制协议、环境变量、窗口与网页运行规则。
---

# 应用类型与运行方式

更新：2026-11-04。来源：[SKILL.md](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[Node 后端协议](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md)、[窗口与网页环境](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md)。

便笺、清单可以从普通页面开始，需要私密服务密钥或服务端接口时使用带本地服务的应用。这页说明 AI 应遵守的运行方式。

## 网页应用 runtime: web

- 使用普通静态网页，可以是 HTML 文件或 Vite、React 等工具的构建结果。不要求启动开发服务器。
- 直接运行 `panda.json` 声明的项目文件夹，没有独立运行副本；入口为 `web.root` 下的 `web.entry`。
- 在 HTML 的应用模块之前引入普通脚本 `/_platform/sdk.js`，取得 `window.panda`，不安装 npm SDK 包。
- 只有声明 `platform.sdkApiVersion: 1` 才能使用 SDK 的 SQL 和附件 API；未启用时不要调用这些接口，平台不会因此创建数据库。

```html
<script src="/_platform/sdk.js"></script>
```

用户配置通过 `GET /_platform/config.json` 或 `window.panda.config()` 读取，配置接口不依赖 SDK 数据能力开关。网页配置公开，不能声明 `secret`；需要密钥的功能放到 Node 后端。

## 网页准备与构建

- `development.build.kind` 只能为 `npm`，`script` 指向已有的 npm 脚本。
- 在 `development.sourceDir`（省略为 `.`）执行受管 `npm ci`，再执行 `npm run <script>`；需要匹配的 npm v2/v3 `package-lock.json`。
- 产物位置以 `web.root` 为准，脚本要生成声明的入口；平台不追加输出目录参数。
- 应用窗口打开时不能准备。要先关闭窗口，再由 AI 构建，或在应用详情点击「准备应用」，成功后重新打开。
- 构建直接修改原目录。失败不回滚脚本已经修改的文件，不保留上一版运行副本；先看日志、Git 状态及实际输出再处理。
- 没有构建声明时直接修改网页文件，完成后重新打开。

## Node 应用 runtime: node

- 使用静态网页加一个 Node 24 后端。网页和后端入口必须是已经可以直接运行的文件。
- 平台不为 Node 应用构建，也不在启动时安装依赖；运行文件和所需依赖要提前准备。
- 后端按协议 1 通过 stdin/stdout 交换逐行 JSON 消息；每条回复带 `type`、`protocolVersion: 1` 和本次的 `instanceId`。
- **stdout 只能输出控制协议消息。普通日志写 stderr**，使用 `console.error` 或将 `console.log` 重定向到 stderr。stdout 出现非协议内容会导致平台终止应用。

## Node 控制协议摘要

1. 接收 `initialize`，核对协议版本，保存本次 `instanceId` 和 token。
2. 监听 `APP_HOST`（`127.0.0.1`）与 `APP_PORT`（`0` 表示系统分配），监听成功回复 `ready`，报告实际 host、port。
3. HTTP 请求核对 `Authorization: Bearer <本次 token>`。`GET /_platform/health` 返回同一 `instanceId`、`protocolVersion: 1` 和 `status: "ok"`。
4. 收到 `ping`，回复同一 `instanceId`、`requestId` 的 `pong`。
5. 收到 `shutdown`，先回复 `stopping`，再停止接收请求、完成保存、关闭监听和自己启动的子进程，最后退出。stdin 关闭时同样收尾，不创建脱离平台的后台服务。

后端启动与页面代理共用 30 秒总预算；健康检查单次最多 10 秒并计入总预算；ping 3 秒；正常停止宽限 5 秒。健康回复头与解码后正文各最多 16 KiB。

业务接口通过 `backend.apiPrefixes` 声明。省略时代理 `/api`、`/tts`；`[]` 不代理业务接口。按完整路径段匹配，不使用平台保留的 `/_platform`。网页同源调用，不写死后端端口，不把 token 交给网页。业务请求流式转发，应用自行校验业务大小；平台代理不支持业务跳转和 WebSocket。

## Node 环境变量

| 变量 | 含义 |
| --- | --- |
| `APP_HOST` | 后端监听地址 `127.0.0.1` |
| `APP_PORT` | 后端监听端口，`0` 表示由系统分配 |
| `APP_DATA_DIR` | 项目的 `data/`，启动前已创建；存放文件数据 |
| `APP_CACHE_DIR` | 平台私有的缓存目录 |
| `APP_TEMP_DIR` | 平台私有的临时文件目录 |
| 配置项的 `name` | 用户保存的对应配置值，只注入目标后端 |

不要依赖宿主 `PATH`、全局 npm 包、作者电脑路径或未声明的环境变量。

## 窗口与打开关闭

- 一个应用一个原生窗口；打开时启动对应服务，真正关窗后停止。再次打开同一应用只聚焦已有窗口，不刷新页面、不启动第二份服务。
- 真正关闭后重新打开，读取最新的 `panda.json`、配置和文件。修改代码、构建前要先关窗。
- `window.width`、`window.height` 都要填写，取 1–32768 的整数；`rememberSize` 默认 `true`。
- 未声明 `window` 时初始大小为 1080×780，并记住用户调整后的大小；实际最小内容区为 640×480。
- 可以使用 `alert`、`confirm`、`prompt`，由原生对话框显示。外部链接和 `window.open` 交系统浏览器，不在平台中新建应用窗口。

## 网页环境与外部访问

- 每实例有固定的 `http://127.0.0.1:<端口>` 地址，从根路径提供网页，支持 `/assets/...` 绝对资源路径。localStorage、IndexedDB、Cookie 随下次打开继续使用。
- 不存在且没有扩展名的页面路径回退首页；资源文件与 `/_platform/` 路径不作该回退。
- `/_platform/sdk.js`、`/_platform/config.json`、`/_platform/api/v1/` 是平台保留接口，不要占用这个路径。
- 网页服务不提供 `data/`、`node_modules/`、隐藏文件和目录、`panda.json`，也不提供越界链接或多个硬链接文件；普通 `assets/data/` 不受影响。
- 允许内联脚本、事件属性、`eval`、HTTPS 脚本／样式／字体／图片／媒体，以及 data:、blob: 图片和媒体；不允许 `<object>` 嵌入或被其他页面嵌入。
- 网页主动请求外部地址时，在 `runtimeRequirements.network.connect` 声明 `origin`、`purpose`、`dataSent`，同时需要对方服务允许跨域。摄像头、麦克风在 `runtimeRequirements.devices` 声明用途并经系统授权。
- Node 网页只经平台代理同源访问后端，外部请求由后端完成。

## 两种起始项目

| 用户入口 | 生成内容 |
| --- | --- |
| 「添加应用 → 从零做一个」 | Vite 网页起始项目，已启用平台 SDK |
| 「添加应用 → 本机文件夹」选择空文件夹 | 零第三方依赖的 Node 起始项目；生成和添加不会自动运行或安装依赖 |

[平台规则总览](/wiki/platform-rules/) · [panda.json 格式](/wiki/panda-json/) · [数据与配置](/wiki/data-and-config/) · [添加要求](/wiki/adding-apps/)
