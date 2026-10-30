# 主界面布局调整：实施与验证

日期：2026-10-30。用户确认先阅读代码和计划后实施；导入百分比暂不添加。本轮沿用现有样式变量和组件样式，只重排结构，没有改动 Rust、命令协议、应用身份、数据根或项目删除归属。

## 已实现

- 唯一主页“我的应用”；顶栏只有品牌、按需任务胶囊和设置。标题含总数与运行数，筛选为全部／运行中／需要处理，右侧依次为从 Git 添加／本机文件夹／新建。测试项目分组继续在末尾。
- 删除首页搜索、手动刷新和平台 AI 说明入口。页脚显示实际工作区路径并可在访达打开。
- 列表与运行态每两秒轻量刷新，重新获得窗口焦点时完整核对维护与构建状态。网页运行态通过现有 Tauri `getAllWindows()` 和实际窗口标签识别；Node 沿用现有状态。保留防重入、删除失效、实例和工作区校验及配置草稿保护；空间统计不进入每两秒循环。
- 任务先按完整集合去重和分类，再限制最近完成数量。只有进行中或需要处理事项才显示入口；当前导入与上次添加恢复独立计数和打开。任务完成时可在已展开的浮层底部看结果，入口不因完成历史常驻。
- Git 添加复用原 `SourceImportSession` 的唯一事务和进度查询。网格显示真实阶段卡片，关闭弹窗继续；成功后按实例 ID 替换为正常卡片。失败可查看原因，明确未提交且已清理完的失败卡可移除。结果未知、清理未完及仍有剩余路径时保留原核对入口；打开已添加应用或改用本地添加也不丢失待清理会话。
- 网页与 Node 使用同一详情内容组件和约 600px 居中弹窗。主操作、AI 提示、配置／文件夹／占用／日志信息与底部删除统一排列。配置在弹窗内切换，保存后返回详情，不自动启动。恢复提示和运行异常在上部显示；文件夹打开失败也直接可见。未声明说明文件时不显示永久禁用的说明按钮。
- 设置分类为通用、存储与清理、运行环境、给 AI 的平台说明、关于与更新、卸载前清除数据。工具和诊断合入运行环境，平台说明从首页及关于迁入独立分类。
- 存储与清理显示项目文件夹和更换位置、应用共占用／可以清理／磁盘剩余、统一清理入口、按大小排列的应用占用与查看全部。更换位置才展开原迁移表单；软件路径和迁移记录收在技术详情。保留迁移原目录收尾和卸载终止锁。
- 清理先展示应用、来源类别、路径和大小，受保护项不可选；重新确认后才写入。历史计划优先，随后逐应用等待真实任务终态。实例、工作区、项目路径和数据代次变化撤销确认；错回执、查询未知或失败停止后续批次，不自动重放写请求。执行期间锁住设置关闭和分类切换。

## 删除的组件、状态和文案

- 删除 `WorkspaceHub.tsx`、`SpaceTab`、`SpaceOverview` 与空间三页签；提取统计函数后删除 `SpaceProjects.tsx` 旧搜索、筛选、表格和逐行管理入口。
- 删除 `AppPage`、`page`、`spaceTab`、`navigatePage`、`openSpace`、搜索状态和两类详情的 tab 状态；`blockedTab` 改为内部配置视图判断。
- 删除 `ProjectStoragePanel`、`ProjectLifecyclePanel`、`StorageScopeDetails` 及其旧概览辅助组件。`StoragePanels.tsx` 仅保留附件浏览器，`LifecyclePanels.tsx` 仅保留 ZIP 导出动作；二者没有保留旧组件的兼容导出。
- 原 `ProjectStoragePanel.css`／`ProjectLifecyclePanel.css` 的现役附件及导出样式分别收在 `ProjectAttachments.css`／`ProjectExport.css`，移除旧外壳选择器，保持原有声明。
- 移除顶栏“我的项目／空间”、首页搜索、刷新按钮、“N 项管理任务正在进行／需要查看”、旧页脚“共 N 个应用 · 数据保存在本机”和“软件本身与记录”分组。
- 设置不再有“存储位置”“开发工具”“诊断信息”独立分类；诊断内容仍保留在运行环境。原“关于”显示为“关于与更新”。

## 网页“更多”保留清单

