# 复制给 AI 的平台要求摘要

日期：2026-11-02。状态：现行。替代此前的"项目兼容评估说明"全文版本。

## 原则

平台规则只有一份详细来源：随应用安装的 skill `skills/local-projects-app/`（SKILL.md 及 references）。复制给 AI 的提示词只给项目信息、skill 的实际安装路径和几条硬规则，其余写"以平台说明为准"。

例外是**平台介绍**、**项目评估／添加失败**和**本地文件夹检查**：用户常把它们发给读不到本机文件的网页 AI，所以在末尾附下面这段"平台要求摘要"，再给出 skill 路径供能读本机文件的 AI 使用。

各提示词的位置：

| 提示词 | 来源 | 附带内容 |
| --- | --- | --- |
| Node 交接 | `crates/platform-core/src/service_node_handoff.rs` | skill 路径、日志路径、硬规则 |
| 网页交接 | `crates/platform-core/src/resources.rs` 的 `handoff` | skill 路径、CLI、构建方式、硬规则 |
| 本地文件夹检查（需要评估时） | `crates/platform-core/src/node_app_directory.rs` | 平台要求摘要 + skill 路径（指向“用法二”） |
| 启动失败求助 | `apps/desktop/src/project-ai-help.ts` | skill 路径、日志、硬规则 |
| 项目评估、添加失败 | `apps/desktop/src/project-ai-help.ts` | 平台要求摘要 + skill 路径 |
| 平台介绍 | `apps/desktop/src/platform-ai-intro.ts` | 平台要求摘要 + skill 路径 |

共用的硬规则（Git 保存与回退、删数据先问、不碰密钥、stdout 规则）在 `docs/ai-prompt-rules.json`。摘要和硬规则同时生成 TS 常量（`apps/desktop/src/project-ai-requirements.ts`）和 Rust 常量（`crates/platform-core/src/ai_prompt_rules.rs`）。修改本文件的摘要或规则后，运行 `node scripts/generate-project-ai-requirements.mjs` 生成 TS 与 Rust 常量；`npm run test:ai-prompts` 检查是否同步。

摘要必须与 skill 一致；改平台规则时先改 skill，再同步这里。

## 平台要求摘要

下面代码块是生成脚本读取的唯一来源，内容原样附在提示词末尾。

```text
【平台要求摘要】
- 两种应用：网页应用（普通静态网页，可以是 Vite、React 的构建结果或单个 HTML 文件，可选用平台 SDK 存 SQLite 数据和附件）；Node 应用（静态网页加一个 Node 24 后端）。不支持 SSR、多个服务、Python、Java、Docker 等其他运行时；只支持 npm。
- 项目根目录放一个 panda.json，写明名称、类型和入口，不能有多余字段。
  网页最小示例：{"manifestVersion":1,"runtime":"web","app":{"name":"我的网页"},"web":{"root":".","entry":"index.html"}}
  Node 最小示例：{"manifestVersion":1,"runtime":"node","app":{"name":"我的应用"},"web":{"root":"web","entry":"index.html"},"backend":{"root":"backend","entry":"server.mjs","nodeMajor":24,"protocolVersion":1}}
- 平台直接运行项目文件夹，不另存副本。网页需要构建时，在 panda.json 的 development.build 声明 npm 脚本（需要 package-lock.json），由 AI 或平台执行；Node 应用的运行文件要事先准备好，平台不为它构建。
- 网页在固定的本机地址根路径运行，普通 Vite、React、纯 HTML 项目一般不用改；允许内联脚本和 https 上的脚本、样式、图片。可用 /_platform/sdk.js（SDK）和 /_platform/config.json（用户配置）。页面主动请求外部地址要在 panda.json 声明；网页不能保存密钥，需要密钥的功能放在 Node 后端。
- Node 后端遵守平台控制协议：stdout 只输出协议消息，日志写 stderr；收到关闭指令要保存并退出。
- 文件数据放在项目的 data/；配置项在 panda.json 声明，由用户在平台表单填写，不用 .env。
- 接入方式：新建项目（平台生成网页起始项目）；添加本地项目（选空文件夹生成 Node 起始项目，或选已有 panda.json 的文件夹）；从 GitHub、Gitee 公开仓库添加。
```
