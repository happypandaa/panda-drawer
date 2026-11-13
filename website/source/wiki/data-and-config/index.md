---
title: 数据、配置、日志和删除
description: 文件与浏览器数据、后台密钥、agent 共享数据库和删除范围。
---

# 数据、配置、日志和删除

更新于 2026-11-13。依据 [平台说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[窗口与数据说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md)、[后台日志说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md)、[agent 数据规则](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/agent-apps.md)、[工作区代码](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/paths.rs)和[清理界面](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/LocalDataCleanupPanel.tsx)。

应用文件、浏览器数据和 API Key 存放在不同位置。删除应用或修改数据库前，先核对数据类型和目录归属。

## 数据存放位置

| 内容 | 位置与规则 |
| --- | --- |
| 应用代码与运行文件 | 登记的应用文件夹。平台与 AI 使用同一份文件。 |
| 平台创建或下载的应用 | 当前应用工作区。新安装开发版默认 `~/PandaDrawer-Dev`，正式版默认 `~/PandaDrawer`。占用时尝试带数字后缀的目录，实际位置以软件显示为准。 |
| SDK 数据库和附件 | `data/main.sqlite`、`data/attachments/`。首次使用时创建，数据库缺失会创建空库，损坏会报错。 |
| 后台文件数据 | `APP_DATA_DIR` 指向应用的 `data/`，缓存和临时文件另用 `APP_CACHE_DIR`、`APP_TEMP_DIR`。 |
| 浏览器数据 | 平台按应用 ID 隔离的窗口存储，包括 localStorage、IndexedDB、Cookie。 |
| 公开配置与密钥 | 平台私有位置。网页只读取公开配置，密钥只注入当前应用的后台。 |
| 后台日志 | `app logs --app <id> --json` 返回日志路径与最近内容。`current.log` 为本次，`previous.log` 为上一次，每个保留约 256 KB，配置中的密钥值已脱敏。 |

新增后台或移除后台并重新加载时，应用 ID、地址、文件夹、`data/`、公开配置和浏览器数据保留。移除后台时要同时移除秘密字段，成功后平台删除旧密钥，失败时保留。

配置值由用户在平台表单填写。密钥保存在当前用户文件权限保护的平台私有文件中。应用不得在网页、日志或代码中暴露密钥。修改运行配置后重新加载应用。

## 读写数据库和附件

- 普通应用运行时，AI 通过平台 CLI 的 `db` 命令修改数据库，网页使用 SDK。完全关闭后可直接用 SQLite 工具。改表结构前读取实际 schema，修改后同步说明文档。
- SDK、后台和 agent 可以共用 `data/main.sqlite`。外部连接设置 5000 ms 锁等待，保持 WAL、外键和现有配置。先在事务外完成读取、联网、下载等耗时工作，再用短 `BEGIN IMMEDIATE` 事务提交。长时间持锁会让界面保存报 `SQL_BUSY`。
- 不替换运行中的数据库或删除 `-wal`、`-shm`。失败时先核对回滚，结果不明时先查询，再决定是否重试。SQL 和附件写入分别提交。
- 写入 `data/attachments/` 的普通文件可直接通过 `panda.files.list`、`get`、`url`、`remove` 使用，仍需通过附件路径与链接检查。
- 外部数据库和附件写入不会产生 `data.changed`。应用可在窗口获得焦点、页面重新可见或用户点击刷新时重新查询。空间统计是扫描快照，外部写入后需要重新扫描才能取得最新占用。

## agent 日常任务的数据边界

- 只写应用规则允许的表和列。先读数据库现状，重复任务按已完成的状态继续。应用负责建库、建表与升级，AI 发现库或表缺失时先请用户打开应用。
- 用参数绑定或可靠的值编码处理 SQL 输入。新增内容用 `INSERT` 或 `INSERT OR IGNORE`，保留用户写入的收藏、备注等字段。不要用 `INSERT OR REPLACE` 或 `REPLACE` 覆盖原行。
- 分开记录开始状态、执行耗时工作与短事务提交。写入失败先确认回滚，再记录失败状态。日常任务不执行 `PRAGMA journal_mode`、`VACUUM` 或文件级清理。
- 表结构升级放在应用程序中，只加表、加列，同步数据库说明和受影响的 agent 步骤。重新加载完成升级后再继续日常任务。

## 备份与删除

代码用 Git 保存，起始项目的 `.gitignore` 默认排除 `data/`。需要长期保存的数据应放在 `data/` 或 SDK 数据库，备份和导出由应用作者与用户安排。

| 目录归属 | 删除应用的结果 |
| --- | --- |
| 用户选择的本机文件夹，包括其中新建的起始项目或下载的 agent 应用 | 保留整个文件夹、代码、Git、`data/`，清除平台配置、密钥、日志、缓存和浏览器数据。 |
| 平台创建、默认下载或通过有效预留登记的目录 | 删除整个受管目录及其中数据，清除平台配置、密钥、日志、缓存和浏览器数据。目录身份改变或归属不明时，平台不会整根删除。 |

同一用户目录删除后重新添加，原实例的浏览器数据无法恢复。用户已保存到外部目录的最终导出、下载和附件副本不属于应用删除范围。删除或清空数据库、文件数据和附件前，要说明影响并取得用户明确同意。

设置中的「清理所有本机项目内容」列出范围并要求两次确认，永久清理受管应用、数据、私有配置和网页存储。用户原文件夹和软件程序保留。以界面列出的范围和实际完成结果为准。

[平台规则总览](/wiki/platform-rules/) · [清单格式](/wiki/panda-json/) · [应用运行方式](/wiki/app-types/) · [添加要求](/wiki/adding-apps/)