1. 编辑名称、简介、图标；声明了文件时查看使用说明。
2. Git 状态、HEAD／分支／变更范围、本机版本列表、源码取回及恢复继续／撤回。
3. 数据版本取回及恢复继续／撤回、数据版本保存收尾、相应任务和取消。版本面板仍独立折叠并按需读取。
4. 用户附件预览与另存下载、完整项目 ZIP 导出。
5. 联网与设备声明、项目身份及对象布局、详细路径、构建来源。
6. 显式重新加载原生窗口及丢弃未保存页面内容前的确认。

“构建更新”继续保留在网页详情主操作行。本轮不决定以上入口的最终去留。

## 剩余引用核对

对 `apps/desktop/src` 检索 `WorkspaceHub / SpaceProjects / OverviewTab / SpaceTab / blockedTab / ProjectStoragePanel / ProjectLifecyclePanel / StorageScopeDetails / radio-primary-nav / radio-toolbar / radio-task-indicator / radio-refresh-button / page === 'storage'`：零引用。

测试中保留少量旧 CSS 选择器，仅用于断言入口已经不存在；不再用空的旧导航集合检查按钮门禁。历史验证文档保留当时说明，现役入口文档及文档地图已更新。任务详情、历史扫描计划、源码／数据恢复、附件、ZIP、迁移和删除流程都是现役功能，相关引用继续保留。

## 测试与验收边界

- `npm run test:ui`：479 项通过，0 失败、0 跳过，覆盖 `apps/desktop/test/*.test.mjs` 和 `scripts/ui-interactions-test.mjs`。
- `npx tsc -b apps/desktop/tsconfig.json`：通过；桌面前端生产构建 `npm run build --workspace @local-projects/desktop`（含 `tsc -b` 和 Vite）：通过。Vite 仍提示主 chunk 大于 500 kB，本次不做代码拆包。
- `git diff --check`：通过。
- 使用隔离的人工 DTO 和原生 API mock，在 Chromium 中查看首页、网页详情、Node 详情和存储设置；两类详情 DOM 实际宽度均为 600px，核对了信息顺序、默认折叠、占用长条、添加按钮和页脚。新建样本与截图只保存在忽略的 `.test-artifacts/`，不提交。
- DOM 测试还覆盖后台导入、成功卡片替换、失败与未知清理门禁、恢复入口并存、窗口状态自动更新、配置保存返回、日志路径复制、清理批次顺序及部分失败、实例变化、删除通知、迟到响应、嵌套弹窗焦点和草稿保留。

本轮没有启动或删除现有开发应用，没有操作真实业务数据或外部原目录，没有执行工作区迁移、真实 Git 联网添加或卸载清理。模拟界面和 Chromium 检查不代表原生 WKWebView、访达实际启动、真实进程停止／删除、系统权限或 Windows 已验收；未进行深色主题的专门视觉验收。现有主题切换和持久化仍由回归覆盖。

## 改动文件

### 界面与状态

