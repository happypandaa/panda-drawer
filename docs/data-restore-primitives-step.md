# 数据取回基础原语（首步）

此步骤为后续 `project-v1` 数据取回建立持久保护和注册发布基础。**尚未提供活库/附件替换、应用入口、继续/撤回入口或终态释放协议**；现有数据版本预览继续只读。本步不改变 Git HEAD、index、源码、程序、项目配置和附件。

## 已实现的边界

- 本基础步骤的所有现存日志均阻挡操作。把 `phase` 改成 `completed`、`undone` 或 `not-applied` 不会解除门禁；完整事务必须另行核验所有数据槽位并完成独立终态释放协议后，才能允许业务继续。
- 固定恢复根 `.platform/project-state/data-restore/<projectId>/<taskId>/`，独立于活库。未完成、损坏、未知、符号链接或超过核验预算的记录阻止业务、开发、删除、工作区迁移及历史内容清理；不自动认领或删除现场。
- `PersistedIdentity` 核验项目根、操作目录、数据库及三个注册槽；稳定卷身份存在时不把设备号变化直接视为新对象。测试中的设备号变化为模拟，不代表实际重启验收。
- 正常及缓存 `ProjectHandle` 读取先检查持久门禁；数据库缺失时也先返回未完成数据取回，避免 SQLite 意外创建空库。恢复访问只依据同一任务、计划和日志摘要获得有限许可，不构建依赖活库存在的普通项目句柄。
- 恢复专用访问持有项目操作租约，并通过现有业务冻结和排空锁取得独占访问。注册准备要求同一项目租约与写锁；调用者必须已排空并关闭 SQLite。此步没有独立后台工作者或外部调用入口。
- 预检数据库、`-wal`、`-shm`、`-journal`、附件目录的五个槽位及父路径：拒绝相等、祖先嵌套，以及 Unicode 分解/大小写名称碰撞。
- 准备阶段只创建两个私有注册文件，提前固定应用和撤回的新 `documentEpoch`。拒绝旧式身份、不完整/未知注册字段、非规范 epoch、已占用候选文件和跨卷数据库候选，保留所有已有材料。
- macOS 上以不覆盖的目录发布将完整准备现场放入固定恢复根，完成文件及父目录持久确认后才返回。注册发布使用保留双方文件的原子交换；原注册记录仍在本次操作槽中。
- 发布前核验当前注册和三个槽的身份、内容摘要、大小及权限，并核验应安装数据库的身份与原始内容。只允许原始状态到预定应用或撤回状态，已提交后只接受同向重复核对，拒绝反向操作。
- 若注册交换后失败，回执依据可见注册证据报告已经提交，门禁继续保留。第三方在系统调用边界替换的文件会保留在交换槽，后续恢复停止；不会把它当成本次原件。

## 内部接口

- `data_restore_guard::inspect/check_business_available/check_development_available/check_workspace_available`：只读门禁与状态。
- `data_restore_guard::publish_prepared`：完整准备现场独占发布及父目录持久确认；没有数据替换行为。
- `data_restore_guard::validate_recovery`：计划和日志摘要对应的同一恢复任务许可。
- `ProjectStore::prepare_data_restore_registration`：持有项目租约和对应写锁时准备固定注册图像。
- `ProjectStore::reserve_data_restore_recovery_task/data_restore_recovery_access`：不依赖活库存在的恢复访问。
- `DataRestoreRecoveryAccess::lock`：冻结并排空业务，保留租约。
- `DataRestoreExclusiveAccess::publish_epoch`：核验已安装数据库后发布固定代次；不安装或移动数据库。

这些都是 `pub(crate)` 基础接口，本步没有 HTTP、原生或 CLI 应用能力。死代码允许仅限定于尚未接入公开流程的已测试原语。

## 验证与限制

验证只在 `.test-artifacts/disposable-data-restore-primitives-845zeR` 的独立源码副本和独立 Rust 构建目录运行，数据库和文件使用临时测试目录；未运行真实应用或操作用户项目。基线为 `7923b067c7068f9d32910665e4a2590f724eb3f8`。

最终数据取回原语定向测试 17/17 通过；相关过滤回归 `projects::` 43/43、`source_restore_guard` 17/17、`workspace::tests` 12/12、`lifecycle::tests` 37/37、`history_cleanup` 19/19 通过（过滤集合可能重叠，不相加声称独立测试总数）。`platform-core --all-targets` strict Clippy 通过。日志为副本内 `primitives-tests-final.log`、`primitives-regression-1.log` 至 `primitives-regression-5.log`、`primitives-clippy-final.log`，不进入版本库。

根端与历史目录占名修复合并后，17项原语测试及全工作区／全部目标严格 Clippy 再次通过。首次格式检查指出两个共享文件的排版差异，统一格式后格式及补丁空白检查通过；无生产语义调整。根端记录在 `.test-artifacts/core-native-20261024/data-primitives-*`，不进入版本库。

后续完整流程仍需：目标与当前摘要的新鲜核对、持久数据移动计划、SQLite worker 关闭与 DB/WAL/SHM/journal/附件逐槽交换、原件保留、明确继续/撤回、提交后同向收尾和任务退出排空。必须在这些能力完整通过故障矩阵后才开放应用入口。当前首期注册交换仅验证 macOS；未做断电或实际重启验收。
