---
title: 数据、配置、日志和删除
description: 应用代码、文件数据、浏览器存储、配置密钥和日志的位置，以及删除与清理边界。
---

# 数据、配置、日志和删除

更新：2026-11-04。来源：[SKILL.md](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)、[窗口与数据说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/app-window-runtime.md)、[Node 日志说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/node-backend.md)、[默认工作区代码](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/paths.rs)、[清理界面](https://github.com/happypandaa/panda-drawer/blob/develop/apps/desktop/src/LocalDataCleanupPanel.tsx)。

应用文件、练习记录和 API Key 不一定存在同一个地方。这页说明 AI 应该访问哪里，以及删除前要保留什么。

## 什么放在哪里

| 内容 | 位置 | 说明 |
| --- | --- | --- |
| 应用代码与运行文件 | `<项目文件夹>` | 运行的就是这份文件，没有独立运行副本 |
| 平台新建或下载的应用 | 应用工作区 | 新安装开发版默认 `~/PandaDrawer-Dev`，正式版默认 `~/PandaDrawer`；实际位置以软件显示为准 |
| 网页 SDK 数据库 | `data/main.sqlite` | 启用 SDK 后首次使用时创建；缺失时新建空库，损坏时报错而不重置 |
| 网页 SDK 附件 | `data/attachments/` | 首次使用时创建，通过 SDK 附件接口访问 |
| Node 文件数据 | `APP_DATA_DIR`，即 `data/` | 启动前创建该目录，由应用保存自己的文件 |
| 浏览器数据 | 平台的窗口存储 | localStorage、IndexedDB、Cookie；按实例隔离，不在项目文件夹里 |
| 配置与密钥 | 平台私有位置 | 不写进项目；秘密值只交给对应后端 |
| Node 运行日志 | `<日志目录>/current.log`、`<日志目录>/previous.log` | 本次和上一次；目录由交接提供，每个文件约 256 KB，密钥值已脱敏 |
| Node 缓存与临时文件 | `APP_CACHE_DIR`、`APP_TEMP_DIR` | 平台私有位置，不作为长期数据目录 |

## 工作区与同名目录

- 默认工作区只用于新安装。已有工作区以当前软件显示的路径为准，不按网站示例搬动目录。
- 默认名字已被文件或非空目录占用时，尝试 `PandaDrawer-Dev-2`、`PandaDrawer-Dev-3` 等编号；正式版对应 `PandaDrawer-2`、`PandaDrawer-3`。不覆盖或接管已有内容。无法取得可用用户目录时，使用平台私有工作区。
- 平台新建网页应用使用可读名称加应用 ID 摘要分配目录；同名时继续加数字后缀，不能用目录名反推实例身份。
- Git 下载以来源名称分配目录，已占用时追加 `-2`、`-3` 等后缀；Windows 不适用的名称会转换成本地可用名称，不改作者文件内容。

目录分配依据：[新建应用目录](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/projects.rs)、[名称生成](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/project_directory_name.rs)、[Git 目录分配](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/source_publication/unix.rs)、[Windows 目录分配](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/source_publication/windows.rs)。

## 数据库操作规则

- 应用打开时，只通过平台 CLI 的 `db` 命令或网页 SDK 修改数据库，不直接修改数据库文件。
- 应用完全关闭后，可以直接使用 SQLite 工具。不要替换使用中的数据库及其 `-wal`、`-shm` 文件。
- 改表结构前先读取实际 schema，同步更新数据库说明文档。不要凭旧文档假定当前结构。
- 不要把构建成功当作数据修改已经撤销。平台不备份、不迁移、不校验业务数据。
- SQL 与附件上传不在同一事务里。写入结果不明时先查实际结果，不自动重放操作。

## Git 与数据

- Git 保存代码，不能代替数据备份。起始项目的 `.gitignore` 默认忽略 `data/`；是否把数据纳入 Git 或另行备份，由用户决定。
- 文件数据、浏览器数据、平台私有配置是三类不同内容。复制源码文件夹或恢复代码版本，不能据此宣称三类数据都已恢复。
- 需要长期保存的数据，放到 `data/` 或 SDK 数据库，并明确备份和导出方式。
- 删除或清空数据库、文件数据、附件前，先向用户说明影响并取得明确同意。

## 配置和密钥

- 在 `panda.json` 的 `configuration` 声明字段、用途和获取方法，真实值由用户在应用配置表单填写。
- Node 配置和密钥按字段 `name` 注入目标后端；网页只能读取普通配置，不能声明 `secret`。
- 密钥以明文保存在受当前用户文件权限保护的平台私有文件中，不是系统加密密钥库。
- 不读取或输出真实 `.env`、平台密钥文件或其他应用配置，不让用户另建 `.env` 或把 Key 发给 AI。
- 密钥的服务商权限、网络与额度，要由实际调用结果确认；保存本机配置不等于服务调用成功。

## 日志、缓存与临时文件

- Node 普通日志写 stderr，由平台写入 `current.log`，上一次运行保存在 `previous.log`；stdout 只输出控制协议。
- 每个日志文件保留最新的约 256 KB，平台将配置中的秘密值替换为 `***`。应用仍不要主动输出密钥或个人敏感内容。
- 使用交接给出的日志目录排查，不修改或清空平台日志；缓存与临时文件使用平台注入目录，不借用其他应用目录。

## 删除一个应用

| 目录归属 | 保留 | 删除 |
| --- | --- | --- |
| 用户选择的本机文件夹，包括空文件夹生成的起始项目 | 整个项目文件夹、代码、Git、`data/` | 该实例的平台登记、配置、密钥、日志、缓存、任务及浏览器数据 |
| 平台新建或从 Git 下载的目录 | 用户已完成并保存到所选外部位置的下载、导出文件和附件副本 | 整个受管项目文件夹及其代码、Git、依赖和文件数据，以及平台私有配置、日志、缓存、任务和浏览器数据 |

- 两种方式都清除浏览器数据。同一个本机文件夹删除后重新添加，也不能读回原实例的 localStorage、IndexedDB 或 Cookie。
- 删除不进入平台回收站或系统废纸篓，不可撤销。以确认窗口列出的当前范围为准。

## 清理所有本机项目内容

- 设置中的「清理所有本机项目内容」先展示范围，再要求两次明确确认，永久清理平台管理的项目代码、数据、依赖及私有配置、密钥、日志、缓存和网页存储。
- 用户选择的原文件夹及 `data/`、范围之外已完成的外部交付文件和软件程序保留。必要的实例锁与环境身份文件、范围外系统缓存和偏好文件可以保留，不宣称整台机器零残留。
- 先停止外部编辑器、AI 和数据库工具访问待清理目录，核对结果；未成功的部分按界面剩余范围处理，不把部分失败当作完成。

[平台规则总览](/wiki/platform-rules/) · [panda.json 格式](/wiki/panda-json/) · [应用类型与运行方式](/wiki/app-types/) · [添加要求](/wiki/adding-apps/)
