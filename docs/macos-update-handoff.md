# Mac 更新前置：有界父进程与 helper 交接

本步新增 `platform-update-handoff`，连接实际父进程退出 witness 与一次性继承 IPC，并为桌面退出协调器提供内部严格回执。没有接入 `main.rs`、菜单、更新 UI、网络或安装路径；专用 parent/helper 仅在测试 feature 编译，不进入应用资源。没有签名、生成密钥、发布、安装或重启应用，开发版也没有自动检查。所有输出均为 `installAuthorized:false`。

## 严格回执的来源

`DesktopShutdown::arm_handoff` 只允许关闭尚未开始时的一次绑定，并核对完整当前父身份；失败也不能改绑。`CleanDrainReceipt` 字段私有，无 Clone、Serialize、Deserialize 或公开构造。既有 `ReadyForNativeExit` 可以包含 `resourceClosed:true` 的业务错误，不能生成这个回执。

协调器必须实际观察 windows、nodes、sessions、confirm_core 四个 Job 的 Join 结果全部为 Ok，fresh core 为 closed、非 blocked 且 issues 为空，sticky 也为空。候选形成在 `changed`/`ready` 回调正常返回之后。取回候选还须实际 poll 并消费原协调器 JoinHandle 的 Ok 结果；Pending 保留原句柄，panic/取消与 sticky unknown 拒绝回执。回执只可消费一次。它证明所列工作成功，不证明整个父进程、manager 或所有系统 WebKit 资源已退出。

`ParentEndpoint<P>` 在 launch 时固定 P，消费同一 SessionBinding 的 P 后才能声明排空。`DrainProof` 是开放的可信适配器契约；其他调用者可以实现别的 P，库本身不能据此认证应用关闭。桌面内部使用的 P 是上述私有回执，JSON Report/Ready 无法构造它。helper 收到的 wire 消息仍是专用通道上的父进程声明，不是跨进程 Rust 类型或代码签名认证。

## 会话、阶段与持久记录

调用方明确打开自己拥有的 0700 ledger 根；本步不猜测应用数据位置。`SessionDirectory::create_in` 使用系统 getentropy 生成单次随机 nonce，绑定完整 PID/开始秒微秒/UID，以 mkdirat 创建唯一私有子目录；nonce不是密钥。launcher 将专用 UnixStream socketpair 和目录 FD 继承到 helper，源 FD 先复制到独立槽，exec 后设 CLOEXEC。协议不接收命令行/环境中的根路径、PID或旧回执。

helper 先在继承目录中 O_EXCL/O_NOFOLLOW 创建 `claim.json`，写入并 sync 文件和目录；再通过实际 `ParentExitWitness::register` 注册当前真实父进程，才回复匹配的 Registered。收到同绑定排空声明后，持久写入 `drain-accepted.json`，才回复 Accepted。Accepted 只确认排空声明已记录，不提交交接。

父端必须显式调用 `leave_witness_armed`，helper 收到同绑定 Arm 后持久写 `witness-armed.json`，才回复 Armed。helper持久记录Arm后，父端不能再承诺可靠取消。只有匹配 Armed 回执产生 `ArmedForParentExit`；Arm 已发送但回复丢失/超时，父端报告 `HandoffCommitUnknown`，不能声称已取消。未发送 Arm 时，丢弃 Accepted endpoint 或正常父退出绝不会形成交接观察。

提交后仍需原 witness 的 opaque ParentExitEvidence，且专用 IPC 已完整到达空 EOF、期间没有 Cancel、额外消息或残帧，才写 `observed-parent-exit.json`。EOF只是消息完整性条件，不是退出证据。消息上限4096字节，拒绝未知字段/类型、重复字段、身份错配和阶段重放；跨轮询保留半帧。任意已有 claim 均拒绝重用；失败/取消也保留，旧 ack/Armed/Observed 不用于恢复或重造 witness。

