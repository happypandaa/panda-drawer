# 清理历史的原生 GUI 验证

日期：2026-10-16 至 2026-10-17（Asia/Shanghai）。固定开发应用 `dev.localprojects.platform.m0` 的原生 WKWebView 清理入口、范围预览、确认、实际清理结果和退出重启验证均通过。使用 CUA 的原生应用接口操作 `tauri://localhost`，没有进入 Chrome 或绕过此前浏览器 URL 限制。此前实际历史残留清理及交付记录见[历史清理验证](history-cleanup-verification.md)。

本轮只创建并操作新样本「GUI 可丢弃 · 清理历史验收 · 20261016」，项目 ID `prj_1bdccb46bc7248ee9a5d9a5fcf120849`。原有「打僵尸」、44 个当前备份文件、5 个保留回收项及操作证据继续保护。工作区仍为固定开发数据根下 `workspaces/current/`，没有迁移工作区或手工变更设置。

## 修复前的实际 GUI 发现

已通过真实界面完成新建项目、创建手动备份、恢复备份、结束维护、将新样本移入回收站。样本手动备份为 `backup_00a40492564845f0bb3f1f20e7f5e84f`，恢复任务为 `task_e3af5d5a08dd4631a11f2287046c1e4d`，回收项为 `trash_83e2cc34d7d14442b2ba001bdeebc964`；恢复自动生成的恢复前备份也继续保留。

实际扫描确认维护中的 original 被禁选，并显示保护原因；所有候选默认不选、核对按钮禁用。结束维护、删除样本后，只有该样本的 original 和新回收 payload 被选择，5 个既有回收项均未选择。错误确认词配合范围勾选仍禁用最终按钮；准确确认词配合勾选才启用，点击取消未提交清理任务。

原生截图与 AX 树发现以下问题，已在 `1e1f8dd` 修复并推送：

- 多个回收项复选框的可访问名称相同，缺少具体项目与目录；复制目录按钮同样没有对象归属。
- 点击核对范围后焦点回到 HTML 根，确认输入仍在可见窗口下方；取消后也未返回核对入口。
- 工作区位置内旧根的清理回执路径包含长任务 ID，溢出卡片及窗口右侧。

私有证据位于 `.test-artifacts/history-cleanup-gui-2026-10-16/`：`01-entry`、`02-fixture-created`、`03-protected-preview`、`04-review-before-fix`、`05-confirm-ready-before-fix`、`06-retired-history`、`07-retired-expanded`、`08-retired-visible` 各有 PNG 和 AX 文本。仅观察既有旧根的已清理记录，没有再次迁移或清理旧根。

## 开发资料保护

任何样本写入前重新采样 34 个保护范围，包含原有当前项目全树、44 个备份文件、5 个回收目录、相关控制记录、迁移证据与既有 operations。库存记录内容 SHA-256、长度、路径类型与链接 target，不跟随链接；新样本备份和操作证据允许新增，需另核归属。

修复前准备操作结束后，`protected-before-final-gui.json` 的 34 个范围全部通过；新增备份条目为新样本目录及其两份归档。不能删除这些备份来人为恢复原有文件数量。

新包重启、范围与确认复测后，`protected-before-delete.json` 的 34 个范围仍全部通过。两份新样本 ZIP 的长度及 SHA-256 单独记录于 `fixture-backups-baseline.json`，备份、恢复及移入回收站的 3 个新操作根内 4 个元数据证据文件另有 SHA-256。

最终提交前、清理后、原生重启后及最后复扫后，`protected-resume-before-delete.json`、`protected-after-gui-cleanup.json`、`protected-after-gui-restart.json`、`protected-after-final-preview.json` 均为 34/34 通过。当前「打僵尸」全树 3922 个普通文件、170651946 逻辑字节及既有 5 个回收目录、相关控制记录、迁移和操作证据保持不变。原有 44 个备份文件 6415971 字节不变，加上本轮两份 ZIP 共为 46 个普通文件、6692404 字节。

