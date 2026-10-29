# Node 应用详情精简与运行日志

2026-10-29 新规则：[Node 单目录运行](node-single-directory.md)取代下方旧安装副本、构建更新和清单摘要门禁。所有 Node 项目直接运行登记文件夹；有效 local-app.json（包括 app.id、版本及配置声明）在下次启动生效，平台实例 UUID 和数据归属不变。旧验证仍只证明当时版本。


2026-10-28 新确认：项目内容改为独占归属，平台管理的源码、程序、数据、配置、密钥及任务／恢复材料随项目完整删除；不做旧数据兼容或旧残留清理。按[四批实施契约](project-complete-deletion.md)逐批交付，第4批完成前不宣称无痕删除；下方旧验证按当时版本保留。

确认日期：2026-10-28。本文覆盖此前 Node 详情中的开发、构建、Git 与源码取回入口，以及 stderr 仅计数丢弃／64 KiB 终止规则；历史验证记录及数量保持原样。

## 当前交互

Node 详情操作为进入项目、复制提示词、修改配置和右侧删除项目。无配置字段时隐藏配置 tab 与按钮。交接说明直接展示全文和已核验源码目录；本地代码修改前先关闭业务窗口，改后重新进入。源码 Git 保存和用户要求的回退由外部 AI 处理。

概览只保留运行／配置摘要、项目文件夹和项目 ID、数据目录、数据库、配置文件路径。项目文件夹使用开发源码路径，不将安装副本当作源码。没有新增日志界面。

仅 managed 应用显示“应用修改”：明确点击才构建并展示准备／确认流程，仍核对目标、来源、摘要、数据契约和用户确认。提交结果未知时只允许核验，不能重新准备绕过门禁。平台的安装副本、后端构建／安装任务及恢复门禁留待后续变更。

配置保存不自动重启；运行中显示“已保存，重启后生效”及“立即重启”，停止时显示“已保存，下次进入时生效”。运行状态 tab 每 1.5 秒只读刷新，切走或关闭停止；管理窗口重新获焦或恢复可见时执行一次状态核对，避免业务窗口关闭后概览操作使用旧状态。状态读取不覆盖配置草稿。

已登记 local-app.json 的摘要校验仍保留。AI 改动现有配置／身份声明可能被拒绝启动；本次不增加清单迁移或原地运行能力。

## 运行日志

每应用控制目录为固定开发数据根的 node-apps/<实例 UUID>/；日志在其中的 logs/current.log 和 logs/previous.log，不在作者源码目录。只有实际启动才轮转，重复进入已有窗口不轮转。

日志保留脱敏 stderr 与平台启动／退出／主动终止事件，每份不超过 262144 字节，滚动保留最新完整内容。目录权限 0700、文件 0600；秘密值取自本次启动的私有配置快照，仅内部传递。stdout 仍只接受原有平台控制协议，普通日志写 stderr。

stderr 超过 64 KiB 不再终止进程。跨块秘密在落盘前替换；超长行与尾部残行使用有限缓冲处理。日志初始化失败拒绝启动；运行中写入失败继续排空输出，并在既有运行状态报告。正常退出与实际观测时间如实记录，平台硬崩溃不伪造准确退出时间。

单项目删除预览及新删除任务明确包含 logs；旧 v1 删除现场按原目标恢复，不扩大旧授权范围。卸载前全量清理现有 node-apps 范围自然涵盖日志，普通卸载的保留规则不变。

## 改动与剩余引用

删除 NodeDevelopmentPanel、NodeGitPanel、NodeSourceRestorePanel、node-git-error 及专用样式和退役 UI 测试。NodeInstallPanel 仍用于获取／安装页面和 managed 的按需确认；NodeDataContractInfo 仍用于添加及安装核验；ObjectLayoutPanel 仍用于网页项目。

Node 详情不再调用 node_development_open_codex；共享网页交接不再调用 project_open_codex。后端命令注册保留；添加项目的独立 AI 帮助仍可使用 Codex，本次只在 Node 详情及其故障帮助关闭该能力。组件 CodexHandoffPanel 保留文件名，HandoffAction 只剩 copy。

