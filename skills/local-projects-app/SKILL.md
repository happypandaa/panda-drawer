---
name: local-projects-app
description: 接管、修改和验证熊猫小抽屉平台内的业务应用。用于外部 AI 阅读项目交接说明、编辑 React 源码、通过平台 CLI 操作 SQLite、构建并在平台原生应用窗口验证；不用于开发平台自身的 Tauri/Rust 工程。
---

# 熊猫小抽屉应用开发

为用户完成当前业务项目的功能，让用户在 AI 离线时仍能通过网页正常查看、录入和管理数据。用户的具体需求和已授权范围优先；不要因为本技能增加额外审批。普通兼容的页面和业务数据修改不强制进入维护；改表和大批量修正先核对数据兼容性，恢复按对应现役操作的预览、确认和中断处理流程执行，不新建 ZIP 基线或维护会话。

本技能随项目放在 `.agents/skills/local-projects-app/SKILL.md`。Codex、Claude Code 等工具可以直接读取它，不依赖客户端自动发现。后文 project-v1 章节的 `PROJECT_ROOT` 指包含 `project.json` 的**业务项目根目录**，不是本技能目录或平台源码目录；业务相对文件路径从这里解析，本技能的 `references/` 资源链接从技能文件所在目录解析。

## 先识别环境、接入类型与窗口约束

默认使用构建好的前端、平台 SDK 和 SQL／附件完成应用，确需后端 SDK、秘密凭据或服务端接口时才引入受管 Node。作者构建时使用 Node/npm，不意味着用户运行也要安装它们；交付前端完整产物、可直接执行的后端 JavaScript 和完整运行依赖。应用运行使用固定私有 Node；本地目录登记和应用启动不安装依赖，从 Git 添加只在项目文件夹准备跳过脚本的生产依赖。Mac 内部版配套 Node/npm 供 project-v1 构建和 Git 项目依赖准备使用；用平台 `env doctor` 核对实际工具，已有显式选择失效不自动回退，开发工具选择不改变运行 Node。公开仓库从平台“从 Git 添加”接入，不再提供独立来源捕获、安装候选或 Node 构建更新入口。Node 的修改、构建和 Git 版本由外部 AI 在项目目录完成；project-v1 的平台构建、历史查看和明确源码／数据取回继续按当前 CLI 提供的入口执行。

Node 尽量只增加一个应用服务，不依赖用户另装 Claude Code、Python、Docker、数据库服务器或额外 CLI。确有外部环境要求时，在选型前说明，分别写明本机软件与在线账号／Key 的准备、用途和缺失影响；声明不代表平台已经支持。具体环境、交付与当前构建边界同样在下述随附约束中，先读后选方案。

先阅读随本技能携带的[应用环境、窗口与后端约束](references/app-window-runtime.md)：应用在平台的一个原生窗口中运行，重复打开只聚焦；Node 类型在打开时后台启动，关闭应用窗口就停止。不要用新浏览器、`window.open`、`target="_blank"`、额外 WebView 或脚本业务弹窗扩展运行方式。原生文件选择和系统权限继续按平台实际能力及用户操作使用。

