# Node 单目录第一批实施与验证记录

日期：2026-10-29。范围为用户确认的实施说明 v2.1 第一批，以及仓库／分支重复检查、逐项依赖测试、三个文件排除、仅提交不推送的补充要求。

## 实现范围

- 所有现役 Node 登记直接运行项目文件夹。旧安装器先拆出依赖准备、旧登记解码、源码归属和删除所需代码，再删除安装副本、候选、程序代次、应用修改、平台 Node 构建／Git／源码取回入口。网页 project-v1 构建和数据／源码恢复保留。
- Git 添加使用一个后端任务和一个平台实例 UUID，贯穿下载、发布、依赖准备、登记、取消和清理。登记保存仓库地址和请求分支；前端重复提示由登记读取这两个字段，不依赖安装 `sourceId`，明确另加一份时建立独立实例。
- 登记为提交点：确定未提交时回滚，确定已提交时保留项目，只清临时材料；未知时保留并待核对。清理等待真实写入进程和 I/O 完成，核对持久目录身份，保护同路径替代目录。成功收尾移除导入日志；失败只保留重试所需状态和路径。单份坏恢复记录不会阻断其他项目。
- Git 依赖只允许根目录或后端目录去重后的一组有效 npm v2/v3 锁定依赖；零依赖跳过。缺锁、双依赖根、缺少声明的运行文件均按“需要修改”失败。拒绝自定义项目 `.npmrc`，隔离 HOME、用户／全局 npm 配置、缓存和日志，只运行禁用脚本的生产依赖 `npm ci`。
- 每次新启动读取有效的 `local-app.json`，接受 `app.id`、名称、版本、入口和配置声明变化。平台 UUID、数据和删除归属不变。运行中继续使用当次清单和配置快照，重开后采用声明修改；文件本身没有副本隔离，编辑前应关闭应用窗口。配置按变量名保留有效值，不注入已删除字段；新增必填项进入配置页；秘密分类变化不会直接泄露原秘密。
- version 2 旧登记显示已停用且不能启动；不依赖旧程序或非必要元数据完整，删除仍覆盖 generations、安装候选、平台源码、数据／配置／日志、任务和归属。外部作者目录保留。
- 空间页计算项目文件夹实际大小，包含 `.git` 和 `node_modules`，另列实例占用，标注平台所有或外部目录及删除保留规则。更新界面、交接／修复提示词、生成要求、skill 及其随附运行约束。
- 保持公开工作区迁移可用，并重绑直接运行的源码路径和目录身份。第二批网页构建代次清理、第三批迁移旧目录自动收尾尚未实施。

## 用户文件隔离

以下三个开工前已有修改的文件未由本批编辑，也不纳入本批提交；核对内容摘要与开工前一致：

- `crates/platform-core/src/paths.rs`
- `crates/platform-core/src/workspace.rs`
- `docs/development.md`

因此本批没有在 `docs/development.md` 添加新规则头；现役规则由 `docs/node-single-directory.md`、README、文档索引及相关旧文档的退役说明指向。未出现必须修改这三个文件才能继续的冲突。

## 检查与验收

实际检查结果如下。测试项目及仓库均为明确命名的临时可丢弃内容，条件测试没有计入默认整套测试的通过数。

