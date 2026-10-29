# 网页构建代次与窗口占用

确认与实施日期：2026-10-29。对应单目录实施说明 v2.1 第二批；第三批迁移自动收尾另行实现。平台继续负责 project-v1 网页构建，不改成外部 AI 直接发布。

## 保留规则

当前发布仍有两个用途明确的路径：项目 `dist/`（清单的 `outputDir`）保存当前发布输出，`.platform/artifacts/<buildId>/dist/` 为版本 URL 提供不可变资源。本批收敛的是构建代次，不宣称物理上只保留一份当前字节，也不重构这套发布事务。

成功提交构建后，保留当前不可变代，以及仍被真实原生窗口或文件读取生产者占用的旧代。其余旧代清除。已结束的成功构建暂存根 `.platform/builds/<taskId>/` 一并移除，包含 `previous`、交换后承载旧输出的 `publication`、`restore`、已清候选的描述文件等材料。失败或未知写入者、未解决的发布／源码／数据恢复不得凭任务状态猜删。

新发布已经提交而清理失败时，网页更新仍为成功。`result.artifactCleanup` 包含 `cleanupPending`、`remainingPaths`、`issues` 和 `removedPaths`；界面说明发布已提交、旧构建收尾未完成，并列出残留位置。确定已提交的错误回执也可在 `error.details.artifactCleanup` 携带同样信息。没有新增任意路径清理或通用历史管理入口。

## 占用与访问

新建／重载原生网页窗口，在同一个选择与清理协调门内取得具体构建 ID 和占用。原生启动 ticket 固定这次入口；之后即使新构建完成，首个 HTML 也不会无声切换到另一代。重复打开已有窗口只聚焦，不另取代次、不刷新草稿。普通 CLI／HTTP 诊断 ticket 保持动态 `/p/<id>/` 首入口，不建立永久浏览器会话占用。

窗口占用交给当次原生 `Receipt`，只有真实 `Destroyed` 回执释放。关闭请求、失去等待者、创建失败的未知状态、注册表暂时没有窗口、过时回调均不是释放许可；旧回调只能释放自己那次资源，不能影响替代窗口。资源释放发生在原生注册表锁之外。

每次 HTTP 静态读取另有短占用。HTML 在完整读取后释放；其他文件由实际阻塞生产者持有占用与项目读门，持续到文件描述符和生产者真实结束。响应 future、Body 或客户端丢失不能提前释放仍在执行的读取。最后一项占用释放后，清理由已登记的后台工作持有，项目删除和全局退出等待其真实完成。

## 无构建来源的现役网页

已有可打开项目即使没有构建来源，仍可从原生窗口使用。首次打开时只为这次窗口捕获完整当前输出，生成 `view_<uuid>` 入口快照，放在固定数据根 `web-project-ownership/<projectId>/artifact-views/<viewId>/dist/`。作者源码和 live 输出不被改写；这不是版本历史，也不是原项目迁移。

捕获前先取得短期项目操作登记与项目读门，再重入代次协调门复核来源；复制实际结束后释放短门，不随窗口长期持有。迁移或删除封口、项目独占写入已开始时，捕获被拒绝，不新建控制材料。

写入 payload 前先落最小收据：项目和文档代次、host 生命周期 nonce、运行状态 scope、owner 目录与 view 容器持久身份、来源路径与身份。创建时核对来源仍是同一次输出；读取只接受本 scope 的 ready 收据。删除以收据中的 owner、容器和生命周期绑定确认归属，不要求来源路径仍存在，不凭目录名收养内容。同一 host 新建状态不会清理另一个仍存活或未知 scope 的快照；上个 host 遗留与已确认结束 scope 的无占用快照才可回收。替代 owner、容器、坏收据均保留并报告。

view 与普通构建资源沿用同一个窗口／读取生命周期。旧状态访问已封口时，晚到释放只能清自己固定控制根内的已核对 view，不能重建控制目录，也不能写入旧或新工作区。项目完整删除先按文档代次与项目根封住该状态见过的实例，再等待读取者和已登记清理；即使最后窗口释放与删除准备交错，晚到释放也只退内存。新文档代次使用同一项目 ID 不受旧实例封口影响。项目完整删除本来就清理其整个 ownership 控制目录。

空间统计将 view 字节和文件计入网页项目 output 与总量，另列 `windowViews`。项目行是引用，不重复加入工作区／应用数据整体合计。缓存用固定控制目录和收据的只读元数据指纹识别 view 增删及 ready 变化，不靠旧 scope 向工作区写失效标记。

