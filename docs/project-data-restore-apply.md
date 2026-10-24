# 项目数据版本取回：应用与同操作恢复

本步骤在本机 Git 数据版本预览之上，提供明确确认后的数据库和附件取回，以及同一操作的继续、撤回和提交后收尾。首期仅支持 macOS、健康的 project-v1 活库、项目根普通标准 Git；旧 `app/.git` 和任意 Node 数据库不纳入。源码、HEAD、索引、程序、配置和当前 `version-data` 均不由本流程替换。不会获取远程对象、生成 ZIP 或删除未知现场。

## 应用与原件保留

应用输入同时绑定明确目标提交、当前 HEAD/分支、源码与 Git 状态、页面代次、实际当前数据摘要、目标快照摘要和预览计划摘要。后台重新读取原始 Git 对象并完整校验目标；取得项目任务租约、业务冻结和独占锁，排空既有业务后关闭 SQLite worker，再生成一致性当前摘要。任一绑定变化均拒绝。业务停写窗口内完成本机槽位核验和交换；大对象的 Git 读取在停写前完成。

完整准备现场独占发布到 `.platform/project-state/data-restore/<projectId>/<taskId>/`。不可变 `plan.json` 固定项目/任务、Git和源码身份、原始及目标文件/目录证明、父目录身份、两个预定注册图像和五个槽：数据库、`-wal`、`-shm`、`-journal`、附件目录。不可变 `journal.json` 绑定计划原始摘要、注册计划和自身稳定身份。计划未发布时的准备材料保留在 `.platform/data-restore-preparing-<taskId>`；不会自动认领或清除失败现场。

所有源、目标和父目录都按不跟随链接的描述符读取，并核对稳定卷身份、inode、类型、权限、大小和内容摘要。五槽预检拒绝重叠、祖先嵌套、大小写及 Unicode 名称碰撞。每一次移动前重新分类全部槽位；使用同卷不覆盖移动，原件保留在本次操作 `slots/<kind>/before`，候选保留在 `after`。目标不包含旧 WAL/SHM/journal；不存在的原伴随文件也有明确“缺失”证明。外部新增、调换或修改的文件不会被覆盖、删除或当作原件采用。

数据库和单个附件上限 256,000,000 字节，数据快照总计 1 GiB，附件文件和目录合计 10,000 项，元数据受大小和数量预算保护。目标数据库完成健康检查，附件文件、空目录和原件证明完整匹配后，才发布事先固定的新 `documentEpoch`。旧页面必须重新打开。撤回同样发布预定的新代次，使操作期间的旧页面失效。

## 持久门禁、完成回执与收尾

持久门禁先于正常或缓存项目读取；活库缺失时也可读取恢复状态和项目展示元数据，不会为了诊断创建空 SQLite。未完成或损坏现场阻止业务、开发、删除、迁移和历史清理；普通源码取回不能绕过这些限制。恢复只接受同任务、计划和不可变日志摘要，不以调用方提供的阶段或现存半截 JSON 推断权限。

真实注册状态为 `before` 时，可继续或撤回；此时数据可能已移动一部分，不能称为“数据未变”。注册发布为 `applied` 后只允许继续同向收尾，发布为 `undone` 后只允许完成撤回方向。`operationOutcome` 继续使用通用 `not_started/not_committed/committed/unknown`；另以 `dataState` 表达注册发布态。每次动作依赖新鲜槽分类和 epoch 证明，不可变 `journalDigest` 不表示现场没有变化。

独立 `completion.json` 绑定计划和日志的原始 SHA、项目/任务、方向、固定 epoch 与规范终态摘要；`terminal.json` 再绑定完成回执原始 SHA。两者只在独占锁、冻结和租约下，并且完整核对槽、原件、目标数据和注册后创建。元数据候选独占创建和同步，发布前后均有故障边界。普通失败候选保留在固定身份的 `metadata-attempts`，不读取其内容作授权、不复用、不删除；最多 128 项，每项 16 KiB，合计 2 MiB，并计入项目和工作区预算。

准备时预留两个固定身份的发布槽和最坏最终记录预算。普通候选达到上限后，独立核实已提交方向和全槽，再按不可变证明重建唯一记录；固定 reserve 槽只接受空内容或该记录的严格前缀。通过已核验的 FD 以 `O_APPEND` 补齐缺失后缀，不截断或覆盖已有字节；未知前缀、换 inode、非 0600 权限、链接或越限继续阻挡并保全。terminal reserve 还要求完整、合法且自身份匹配的 completion。

terminal 发布后仍为 0600，门禁保持 pending。全部数据、注册、记录和左右父目录完成同步并再次核验后，才通过已核验 FD 将 terminal 封存为 0400，再同步文件。0400 可见之前，核心命名空间已完成持久确认；0600 的合法 terminal 只允许同方向继续收尾。有效的封存终态才解除门禁。完成历史只核对绑定的不可变内容，不再核对旧活库字节、旧 inode 或当前注册代次；因此之后的正常业务、元数据修改、新一轮取回和公开工作区迁移不会被旧历史阻挡。历史终态从不授权再次恢复。