业务窗口可通过清单可选顶层 `window` 声明初始尺寸，之后默认记住每个项目的普通窗口尺寸；字段、`rememberSize` 与有效 Node 清单在下次启动生效的规则见随附[应用窗口尺寸](references/app-window-runtime.md#应用窗口尺寸)。修改有效 Node 清单中的尺寸在下次真正启动时接受；已有窗口仍只聚焦，本机记忆尺寸按原规则优先。这不开放网页原生窗口权限，也不改变平台管理窗口尺寸。

平台入口统一为“我的项目”，网页／Node 用类型图标区分，详情与配置收在页面内带 tabs 的弹窗；缺必填时点击进入先开配置 tab，保存不自动启动。macOS 管理窗口 X 只隐藏并保留 Dock、已开的业务窗口和受管服务，Dock／Reopen／重复启动恢复同一个管理窗口及页面状态；菜单退出／Cmd+Q 才安全收尾全部，非 macOS 管理窗口 X 维持正常退出。业务窗口 X 只收尾本应用受管 Node 及普通子进程，不杀用户独立 Node。隐藏管理窗口不等于服务已停止；不能因窗口消失或主进程退出就假报清理成功，硬崩溃／脱组后代按真实能力记录限制。以上覆盖旧“管理窗口 X 退出”要求，旧项目随附说明不自动改写。

本技能下文的 SQLite、构建、维护和迁移命令针对已有 `project.json`、经当前 CLI 确认的 **project-v1 业务项目**。只有 `local-app.json`、在“我的项目”以 Node 类型展示的本机源码槽属于另一条内部接入路径：先读其 `documentation.readme`、`documentation.development` 和声明中的配置；列表统一不合并存储身份，不要伪造 project ID、补写数据库、调用未开放的 Node CLI，或套用 project-v1 的备份／导出／迁移能力。作者修改前保全已有源码和真实数据，不从原 `.env` 或其他应用借 Key。

## 接手项目

先读 `PROJECT_ROOT/README.md`、`project.json`、`docs/database.md` 和 `docs/platform.md`。README 保存实际项目 ID、CLI 路径、业务目的、使用流程和上次交接；数据库说明解释业务含义，真实结构还需查询。若这些文件缺失，先检查现有目录、清单和平台详情，基于真实信息补齐，不能套用另一个项目的 ID 或 schema。

从 README 或平台复制出的交接文字取得 CLI 的绝对路径。路径含空格时按 shell 规则引用。以下 `PROJECT_CLI`、`PROJECT_ID`、`TASK_ID` 是本次已核对的实际值，不是固定命令名称或示例 ID：

```sh
"$PROJECT_CLI" service status --json
"$PROJECT_CLI" project info --project "$PROJECT_ID" --json
"$PROJECT_CLI" db schema --project "$PROJECT_ID" --json
```

核对清单、返回 ID、项目目录和服务身份一致再写入。当前开发版为 `local-projects-dev`，服务标识为 `dev.happypandaa.pandadrawer`，环境为 `development`；开发数据与正式数据隔离。返回旧身份时先核对正在使用的安装与最新交接，不自行改标识、搬目录、替换控制记录或清空资料。2026-10-25 的旧开发环境清空是用户针对平台维护的一次独立授权，不适用于普通业务项目操作，也不提供身份迁移命令。不要改服务发现配置来接入另一环境。CLI 会验证服务身份并处理管理认证，不要自己读取或复制控制令牌、猜端口或调用未认证管理地址。若 CLI 路径失效，从平台重新取得交接说明；若服务离线，启动对应桌面应用后重试诊断。CLI 本身不会启动服务。

## 目录与运行边界

| 路径 | 用途与修改方式 |
| --- | --- |
| `app/src/` | React + TypeScript 源码；通常从 `App.tsx` 开始 |
| `app/public/` | 随构建发布的静态资源 |
| `app/package.json`、`app/package-lock.json` | 依赖与精确锁定；有意改依赖时同步更新锁文件 |
| `app/vendor/` | 项目自带的固定版本 SDK tarball，属于源码材料 |
| `dist/`、`.platform/` | 平台发布产物、任务与来源状态；通过构建更新，不手工覆盖 |
| `data/main.sqlite` | 此项目独立的持久化数据库 |
| `data/attachments/` | 用户照片、音频、文档等持久化附件 |
| `project.json`、`meta/` | 项目元数据和能力声明；保持 ID 与存储路径稳定 |

标准新项目自带可打开的起始页，不需要用户先安装 Node 或执行构建。修改源码后才需要重新构建。旧项目是否可打开以其实际产物和平台状态为准，不自动补模板或升级 SDK。

对既有 project-v1，Rust 平台提供本地服务、SQLite 和附件访问，静态类型运行不需要独立 Node 后端、Vite 开发服务或 Docker。应用从平台原生窗口打开；旧浏览器接口仅按实际能力用于必要诊断，不作为新应用的默认打开方式。Node 类型由平台管理后端，不照搬本段的静态运行结论。业务持久化使用平台 SDK，不以 localStorage、IndexedDB 或源码中的 JSON 文件替代项目数据库。

尽量复用项目已有的黑白风格组件和样式，可按用户需要扩展。不要为接手项目而重建组件体系或隐式升级依赖。SDK 接口、类型和行为以项目自己的 `docs/platform.md`、包版本及类型声明为准；平台更新不代表项目依赖已经更新。

## 操作数据库与附件

先查询 schema，再结合 `docs/database.md` 决定 SQL。外部 AI 使用 CLI，网页使用 `@local-projects/sdk`。将 JSON 请求写入文件，使用 `--input` 提交；不要把用户内容插入 SQL 文本。

```sh
"$PROJECT_CLI" db execute --project "$PROJECT_ID" --input request.json --json
"$PROJECT_CLI" db transaction --project "$PROJECT_ID" --input transaction.json --json
```

单语句请求形如 `{"sql":"SELECT ? AS value","params":["hello"]}`。多语句请求为 `{"statements":[{"sql":"...","params":[]}]}`，由平台托管整个事务；不要跨请求发送 `BEGIN` / `COMMIT`。保存失败时展示实际错误，不将失败显示为成功。写入响应丢失或结果未知时，先查询确认是否已提交，再决定下一步，不能自动重放写入。

每次 schema 变化都同步数据库说明：表用途、列类型与可空性、主外键、索引、默认值、单位、枚举、时间约定、变更 SQL 和已有数据兼容性。不要在构建脚本或页面启动时清空、重置业务数据库。尽量保持旧网页可继续使用变更后的数据：**构建失败保留旧网页，不会撤销此前已经提交的 schema 或数据变更**。

保留已注册 SQLite 文件及其 WAL/SHM，不用替换文件、删除数据库或改注册状态来修复问题。常规写入走平台接口；若用户明确要求外部数据库工具操作，另行核实平台的运行限制，完成后用 `project refresh` 通知页面重新查询，不能把它当作构建或数据库恢复命令。

网页附件通过 SDK `files` 接口保存和读取。数据库保存相对 `data/attachments/` 的逻辑路径，不保存临时访问 URL。用户素材不放进 `app/public/` 或 `dist/`。SQL 与附件上传不是一个事务，业务流程需处理部分成功；SDK 的具体调用及限制见项目 `docs/platform.md`。

## Git 版本与恢复状态

修改前先读取当前 CLI 帮助及 `project git status`，按用户确认的作者与范围保存 Git 版本。源码版本和数据版本分别核对，不把源码提交称作数据库备份；Git 不可用时明确未保存，不另建 ZIP 或私有恢复仓库。普通源码编辑、SQL 和候选构建不要求 ZIP 基线；优先采用兼容的数据事务，由平台的具体替换操作协调必要停写、失败保留与重启检查。

### 数据库与附件的 Git 取回

project-v1 在 macOS 可显式预览并取回本机 Git 中已保存的数据库和附件；源码取回与数据取回是两项独立操作。只有用户明确要求取回数据时才使用 `project git data-restore-preview` 并核对当前 CLI 的全部新鲜绑定，再明确提交 `data-restore`；预览不自动执行。先查 `project git data-recovery`；pending 或 blocked 时保留原件、日志和候选，仅按核实后的允许方向继续或撤回同一操作，不删除现场、不重新登记、不直接替换 SQLite。not_committed 也可能已有部分文件移动，不能据此说数据未变。完成后页面代次已更新，先处理草稿再从平台明确重新打开。任意 Node 应用自己的数据库不适用这一流程。

`maintenance status --project "$PROJECT_ID" --json` 仍提供活动任务、构建状态和恢复门禁。旧备份读取／恢复和维护开始／结束命令已退役，不再调用；存在恢复门禁时按对应现役操作查询和处理，不修改控制记录跳过保护。

## 导入导出、复制与直接删除

仅在用户任务涉及这些操作时使用；正常编辑项目不需要先复制或删除。先核对当前 CLI 帮助，旧安装包可能不支持 M4。所有变更返回任务 ID，等待原任务最终状态；写入响应丢失时查询任务和项目，不自动再次复制、导入或删除。

```sh
"$PROJECT_CLI" project export --project "$PROJECT_ID" --input export.json --json
"$PROJECT_CLI" project import --input import.json --json
"$PROJECT_CLI" project copy --project "$PROJECT_ID" --input copy.json --json
"$PROJECT_CLI" project delete --project "$PROJECT_ID" --json
```

导出输入为 `{"destinationPath":"/绝对路径/project.zip"}`，必须使用新文件名，不覆盖已有文件。普通导入为 `{"archivePath":"/绝对路径/project.zip"}`，可选 `name`；每次都生成新平台实例 ID，不提供恢复原 ID 的导入模式。`copy.json` 可指定副本名称。既有控制状态要求 `maintenanceId` 时先读取当前状态，不猜测 ID 或绕过恢复门禁；完整快照前暂停外部直接文件写入。

导入与复制保留业务主键、业务 UUID 和附件名。完成后使用返回的新平台 ID 和 `project handoff` 核对项目根，不依赖复制过来的业务 README 内旧路径或旧 ID。平台不会全局重写用户文字，也不因复制升级 SDK。已有可复用网页无需 Node 即可打开。

新实例导入／复制只接受状态明确可用、无待构建要求且数据库关系检查通过的归档。维护、未知、仍需构建或有外键问题时返回 `LEGACY_ARCHIVE_REQUIRES_REVIEW`，在登记实例前拒绝并保留原归档和候选位置；不创建 ZIP 基线、不清除原要求。先检查来源实例和错误原因，不自动重复导入或修改归档状态冒充通过。

`project delete` 直接永久删除选定实例的全部平台管理内容，包括源码与 Git、网页、数据库、附件、依赖、配置、任务和恢复材料，不进入回收站，也不保留成功删除记录。执行前核对项目 ID、实际目录和用户要求删除的范围，不将删除作为普通编辑或冲突处理的例行步骤。用户选择的外部原始目录、原始导入 ZIP，以及已完成交付到用户自选外部目录的导出 ZIP、网页下载和附件副本保留；项目内原件、平台快照／归档、内部暂存和已登记外部未完成文件仍随项目清除。部分失败或结果未知时核对原任务与残留，不自动重放删除。

删除 JSON 可显式给出已核对的 `expectedDocumentEpoch`，CLI 不替换该值；省略时先只读查询当前 `project info.documentEpoch` 再绑定提交。读取失败或身份缺失时不会删除；过期身份由服务拒绝，不重新读取身份来自动重试永久删除。HTTP 直接调用必须自行提供该字段，不允许无绑定删除。

Node 实例在原生管理窗口预览并确认删除，没有 Node CLI 删除入口。平台下载或生成的项目文件夹（含 Git、依赖和 AI 改动）与实例数据、配置／秘密、日志、缓存、任务和归属一起删除；用户选定的外部原始源码及普通 Git 内容保留，平台私有内容和明确归属的内部材料仍清除。旧 version 2 项目在列表和详情显示不可用，不能启动；删除仍覆盖旧程序代次、安装候选和平台源码，再从 Git 或本地目录重新添加。不要手工递归清空 Node 槽或改控制记录绕过删除核对。

## 工作区迁移后的继续工作

工作区迁移影响全部项目，仅用于用户要求移动工作区的任务。使用 `workspace info --json` 查实际位置；迁移请求为 `workspace migrate --input migration.json --json`，文件内容为 `{"destinationPath":"/绝对路径/新的空工作区"}`。目标必须是独立、空、可写的本机 APFS 目录，不能是源的父子目录、同步盘、应用包或固定控制目录。不要直接改 settings、移动项目注册树或建立兼容链接绕过协议。

迁移前停止外部编辑器、AI 和 SQLite 工具的写入，并处理未解决的 `recoveryRequired`；平台会等待自身任务，保留维护与全部历史现场后复制校验。只查询同一迁移任务，不因连接切换自动再次提交。提交新位置后即使目标暂不可用，也不自动回退到旧副本；旧根是保留现场，不继续编辑或手工清理。

迁移完成后重新获取服务状态与 `project handoff`，改到返回的新项目绝对路径工作。旧编辑器工作目录不会自动跟随；旧管理凭据和网页会话失效，旧文档需要用户明确重开。固定开发身份不随工作区换位置。需要清理迁移原目录时，使用当前 `workspace cleanup-plan` 查看服务生成的范围，再按预览和明确选择调用 `workspace cleanup`；不手工递归删除旧根，也不把历史管理讨论稿中的其他接口当作已实现。

## 修改、构建与验证

1. 根据用户需求修改业务源码，设计可用的空状态、加载状态、表单校验、保存反馈和错误恢复。无需 AI 在线才能完成日常操作。
2. 需要新依赖时有意修改 `app/package.json`，用 npm 更新并保留对应锁文件。不要删除锁文件绕过安装错误，也不要用不同内容覆盖同版本 SDK 包。SDK 升级应明确更新版本、vendor、锁文件和说明。
3. 用 `env doctor --json` 检查**桌面服务实际选用**的 Node/npm。当前支持 Node `^22.12.0 || ^24.0.0`，推荐 24 LTS；有受管资源的环境默认使用配套工具，已有显式本机路径优先且失效不回退。不是任意更高主版本都已支持。缺少构建工具影响构建，不影响已发布网页和数据操作。
4. 提交平台构建，再检查任务状态、最终结果和错误原因。构建脚本必须接受 `--outDir ABSOLUTE_DIRECTORY`；平台复制 `sourceDir` 内普通源码，每次在独立候选执行 `npm ci`，构建到暂存目录并验证后发布。不复用或修改原源码 `node_modules`；只支持源码目录内 file/workspace 依赖，候选不带根清单、数据或 Git，脚本不能假设它们存在。候选不是系统权限沙箱。不要自行向 `dist/` 拷贝产物，也不要在构建中操作业务数据或启动常驻进程。

```sh
"$PROJECT_CLI" env doctor --json
"$PROJECT_CLI" project build --project "$PROJECT_ID" --json
# 从提交结果读取 taskId，再设置 TASK_ID。
"$PROJECT_CLI" task wait --task "$TASK_ID" --timeout 30 --json
"$PROJECT_CLI" project info --project "$PROJECT_ID" --json
```

`task wait` 返回 `waitTimedOut: true` 只表示本次等待结束，任务仍可能运行；继续查询同一任务。平台不再保存独立构建／npm日志；失败时读取同一任务的错误码和 `error.details.processDiagnostics`，其中包含命令、实际退出码／信号、工具版本与有界脱敏输出。旧任务没有该字段时不能补猜原报错；需要时在已授权的独立副本复现。只有任务为 `succeeded` 才表示发布成功；成功进程的警告不代表失败。失败时查明原因、修复并重新构建，旧产物与业务数据保留。构建输入在过程中变化会导致发布失败，构建期间不要继续编辑同一项目。`buildInfo.sourceState` 描述当前源码与发布产物的匹配情况，不代替任务结果或页面验证。

成功后优先通过平台原生应用窗口验证。既有 project-v1 如需诊断当前会话，可使用实际提供的入口命令：

```sh
"$PROJECT_CLI" project url --project "$PROJECT_ID" --json
```

在平台的唯一原生应用窗口验证核心操作、数据持久化与关闭／重开；必要的 `launchUrl` HTTP／浏览器诊断只能补充证据，不替代原生窗口验收。不把直接双击 HTML、Vite 地址或外部浏览器页面当作标准应用运行。`launchUrl` 含短期、单次使用的入口凭据，不写进源码、文档或日志；平台重启或入口过期后重新获取。

新构建通知 `project.updated` 应显示明确的重新加载入口。`PROJECT_RELOAD_REQUIRED` 表示需要重新打开页面；不要自动刷新丢弃草稿，也不要静默改变 SDK 持有的文档代次后重放操作。打开已有桌面项目通常复用窗口，仅聚焦不会载入新构建；验证新版本时使用明确的重新加载操作。若无法操作原生窗口，完成可做的数据和 HTTP 诊断，并如实记录原生窗口验证未完成，不另开多窗口绕过要求。

## 当前能力与交接

当前开发版提供 `service status`、`env doctor`、`env node set/clear`、`env git doctor/set/clear/check`、`project list/create/info/url/refresh/build/handoff/export/import/copy/delete`、`db schema/execute/transaction`、`task get/wait`、`maintenance status`、`storage info/scan/files/cleanup` 和 `workspace info/storage/migrate/cleanup-plan/cleanup`。旧项目自带的 skill 与 SDK 不随升级覆盖，实际命令以安装包 CLI 帮助为准。空间与历史清理只使用当前 CLI 提供的预览及明确选择流程，不调用已退役的备份、回收站或独立源码命令。

`env git doctor --json` 只读取服务选定 Git 与系统 SSH 的工具状态；系统 SSH 仅检查 `-V`，`authentication.state: not-checked` 不代表账号或仓库访问已通过。`env git set --path /绝对路径/git --json` 保存独立选择，`env git clear --json` 明确清除后恢复平台受管默认；不改变 Node 设置。旧显式路径优先，失效不自动回退；设置无法读取／解析时 `selection: unresolved`，不推测已有默认选择。本机历史列表和源码取回有独立命令；源码初始化、保存与新建分支由外部 AI 使用作者仓库的标准 Git 负责，不将工具可用当作额外能力已实现；不为工具诊断读取或复制私钥、修改用户全局 Git／SSH 配置。Git 不可用不影响成品应用运行。

需要检查指定公开仓库时，明确执行 `env git check --url https://github.com/owner/repository.git --json`；也支持标准 Gitee HTTPS 仓库地址。检查在 20 秒执行预算内匿名读取 HEAD，不读取账号凭据、不克隆或下载仓库、不推送或修改项目。结果绑定该地址与本次时间，不改变 `doctor` 的认证未验证状态；`readable-no-head` 不表示仓库为空。不要把远端检查加入普通离线工具诊断，不将其扩为 SSH／私有仓库认证、仓库获取或完整路线已交付。

Node 项目从平台“从 Git 添加”入口接入公开仓库，由后端统一完成下载、检查、依赖准备和登记。项目文件夹就是运行目录，不创建安装副本；Git 项目由平台管理并随项目删除，外部原始目录保留。平台只执行跳过脚本的 npm ci，不执行仓库构建或生命周期脚本；需构建但缺运行文件时，让 AI 构建后用“添加本地项目”添加。失败或取消先确认写入进程已停止，再回滚；登记结果未知时查询原任务，禁止重复添加或自行删除材料。源码资料库和独立捕获／安装候选入口已移除。

源码版本交给外部 AI 使用项目自己的标准 Git 管理。平台不再提供 `project git init/save/save-data/branch-create`，也不新增平台保存事务 journal。提交前核对作者身份、变更范围和秘密；不默认提交数据库、附件、配置、凭据、依赖或运行产物，不自动推送业务数据。已有 `version-data` 数据版本与源码／数据恢复功能保留；旧未完成数据保存仍可使用 `project git data-save-recovery` 只读核对，并在明确授权后以新鲜摘要调用 `data-save-finalize` 完成原任务，不能据此新建版本。

`project git status --project PROJECT_ID --json` 只读检查当前范围与改动；`project git versions --project PROJECT_ID --limit 50 --json` 查看当前 HEAD 可达的本机历史。Node 的 Git、构建和回退由外部 AI 在项目目录完成，关闭窗口后修改、重新进入生效；源码 Git 不恢复业务数据。可以修改有效 local-app.json 的 app.id、版本号和配置声明，平台实例 UUID 及数据不变。

用户明确要取回已保存源码时，先停止对该项目的外部编辑，保留并保存现有改动。用 `project git source-restore-preview --project PROJECT_ID --input FILE --json`，输入 `targetCommit` 为完整40位小写SHA-1本机提交号；核对允许范围、逐文件增删改和 `canApply`。不接受任意revision／SHA-256，不联网取对象，目标清单必须逐字节一致。确认后调用 `project git source-restore --project PROJECT_ID --input FILE --json`，完整输入来自本次预览的 `targetCommit`、`expectedHead`、`expectedBranch`、`expectedSnapshot`、`expectedPlanDigest`；等待同一任务最终结果，不自动重发。

取回只将允许源码放回工作树作为待保存改动；不改变HEAD／索引，不回退活库、附件、当前程序，不自动commit/build。取消或断开可能已改变部分源码：用 `project git source-recovery --project PROJECT_ID --json` 读取真实现场。仅在允许动作且当前任务已授权该处理时，调用 `project git source-recover --project PROJECT_ID --input FILE --json`，输入当前 `restoreTaskId`、`expectedPlanDigest`、`expectedJournalDigest`、`action:"continue"` 或 `"undo"`。新结果到来前不重试，不删除或修改journal，不用强制重置解除保护。未完成现场暂停开发写入／新的AI接手，当前网页、SQL与附件继续使用；遇第三种文件状态保留双方内容并说明阻断。成功取回后按需求再保存版本、构建和验证数据。

当前开发版资源限额标记为 `internalTest`，查询运行时限制并处理超限错误，不在业务说明中承诺这些数值是正式产品默认值。联网仅按项目 `runtimeRequirements` 声明的公开接口使用；不把私有 API 密钥、管理令牌放进前端、清单或数据库。录音、拍照等能力还需要浏览器权限和实际环境支持，声明本身不等于授权成功。

把功能与用户操作方式、源码/依赖/schema 变化、构建任务 ID 和结果、原生窗口验证及补充诊断、已知限制写回 README 和相应文档。只记录实际完成的检查；保留已有数据，不将开发测试结果扩张为正式环境保证。下一位 AI 应能仅凭项目文档继续工作，无需读取平台源码或依赖上一段聊天。