`after-cleanup-final-check.json` 与 `after-restart-final-check.json` 另行核对项目集合仍只有原有项目，5 个既有回收项归属、名称及可恢复状态一致；样本两份 ZIP 的长度、SHA-256 一致，3 个样本操作根的 4 个元数据文件摘要不变。只新增同属该样本的已成功、已提交且已解决的 trashPurge 子操作根及 lifecycle 记录。两个获批 payload 均已不存在，样本回收记录为 purged、不可恢复。原有项目仍为 ready，busy 与 recoveryRequired 均为 false。

## 代码检查与新包

33 项完整 UI DOM 回归、8 项清理相关定向测试、TypeScript/Vite 构建与 diff 检查通过。新增 4 项回归检查同名候选的准确选择及复制归属、确认与取消焦点、任务受理及首次最终状态的结果定位，以及隐藏页签/重复轮询不抢焦点。测试由协作代理在同一工作树实际执行，输出保留于本轮工具记录，未另外持久化测试日志。本轮只改管理前端和 CSS，没有重跑既有 183 项 Rust 测试；打包实际编译原生桌面程序。

`npm run build` 通过，日志 `build-dmg.log`。上一版 app/DMG 在覆盖输出前完整保存在 `.test-artifacts/history-cleanup-delivered/8b5363997456af601d6babac4fa68dbade2c78124fafe85da23d30797cc81dbf/`；54 个包内条目及根目录核对一致，旧 DMG 摘要不变。新包的 `package-verification/verification.json` 为 passed，验证源 app 和只读挂载 DMG 内 app 的严格深度签名、arm64 架构、macOS 15 最低版本、固定身份、权限和 54 条目一致性，挂载已卸载。8 处 SDK 0.1.0 仍为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9`。

| 项目 | 新交付 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 长度 | `8282103` 字节 |
| DMG SHA-256 | `c08f7b51bf868bea3dd3795df2d6c97bf48866879508719a28980b6e8b2f3909` |
| 身份 / 版本 | `dev.localprojects.platform.m0` / `0.0.0`，ad-hoc 签名 |

## 修复后的原生界面复测

运行环境为本机 macOS `27.0.1 (26A434)`、arm64。通过 CUA 向旧实例发送原生 `⌘Q`，工具返回 App quit，随后独立确认旧 PID `31114` 已退出；使用 CUA 按新 app 的准确路径启动，得到 PID `40261`、实例 `70e35c61-bbbe-4ff1-9426-34578e5fd5a7`。项目和回收项集合与重启前一致，固定身份与工作区不变，证据 `gui-quit-old-result.json`、`gui-new-package-restart.json`。这是实际原生快捷键退出和应用启动，不是 SIGTERM 替代。

新包已实际验证以下状态，使用截图和 AX 树互相核对：

- 总览“查看历史清理范围”快捷入口切到清理历史并聚焦该页签；初次展示持久历史结果不会抢走入口焦点。
- 新计划列出 7 项（新样本 original、新回收项和 5 个既有保留回收项），全部默认不选。选择框和复制按钮的可访问名称包含项目名称或类别、项目 ID、完整路径，原有多个同名「打僵尸」也能区分。
- 只选新样本两项后，点击核对将焦点置于确认标题并实际滚动展示范围与输入区，按 Tab 能进入确认输入。仅准确词而未勾选、或错误词且已勾选，最终按钮都禁用。
- 取消返回核对按钮；重新核对后确认词为空、勾选清零。选择变化立即移除确认；重新扫描移除旧计划、选择与确认，新结果仍为零选择；切走页签后切回也撤销确认。
- 工作区位置的已清理旧根记录仍可读取，长回执路径完整换行并留在卡片内，不再溢出右边缘。没有对既有旧根执行新的删除。

对应私有证据为 `09-fixed-preview`、`10-fixed-review-focus`、`11-fixed-cancel-focus`、`12-selection-invalidates`、`13-refresh-resets`、`14-fixed-receipt-wrap` 的 PNG/AX，以及 `15-tab-invalidates-ax.txt`、`16-final-confirm-ready` 的 PNG/AX。确认前准备计划 `cleanup_plan_0f84182a34334601ba907d4c8679274d` 的两项准确选择另存 `final-gui-selection.json`；未提交该计划。

## 获批后的实际清理与结果

用户于执行时明确确认仅清理上述新样本的两个目录。回到真实界面时，准备计划已过期，选择、核对及最终按钮均禁用，见 `17-expired-plan`。通过 GUI 重新扫描，预览任务 `task_cfdd06a3d2e24fe48d9fa88baa04e6fc` 生成新计划 `cleanup_plan_0cfa74dc385a412695939d49ef29f294`，仍为同样 7 项且默认不选，见 `18-refreshed-approved-preview`；只读核对准确归属、路径及长度后另存 `approved-refreshed-selection.json`。

只选择获批的两项，重新核对并填写准确确认词、勾选范围确认，然后实际点击原生界面的“确认永久清理”。GUI 提交清理任务 `task_5fda2b338c3c4febacddefa7e1732dc2`，没有以 CLI 写操作替代。受理时显示等待执行、管理写入门槛及已使用计划，焦点移到结果标题；任务完成后仍聚焦结果标题，显示两项成功及实际计数，见 `19-gui-cleanup-accepted`、`20-gui-cleanup-result`。

以下路径相对于当前工作区 `workspaces/current/`，没有选择任何既有回收项：

| 获批范围 | 实际删除逻辑字节 | 文件或链接 | 含目录的全部条目 |
| --- | --- | --- | --- |
| `.platform/operations/task_e3af5d5a08dd4631a11f2287046c1e4d/original` | 654356 | 36 | 56 |
| `.platform/trash/trash_83e2cc34d7d14442b2ba001bdeebc964/project` | 621420 | 34 | 54 |
| 合计 | 1275776 | 70 | 110 |

任务为 final、succeeded、completed；两项均为 removed，accountingComplete 为 true、countsAreLowerBounds 为 false，没有失败或跳过。界面显示历史备份已保留、逐文件记录入口和固定清理回执目录 `cleanup/receipts/task_5fda2b338c3c4febacddefa7e1732dc2`。展开逐文件记录位置并滚动到回执后，完整长路径仍在卡片内换行，见 `21-gui-receipt-result`。

随后通过原生 `⌘Q` 退出 PID `40261`，工具返回 App quit，独立确认进程已退出；用 CUA 从同一新 app 路径重新启动，得到 PID `48728`、实例 `bdb7c490-2491-4277-81d4-4a7f36bfb416`。固定工作区、项目和回收项集合一致，同一清理任务对象与清理后逐字段相同，两个目录没有重建。界面重新进入清理历史可读取持久化的两项结果、计数及回执，焦点保持在页签，没有被历史结果抢走。证据为 `after-cleanup-gui-quit.json`、`after-cleanup-gui-restart.json`、`22-restarted-result-loaded` 和 `23-restarted-result-visible`。

重启后再通过 GUI 扫描，任务 `task_85857419fc8f4b04bfd56182bc14387c` 成功；新计划只剩准确归属的 5 个既有保留回收项，两个已清理样本目录不再出现。5 项全部未选、核对按钮禁用，没有再次提交清理。证据为 `after-restart-preview.json`、`after-restart-preview-check.json` 和 `24-restarted-remaining-preview` 的 PNG/AX。

最终复核源 app 的严格深度签名及 DMG 长度、SHA-256 与上表一致。上述最终记录只修改仓库文档；包内使用的 `docs/development.md` 与资源 README 仍一致，未因记录更新再次构建相同程序。

## 验证边界

本轮结论只覆盖实际操作到的原生管理界面。jsdom 回归单独记录，不能替代 WKWebView 截图、原生键盘焦点与实际后端结果；本轮不扩展为 Safari/Chrome、相机/麦克风、迁移全流程或磁盘物理块释放量验收。