- 修改 [apps/desktop/src/App.tsx](../apps/desktop/src/App.tsx)
- 新增 [apps/desktop/src/ApplicationDetails.tsx](../apps/desktop/src/ApplicationDetails.tsx)
- 修改 [apps/desktop/src/CodexHandoffPanel.tsx](../apps/desktop/src/CodexHandoffPanel.tsx)
- 新增 [apps/desktop/src/ImportProjectCard.tsx](../apps/desktop/src/ImportProjectCard.tsx)
- 修改 [apps/desktop/src/LifecyclePanels.tsx](../apps/desktop/src/LifecyclePanels.tsx)
- 修改 [apps/desktop/src/NodeAppsPanel.tsx](../apps/desktop/src/NodeAppsPanel.tsx)
- 修改 [apps/desktop/src/NodeDeletionTasksNotice.tsx](../apps/desktop/src/NodeDeletionTasksNotice.tsx)
- 修改 [apps/desktop/src/PlatformPanels.tsx](../apps/desktop/src/PlatformPanels.tsx)
- 新增 [apps/desktop/src/ProjectAttachments.css](../apps/desktop/src/ProjectAttachments.css)
- 修改 [apps/desktop/src/ProjectCard.tsx](../apps/desktop/src/ProjectCard.tsx)
- 修改 [apps/desktop/src/ProjectDetails.css](../apps/desktop/src/ProjectDetails.css)
- 新增 [apps/desktop/src/ProjectExport.css](../apps/desktop/src/ProjectExport.css)
- 删除 `apps/desktop/src/ProjectLifecyclePanel.css`
- 修改 [apps/desktop/src/ProjectOverview.tsx](../apps/desktop/src/ProjectOverview.tsx)
- 删除 `apps/desktop/src/ProjectStoragePanel.css`
- 修改 [apps/desktop/src/ProjectTask.tsx](../apps/desktop/src/ProjectTask.tsx)
- 修改 [apps/desktop/src/SettingsDialog.tsx](../apps/desktop/src/SettingsDialog.tsx)
- 修改 [apps/desktop/src/SourceAcquisitionPanel.tsx](../apps/desktop/src/SourceAcquisitionPanel.tsx)
- 修改 [apps/desktop/src/SourceRecoveryNotice.tsx](../apps/desktop/src/SourceRecoveryNotice.tsx)
- 删除 `apps/desktop/src/SpaceProjects.tsx`
- 修改 [apps/desktop/src/StoragePanels.tsx](../apps/desktop/src/StoragePanels.tsx)
- 修改 [apps/desktop/src/TaskCenter.tsx](../apps/desktop/src/TaskCenter.tsx)
- 修改 [apps/desktop/src/WorkspaceCleanupPanel.tsx](../apps/desktop/src/WorkspaceCleanupPanel.tsx)
- 删除 `apps/desktop/src/WorkspaceHub.tsx`
- 修改 [apps/desktop/src/WorkspaceLocation.tsx](../apps/desktop/src/WorkspaceLocation.tsx)
- 新增 [apps/desktop/src/WorkspaceStoragePanel.tsx](../apps/desktop/src/WorkspaceStoragePanel.tsx)
- 新增 [apps/desktop/src/git-import-view-state.ts](../apps/desktop/src/git-import-view-state.ts)
- 修改 [apps/desktop/src/history-scan-store.ts](../apps/desktop/src/history-scan-store.ts)
- 修改 [apps/desktop/src/node-configuration-state.ts](../apps/desktop/src/node-configuration-state.ts)
- 修改 [apps/desktop/src/styles.css](../apps/desktop/src/styles.css)
- 新增 [apps/desktop/src/workspace-storage-data.ts](../apps/desktop/src/workspace-storage-data.ts)

### 测试

- 修改 [apps/desktop/test/feedback-ui.test.mjs](../apps/desktop/test/feedback-ui.test.mjs)
- 修改 [apps/desktop/test/git-remote-check.test.mjs](../apps/desktop/test/git-remote-check.test.mjs)
- 修改 [apps/desktop/test/local-data-cleanup-ui.test.mjs](../apps/desktop/test/local-data-cleanup-ui.test.mjs)
- 修改 [apps/desktop/test/node-admission-ui.test.mjs](../apps/desktop/test/node-admission-ui.test.mjs)
- 修改 [apps/desktop/test/node-app-ui.test.mjs](../apps/desktop/test/node-app-ui.test.mjs)
- 修改 [apps/desktop/test/project-export-ui.test.mjs](../apps/desktop/test/project-export-ui.test.mjs)
- 修改 [apps/desktop/test/project-storage-ui.test.mjs](../apps/desktop/test/project-storage-ui.test.mjs)
- 修改 [apps/desktop/test/radio-shell.test.mjs](../apps/desktop/test/radio-shell.test.mjs)
- 修改 [apps/desktop/test/source-import-ui.test.mjs](../apps/desktop/test/source-import-ui.test.mjs)
- 新增 [apps/desktop/test/workspace-storage-ui.test.mjs](../apps/desktop/test/workspace-storage-ui.test.mjs)
- 修改 [scripts/ui-interactions-test.mjs](../scripts/ui-interactions-test.mjs)

### 文档

- 修改 [docs/add-project-flow.md](../docs/add-project-flow.md)
- 修改 [docs/app-window-runtime-contract.md](../docs/app-window-runtime-contract.md)
- 修改 [docs/documentation-map.md](../docs/documentation-map.md)
- 修改 [docs/history-cleanup.md](../docs/history-cleanup.md)
- 修改 [docs/import-progress.md](../docs/import-progress.md)
- 修改 [docs/local-data-cleanup.md](../docs/local-data-cleanup.md)
- 新增 [docs/main-layout-verification.md](../docs/main-layout-verification.md)
- 修改 [docs/node-detail-simplification.md](../docs/node-detail-simplification.md)
- 修改 [docs/space-and-settings.md](../docs/space-and-settings.md)
