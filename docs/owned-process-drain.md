# 普通子进程与管道的归属（第一步）

本步提供共享 registry 和分阶段退出接点，还没有迁移现有 Git/npm/probe helpers。当前 `resourceClosed` 的产品覆盖边界仍见 `phased-shutdown.md`；此模块不能单独许可桌面退出、更新安装或重启。

## 生命周期与准入

- `OwnedProcessRegistry::start(command, options, keep_alive, make_io)` 与 `seal()` 使用同一准入锁。没有当前执行器、I/O/timer 不可用、已封闭准入或不支持的操作系统会在 spawn 前拒绝。macOS 以独立进程组启动普通子进程。
- 三个 UnixStream stdio 通道在 spawn 前创建并注册 parent 端（仅 parent 端设 nonblocking），子端保持普通阻塞标准 I/O；直接持有 `std::process::Child`，避免 Tokio 在 spawn 后注册管道/信号失败而丢失 Child 的窗口。spawn 后立即关闭 Command 保留的子端；确认退出之后才执行同步、立即返回的 wait/reap。没有使用 Tokio 孤儿回收作为关闭证明。
- spawn 后登记的 record 持有 `Child`、未释放的 PID/PGID 身份、keepalive、全部管道任务及 driver 的 `JoinHandle`。工厂在 driver 内、准入锁外执行；工厂 panic 不丢已登记的原始管道。
- registry 先拥有原始 stdin/stdout/stderr 的三个独立 pump。会话得到各自 64 KiB 有界 `DuplexStream`；输出消费端早退后 pump 转为丢弃内容，继续排空真实 OS 管道。stdin 会实际关闭并 join。不得将独立 reader 藏在会因另一个错误而被丢弃的 `try_join!` 或超时 future 中。
- `IoPlan` 返回一个交互 session 和独立 drain futures。工厂只构造这些 futures；组件自己启动的其他任务、阻塞文件写入或 `tokio::fs` 后台写者不会自动登记。适配时必须将实际工作完成回执纳入受管任务，不能仅等它的可取消包装 future。
- `keep_alive: Send + 'static` 必须实际拥有本次 operation 的项目/源码租约、gate 和候选临时目录。产品 helper 不得把保护资源留在可能先返回或取消的调用者栈中。该登记替代 helper 自身的进程生命周期保护，不应额外持有退出前序正在等待排空的全局 I/O lease，形成循环等待。

## 结果和清理证据

`ProcessOutput { status, value }` 仅在 leader 真正 wait/reap、同组普通后代不再活动、全部已登记 I/O 任务实际 join 后交付。非零退出码或 reader 正常返回的业务错误可以得到干净的资源收尾；任务 panic、丢失 child/driver 归属与不完整组枚举会保留不可核销的失败证据。

`ProcessRun` 的 Drop/cancel 只发停止信号，不 abort 实际 driver。工作期限转入 TERM→KILL 和排空；清理期限仅交付 `PROCESS_CLEANUP_PENDING`，后台 owner 继续持有资源和 keepalive。此错误不代表任何资源已关闭。随后真实完成才能核销该 retryable issue；未知/panic 证据不能由之后的成功操作或空句柄擦除。

