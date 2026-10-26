# 熊猫小抽屉

Tauri 2 + Rust + SQLite 的本地应用平台。当前包含 M1 项目与数据核心、M2 固定 SDK、模板与构建、M3 备份维护恢复，M4 导入导出、复制、删除、旧回收站兼容和工作区迁移，以及 M5 项目概览、基本信息、空间管理、运行声明与标准媒体助手。产品名已确认为 **熊猫小抽屉**；[名称与图标](docs/branding.md)已接入内部代码。2026-10-25 用户明确选择[清空旧开发环境并重新开始](docs/development-identity-reset.md)，新固定身份为 `dev.happypandaa.pandadrawer`，实际清理与新环境验收另记。实际验收范围见各阶段记录，仍是隔离的内部开发版。

2026-10-24 已确认先收尾制作／获取／定制，再完成数据 Git 与旧备份退役，然后做 Mac 正式发行和自动更新；分享及 Windows 后置。[Node 开发与更新](docs/node-development-workflow.md)现已选择性接入：同一作者源码仓库用于本机 Git、隔离构建及显式更新，运行副本与源码分离；公开源码可明确选择分支，声明构建的来源须先明确构建。更新前保留当前数据契约检查，不引入其他分支的删除规则。当前真实工具及模拟界面证据见该页，不代表三场景原生、全部异常或正式发行已验收。

2026-10-24 新增[project-v1 的 Git 数据版本保存](docs/project-data-git.md)：明确保存当前一致性数据库、附件和源码，普通源码保存保留之前的数据版本；提交中断可独立核对并完成收尾。仅支持项目根标准 Git，Node 自有数据库未纳入，数据取回尚待后续实现。

2026-10-21 用户已确认[产品最终目标与四类使用场景](docs/product-goal.md)：帮助普通用户借助外部 AI 制作、获取、使用、定制和分享个人小应用，并由平台承担受支持应用的运行、配置、版本与数据保护。用户进一步明确技术优先，随后完善设计、交互和使用提示；后续按目标、范围、验收、能力缺口和整体计划重新推导批次。Git 纳入优先技术方案细化，具体实现和分批范围仍待定稿。

据此收敛的[技术实施计划](docs/technical-implementation-plan.md)按依赖列出 10 项修改：对象与目录、工具环境、获取检测、Git 版本、旧备份退役、受管构建、安装运行、AI 定制更新、干净分享及分系统验收。范围取舍已经确认，具体推荐实现仍需细化；计划不表示功能已经交付。

最新取舍为**仅保留项目 Git 版本管理，退役独立本地备份体系**，也取消另建私有 Git 恢复仓库的建议。[原备份精简评估](docs/git-backup-simplification-review.md)保留为历史盘点；数据库提交方向保留，`dist` 是否提交待评估。完整备份、恢复、复制与回收不再统一作为新类型安装前置，正常持久化、必要停写和失败保留当前实例仍须完成。[新建备份与维护基线入口已退役](docs/backup-retirement-admission-step.md)，新实例导入／复制已解除 ZIP 基线依赖；旧归档读取、同实例恢复、既有维护结束和中断核对继续兼容，旧资料完整保留。

2026-10-23 用户确认[直接删除规则](docs/application-deletion.md)：平台新建／导入的项目与已安装应用，确认范围后直接永久删除，不进入平台或系统回收站。外部目录引用只删除平台登记及本机私有配置／数据，外部文件夹完整保留。旧备份、旧回收项和恢复证据不随规则变更清理；旧回收 API 仅兼容既有历史。

2026-10-26 用户确认[移除旧版回收站、旧备份与维护](docs/legacy-backup-trash-removal.md)：开发环境已于 10-25 重置，旧资料不存在，也不会再产生；回收站、备份列表／恢复、进入／结束维护的服务、CLI、原生命令和界面全部删除，项目导出／导入／复制所用的 ZIP 能力保留。下文 M3／M4 段落为历史记录。

