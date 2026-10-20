# 本地项目平台

Tauri 2 + Rust + SQLite 的本地应用平台。当前包含 M1 项目与数据核心、M2 固定 SDK、模板与构建、M3 备份维护恢复，M4 导入导出、复制、回收站和工作区迁移，以及 M5 项目概览、基本信息、空间管理、运行声明与标准媒体助手。临时名称为 **Local Projects Dev**；实际验收范围见各阶段记录，仍是隔离的内部开发版。

[项目与应用包格式 v1](docs/project-and-app-format-v1.md)集中说明现有 `project.json`、源码 `package.json` 与外部包 `local-app.json` 的分工、目录、字段、JSON Schema、最小示例及校验层级。现有项目和完整 ZIP 已实现；外部包仍待实现，初始数据、更新和检测预算等细则继续标为候选。[原安装与分享设计](docs/web-app-package-design.md)保留详细流程，不改变当前导入与备份规则。

PWA 已确认不强制、作为可选增强。[外部应用包实施规划](docs/app-package-implementation-plan.md)拆为 AP0–AP6 共 7 阶段、20 小步，先做检测和无源码类型的保护基础，再开放静态安装，之后扩展数据应用与手动更新；本轮仅规划，尚未开始安装实现，也不改变 M0–M6 或 Windows 的完成状态。

[应用文档分工](docs/application-documentation.md)区分用户的使用/配置说明与 AI 的技术/架构/开发说明，复用 `documentation.readme` 和 `documentation.development`，并提供两份独立 Markdown 示例；展示仍待实现，正文预算和强制携带策略待定。

用户已确认[内置 Node 与应用后端统一管理](docs/node-app-runtime.md)：平台携带私有运行时，运行应用自己的 Node 后端并管理启动、停止和状态，业务 SDK 由应用负责。[启动规范草案](docs/node-app-startup-spec.md)区分机器清单与用户/AI说明；[NR0–NR7 规划](docs/node-app-implementation-plan.md)拆为 8 阶段、20 小步，稳定优先。本轮只整理文档，当前安装包尚未携带 Node，新格式、数据归属和协议仍待定稿；现有静态 v1 与系统 Node/npm 构建流程保持。此前固定连接器讨论保留为历史，Claude Code 不再是必选依赖。

2026-10-20 用户进一步确认[可视化配置应用环境变量](docs/node-app-configuration.md)：作者在清单中声明变量、中文用途和获取步骤，用户在平台界面填写 AI 地址、API Key 等配置，无需编辑 `.env`。声明与实际值分开，秘密凭据保存、目标后端注入、生效及备份复制规则按草案纳入原 20 步；本轮仍只整理文档，没有配置界面或凭据运行实现。