| 检查 | 结果 |
| --- | --- |
| `caffeinate -i -s npm test -- -- --test-threads=1` | 1032 通过、0 失败、39 条件忽略；core 872、登记集成 10、CLI 25、桌面 78，其余工具和文档 47 |
| `npm run test:ui` | 453 通过、0 失败、0 忽略 |
| `node scripts/rust.mjs clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `npm run test:ai-prompts` | 通过，维护文档与生成常量一致 |
| 临时诊断撤回后的三个曾失败用例短回归 | 3 通过；Git 远端取消／超时、Node 独立停止、runtime handle 丢失收尾 |
| 固定私有 Node 的 `pinned_node_` 条件测试 | 2 通过；使用随包 Node 24.22.0，验证协议与普通后代进程收尾 |
| 真实 Node/npm 的 `real_npm_skips_all_repository_lifecycle_scripts_in_the_published_directory` | 1 通过；无外部依赖下载，安装钩子哨兵均未执行 |
| `npm run tauri -- build --debug --bundles app` | 通过；TypeScript、Vite、原生编译、完整工具资源和签名校验通过。现有 Vite 大分块提示仍在 |

完整通过的一轮保留了仅在测试超时时执行的临时诊断，没有触发输出；之后已撤回这些诊断，通用进程模块最终无代码差异。前端、strict clippy 和三个短回归在撤回后通过。此前整套失败的实际记录见本节后文，不能只用这轮通过宣称时序问题已经修复。

开发包位于 `target/debug/bundle/macos/熊猫小抽屉.app`，固定身份为 `dev.happypandaa.pandadrawer`，大小约 472.96 MiB。它是本机 ad-hoc 签名的 debug 应用包，未公证、未生成 DMG，也未安装到 Applications。此前中断的打包事务已核对签名、身份、进程退出和记录路径，恢复其旧生成包后正常重打；成功打包流程已移除该标准位置的上一份生成包，不涉及项目或业务恢复资料。

在最终开发包的真实原生管理窗口完成只读检查：

- 现有 version 2 旧项目在列表显示完整停用提示；详情“进入项目”和交接按钮不可用，删除入口仍存在。位置标为“旧程序位置”，不再提示在旧副本修改代码；未点击删除或启动业务程序。
- Git 添加表单显示单份项目路径、生产依赖与不执行仓库脚本的说明；高级选项包含可选分支和名称。未提交仓库地址、未下载新项目。
- 空间“按项目”页显示项目文件夹与平台数据分项、旧安装状态和外部目录保留说明；没有执行清理。
- 完成后从菜单退出，确认开发应用不再运行。没有改动现有项目内容；启动、列表读取和空间统计产生的正常平台状态不等于业务端到端验收。

原生检查发现并修正了旧安装详情仍提示“AI 在这里改代码”的遗留文案，补充既有退役 UI 用例断言；最终 453 项前端测试重新通过，随后重新打包并在原生窗口确认新文案。

定向自动化覆盖：

| 验收项 | 自动化证据 |
| --- | --- |
| Git 添加只有一份源码，无 generations／安装候选；仓库和分支持久保存 | `node_git_import` 成功、幂等、来源登记与删除用例；前端来源及重复检查用例 |
| 缺锁、双依赖根、需构建却缺少运行文件 | `invalid_dependency_or_missing_build_inputs_rollback_without_executing_scripts` 分项断言 |
| 项目 `.npmrc` 不能开启脚本；安装钩子不执行 | 依赖单元测试、Git 导入拒绝用例、真实 npm 的 preinstall／install／postinstall 哨兵检查 |
| 下载／检测／发布／依赖失败、取消和清理失败 | 进程与 I/O 归属、清理重试、部分发布、接纳中断及恢复用例 |
| 登记已提交或结果未知，不回滚项目、不重复添加 | 提交对账、响应丢失、重启幂等及 UI 未知状态用例 |
| 清单可变但实例数据不变；无效清单拒绝启动 | 清单刷新、真实子进程／HTTP 生命周期、配置投影及无效入口用例 |
| 新增必填配置时跳配置页；秘密分类安全 | 配置模块及 Node 配置表单／启动 UI 用例 |
| 旧 v2 不可启动，仍可完整删除 | 旧登记解码、损坏非必要元数据、完整删除与退役卡片／详情用例 |
| 文件夹占用、外部原件保留及迁移后删除 | storage、local_cleanup、源码归属迁移与生命周期删除用例 |

测试使用本机真实 Git 操作临时仓库，公开 URL 映射到本地测试来源；导入 npm 部分使用受控工具。另行执行的真实 npm 检查只使用无外部包的锁文件，不等于验证了公网仓库或外部依赖下载。人工解释器子进程测试证明所有权、HTTP 和清理行为，不等于 Node 兼容性；固定私有 Node 的独立测试另行记录。

此前整套串行测试曾在现有进程测试中出现时序失败：两次 Node 生命周期用例在首次版本探测处超时，一次 Git 远端用例等待人工进程启动标记超时，另一次 Node runtime 用例在版本探测处超时。相同 Node 子集、两组前序组合和后续完整运行中的先前失败用例均通过。其中一次运行横跨 18:08:30 合盖睡眠至 18:51:31 完整唤醒期间的多次 Sleep／DarkWake，受宿主睡眠干扰；保持唤醒的后续整套运行仍复现，因此睡眠不是全部解释。500 次独立新脚本路径的探测全部通过（280.70 秒），完整 389 项前序组合也通过（382 通过、7 忽略、135.88 秒）；这些仅用于调查，临时压力测试不纳入交付。未修改生产期限、进程收尾逻辑或原断言，也不声称仅凭重跑就修复了间歇问题。旧历史清理服务用例引用已退役 Node 历史类型的问题已改为现役网页源码恢复夹具，保留其权限、范围及幂等检查。

未清空开发数据，未删除现有项目，未执行迁移旧根自动删除，未推送。尚未完成的跨机器／新安装、公网 Git 导入及真实业务窗口端到端验收会在交付记录中明确保留。

## 文件清单

不含三个用户文件、打包产物和本地测试日志。

### 新增或修改（85 个）

- `README.md`
- `apps/desktop/src-tauri/src/main.rs`
- `apps/desktop/src-tauri/src/node_development.rs`
- `apps/desktop/src-tauri/src/node_windows.rs`
- `apps/desktop/src-tauri/src/source_import.rs`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/LocalDirectoryPanel.tsx`
- `apps/desktop/src/NodeAppsPanel.tsx`
- `apps/desktop/src/NodeConfigurationForm.tsx`
- `apps/desktop/src/ProjectCard.tsx`
- `apps/desktop/src/ProjectTask.tsx`
- `apps/desktop/src/SourceAcquisitionPanel.tsx`
- `apps/desktop/src/SourceRecoveryNotice.tsx`
- `apps/desktop/src/SpaceProjects.tsx`
- `apps/desktop/src/StoragePanels.tsx`
- `apps/desktop/src/WorkspaceCleanupPanel.tsx`
- `apps/desktop/src/WorkspaceHub.tsx`
- `apps/desktop/src/git-repository.ts`
- `apps/desktop/src/node-app-types.ts`
- `apps/desktop/src/node-configuration-state.ts`
- `apps/desktop/src/platform-ai-intro.ts`
- `apps/desktop/src/project-ai-help.ts`
- `apps/desktop/src/project-ai-requirements.ts`
- `apps/desktop/src/source-import.ts`
- `apps/desktop/src/styles.css`
- `apps/desktop/test/node-admission-ui.test.mjs`
- `apps/desktop/test/node-app-ui.test.mjs`
- `apps/desktop/test/project-ai-help-ui.test.mjs`
- `apps/desktop/test/radio-shell.test.mjs`
- `apps/desktop/test/source-import-ui.test.mjs`
- `apps/desktop/test/source-import.test.mjs`
- `crates/platform-cli/src/main.rs`
- `crates/platform-core/src/builds.rs`
- `crates/platform-core/src/history_cleanup.rs`
- `crates/platform-core/src/history_cleanup_tests.rs`
- `crates/platform-core/src/lib.rs`
- `crates/platform-core/src/local_cleanup.rs`
- `crates/platform-core/src/local_cleanup_tests.rs`
- `crates/platform-core/src/node_app_configuration.rs`
- `crates/platform-core/src/node_app_delete.rs`
- `crates/platform-core/src/node_app_development.rs`
- `crates/platform-core/src/node_app_directory.rs`
- `crates/platform-core/src/node_app_directory_tests.rs`
- `crates/platform-core/src/node_app_manifest.rs`
- `crates/platform-core/src/node_app_operations.rs`
- `crates/platform-core/src/node_apps.rs`
- `crates/platform-core/src/node_apps_lifecycle_tests.rs`
- `crates/platform-core/src/node_apps_manifest_refresh_tests.rs`
- `crates/platform-core/src/node_deletion_tasks.rs`
- `crates/platform-core/src/node_dependency_prepare.rs`
- `crates/platform-core/src/node_dependency_prepare_tests.rs`
- `crates/platform-core/src/node_git_import.rs`
- `crates/platform-core/src/node_git_import_tests.rs`
- `crates/platform-core/src/node_process_scope.rs`
- `crates/platform-core/src/node_project_ownership.rs`
- `crates/platform-core/src/node_registration.rs`
- `crates/platform-core/src/node_source_ownership_tests.rs`
- `crates/platform-core/src/project_git.rs`
- `crates/platform-core/src/service.rs`
- `crates/platform-core/src/service_history_cleanup_tests.rs`
- `crates/platform-core/src/service_node_development.rs`
- `crates/platform-core/src/service_shutdown.rs`
- `crates/platform-core/src/service_source_tests.rs`
- `crates/platform-core/src/source_acquisition.rs`
- `crates/platform-core/src/source_acquisition_process.rs`
- `crates/platform-core/src/source_acquisition_tests.rs`
- `crates/platform-core/src/source_publication.rs`
- `crates/platform-core/src/storage.rs`
- `crates/platform-core/src/storage_tests.rs`
- `crates/platform-core/tests/node_apps_registration.rs`
- `docs/app-development-environment.md`
- `docs/app-window-runtime-contract.md`
- `docs/documentation-map.md`
- `docs/git-app-import.md`
- `docs/node-detail-simplification.md`
- `docs/node-development-workflow.md`
- `docs/node-managed-installation.md`
- `docs/node-native-configuration.md`
- `docs/node-single-directory-verification.md`
- `docs/node-single-directory.md`
- `docs/project-compatibility-review-prompt.md`
- `scripts/ui-interactions-test.mjs`
- `skills/local-folder-review/SKILL.md`
- `skills/local-projects-app/SKILL.md`
- `skills/local-projects-app/references/app-window-runtime.md`