持久重放保护只覆盖同一个保留的 ledger 根。0700、create_new 和 fsync 不防同 UID 删除/替换根或伪造声明，不证明签名，也不声称物理断电耐久性。

## 实际进程拥有权与预算

专用 worker 持有真实 Child 和 IPC，独立于 core/shared OwnedProcessRegistry，避免把等待父退出的 helper 排入父进程自身 drain。取消/失败关闭通道并实际 Child::wait；丢失 API waiter 只请求取消，独立 registry 保留 worker JoinHandle。只有实际 Child wait 和 worker join 均成功观察后才报告 Reaped；Reaped 不等于 Observed。未知或未结束的 owner 保留，不发数字 PID 信号。

每个父进程最多保留32个会话 owner（包含已结束的记录），达到预算拒绝新会话；不会通过丢弃未知 Child 释放容量。

显式交接成功后 worker 关闭 IPC并继续持 Child wait；真实父 main 正常返回时，helper独立等待已注册的父退出事件。此前没有 Arm 的 EOF/父退出均拒绝。该步父端协议从 Hello 起固定30秒，helper各阶段最多30秒；调用方预算最多60秒。这是有限会话原语，尚未实现产品长时间排空续期。非合作 helper 可能使实际 wait 未结束，此时 registry 保留 owner、API有界返回 Retained/Unknown，不能报关闭成功。

## 检查与后续边界

检查入口：

```sh
CARGO_INCREMENTAL=0 node scripts/rust.mjs test --locked -p platform-update-handoff --features test-process-fixture -- --test-threads=1
CARGO_INCREMENTAL=0 node scripts/rust.mjs test --locked -p platform-desktop desktop_shutdown::tests -- --test-threads=1
CARGO_INCREMENTAL=0 node scripts/rust.mjs clippy --locked -p platform-update-handoff --all-targets --features test-process-fixture -- -D warnings
CARGO_INCREMENTAL=0 node scripts/rust.mjs clippy --locked -p platform-desktop --all-targets -- -D warnings
```

2026-10-25 在独立源码副本、macOS 27.0.1 arm64 实际通过：4项协议/持久认领单测、12项自有父/helper实际进程测试、14项桌面协调器定点测试（另34项被筛选），0失败/忽略；两crate上述全目标strict Clippy与格式检查通过。尾帧、Accepted后直接Drop、持久失败重放、FD早错清理、实际Join与回调panic/阻塞均有对应回归。独立只读安全复审确认这些边界闭合；主工作区集成结果另记。fixture只用明确可丢弃临时目录、人工文件和自有父/子进程，不操作真实项目或GUI。

整合后主工作区再次运行上述4项单元／12项真实进程，以及完整桌面48项，均0失败／忽略；磁盘监测未触及8GiB停止线。冻结补丁14份文件的整合前后摘要完全匹配，只有本文后补实际记录。协调器测试使用人工backend与真实Tokio工作／Join，不能代替实际RuntimeService、原生窗口或全部WebKit资源的关闭证明。

首轮人工held-ready回调测试曾阻塞Tokio计时驱动，仅确认的自有测试进程被结束并由Cargo回收；改为runtime外有界等待和RAII释放后通过，原失败日志保留。早期资源／fixture全目标编译问题及最终验证记录保存在忽略的 `.test-artifacts/disposable-update-handoff-20261025/verification-v1.json`、`root-validation.json` 和相应日志；没有强杀真实开发应用或服务。

此步仍无 helper 的签名代码身份准入、正式资源锚点、更新包字节与签名校验、下载暂存、安装锁/文件交换/恢复、生产父进程及 WebKit 最终生命周期接线、正式包、最低 macOS 15或干净机器验收。后续不能把本库日志、ledger文件、排空回执、Armed或父退出见证直接当安装许可。[前一退出 witness](macos-parent-exit-witness.md)及[发行合同](release-and-updates.md)继续适用。