同日确认按运行形态分为“网页项目”和“Node 服务项目”，清单明确声明，平台卡片、详情与安装预览以固定类型图标和中文文字区分；类型与运行状态、用户自定义项目图标分别展示。规则见[两类运行形态](docs/node-app-runtime.md#11-两种项目运行类型)及[项目概览](docs/project-metadata-and-overview.md)，本轮未写界面或修改图标资产。

已确认 Windows 10 22H2 x64 与 Windows 11 x64 支持目标；[W1–W4 规划](docs/windows-support-plan.md)按稳定优先拆为 20 小步，先验证启动、新项目、数据保存和构建，备份恢复、导入导出、迁移和清理等复杂功能后置并设置后端门禁。当前 W1.1 本机工具准备和 W1.2 首片共享启动门禁进行中，初期范围为本机内置 NTFS 与固定默认工作区；Windows 运行时在安全文件及现场识别就绪前继续关闭。实际结果及剩余项见 [W1 记录](docs/windows-w1-verification.md)；平台 Windows 编译、启动和安装仍未验收。早期内部版仅面向可丢弃样本。[M0–M6 主里程碑](docs/development-milestones.md)保持编号，按平台分别验收；以下开发与打包命令仍为现有 Mac 流程。

Windows 开发环境可用 `npm run env:windows` 只读检查系统、WebView2、Node、Rust、MSVC/SDK 和卷信息；直接执行 `node scripts/windows-environment.mjs --json` 可取得 JSON。这个脚本不启动平台、不初始化数据根，也不表示平台已经兼容 Windows。成品用户运行当前静态项目无需系统 Node；系统 Node/npm 在修改源码后构建时需要，Rust/MSVC/SDK 用于开发平台本身。未来 Node 后端使用另行验证的私有运行时。

macOS 重启后的工作区设备号误判修复及旧迁移记录的离线预检/绑定规则见[工作区持久身份](docs/workspace-identity.md)。当前数据已完成公开修复，新包启动、样本保存/备份/删除恢复及应用重启结果见[实际验收记录](docs/workspace-identity-repair-verification.md)。

## 开发

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
npm run build:app
npm run build
```

`env doctor` 在线时读取桌面服务实际选定的 Node/npm、路径、版本和可用性；离线时不会把终端 PATH 当作桌面环境。最后两个命令分别生成内部测试 `.app`、`.app` 与 DMG。包使用开发标识和 ad-hoc 签名，未进行 Developer ID 签名、公证或对外发布。

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

新项目有可直接打开的欢迎应用、与其匹配的标准源码、独立空数据库、固定 SDK 和真实 README。欢迎应用随平台预先构建，创建和打开不需要 Node、npm 或联网；修改源码后才需要系统 Node 和平台构建。初始来源标为 `bundledTemplate`，不生成虚假的用户构建任务。首次用户构建失败仍保留欢迎应用。

每个新项目在 `.agents/skills/local-projects-app/SKILL.md` 保存 AI 接管说明，README、AGENTS.md 和 CLAUDE.md 提供入口；“复制给 AI”包含可实际读取的绝对路径。不同 AI 工具可显式读取该文件，不依赖它们自动发现 skill。旧项目不自动注入或覆盖任何文件；旧项目交接可以引用安装包内的后备 skill。

M3 提供完整 ZIP、SQLite 一致性副本、短暂备份冻结、成功基线后的持久维护和带原现场保留的恢复。恢复完成后仍在维护，显式结束才重新开放；旧页面提示重载，不能继续使用旧代码写入。手动备份不自动清理，自动备份按已确认规则保留最近 10 份可清理归档，保护当前基线和恢复依据。

M4 复用完整 ZIP 导入导出和复制，副本使用新平台 ID，保留业务主键。删除先移入回收站；恢复保留原 ID，永久清除只作用于选中回收项，历史备份和外部 ZIP 继续保留。工作区可通过公开迁移任务移动到独立、空的本机 APFS 目录；固定开发身份与发现位置不变，全部持久历史随工作区复制。提交后保留旧根，新位置不可用时不自动回源。M5 已提供空间扫描、附件分页及白名单清理；历史备份另存/删除仍未定稿，迁移旧根与已完成恢复原现场的显式预览及确认清理见[历史副本清理补充](docs/history-cleanup.md)。

M5 的元数据修改带 `expectedRevision` 冲突检查，不改项目 ID、不触发构建或重载业务页面。运行需求分别控制 CSP 的连接、图片、媒体来源及设备用途；新模板提供文件、剪贴板、短录音和拍照助手，打开时不请求设备或创建业务表。旧项目继续使用自己的固定 SDK、源码和文档。

SQL、上传、构建、媒体与日志预算仍为内部测试参数；旧代产物及任务暂不自动删除，正式保留规则未定。合法的旧项目可能仍为“等待构建”，已有项目的源码、SDK、锁文件和说明不会随平台升级重写。初始应用与 skill 的实际补充验收见[验证记录](docs/starter-skill-verification.md)。

从管理窗口可创建项目、复制交接说明给外部 AI、构建、查看日志和 Node 环境；已有网页产物的项目可在独立桌面窗口或浏览器打开。`⌘⇧H` 返回项目管理，同项目重复打开只聚焦。

管理首页采用简洁项目卡片，详情集中在弹窗中的概览、空间与文件、备份维护和更多操作。删除入口先展示项目及回收范围确认；空间与工作区设有空间总览、工作区位置、回收站和清理历史四个页签。草稿关闭确认、键盘焦点、迟到响应和任务状态经过 React DOM 模拟回归，`npm run test:ui` 不打开浏览器或接触真实项目数据；它不替代实际 WebKit 视觉和原生交互验收。

项目详情提供“复制 AI 接手说明”。“空间与工作区 → 清理历史”可扫描旧工作区、已完成恢复原现场和回收项，展示目录与保护范围，再显式选择和确认；当前项目、当前备份和未解决现场继续保护。本轮 29 项 DOM、183 项 Rust、全目标 Clippy 与新 app/DMG 核验通过。用户确认保存后已启动新包，公开流程实际清理 1 个旧根、27 个测试回收 payload、12 个历史 original，合计 1306928185 逻辑字节；当前「打僵尸」、全部 44 个当前备份文件和 5 个保留回收项的内容保持及重启复核通过。交付摘要、实际状态及边界见[历史清理验证](docs/history-cleanup-verification.md)。

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
npm run cli -- backup create --project prj_实际项目ID --json
npm run cli -- backup list --project prj_实际项目ID --json
npm run cli -- maintenance status --project prj_实际项目ID --json
npm run cli -- project export --project prj_实际项目ID --input export.json --json
npm run cli -- project import --input import.json --json
npm run cli -- project copy --project prj_实际项目ID --input copy.json --json
npm run cli -- project delete --project prj_实际项目ID --json
npm run cli -- trash list --json
npm run cli -- workspace info --json
```

`export.json` 使用 `destinationPath`（可选 `overwrite`），`import.json` 使用 `archivePath` 和明确的 `mode: "original" | "copy"`，`copy.json` 可指定 `name`；维护中的项目操作携带本轮 `maintenanceId`。归档路径使用绝对路径。迁移请求及回收站恢复/永久清除详见[开发说明](docs/development.md)。写入超时或服务切换后查询原任务，不自动重发。

运行 `npm run prepare:desktop` 并启动开发应用后，`npm run test:m1:live` 会明确新建两个可丢弃验证项目、安装 `examples/m1-runtime` 受控产物并检查接口。随后完整退出并重启，再运行 `npm run test:m1:restart` 验证数据保留和旧会话失效。测试不会清空已有工作区，也不把受控产物称为 AI 构建。

SDK 测试运行 `npm run test:sdk`。M2 的独立 AI、桌面与浏览器完整流程结果以验证记录为准；编译通过不替代这些验收。构建脚本是普通本机代码，不是沙盒：只运行可信项目，禁止启动后台或脱离进程组子进程，平台清理普通构建进程组。

M3 的新项目实际实例验证使用 `node scripts/m3-live-test.mjs`。维护重启验证先执行 `node scripts/m3-restart-test.mjs prepare`，完整退出并重启开发包后执行 `verify`。测试只写新建的 `M3 可丢弃` 项目，报告在 `.test-artifacts/`；运行脚本不等于所有验收已通过，以 [M3 记录](docs/m3-verification.md)为准。

M4 使用 `node scripts/m4-live-test.mjs` 创建新的 `M4 可丢弃` 样本。`m4-compatibility-test.mjs` 核对升级前 17 个已有项目；`m4-migration-test.mjs prepare|verify` 配合公开迁移和完整重启验证全工作区。132 项 Rust、12 项 SDK、8 组运行提示回归和真实服务 live、公开迁移及进程重启验证已通过。原 17 个项目兼容核对通过，M4 验收时工作区已迁至开发数据根下 `workspaces/current/`，源 `workspace/` 当时完整保留；后续旧根的显式清理状态见[历史清理验证](docs/history-cleanup-verification.md)。M4 生命周期 GUI 全流程因工具访问限制未完成，实际证据、最终包摘要与边界见 [M4 记录](docs/m4-verification.md)。

M5 已通过 164 项 Rust（147 core、12 CLI、5 desktop）、Clippy/fmt、12 项 SDK、9 组运行提示及 15 项媒体助手模拟测试；真实服务 7 组、109 附件分页、元数据和附件重启保留、原有 26 项目兼容及坏库诊断通过。最终 app/DMG 的签名、架构、媒体权限、54 条目一致性与 8 处 SDK 摘要校验通过。管理 GUI、真实录音拍照、Safari/Chrome 和原生保存框尚未实测；工具访问限制没有通过其他方式绕过。实际证据与交付边界见 [M5 记录](docs/m5-verification.md)。