本轮新增[Node 源码副本、安装与更新](docs/node-managed-installation.md)：获取或接入源码后，先准备独立运行材料，再确认安装；同实例更新保留数据和配置。旧外部目录登记保持不变，实际原生通过范围持续登记在[三场景记录](docs/three-scenario-verification.md)。

2026-10-23 已确认[本地目录接入三条路径](docs/local-directory-onboarding.md)：“添加本地应用”遇到空目录时，先确认生成零第三方依赖的 Node 起始项目，再预览添加；已有有效 Node 声明时先预览确认；其他目录提供包含目录、要求和独立 skill 全文的“复制给 AI 检查”，不推断一定兼容、不自动改造或发送。已有目录检查不写入 skill，也不安装、启动或读取密钥／业务数据库；project-v1 仍使用原有完整 ZIP 等入口。

2026-10-23 起，新建、复制和完整 ZIP 导入的用户项目使用[可读目录名](docs/project-directory-names.md)：固定 npm `transliteration` 数据转写名称，原生离线生成安全目录名及完整 ID 派生短码。完整 ID 不变，已有目录、Node 私有槽和用户选定的外部目录不改名；转写为读音，不做语义翻译。操作时读取实际 `projectRoot`，不按 ID 拼接目录。

文档入口见[分类地图与阅读顺序](docs/documentation-map.md)。最新确认的[应用窗口约束](docs/app-window-runtime-contract.md)要求一个应用一个原生窗口：打开时后台启动本应用 Node，关窗就停止，重复打开只聚焦，禁止额外窗口和业务弹窗；浏览器试跑只保留为补充验证。当前独立 Node 源码接入、配置与 本机私有秘密文件边界见[本机配置与运行](docs/node-native-configuration.md)，不等于完整 project-v1 安装与生命周期已交付。

应用作者和 AI 选型先读[应用开发环境与交付要求](docs/app-development-environment.md)：优先前端、平台 SDK 和 SQL，确需后端时尽量一个受管 Node 服务，外部软件与在线服务前提提前说明。作者交付构建好的前端及完整后端运行文件；应用运行使用固定私有 Node，不在登记或启动时安装依赖。本轮 Mac 内部版已将配套受管 Node/npm 接入现有 project-v1 构建，保留明确选择兼容本机环境的入口；构建选择不影响应用运行 Node。执行摘要随项目 skill 的同一份环境／窗口约束分发。