## 接口契约

- `GitDataRestoreApplyInput`：`targetCommit, expectedHead, expectedBranch, expectedSnapshot, expectedDocumentEpoch, expectedLiveDataDigest, expectedTargetSnapshotDigest, expectedPlanDigest`，全部必填，拒绝未知字段。
- `GitDataRestoreRecoverInput`：`restoreTaskId, expectedPlanDigest, expectedJournalDigest, action`，`action` 为 `continue` 或 `undo`，拒绝未知字段。
- RuntimeHandle / 原生管理命令：`project_git_data_restore_apply`、`project_git_data_restore_status`、`project_git_data_restore_recover`。原生命令须经过 manager 认证和 owned I/O wrapper，调用方丢失不使真实工作脱离退出排空。
- HTTP：`POST /api/v1/projects/{id}/git/data-restores`、`GET .../data-recovery`、`POST .../data-recoveries`。三者均需管理认证；GET 不接受查询参数。两个提交入口返回 202 任务，pending/blocked 业务请求返回 409。
- CLI：`project git data-restore --project <id> --input <文件>`、`project git data-recovery --project <id>`、`project git data-recover --project <id> --input <文件>`。
- 任务类型：`gitDataRestoreApply`、`gitDataRestoreContinue`、`gitDataRestoreUndo`，均支持明确取消。任务 runner 始终等待阻塞工作者；关闭时停止新工作并等待实际持锁 I/O 完成。

恢复状态 `{ state, businessBlocked, operations, issues }` 中，`state` 为 `clear/pending/blocked`。每个 operation 返回任务、目标、阶段、计划/日志摘要、标准 `operationOutcome`、`dataState`、可空的 `documentEpoch`、`canContinue/canUndo`、`completionPublished`、`originalRetained`、`retainedPath`、五槽分类和问题。`originalRetained` 仅在原件证明核实后为 true。

任务结果和错误回执保留 `dataState`、标准 `operationOutcome`、`liveDataChanged`、`pending`、`businessBlocked`、`restoreTaskId` 和 `retainedPath`，按实际发布点报告；中断状态与普通失败分开记录。保护开启时，恢复入口独立于普通 Git inspection 和活库读取。

## 验证边界

实现和测试在独立源码副本及独立 Rust target 运行，只使用 TempDir 项目及显式指定的官方 Git 工具包。故障注入覆盖进程内交换/同步/发布边界，并重新构建管理器验证持久门禁。它不等于实际断电、系统重启、256 MB 对象压力或最低系统版本验收。原生桌面、签名安装包与干净机器验证由主任务单独记录；本步骤核心/HTTP测试不冒称完成这些验收。

2026-10-25 在本机 macOS 的实际专项检查：普通应用覆盖 41 个移动、同步、epoch、完成回执及 terminal 封存故障边界；撤回覆盖 39 个边界，全部明确恢复成功。最新代码的 9 项真实 Git/SQLite/HTTP短项复验通过，包括缺库时的管理认证、202 任务受理、409 业务保护、项目元数据可读、旧页面失效、空目录/完整附件、HEAD/index保持、恢复后实际 SQL 读写和公开工作区迁移。缺失/损坏/同字节新 inode 的计划、日志、回执，以及错误 reserve 前缀和未知内容均保全并拒绝；完成历史允许普通业务修改、后续取回及保留权限的迁移复制。

`platform-core --all-targets` 严格 Clippy 和 CLI 23 项检查通过。原始失败和后续重跑日志保留在忽略的验证副本中：早期测试把正确的 `Interrupted` 当成 `Failed`、硬编码候选故障点偏移，已按真实语义修正；strict-identity单测使用 macOS `/var` 别名，被 no-follow 文件检查正确拒绝后，测试改为 canonical 根。容量矩阵最初把多次 128 项保留历史累计在同一项目，正常耗尽项目元数据预算，已拆成每个故障点独立 TempDir 项目。未通过放宽生产保护或删掉保留历史使测试通过。

最终满容量专项通过 19 个 completion/terminal 创建、半截追加、同步、独占发布和封存故障边界，每个边界均使用独立项目，并在失败后重新建立管理器继续同一操作；两个固定 reserve 槽都覆盖，128 项既有失败证据保留。该专项运行 377.42 秒。全 core 默认回归第二轮为 621 项通过、51 项按要求忽略，Node 登记集成 10 项通过；第一轮唯一临时路径测试失败的原日志保留。工作区格式检查通过。普通 80 边界、容量 19 边界与 9 短项分别运行，不合并声称某一条命令一次全绿；早期矩阵中的 fixture 配额失败也未改写。