macOS 使用 `waitid(WNOWAIT)` 保留退出 leader，先核查组再 disarm 并 reap；reap 后不再按旧数字 PGID 发信号。`proc_listpids` 的零返回（也可能表示系统调用失败）、满预算、不合法长度均拒绝作为空组证明；完整列表须含唯一的保留 leader，拒绝非正或重复 PID。列出成员的消失/身份变化要求重新检查；只有两次完整的、相同 PID 与启动时间集合均只包含已验证 zombie，才确认无活动普通后代。系统行为依据为 [Apple 的 libproc 包装层](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c#L40-L54)及[包含 zombie 的进程列表实现](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c#L377-L448)，并在本机通过真实进程验证。

会话值或 Git 的提交确认与清理状态是两个维度。清理 pending 时适配器必须保存已收到的提交确认，不得把 pending 一概解释为未提交；没有提交确认也不能仅因调用者收到错误就释放 index 锁或 `safeAbort`。

driver 实际 join 晚于正常输出交付。registry 保留其句柄和终态证据，退出必须 join 后才能将 `ProcessSnapshot.resource_closed()` 判真。目前完成记录在运行会话内保留，长会话内存的有界 reaper 尚未实现；未来清理不得丢失未知/失败记录。

## 服务、迁移与本步限制

registry 位于 `RuntimeSwitch`，实际工作区迁移前后共享同一 `Arc`。退出先封闭其准入并向管理器同步发停；主线在 manager join 前检查受管进程，因为业务 runner 可能需要等真实进程关闭后才能恢复输出。若进程或 I/O 仍未确认，停在 `processes` 阶段报告 Blocked，保留 manager runner、SQLite worker、HTTP join owner、实例锁与发现文件。显式重试只有取得新的实际回执才继续关闭后续资源。资源已确认的其他 sticky issue 仍阻止 Complete/发现清理；不能用 None 重释为成功。

产品 helper 分组迁移网页构建、ProjectGit（含 raw materialize）、source acquisition、Git remote、Git environment probe、Node/npm environment probe、Node development build、Node install。独立 fixture 可用自己的 registry，产品 helper 不可另建默认 registry。独立冻结版本尚未连接 store/manager 或修改旧 Trial；主线共享注入及 Trial 枚举补充见下节，版本 probe/Drop 仍待核对。

边界是本平台受管的普通同组后代；不是 OS 沙箱，不承诺追踪恶意 `setsid`/换组逃逸。当前实际实现和真实进程验收仅为 macOS，其他平台在 spawn 前拒绝；保留默认非 Unix 源码编译边界，不声称实际 Windows 验收。没有操作现有项目、凭据或真实用户数据，也没有制作/发布正式安装包。

## 本步验证

独立 disposable 源码和独立构建目录执行：真实 stdout/非零退出、实际 stdin EOF/完整 UTF8 回显、消费者早退后 4 MiB 输出继续排空、工作超时、TERM 忽略子进程、连续六轮祖孙写入与丢失等待者、实际 TempDir keepalive、清理 pending→继续 join、工厂 panic、reader panic/sticky、reader 业务错误广播停止、无执行器/缺 I/O runtime/已关闭 runtime 与 submit/seal 竞态、失去 unreaped 身份不再发信号、受控枚举变化/零返回预算，以及实际 RuntimeService 的 SQLite/HTTP 保留与真实公开迁移后的同一登记。精确执行条数和检查日志由本步交付 ledger 记录；不将受控 I/O 暂停当作断电或恶意进程沙箱验收。

## 主线整合

主线为 `ProjectStore` 增加内部共享构造入口；初始状态和公开迁移后的新 store 均接收 `RuntimeSwitch` 的同一 registry，`make_state` 拒绝不匹配的 store。独立 fixture 的原公开构造保持默认独立登记。真实迁移与 pending 退出回归改从 store 的实际登记启动人工工作，并核对新 store 继续共享该实例；各产品 manager 的具体命令适配仍分组进行。

`NodeRuntimeTrial` 已复用同一收紧后的组枚举器，仍由其原 `ProcessOwnership` 持续保留 unreaped leader，保留原 Trial 错误码。此整合修复零返回和成员变动的误判边界；它没有改写 Trial 的版本探针、控制 reader 或 Drop 生命周期，不代表整个 Trial 已接入新的进程 owner。

整合的前两轮全库回归均为 699 通过、1 失败、54 忽略，原失败日志均保留。实际定位为 `PROC_PIDTBSDINFO` 使用 arg=0，导致仍由存活父进程保留的 zombie 返回 ESRCH，无法取得稳定身份。按 [Apple 的 zombie 查询分支](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/proc_info.c#L2046-L2063)改用 arg=1，而非把 ESRCH 当作退出证明。独立真实 fixture 在本机核对 arg=0 的 0/ESRCH、arg=1 的完整 SZOMB/PID/父 PID/组/启动时间；不向旧数字 PID/PGID 发信号，保留自己的 Child 并正常 wait/reap。原 zombie 回归改为等待稳定组确认，修复后通过。

主线最终完整串行核心回归 **700 通过、0 失败、54 忽略**（218.00 秒），Node 登记另 **10 项通过**。进程命名选集 **21 项通过**（其中含两项服务整合回归），退出协调器全集 **22 项通过**；两选集有重叠，不作为 43 项独立测试累计。新增整合回归还确认管理器等待进程真正关闭时，退出能先报告 Blocked，再于实际 writer 结束后 join 管理器并关闭数据库。全工作区全目标严格 Clippy、格式及差异检查通过；测试不涉及原生 GUI、真实业务内容或自动更新安装。

另显式使用已核验的配套 Node 运行人工 JavaScript，两项 opt-in 回归均通过（11.59 秒），包含正常控制/健康检查及清除同组子孙而保留独立 Node。这不代表旧 Trial 的全部生命周期、真实业务服务或最低系统版本已经验收。