删除／替换的界面文案包括：在 Codex 打开、程序来源、配置与数据、开发与更新、应用更新、源码版本、详细运行信息、重新读取配置、刷新运行状态；应用位置改为项目文件夹，打开源码改为打开文件夹。网页项目的构建／版本功能及其公开 CLI 契约保留。

## 实际验证

- `npm run test:ui`：512 项通过，0 失败／跳过。最终状态提示小修后，Node 详情专项 99 项再次通过；TypeScript/Vite 由最终应用打包重新完成。
- `node scripts/rust.mjs test --locked --workspace -- --test-threads=1`：991 项通过，0 失败，55 项按既有工具／平台条件忽略（含文档测试）。最终配置文件路径修正另跑 object_layout 13 项通过；这些定向结果属于子集，不重复相加。
- 明确设置 `NODE_RUNTIME_TRIAL_TEST_NODE` 为已校验的随包私有 Node，两项 opt-in 真实 Node 测试通过，包含主／子／孙受管进程与独立 Node 隔离。
- 全工作区全部目标严格 Clippy（`-D warnings`）、Rust fmt、生成目录说明同步、starter 协议回归和差异空白检查通过。Vite 既有大 chunk 提示仍存在。
- 首轮 Rust 全组有一项旧生命周期 fixture 依靠 stderr 上限触发失败；改为实际 stdout 协议错误后，保留原有优雅关闭、组退出、退出码、运行标记与可删除断言，单测及最终全组均通过。没有以删断言或放宽协议换取通过。

2026-10-28 在本机 macOS 上，正常退出 `/Applications/熊猫小抽屉.app` 后，从本仓库 `target/debug/bundle/macos/熊猫小抽屉.app` 启动新包，进程路径已核对。最终包约 477.24 MiB，固定身份 `dev.happypandaa.pandadrawer`，深度严格签名校验通过；开发包为临时签名、未公证，未制作 DMG，也未替换 `/Applications` 中的旧包。

真实 Radio Bob（实例 `83628105-6831-4cc0-8aa3-e3a25c755809`）的概览、配置、运行状态三页和复制提示词已检查：源码路径为 project-sources/radio-bob-4，主操作顺序、单行提示、managed 应用修改按钮、平铺路径和无旧入口符合要求。实际复制正文含本实例两份日志路径、Git 保存／回退、stdout 约束和“应用修改”，不含 Codex 或旧构建入口。没有启动 Radio、修改其源码、密钥或业务数据，也没有对它提交构建／安装；managed 完整确认链由 DOM 与既有后端测试覆盖，不宣称本轮对真实 Radio 完成上版。

只新增并保留人工本地样本“可丢弃·详情与日志验收20261028”，实例 `9be6e6ec-d33f-4e0c-a775-a40706388cb0`，源码位于仓库忽略目录 `.test-artifacts/disposable-node-detail-20261028/source/`。无第三方依赖、外部服务或真实账号，已实际登记；本地详情没有“应用修改”。人工普通配置和脱敏样本保存后停止状态显示“已保存，下次进入时生效”；运行中修改并保存后显示“已保存，重启后生效”和“立即重启”。点击后业务窗口实际从 native-config-A 变为 native-config-B。业务窗口正常关闭后管理页自动显示已停止，active.json 已撤销，源码及人工配置保留。

样本每次启动通过 stderr 输出 9000 行（超过 64 KiB 和 256 KiB），服务仍可通过实际原生页面访问。首次 current.log 为 210868 字节；重启后 current.log 为 211333 字节、previous.log 为 211057 字节（关闭前观测），均不超过 262144 字节；目录 0700、文件 0600，人工秘密原文均不存在，替换符 *** 存在。上一份包含 A、restart 终止原因和退出事件，新一份包含 B；最终关窗还记录 window-closed 与退出事件。运行日志模块另覆盖长短行、UTF-8 边界、跨块／重叠秘密、token／nonce、文件链接拒绝、写失败继续排空及自然退出／遗留后代清理。原生验收首个人工清单遗漏必需的 howToGet，静态检查拒绝；补齐后重新检查通过，未据此修改产品校验。

