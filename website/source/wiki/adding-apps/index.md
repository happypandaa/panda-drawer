---
title: 添加方式与要求
description: 应用链接、本机文件夹、新建应用、agent 目录归属与推荐下载线路。
---

# 添加方式与要求

更新于 2026-11-13。依据 [平台说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[添加界面](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/AddApplicationDialog.tsx)、[链接添加流程](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/SourceAcquisitionPanel.tsx)、[推荐清单](https://github.com/happypandaa/panda-drawer/blob/develop/catalog/apps.json)和[下载线路配置](https://github.com/happypandaa/panda-drawer/blob/develop/distribution/download-sources.json)。

用户可以用应用链接添加、选择已有文件夹，或新建一个起始项目。AI 可以先检查应用是否符合当前格式。

## 有应用链接

在「添加应用」中填写「有应用链接」。平台支持 GitHub 和 Gitee 的公开 HTTPS 仓库，根目录须包含版本 2 的 `panda.json`。

- 已有网页入口就使用现成运行文件。缺网页产物且声明了 `development.build` 时，平台请用户确认后准备网页。缺入口又没有构建声明时，需要先修复应用。
- 声明 `backend` 时，后台运行文件须已提交，非零依赖后台须有 `package.json` 和匹配的 npm v2/v3 `package-lock.json`。链接添加时平台用 `npm ci --omit=dev --ignore-scripts` 准备生产依赖。网页准备仍按用户确认的脚本执行。
- 添加结果显示缺必填配置时，通过平台表单填写再打开。添加结果不明确时先核对原任务。

## 本机文件夹与新建应用

- 在「添加应用 → 本机文件夹」选择整个应用目录。有效的 `panda.json` 和运行文件满足检查要求后，平台登记原目录。文件夹归用户所有。
- 「新建」和在用户选择的空文件夹里新建，都会生成启用 SDK 的 Vite 网页起始项目。平台工作区新建的目录归平台所有，用户选择的文件夹归用户所有。
- 平台运行时，也可用 `app create --name "应用名称" --description "一句话简介" --json` 在工作区创建同样的起始项目。命令返回应用 ID 和目录。先与用户说清需求，再创建。
- 普通本机目录需要预先具备后台运行文件和依赖。缺网页产物但已声明构建时，可在添加后通过「准备应用」完成网页准备。

## 只读检查与修复添加

- 先说明实际读到的目录或仓库版本。只读源码、README、清单和构建配置。真实 `.env`、密钥、业务数据库和私人附件不在接入检查范围内。
- `app check --dir <文件夹> --json` 检查本机目录。先确定真正的应用根目录，再判断可直接接入、需要修改、当前不推荐或信息不足。说明依据和改动大小。旧账号和业务数据迁移另行核对。
- 只读评估阶段不安装、不运行、不构建、不登记。需要修改时先说方案，等用户同意。适配后用 `app add --dir <文件夹> --json` 添加用户目录。
- 链接添加失败后，可用「交给 AI 处理」修复并交给平台管理。先用 `app reserve --json` 取得平台预留的空目录和 ID，用平台 Git 下载到预留目录，完成适配后运行 `app add --reservation <id> --json`。平台会核对目录身份。预留 24 小时后只清理身份一致的空目录，非空内容保留并报告。

## agent 应用放在哪里

macOS 从推荐或链接添加声明 agent 的应用时，可以使用当前工作区，也可以选择或新建空文件夹。默认工作区归平台管理，选择的文件夹归用户。下载直接放进选定文件夹，`.git`、`.env` 等仍算已有内容，空目录检查只忽略允许的系统元数据。Windows 当前不提供这项位置选择。

应用添加后保持目录原位置。删除时按登记归属处理。 [数据和删除规则](/wiki/data-and-config/)

## 推荐应用与下载线路

- 推荐清单包含拾闻、Clack、Readio 和 Radio Bob。`catalog/apps.json` 记录应用地址、分支、所需配置和简介。拾闻的 `agent` 标记用于展示，接入时仍检查应用自己的 `panda.json`。
- 清单的 `i18n` 支持 `zh-CN`、`en`，用于覆盖推荐卡片的 `name`、`summary`、`needs`、`note`。它独立于安装后应用清单的 `localization` 和应用内部语言。
- 设置中的「下载线路」默认「自动（推荐）」，可改为「国内线路（Gitee）」或「国外线路（GitHub）」。推荐目录与推荐应用按线路获取，自动模式遇到下载失败可尝试另一条线路。用户手动填写的链接按实际地址添加。

[平台规则总览](/wiki/platform-rules/) · [清单格式](/wiki/panda-json/) · [应用运行方式](/wiki/app-types/) · [数据与配置](/wiki/data-and-config/) · [入门指南](/wiki/getting-started/)