第一版应用兼容范围已收敛为[Radio 类应用](docs/app-development-environment.md#首版兼容基线radio-类应用)：静态网页、单个受管 Node、同进程 SDK 与外部 HTTP 服务。标准应用包、新电脑交接、数据保护与 Windows 仍按各自实际进度推进。受管 Node/npm 已接 project-v1 候选构建和 Node 安装依赖准备；私有 Git 2.56.0 提供选择与诊断、公开 GitHub/Gitee HTTPS HEAD 检查与源码获取，以及本机源码保存、历史列表、新建定制分支和显式源码取回／中断处理。系统 SSH 只检查版本，私有认证、已有分支切换与数据取回尚未实现；project-v1 数据保存以独立说明为准。不把这些子步视为技术计划整体完成，实际验证与限制见[技术实施计划](docs/technical-implementation-plan.md)。

[项目与应用包格式 v1](docs/project-and-app-format-v1.md)集中说明现有 `project.json`、源码 `package.json` 与外部包 `local-app.json` 的分工、目录、字段、JSON Schema、最小示例及校验层级。现有项目和完整 ZIP 已实现；外部包仍待实现，初始数据、更新和检测预算等细则继续标为候选。[原安装与分享设计](docs/web-app-package-design.md)保留详细流程，不改变当前导入与备份规则。

PWA 已确认不强制、作为可选增强。[外部应用包原实施规划](docs/app-package-implementation-plan.md)的 AP0–AP6 编号保留；其中完整备份、恢复及强制安装前置已被最新技术计划取代。获取检测和 Node 源码安装按本轮独立子步推进，不等于通用外部包交付，也不改变 M0–M6 或 Windows 的完成状态。

[应用文档分工](docs/application-documentation.md)区分用户的使用/配置说明与 AI 的技术/架构/开发说明，复用 `documentation.readme` 和 `documentation.development`，并提供两份独立 Markdown 示例；展示仍待实现，正文预算和强制携带策略待定。

用户已确认[内置 Node 与应用后端统一管理](docs/node-app-runtime.md)：平台携带私有运行时，运行应用自己的 Node 后端并管理启动、停止和状态，业务 SDK 由应用负责。[启动规范草案](docs/node-app-startup-spec.md)区分机器清单与用户/AI说明；[NR0–NR7 规划](docs/node-app-implementation-plan.md)拆为 8 阶段、20 小步，稳定优先。现已按用户授权开始[隔离运行试验](docs/node-runtime-trial-verification.md)，增加私有 Node 准备、实验清单和受管进程；后续已接独立本机源码槽、配置表单、本机私有秘密文件和启动注入，原生窗口流程按最新约束验收；完整安装与新类型数据契约仍待定稿；现有静态 v1 构建流程保留，工具来源按本轮受管／明确本机选择规则扩展。此前固定连接器讨论保留为历史，Claude Code 不再是必选依赖。

2026-10-20 用户进一步确认[可视化配置应用环境变量](docs/node-app-configuration.md)：作者在清单中声明变量、中文用途和获取步骤，用户在平台界面填写 AI 地址、API Key 等配置，无需编辑 `.env`。声明与实际值分开，秘密凭据保存、目标后端注入和生效仍属于路线，原备份复制扩展按最新取舍取消。首个隔离试验只校验声明并注入普通默认值；后续本机源码入口的表单、本机私有秘密文件和注入子集见当前接入说明，不读取宿主密钥，不代表公开 Node 安装完成。

后续新增[配置声明实验规范、Schema 和只读检查器](docs/node-configuration-v1/README.md)，radio 配置已收敛为 AI 地址/密钥/模型、语音密钥/音色/模型六项，说明用途与获取步骤；可检查缺少哪些必填配置，不启动应用或读取真实密钥。后续配置表单与本机保存已接入独立 Node 源码入口；完整可分发安装仍未交付。

同日确认按运行形态分为“网页项目”和“Node 服务项目”，清单明确声明，以固定类型图标和中文文字区分；类型与运行状态、用户自定义项目图标分别展示。当前“我的项目”卡片与详情已接类型展示，安装预览尚未实现；规则见[两类运行形态](docs/node-app-runtime.md#11-两种项目运行类型)及[项目概览](docs/project-metadata-and-overview.md)。

已确认 Windows 10 22H2 x64 与 Windows 11 x64 支持目标；[W1–W4 规划](docs/windows-support-plan.md)按稳定优先拆为 20 小步，先验证启动、新项目、数据保存和构建，备份恢复、导入导出、迁移和清理等复杂功能后置并设置后端门禁。当前 W1.1 本机工具准备和 W1.2 首片共享启动门禁进行中，初期范围为本机内置 NTFS 与固定默认工作区；Windows 运行时在安全文件及现场识别就绪前继续关闭。实际结果及剩余项见 [W1 记录](docs/windows-w1-verification.md)；平台 Windows 编译、启动和安装仍未验收。早期内部版仅面向可丢弃样本。[M0–M6 主里程碑](docs/development-milestones.md)保持编号，按平台分别验收；以下开发与打包命令仍为现有 Mac 流程。

Windows 开发环境可用 `npm run env:windows` 只读检查系统、WebView2、Node、Rust、MSVC/SDK 和卷信息；直接执行 `node scripts/windows-environment.mjs --json` 可取得 JSON。这个脚本不启动平台、不初始化数据根，也不表示平台已经兼容 Windows。成品用户运行当前静态项目无需系统 Node。Mac 内部版的 project-v1 构建已接配套受管 Node/npm；Windows 的工具分发与构建仍须单独验收，不沿用 Mac 结果。Rust/MSVC/SDK 用于开发平台本身；Node 应用运行使用固定私有运行时，与构建环境选择分开。

macOS 重启后的工作区设备号误判修复及旧迁移记录的离线预检/绑定规则见[工作区持久身份](docs/workspace-identity.md)。当前数据已完成公开修复，新包启动、样本保存/备份/删除恢复及应用重启结果见[实际验收记录](docs/workspace-identity-repair-verification.md)。

## 开发

源码仓库：[happypandaa/panda-drawer](https://github.com/happypandaa/panda-drawer)。默认源码目录为 `/Users/ethan/dev/panda-drawer/`；该目录只保存平台工程，不是用户应用工作区。本次[旧开发环境重置](docs/development-identity-reset.md)保留该工程和外部原作者源码，清空旧平台管理的项目及业务数据后使用新身份空环境。

使用 macOS 15+、Apple Silicon、Node 22.12+（支持 Node 22/24，推荐 24 LTS）、npm、Rust 和 Xcode 工具链。

```sh
npm ci
npm run dev
```

本工程支持优先使用 `.tools/cargo` 与 `.tools/rustup` 中的本地 Rust 工具链；未配置时使用系统 Rust。不修改全局 PATH。

```sh
npm run check
npm test
npm run test:ui
npm run cli -- service status --json
npm run cli -- env doctor --json
npm run cli -- env git doctor --json
npm run build:app
npm run build
```

`env doctor` 在线时读取桌面服务实际选定的 Node/npm、路径、版本和可用性；离线时不会把终端 PATH 当作桌面环境。已有显式 Node 路径优先且失效不自动回退；没有显式选择时，提供受管资源的环境默认使用配套私有 Node/npm，资源缺失或损坏明确报错。没有受管资源位置的开发服务仍可自动探测本机环境。“恢复默认工具”／`env node clear` 清除显式选择，默认受管位置本身不代表检测通过。最后两个命令分别生成内部测试 `.app`、`.app` 与 DMG。包使用开发标识和 ad-hoc 签名，未进行 Developer ID 签名、公证或对外发布。

Git 使用独立的 `git-toolchain.json`：默认平台受管、显式本机路径优先，失效不自动回退；`env git set --path /绝对路径/git` 与 `env git clear` 保存或明确清除该选择，不改变现有 Node 设置。`env git doctor` 对应服务诊断，只显示 Git 与系统 SSH 工具状态，认证保持 `not-checked`，不读取／复制私钥或访问仓库，也不修改全局配置。成品应用运行不依赖 Git／SSH。

需要检查某个公开仓库时，明确执行 `env git check --url https://github.com/owner/repository.git --json`，也支持对应的 Gitee HTTPS 仓库地址。检查使用服务选定的 Git，在 20 秒预算内匿名读取远端 HEAD，不读取账号凭据、不下载或克隆仓库、不推送。结果只说明该地址当时的公开读取情况，不改变 `doctor` 的未验证认证状态；没有 HEAD 不表示仓库为空。地址、结果与版本支持范围见[服务接口说明](docs/local-service-and-ai-tools.md)。

私有 Git 从官方完整源码在本机按固定 recipe 构建，随附完整源码、许可证与构建说明，包内逻辑内容约增加58.3 MiB。最低部署目标仍为 macOS 15；复现构建使用 SDK27 等固定开发工具，不要求用户安装这些工具或升级到 macOS27。被裁剪组件、复现条件和验收限制见[Git 开发工具说明](docs/development.md#71-git-开发工具与系统-ssh-诊断)；没有将工具检查当作新包、Windows 或干净电脑验收。

详情见[开发环境与隔离](docs/development.md)、[M0 验证记录](docs/m0-verification.md)、[M1 验证记录](docs/m1-verification.md)、[M2 验证记录](docs/m2-verification.md)、[M3 验证记录](docs/m3-verification.md)、[M4 验证记录](docs/m4-verification.md)、[M5 验证记录](docs/m5-verification.md)、[管理界面改造验证](docs/ui-refresh-verification.md)、[空间占用与 AI 接手说明验证](docs/storage-ai-handoff-verification.md)、[历史清理验证](docs/history-cleanup-verification.md)、[里程碑计划](docs/development-milestones.md)、[架构约定](docs/platform-development-contract.md)。开发、检查和打包命令按顺序执行；切换源码版与打包版前先退出当前开发实例。

## 工程边界

| 路径 | 用途 |
| --- | --- |
| `apps/desktop` | 平台管理界面与 Tauri 宿主 |
| `crates/platform-core` | 开发隔离、SQLite、本地服务、会话/附件、构建、备份恢复、项目生命周期及工作区迁移 |
| `crates/platform-preflight` | 无文件访问的遗留记录字节分类；可单独原生测试，不提供运行或恢复许可 |
| `crates/platform-cli` | 只连接固定开发环境的原生 CLI |
| `packages/sdk` | 内部 SDK `@local-projects/sdk 0.1.0`、类型、说明与固定 tarball |
| `templates/react` | `react-ts-vite-0.1.4` 标准模板、组件、锁文件和项目自带 SDK；打包时生成匹配的初始网页 |
| `skills/local-projects-app` | 外部 AI 接管业务项目的 skill，随新项目保存 |
| `docs` | 已确认架构、计划及实际验证记录，包括 [M5 权限拒绝、设备释放与断网恢复实测](docs/m5-resilience-real-verification.md) |

新项目有可直接打开的欢迎应用、与其匹配的标准源码、独立空数据库、固定 SDK 和真实 README。欢迎应用随平台预先构建，创建和打开不需要 Node、npm 或联网；修改源码后才需要检测通过的构建 Node/npm 和平台构建入口。初始来源标为 `bundledTemplate`，不生成虚假的用户构建任务。首次用户构建失败仍保留欢迎应用。

每个新项目在 `.agents/skills/local-projects-app/SKILL.md` 保存 AI 接管说明，README、AGENTS.md 和 CLAUDE.md 提供入口；“复制给 AI”包含可实际读取的绝对路径。不同 AI 工具可显式读取该文件，不依赖它们自动发现 skill。旧项目不自动注入或覆盖任何文件；旧项目交接可以引用安装包内的后备 skill。

M3 提供完整 ZIP、SQLite 一致性副本、短暂备份冻结、成功基线后的持久维护和带原现场保留的恢复。恢复完成后仍在维护，显式结束才重新开放；旧页面提示重载，不能继续使用旧代码写入。手动备份不自动清理，自动备份按已确认规则保留最近 10 份可清理归档，保护当前基线和恢复依据。

M4 复用完整 ZIP 导入导出和复制，默认导入与复制使用新平台实例 ID，保留业务主键。2026-10-22 用户确认普通下载后安装同样生成新实例 ID，允许重复获取并独立并存；编辑既有实例及显式同实例更新保留 ID。作者 `app.id` 与本机实例分开，首版不做自动去重、派生或合并，不重新分配既有 ID、不改动既有用户数据。当前删除为确认后的直接永久删除；旧回收站恢复保留原 ID，旧回收项永久清除仍只作用于选中范围，历史备份和外部 ZIP 继续保留。工作区可通过公开迁移任务移动到独立、空的本机 APFS 目录；固定开发身份与发现位置不变，全部持久历史随工作区复制。提交后保留旧根，新位置不可用时不自动回源。M5 已提供空间扫描、附件分页及白名单清理；历史备份另存/删除仍未定稿，迁移旧根与已完成恢复原现场的显式预览及确认清理见[历史副本清理补充](docs/history-cleanup.md)。

M5 的元数据修改带 `expectedRevision` 冲突检查，不改项目 ID、不触发构建或重载业务页面。运行需求分别控制 CSP 的连接、图片、媒体来源及设备用途；新模板提供文件、剪贴板、短录音和拍照助手，打开时不请求设备或创建业务表。旧项目继续使用自己的固定 SDK、源码和文档。

SQL、上传、构建、媒体与日志预算仍为内部测试参数；旧代产物及任务暂不自动删除，正式保留规则未定。合法的旧项目可能仍为“等待构建”，已有项目的源码、SDK、锁文件和说明不会随平台升级重写。初始应用与 skill 的实际补充验收见[验证记录](docs/starter-skill-verification.md)。

从“我的项目”统一查看网页与 Node 项目，以类型图标区分，均通过“进入项目”打开独立原生窗口；详情与应用配置放在带标签页的弹窗中，缺必填配置时先引导填写，保存不自动启动。从管理窗口还可创建项目、复制交接说明给外部 AI、构建、查看日志和 Node 环境。`⌘⇧H` 返回项目管理，同项目重复打开只聚焦。关闭业务窗口收尾本应用的受管 Node 及普通后代。macOS 管理窗口 X 只隐藏并保留 Dock，已开的业务窗口和服务继续；点击 Dock、Reopen 或重复启动恢复同一个管理窗口。菜单“退出”／Cmd+Q 才安全收尾全部受管应用及平台服务，非 macOS 管理窗口 X 维持正常退出。当前规则见[窗口契约](docs/app-window-runtime-contract.md)，旧版本关闭实测仍保留其原范围。

管理首页采用简洁项目卡片，详情集中在弹窗中的概览、空间与文件、备份维护和更多操作。删除入口先展示项目身份、永久删除及保留范围，明确提示无法撤销；外部目录引用单独说明保留外部文件夹。“空间”页分为概览、按项目和历史资料，见[空间页、任务与设置](docs/space-and-settings.md)。草稿关闭确认、键盘焦点、迟到响应和任务状态经过 React DOM 模拟回归，`npm run test:ui` 不打开浏览器或接触真实项目数据；它不替代实际 WebKit 视觉和原生交互验收。

2026-10-25 用户另行确认设置中的“清理所有本机项目内容”，通常在卸载前使用：至少两次明确确认后，永久删除小抽屉管理的本机数据和代码，包括受管 Node、当前及可核验历史工作区、配置／Key 和旧备份／回收／恢复资料；外部原始源码、独立导出文件、平台程序及最小安全控制记录保留。普通卸载继续默认保留数据。范围、关闭门禁、部分失败和验证边界见[全量清理契约](docs/local-data-cleanup.md)，不以该确认宣称 Windows 已开放或真实安装卸载已验收。

项目详情提供“复制 AI 接手说明”。“空间 → 历史资料”（当时名为“空间与工作区 → 清理历史”）可扫描旧工作区、已完成恢复原现场和回收项，展示目录与保护范围，再显式选择和确认；当前项目、当前备份和未解决现场继续保护。本轮 29 项 DOM、183 项 Rust、全目标 Clippy 与新 app/DMG 核验通过。用户确认保存后已启动新包，公开流程实际清理 1 个旧根、27 个测试回收 payload、12 个历史 original，合计 1306928185 逻辑字节；当前「打僵尸」、全部 44 个当前备份文件和 5 个保留回收项的内容保持及重启复核通过。交付摘要、实际状态及边界见[历史清理验证](docs/history-cleanup-verification.md)。

后续已用新建可丢弃项目补测原生管理界面的清理入口、范围、确认门槛、取消、选择/刷新/页签变化撤销确认，并修复候选控件同名、确认与结果焦点定位、旧根回执长路径溢出。修复通过 33 项 DOM 回归及新包核验；经用户确认，仅通过 GUI 永久清理该样本的两个目录，共 1275776 逻辑字节，实际结果展示、原生退出重启及剩余范围复扫均通过。当前项目、原有 44 个备份文件、5 个保留回收项和样本两份新备份保持不变，详见[清理历史 GUI 验证](docs/history-cleanup-gui-verification.md)。

M5 浏览器补验已在新可丢弃项目的真实 WKWebView 完成文件选择、原生下载保存/取消、主动复制粘贴、真实录音与单张拍照保存、从持久附件重新解码和播放，以及声明 HTTPS 的 CORS 请求和未声明来源的 CSP 拒绝。实测发现的维护按钮门槛问题已修复到模板 0.1.3，并通过 16 项新增组件回归和真实窗口复测；固定 SDK 与旧项目源码保持不变。Safari/Chrome、拒绝许可、独立系统设备释放和物理断网仍留作未完成项，准确证据及交付状态见 [M5 浏览器实测续记](docs/m5-browser-real-verification.md)。

```sh
npm run cli -- project list --json
npm run cli -- project create --input request.json --json
npm run cli -- project metadata get --project prj_实际项目ID --json
npm run cli -- project metadata set --project prj_实际项目ID --input metadata.json --json
npm run cli -- storage info --project prj_实际项目ID --json
npm run cli -- storage scan --project prj_实际项目ID --json
npm run cli -- storage files --project prj_实际项目ID --input filters.json --json
npm run cli -- storage cleanup --project prj_实际项目ID --input cleanup.json --json
npm run cli -- workspace storage --json
npm run cli -- project handoff --project prj_实际项目ID --json
npm run cli -- db schema --project prj_实际项目ID --json
npm run cli -- project build --project prj_实际项目ID --json
npm run cli -- task wait --task task_实际任务ID --timeout 30 --json
npm run cli -- task logs --task task_实际任务ID --json
npm run cli -- project url --project prj_实际项目ID --json
npm run cli -- maintenance status --project prj_实际项目ID --json
npm run cli -- project export --project prj_实际项目ID --input export.json --json
npm run cli -- project import --input import.json --json
npm run cli -- project copy --project prj_实际项目ID --input copy.json --json
npm run cli -- project delete --project prj_实际项目ID --json
npm run cli -- workspace info --json
```

`export.json` 使用 `destinationPath`（可选 `overwrite`），`import.json` 使用 `archivePath`，省略 `mode` 时默认 `"copy"`，每次生成新实例 ID；仅显式 `mode: "original"` 保留归档身份，冲突时仍拒绝覆盖。`copy.json` 可指定 `name`；维护中的项目操作携带本轮 `maintenanceId`。`project delete` 直接永久删除选中项目，调用前须核对目标和范围。归档路径使用绝对路径。迁移请求详见[开发说明](docs/development.md)。写入超时或服务切换后查询原任务，不自动重发。

运行 `npm run prepare:desktop` 并启动开发应用后，`npm run test:m1:live` 会明确新建两个可丢弃验证项目、安装 `examples/m1-runtime` 受控产物并检查接口。随后完整退出并重启，再运行 `npm run test:m1:restart` 验证数据保留和旧会话失效。测试不会清空已有工作区，也不把受控产物称为 AI 构建。

SDK 测试运行 `npm run test:sdk`。M2 的独立 AI、桌面与浏览器完整流程结果以验证记录为准；编译通过不替代这些验收。构建脚本是普通本机代码，不是沙盒：只运行可信项目，禁止启动后台或脱离进程组子进程，平台清理普通构建进程组。

M3 的备份、维护与恢复已于 2026-10-26 移除，其 live／重启／兼容脚本随之删除；历史结果见 [M3 记录](docs/m3-verification.md)。

M4 的旧版软删除 live 与迁移脚本（`m4-live-test.mjs`、`m4-migration-test.mjs`）已随回收站移除；`m4-compatibility-test.mjs` 保留。以下为当时的历史结果：132 项 Rust、12 项 SDK、8 组运行提示回归和真实服务 live、公开迁移及进程重启验证已通过。原 17 个项目兼容核对通过，M4 验收时工作区已迁至开发数据根下 `workspaces/current/`，源 `workspace/` 当时完整保留；后续旧根的显式清理状态见[历史清理验证](docs/history-cleanup-verification.md)。M4 生命周期 GUI 全流程因工具访问限制未完成，实际证据、最终包摘要与边界见 [M4 记录](docs/m4-verification.md)。

M5 已通过 164 项 Rust（147 core、12 CLI、5 desktop）、Clippy/fmt、12 项 SDK、9 组运行提示及 15 项媒体助手模拟测试；真实服务 7 组、109 附件分页、元数据和附件重启保留、原有 26 项目兼容及坏库诊断通过。最终 app/DMG 的签名、架构、媒体权限、54 条目一致性与 8 处 SDK 摘要校验通过。管理 GUI、真实录音拍照、Safari/Chrome 和原生保存框尚未实测；工具访问限制没有通过其他方式绕过。实际证据与交付边界见 [M5 记录](docs/m5-verification.md)。
