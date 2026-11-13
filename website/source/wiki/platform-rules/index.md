---
title: 平台规则总览
description: 熊猫小抽屉的应用构成、AI 交接、修改边界和命令行规则。
---

# 平台规则总览

更新于 2026-11-13。依据 [平台说明 SKILL.md](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[交接提示词](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/project-ai-requirements.ts)和 [交接生成代码](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/resources.rs)。

把应用交给自己的 AI 后，它可以按这页了解如何检查、修改和重新加载应用。执行时先读交接给出的本机平台说明。

## 应用和平台的分工

- 每个应用有网页，可选 Node 24 后台和 agent 规则。项目根目录的 `panda.json` 使用版本 2，通过 `backend` 声明后台，通过 `agent.instructions` 声明 agent 规则。
- 平台直接使用登记的应用文件夹，负责添加、启动、停止、网页构建、保存配置、日志和删除。AI 修改代码、按声明准备运行文件，并用 Git 保存或回退代码。
- 用户使用自己选择的 AI。agent 应用的日常任务由用户在 AI 对话中发起。平台不运行 agent、不读取对话、不调度任务。
- 网页是静态文件或构建结果。后台使用一个遵守平台协议的 Node 24 服务。平台的依赖准备和构建使用 npm 与 v2/v3 `package-lock.json`。SSR、多后台服务、Python、Java 和 Docker 超出当前运行契约。

## 接手应用

1. 先读本机平台说明、`panda.json`、README 和声明的应用文档。有后台时用 `app logs` 查看日志。先简要说明理解到的功能，再问用户想改哪里。
2. 文字、颜色和布局等小改可说明后直接做。涉及数据、表结构或较大改动时，先说明计划并等用户同意。
3. 用 Git 保存当前代码，保留已有未提交改动。只改用户交给你的应用文件夹，平台登记、私有配置、密钥和日志由平台管理。不要读取真实 `.env`、要求用户发送 Key，或把密钥写进代码。真实值由用户在平台表单填写。
4. 删除或清空 `data/`、数据库或附件前，先说明影响并取得明确同意。Git 保存代码，数据备份要另作安排。
5. 改好并准备运行文件后，确认未保存输入已经处理，再用 `app reload --app <id>` 或请用户点窗口上的「重新加载」。有后台时会重启后台，应用未打开时会打开。使用平台构建时，先关闭应用窗口。
6. 说明改动、数据影响、已完成的检查和仍待验证的内容，并同步应用文档。CLI 连不上时保留代码，告知用户缺少的步骤。

## 交接提示词

| 交接用途 | AI 要做的事 |
| --- | --- |
| 修改应用 | 读取当前应用的目录、平台说明和 CLI 路径，先理解应用再动手。有后台时同时提供日志位置和查询命令。 |
| 修改 agent 应用 | 同时读应用声明的规则文件。若用户只是要求执行日常任务，按该文件完成任务。改规则前先告诉用户改什么。表结构升级只加表、加列，同步数据库说明和受影响的日常步骤，升级要在重新加载后完成。 |
| 新建应用 | 读取已生成的起始项目，先问应用给谁用、最想做什么，再提出第一版方案。 |
| 文件夹检查和接入 | 结合平台检查结果先只读评估。用户同意改动后保存代码版本，完成适配再添加。 |
| agent 日常任务 | 使用应用目录和它的 `AGENTS.md` 或声明的规则文件。应用可用 SDK 交接自己的任务文字，用户核对后发送。 |

本机 `SKILL.md` 和 `references/` 随平台安装。读不到交接中的本机路径时，先告诉用户。公开版本可帮助了解格式。 [公开平台说明](https://github.com/happypandaa/panda-drawer/tree/develop/skills/local-projects-app)

## 平台命令行

使用当前提示词中的 CLI 路径。可用 `app list` 核对应用 ID。以下命令中 `<cli>`、`<id>`、`<文件夹>` 与 `TASK_ID` 是占位符。

```sh
<cli> app list --json
<cli> app info --app <id> --json
<cli> app check --dir <文件夹> --json
<cli> app reload --app <id> --json
<cli> app logs --app <id> --json
<cli> app build --app <id> --json
<cli> db schema --app <id> --json
<cli> db execute --app <id> --input request.json --json
<cli> task wait --task TASK_ID --json
<cli> env doctor --json
<cli> env git doctor --json
```

- `env doctor --json` 的 `buildEnvironment` 给出 Node/npm 路径，`env git doctor --json` 给出 Git 路径。
- `app delete`、`storage cleanup`、`env node/git set/clear` 要先说明影响。平台会在主窗口请用户确认，拒绝或两分钟未确认即不执行。
- 写入结果不明时，先检查实际结果，再决定后续操作。

[清单格式](/wiki/panda-json/) · [应用运行方式](/wiki/app-types/) · [数据与配置](/wiki/data-and-config/) · [添加应用](/wiki/adding-apps/) · [交给你的 AI](/wiki/ai-workspace/)
