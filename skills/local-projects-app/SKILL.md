---
name: local-projects-app
description: 熊猫小抽屉的平台说明。把开源软件改成小抽屉应用、检查并添加本机项目、新建应用，或接手、修改、修复平台里的应用时使用。agent 应用的日常任务以应用自己的 AGENTS.md 为准。
---

# 熊猫小抽屉平台说明

熊猫小抽屉是本机小应用平台。平台负责启动和停止应用、保存配置和密钥、记录运行日志、按需构建；改代码、版本管理和回退由 AI 完成。

这份说明随平台安装，是平台规则的唯一详细来源。复制给 AI 的提示词给出应用信息和硬规则，其余以这里为准。

同一目录下的资料（路径相对本文件）：

- `references/panda-json.md`：panda.json v2 字段、规则和示例。
- `references/node-backend.md`：可选 Node 后台的控制协议、环境变量和日志。
- `references/app-window-runtime.md`：窗口、网页运行环境、数据和密钥。
- `references/agent-apps.md`：agent 工作方式与数据库短事务规则。
- [references/porting.md](references/porting.md)：评估和改造现有项目或开源软件，核对数据存放与运行方式。
- `../../schemas/panda.schema.json`：修改清单时用于校验的 JSON Schema。
- `../../sdk/panda.d.ts`：网页 SDK 类型。

## 一种应用

应用由**网页（必有）+ 后台（可选）+ agent 规则（可选）**组成。项目根目录的 `panda.json` 使用 `manifestVersion: 2`，不写 `runtime`。有没有后台只看是否声明 `backend`，agent 标记与后台互相独立。

所有应用直接运行登记的项目文件夹，不另存运行副本、构建历史或版本。网页可以是普通 HTML、Vite 或 React 的构建结果；声明 `platform.sdkApiVersion: 1` 后可使用平台 SQLite、附件和 AI 交接 SDK，有没有后台都一样。后台使用 Node 24，遵守平台控制协议。

不支持 SSR、多个后台服务、Python、Java、Docker 等其他运行时。平台管理的依赖准备和构建使用 npm 及 npm v2/v3 格式的 package-lock.json。

## 数据放哪

- 需要保存用户数据时，默认用 `data/main.sqlite`。项目文件夹带着数据一起走，AI 也能用 CLI 的 `db` 命令查询和修改。
- 只有网页的应用用 SDK 的 SQL，图片和文件用 `panda.files`，存到 `data/attachments/`。
- 有后台的应用用 Node 24 自带的 `node:sqlite` 打开 `APP_DATA_DIR/main.sqlite`，遵守下文的短事务规则。网页可以通过自己的后台读写。避免使用依赖安装脚本或原生二进制的数据库库，例如 better-sqlite3、sqlite3 或使用原生引擎的 Prisma。
- 以上两种应用都在清单中声明 `platform.sdkApiVersion: 1`，这是 CLI 查表和写库的条件。应用首次打开时自行建表，使用 `CREATE TABLE IF NOT EXISTS` 和表结构版本号，将表结构写到 `docs/database.md`。
- localStorage 和 IndexedDB 只用于主题、上次打开的页面等可以丢弃的偏好。CLI 读不到这些浏览器数据，同一文件夹重新添加也无法恢复。
- 大文件存 `data/` 或附件，缓存放 `APP_CACHE_DIR`。确实需要云端同步、多人共享，或用户明确要求保留云服务时，可以保留相应的云端存储。

## 多语言

平台和应用各自管理语言。平台语言影响管理界面，以及清单中供平台展示的名称、简介、配置说明、网络和设备用途；应用内部界面和应用自行编写的 AI 提示词由应用管理。应用可以使用 `navigator.language` 跟随浏览器语言，也可以提供自己的语言设置，不要求特定国际化库。

平台不通过 SDK 或环境变量指定应用语言，也不将平台切换语言通知给应用。不要假设平台语言与应用内部语言相同。

需要翻译平台展示文字时，在 `panda.json` 的可选 `localization` 中按 `zh-CN` 或 `en` 提供覆盖，缺失字段回退原文。例如 `"localization":{"en":{"app":{"name":"My App","description":"A personal tool"}}}`。配置翻译按字段 `name` 引用，选项按 `value` 引用；翻译不改变配置值或权限。完整格式及校验规则见 `references/panda-json.md`。

## agent 应用

声明 `agent.instructions` 的应用采用 agent 工作方式，应用根目录也是 agent 的工作目录。