### 删除（31 个）

- `apps/desktop/src/NodeApplyChangesPanel.tsx`
- `apps/desktop/src/NodeDataContractInfo.tsx`
- `apps/desktop/src/NodeInstallPanel.tsx`
- `apps/desktop/src/SourceLibraryPanel.tsx`
- `apps/desktop/test/node-development-ui.test.mjs`
- `apps/desktop/test/node-install-ui.test.mjs`
- `crates/platform-core/src/node_app_development_tests.rs`
- `crates/platform-core/src/node_app_install.rs`
- `crates/platform-core/src/node_app_install_policy_tests.rs`
- `crates/platform-core/src/node_app_install_tests.rs`
- `crates/platform-core/src/node_apps_install_tests.rs`
- `crates/platform-core/src/node_build_owned_process_tests.rs`
- `crates/platform-core/src/node_development_build.rs`
- `crates/platform-core/src/node_development_build_tests.rs`
- `crates/platform-core/src/node_development_git.rs`
- `crates/platform-core/src/node_development_git_tests.rs`
- `crates/platform-core/src/node_development_restore.rs`
- `crates/platform-core/src/node_development_restore_tests.rs`
- `crates/platform-core/src/node_development_tasks.rs`
- `crates/platform-core/src/node_git_owned_process_tests.rs`
- `crates/platform-core/src/node_install_owned_process_tests.rs`
- `crates/platform-core/src/node_install_tasks.rs`
- `crates/platform-core/src/node_install_tasks_tests.rs`
- `crates/platform-core/src/service_node_development_tests.rs`
- `crates/platform-core/src/service_node_git_owner_tests.rs`
- `crates/platform-core/src/service_node_install_tests.rs`
- `crates/platform-core/src/service_source_publication.rs`
- `crates/platform-core/src/service_source_recovery.rs`
- `crates/platform-core/src/source_acquisition_owned_process_tests.rs`
- `crates/platform-core/src/source_import_cleanup.rs`
- `crates/platform-core/src/source_import_cleanup_tests.rs`
