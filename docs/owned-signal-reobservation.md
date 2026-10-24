# macOS 自然退出与组信号的有限重观察

本步修正一个可复现的退出竞态，基于 c823524 独立源码副本，不修改既有[进程登记与排空合同](owned-process-drain.md)。没有扩大进程归属、普通后代或 updater 准入；桌面 `ReadyForNativeExit` 仍只用于正常 Quit，不是 installer permit。

## 已观察的问题

在本机 macOS 27.0.1／arm64 的直接受保留子进程实验中，保留 WNOWAIT 身份的 zombie-only 组对 TERM／KILL 返回 EPERM=1；同 UID、仍有存活同组成员时信号正常成功。自然退出与信号相交时，kill 已返回 EPERM，紧接的 WNOWAIT 却仍可能暂未给出 exited 回执；后续才观察到自然退出。原先一次即时 fresh 检查不能将这个窗口当完成，于是会留下 `PROCESS_SIGNAL_FAILED` 的 sticky unknown。

独立 C 诊断两轮各 200 次／2,200 signals：自然 exit→TERM 第一轮 200 个 EPERM 中 139 个即时尚无 exited 回执；第二轮 199 个 EPERM 中 139 个即时尚无回执，另 1 个 TERM 成功。第二轮那 139 个在后续首次实际检查于 0–324 微秒取得自然退出回执（所测中位 158 微秒，不是通用时间上界）。原旧 Git 第七轮日志只有 code、没有 errno，因此不能断言那次历史失败必定源于这个机制。原始材料已独立冻结，没有用测试改名掩盖它。

## 最小行为变化

crate-private `retry_owned_signal` 最多执行 4 次尝试；仅对当前路径明确的 `PROCESS_SIGNAL_FAILED` 或 `TRIAL_SIGNAL_FAILED`，在尝试间使用 Tokio timer 异步退避 2ms。每次都重新进入原同步 signal 逻辑：先证明同一个直接 Child 的 WNOWAIT 身份，发信号后遇非 ESRCH 错误，只有 fresh exited 加完整稳定的无存活后代扫描才可忽略该错误。

等待次数或耗时本身不算关闭；成功发出信号也不算关闭。Owned driver 后续仍执行原 WNOWAIT、完整组检查、disarm-before-reap、真实 Child wait 和全部 I/O／driver join；keepalive 仍到真实终态才释放。持续系统调用错误到次数上限后仍返回错误，原 unknown 分支保留 Child、I/O 与 G。归属失去或扫描不完整的不同错误码立即返回，不继续按旧数字 ID 发信号。初始 errno 保存在最终错误详情；恢复只记录安全的错误码、初始 errno、尝试数，不输出命令参数、令牌或业务内容。

Trial 的真实 async stop 四个 TERM／KILL 接点及 unexpected cleanup 使用同一策略；同步 `ProcessOwnership::signal` 增加与 foundation 同样的 fresh exited＋完整空组窄例外，并保留给 best-effort Drop。Trial 的 std MutexGuard 只存在于每次同步检查／syscall 中，在任何异步退避前已经释放；若等待期间已 disarm，下一次检查立即拒绝，不发送信号。Drop 仍不是完整排空或安装许可，本步没有重写其原有 best-effort 限制。

## 验证范围

验证只使用独立源码／target、直接受保留的人工 Child、TempDir、回环／人工协议样本和 pipe。不读取实际用户资料根或 credentials，不启动 GUI／真实业务，不签名、不安装。

新增专项覆盖：真实自然退出竞态重复、稳定 zombie-only Trial 信号、实际持续 EINVAL syscall 错误达到上限而进程仍活着、异步 timer 允许其他任务运行、失归属立即拒绝、不完整扫描错误不重试、等待中 disarm 后不得再次发信号，以及空原组仍有独立 setsid writer 持 pipe。该 writer 是本次直接创建并独立持有 Child 的 sibling，用于模拟脱组 I/O 形状；不声称能够定位／拥有任意真实逃逸孙进程。

同组普通后代、真实 I/O 与 sticky G 保留由既有 owned-process 和 Trial 回归继续核对。持续 EINVAL 验证是实际 syscall 的边界，不冒充制造了活进程上的持续 EPERM；可恢复 EPERM 的实际内核证据来自 C 诊断及 Rust 自然退出重复样本。没有把空组证明扩成 pipe EOF，也没有把信号重试成功扩成资源关闭。

## 本步实际检查（2026-10-25）

- 新增 `signal_reobserve` 专项 **8 通过、0 失败**；最终轮 foundation 100 个真实自然退出样本与 Trial 50 个样本全部完成实际 reap。即时 EPERM 出现数随调度变化，原始日志保留；不是依赖概率命中才能通过的定时断言。
- Owned process 回归 **26 通过、0 失败**；Trial 回归 **24 通过、0 失败、2 忽略**；NodeApps 回归 **56 通过、0 失败**；原 Git process 选定集 **5 通过、0 失败**。这些集合部分包含新增专项，不能相加声称互不重复总量。
- `cargo clippy -p platform-core --all-targets -- -D warnings`、`cargo fmt --all -- --check`：通过。没有重复全库故障矩阵或声称已跑全工作区。

两项忽略的 Trial 用例需要显式验证的配套 Node 路径，本副本没有为此复制或执行正式资源；当前实际协议回归使用人工解释器身份。完整扫描异常分类的新增断言使用人工错误回执，不冒充真实内核无法读出的列表；既有完整性检查与其预算／代次回归未放宽。GUI、真实公网应用、签名、公证与安装均未验证。

## 主线整合检查（2026-10-25）

在已整合桌面退出、网页 Git 制作保护和 Node 制作／安装／版本服务的 `9efa157` 上应用本步五文件，逐字节摘要匹配。完整 `test --locked --workspace -- --test-threads=1` 通过：核心 763 通过／58 显式 opt-in 忽略（295.86 秒），Node 登记集成 10 通过，CLI 单元 23 与 build-info 集成 1 通过，桌面 44 通过，preflight 37 通过；合计 878 通过、0 失败。忽略项不计为已执行通过。

两项配套官方 Node 的真实 JavaScript 运行／停止及子孙清理用例另显式执行，2 通过（12.12 秒）；它们仍核对独立 Node 不被停止。全工作区全目标严格 Clippy、格式和空白检查通过，独立只读交叉审查未发现可操作 P1/P2。该轮没有重跑整个数据取回故障矩阵、创建真实窗口、生成或替换安装包；不代表最低系统、物理设备、公证或更新安装验收。