- **日常任务**：按应用 `AGENTS.md`（或声明的规则文件）执行，不需要重读平台说明。
- **修改应用**：按下面“接手和修改”，同时读应用规则。修改规则前告诉用户改什么；表结构升级只加表、加列，并同步数据库说明和受影响的 agent 步骤。改完先重新加载，让升级完成后再执行日常任务。
- **创建 agent 应用**：读 `references/agent-apps.md`，把其中底线写进应用规则。

## 用法一：接手和修改项目

1. **先理解，不修改。** 读 panda.json、README、声明的开发文档和 agent 规则。有后台时用 `app logs` 看日志。用一两句话说明应用做什么，再问用户想改哪里。
2. **按改动大小沟通。** 文字、颜色和布局小改说一句后直接做；涉及数据、表结构或较大的改动，先说明计划，等用户同意。
3. **用 Git 保存版本。** 检查 Git 状态并保留已有未提交工作；回退代码用 Git。Git 不能代替数据备份。
4. **只改应用目录。** 不修改平台登记、私有配置、密钥或日志，不读取或输出真实 .env 和密钥。配置项在清单声明，由用户在平台表单填写，不让用户把 Key 发给 AI。
5. **动数据先问。** 删除或清空 data/、数据库和附件前，说明影响并取得用户明确同意。
6. **修改后重新加载。** 代码改完、运行文件准备好后，确认用户没有尚未保存的编辑，再用 `app reload --app <id>`，或请用户点标题栏「重新加载」。有后台时会重启后台。没打开的应用会被打开；输入内容可能丢失。清单无效或缺配置时旧实例继续运行；启动失败会明确显示原因，修好后再 reload，平台不回退代码。
7. **交代实际结果。** 说明改了什么、数据是否变化、做了哪些检查和哪些未验证，同步应用文档。用户可以继续在当前对话提需求。
8. **使用平台提供的工具。** `env doctor --json` 的 `buildEnvironment` 给出 Node/npm 路径；`env git doctor --json` 给出 Git 路径。CLI 连不上时保留代码并说明缺的步骤，不绕过平台限制。

### 网页、构建和 SDK

- `development.build` 对所有应用可用，只构建网页。按 `development.sourceDir`（默认 `.`）及声明脚本执行，产物位置以 `web.root` 为准。也可以运行 `app build` 后用 `task wait` 等待。构建失败不会恢复原文件。
- HTML 用 `<script src="/_platform/sdk.js"></script>` 引入 SDK，得到 `window.panda`，无需安装 SDK npm 包。SQL/附件需 `platform.sdkApiVersion: 1`，API 在 `/_platform/api/v1/`。不要从 URL 猜应用 ID。
- 页面通过 `/_platform/config.json` 或 `panda.config()` 获取公开配置；密钥只注入后台，不能返回给页面。
- 应用运行时，AI 修改数据库用 CLI。agent 日常任务和应用后台可按下述短事务规则共用同一数据库。备份恢复见 [改造参考](references/porting.md)。

### 添加或移除后台

需要后台或密钥时：

1. 在 `panda.json` 加 `backend`，填写后台入口、Node 24、协议版本 1 和业务 API 前缀；前缀不能以 `/_platform` 开头。
2. 准备可直接运行的后台文件与依赖，平台网页构建不负责编译后台。配置/密钥在 `configuration` 声明，由用户填写。
3. 按 `references/node-backend.md` 实现控制协议：stdout 只输出协议消息，普通日志写 stderr；收到关闭指令要保存并退出。
4. 运行 `app reload --app <id>`。缺必填配置时请用户在主窗口填写，保存后再 reload。失败用 `app logs --app <id>` 查原因。

移除后台时，同时移除所有 `secret: true` 的配置声明，再运行 reload。切换成功后平台删除旧密钥并提示；失败时保留密钥。ID、地址、文件夹、data/、公开配置和浏览器数据均保留，不需要删除重加。

后台文件数据用 `APP_DATA_DIR`，缓存和临时文件用 `APP_CACHE_DIR`、`APP_TEMP_DIR`。日志为 `current.log` / `previous.log`，平台脱敏后供 `app logs` 查看。

### SDK 与后台共用数据库

每个后台连接设置 5000 ms 锁等待，保持 WAL、外键与现有数据库配置。读取、联网、下载、缩图等耗时工作在写事务外完成，最后以一个短 `BEGIN IMMEDIATE` 事务提交。不要替换数据库或删除 WAL/SHM；失败先确认回滚，结果不明先查实际结果，不盲目重放。外部写库不产生 SDK `data.changed` 事件，应用应在获得焦点和显式刷新时重新查询。

## AI 直接创建应用