## 恢复与迁移边界

启动顺序是发布日志恢复、任务提交对账，然后清理当前之外且无占用的代次。损坏当前产物、未解发布日志、同次开机尚未确认结束的写入者均不能授权清理。每个删除目标绑定打开的原目录描述符与持久身份；路径中途被替换，不会把替代目录当成原目标。

`RuntimeHandle::artifact_access()` 返回绑定当时运行状态的句柄，提供 `seal_access`、`resume_access` 和 `wait_readers_drained`。迁移应保留旧句柄，撤销旧访问并等待真实文件读者；窗口长期 pin 不计入 reader 排空，也不要求刷新或销毁已有草稿。旧 scope 释放不读写迁移后的工作区。第三批使用这一边界，不能在状态切换后查询新句柄冒充旧读者完成。

## 实现与检查

核心入口为 `build_artifacts.rs`、`build_window_views.rs`、`builds.rs`、`service.rs`、`project_cleanup.rs` 和 `service_shutdown.rs`。原生接入在 `main.rs` 和 `business_window_lifetime.rs`；空间统计在 `storage.rs`。没有删除源码模块；运行时删除范围为旧不可变代、已完成成功构建暂存根、已释放且身份有效的窗口快照。

本批文件如下；两个未接入模块的第三批迁移草稿不在本批范围内。

- `crates/platform-core/src/`：新增 `build_artifacts.rs`、`build_artifacts_tests.rs`、`build_window_views.rs`；修改 `builds.rs`、`build_pipeline_candidate_tests.rs`、`history_cleanup.rs`、`history_cleanup_tests.rs`、`lib.rs`、`project_cleanup.rs`、`service.rs`、`service_io_lifetime_tests.rs`、`service_m2_tests.rs`、`service_shutdown.rs`、`storage.rs`、`storage_tests.rs`。
- `apps/desktop/src-tauri/src/`：`business_window_lifetime.rs`、`business_window_lifetime_tests.rs`、`main.rs`。
- 前端：`apps/desktop/src/ProjectTask.tsx`、`scripts/ui-interactions-test.mjs`。
- 文档：新增本文；修改 `docs/app-window-runtime-contract.md`、`docs/development.md`、`docs/documentation-map.md`、`docs/local-service-and-ai-tools.md`、`docs/platform-development-contract.md`、`docs/remaining-technical-decisions.md`。

验证使用命名的可丢弃 TempDir、真实文件描述符、loopback HTTP、既有本机 npm 构建夹具和原生 lifetime 逻辑测试，不操作现有开发项目。2026-10-29 定向检查结果：

- `node scripts/rust.mjs check --locked --workspace --all-targets` 通过。
- `node scripts/rust.mjs test --locked --workspace -- build_artifacts history_cleanup storage business_window_lifetime service::m2_tests builds::tests io_lifetime --test-threads=1`：核心 113 通过、2 忽略；桌面 12 通过；无失败。
- 后补的 `deletion_seals_the_last_pin_retirement_gap` 回归单独运行，1 通过，覆盖最后 pin 从注册表移除后、后台工作登记前与删除准备的交错。
- 捕获操作补齐短期登记后，`node scripts/rust.mjs test --locked -p platform-core build_artifacts -- --test-threads=1`：最终 15/15 通过，包含上述交错回归与两项捕获写入登记／拒绝检查。
- `node scripts/rust.mjs clippy --locked --workspace --all-targets -- -D warnings` 通过。
- `npm run test:ui`：455/455 通过；TypeScript／Vite 前端构建通过，仅既有大于 500 kB 的 chunk 提示。
- 标准 `npm test -- -- --test-threads=1`：核心 893 通过、39 条件忽略，既有 `blocked_instance_stop_keeps_status_and_another_instance_available_after_waiter_loss` 在首次版本探测处出现 `PROCESS_WORK_TIMEOUT`，使该命令以 1 项失败退出。按用户要求不调查或修改该偶发问题；同一工作区构建的该用例随后单独复跑通过。其余工作区 crate 另行完整执行，153 通过；登记集成另行执行，10/10 通过。没有把多次运行合写成一次无失败的完整运行。

以上尚未证明真实原生窗口跨三次构建、进程硬崩溃或断电恢复。最终开发包的真实原生检查与公开 Node 仓库端到端由整批统一验收另行记录；第三批仍须排空附件、任务和其他管理 I/O，不能用静态资源 reader 排空冒充所有旧目录读写均结束。