原生检查发现原技术信息把配置目录称为文件路径，已修正为 configuration/configuration.json，最终包实际重新打开 Radio 详情确认。停止保存后首次进入的旧成功提示会被清除，避免新运行已使用配置仍错误显示待重启；对应回归通过。

未重验 Windows、干净电脑、硬崩溃／断电、物理媒体或系统权限；未进行付费 AI／语音调用。无配置字段隐藏由 DOM 回归验证，未另增第二个原生无字段样本。日志为有界滚动内容，旧启动事件可能随最旧行一起被截断；不能将两份日志当完整历史审计。

人工样本、测试输出与日志核对结果保留在忽略目录，不提交、不随包分发。

## 文件清单

| 状态 | 文件 |
|---|---|
| 修改 | `apps/desktop/src-tauri/src/node_windows.rs` |
| 修改 | `apps/desktop/src/AiProjectHelp.tsx` |
| 修改 | `apps/desktop/src/App.tsx` |
| 修改 | `apps/desktop/src/CodexHandoffPanel.tsx` |
| 修改 | `apps/desktop/src/NodeAppsPanel.tsx` |
| 修改 | `apps/desktop/src/NodeConfigurationForm.tsx` |
| 删除 | `apps/desktop/src/NodeDevelopmentPanel.tsx` |
| 删除 | `apps/desktop/src/NodeGitPanel.tsx` |
| 修改 | `apps/desktop/src/NodeInstallPanel.tsx` |
| 删除 | `apps/desktop/src/NodeSourceRestorePanel.tsx` |
| 修改 | `apps/desktop/src/ProjectDetails.css` |
| 修改 | `apps/desktop/src/SourceAcquisitionPanel.tsx` |
| 删除 | `apps/desktop/src/node-git-error.ts` |
| 修改 | `apps/desktop/src/project-ai-help.ts` |
| 修改 | `apps/desktop/src/styles.css` |
| 修改 | `apps/desktop/test/node-admission-ui.test.mjs` |
| 修改 | `apps/desktop/test/node-app-ui.test.mjs` |
| 修改 | `apps/desktop/test/node-development-ui.test.mjs` |
| 修改 | `apps/desktop/test/node-install-ui.test.mjs` |
| 修改 | `apps/desktop/test/project-ai-help-ui.test.mjs` |
| 修改 | `crates/platform-core/examples/node_runtime_trial.rs` |
| 修改 | `crates/platform-core/src/lib.rs` |
| 修改 | `crates/platform-core/src/local_cleanup_tests.rs` |
| 修改 | `crates/platform-core/src/node_app_configuration.rs` |
| 修改 | `crates/platform-core/src/node_app_configuration_file_tests.rs` |
| 修改 | `crates/platform-core/src/node_app_delete.rs` |
| 修改 | `crates/platform-core/src/node_app_development.rs` |
| 修改 | `crates/platform-core/src/node_app_directory.rs` |
| 修改 | `crates/platform-core/src/node_apps.rs` |
| 修改 | `crates/platform-core/src/node_apps_install_tests.rs` |
| 修改 | `crates/platform-core/src/node_apps_lifecycle_tests.rs` |
| 修改 | `crates/platform-core/src/node_deletion_journal.rs` |
| 修改 | `crates/platform-core/src/node_deletion_journal_tests.rs` |
| 修改 | `crates/platform-core/src/node_deletion_tasks.rs` |
| 修改 | `crates/platform-core/src/node_runtime_trial.rs` |
| 修改 | `crates/platform-core/src/object_layout.rs` |
| 修改 | `crates/platform-core/src/resources.rs` |
| 修改 | `crates/platform-core/src/service_node_development.rs` |
| 修改 | `crates/platform-core/src/service_node_development_tests.rs` |
| 修改 | `docs/node-development-workflow.md` |
| 修改 | `docs/node-native-configuration.md` |
| 修改 | `docs/node-runtime-trial-verification.md` |
| 修改 | `docs/operation-record-simplification.md` |
| 修改 | `scripts/ui-interactions-test.mjs` |
| 新增 | `apps/desktop/src/NodeApplyChangesPanel.tsx` |
| 新增 | `crates/platform-core/src/node_runtime_log.rs` |
| 新增 | `docs/node-detail-simplification.md` |
