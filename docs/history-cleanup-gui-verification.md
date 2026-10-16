# 清理历史的原生 GUI 验证

日期：2026-10-16。范围为固定开发应用 `dev.localprojects.platform.m0` 的原生 WKWebView 管理界面，使用 CUA 的原生应用接口操作 `tauri://localhost`，没有进入 Chrome 或绕过此前浏览器 URL 限制。此前实际历史残留清理及交付记录见[历史清理验证](history-cleanup-verification.md)。

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

新包重启、范围与确认复测后，`protected-before-delete.json` 的 34 个范围仍全部通过。两份新样本 ZIP 的长度及 SHA-256 单独记录于 `fixture-backups-baseline.json`，备份、恢复及移入回收站的 3 个新操作根内 4 个元数据证据文件另有 SHA-256，最终清理后须连同既有资料一起复核。

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

对应私有证据为 `09-fixed-preview`、`10-fixed-review-focus`、`11-fixed-cancel-focus`、`12-selection-invalidates`、`13-refresh-resets`、`14-fixed-receipt-wrap` 的 PNG/AX，以及 `15-tab-invalidates-ax.txt`、`16-final-confirm-ready` 的 PNG/AX。最终真实计划 `cleanup_plan_0f84182a34334601ba907d4c8679274d` 的两项准确选择另存 `final-gui-selection.json`；总逻辑长度 `1275776` 字节，没有选择任何既有回收项。

实际永久清理及结果展示验证仍待 CUA 要求的执行时确认。按钮与准确范围已经就绪，未以 CLI 执行代替 GUI 提交。本段不将已就绪状态记为删除和结果验收通过。

## 验证边界

本轮结论只覆盖实际操作到的原生管理界面。jsdom 回归单独记录，不能替代 WKWebView 截图、原生键盘焦点与实际后端结果；本轮不扩展为 Safari/Chrome、相机/麦克风、迁移全流程或磁盘物理块释放量验收。