平台运行时，AI 可以直接用提示词提供的 CLI 路径创建应用：

```sh
<cli> app create --name "应用名称" --description "一句话简介" --json
```

命令在平台工作区生成启用 SDK 的网页起始项目，返回应用 ID 和目录，归平台所有。用户还没说清需求时先讨论，确认后再创建。用户在平台点「新建」或选择空文件夹新建，生成相同起始项目；用户选择的文件夹归用户。

## 用法二：评估、改造并添加应用

适用于开源软件改造、已有本机文件夹的检查与修复、链接添加失败后的修复。读取 [改造参考](references/porting.md)，结合实际读到的代码判断。

### 改造开源软件或修复链接添加失败的软件

1. 默认交给平台管理。先用 `app reserve --json` 获得空文件夹和预留 ID，通过 `env git doctor --json` 找到平台 Git，把仓库克隆到预留文件夹本身，保留原 Git 历史。为阅读而下载可以在确认改造前完成。
2. 只读检查代码、README、清单和构建配置，说明实际读到的仓库和提交版本。读不到时直接说明。先给结论（可以改／改动较大／当前不推荐／信息不足），再说要改的地方、可能失去的功能、数据存放位置、需要用户填写的配置，以及是否有旧数据要另行导入。此阶段只检查源码和配置说明，真实 `.env`、密钥、业务数据库和私人附件留在原处。安装、运行、构建和登记都等用户同意。
3. 用户同意后，先用 Git 记录原始版本，原仓库保留为 `upstream`。保留 LICENSE 和版权声明，在 README 写明来源。改造完按参考里的清单逐项核对，再用 `app check --dir <文件夹> --json` 检查、用 `app add --reservation <id> --json` 登记。平台核对原目录身份后赋予平台归属。完成后告诉用户在「我的应用」里打开，以及需要填写的配置。

预留必须使用平台新建的空目录，保留目录本身的身份。应用根目录须是实际项目目录，管理目录、嵌套应用或链接均不适用。预留 24 小时后平台只清理身份一致的空目录，非空目录保留并报告。不要自行删除未知下载内容。

### 检查或修复本机文件夹

先说明实际读到的文件夹或版本，再只读检查并用 `app check --dir <文件夹> --json` 看平台检查结果。需要改动时先给方案，代码接入与旧账号或数据导入分别说明。用户同意、完成修改和检查后，用 `app add --dir <文件夹> --json` 添加，归用户所有。失败时说明实际原因。平台未运行时请用户先打开平台。

### 链接和位置

平台可从 GitHub、Gitee 公开 HTTPS 仓库添加。运行文件须随仓库提供；网页缺产物但声明构建时，用户确认后平台可以准备网页。后台依赖使用 package.json 和 npm v2/v3 package-lock.json；零依赖后台不需要安装。

macOS 从推荐列表或链接添加 agent 应用时，可选择默认工作区或用户选择的空文件夹。默认下载归平台，用户选择的空文件夹归用户。Windows 位置选择保持关闭。链接失败时「告诉你的 AI 工具出了什么问题」使用上面的 reserve 流程。

## 命令行使用规则

CLI 路径由平台提示词提供。所有应用使用 `--app <id>`：

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

- `app list/info/check/add/create/reserve/reload/build/handoff/logs`、`db schema/execute/transaction`、`task get/wait`、环境 doctor/远端 check、`storage info/files` 可按用户任务直接使用。数据库写入仍遵守上面的数据保护规则。
- `app delete`、`storage cleanup`、`env node/git set/clear`：先说明影响，再提交命令。平台会在主窗口显示请求和影响，由用户亲自确认；拒绝或 2 分钟未确认时什么都不做。没有跳过确认的命令行参数，不重复提交以绕过用户选择。

## 数据和删除

文件数据在应用 `data/` 下，由用户和应用安排备份，目录默认排除在 Git 外。浏览器数据（localStorage、IndexedDB、Cookie）在平台按应用 ID 隔离的窗口存储中，reload 和增减后台会保留。移出或删除应用都会清除这份存储，同一目录重加也无法恢复。

用户目录只能「移出小抽屉」，保留代码、Git 和 `data/`。删除平台管理的应用时，默认同时删除整个应用文件夹。用户可以取消勾选「同时删除电脑上的应用文件夹」，将应用移出并把文件夹留在原路径，之后可以用「本机文件夹」重新添加为用户目录。两种操作都会清除平台配置、密钥、日志、缓存和浏览器数据。归属不明或目录身份改变时平台保护应用目录。已保存到用户外部目录的最终导出和下载副本独立保留。
