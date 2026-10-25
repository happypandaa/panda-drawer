# Mac 更新 helper 前置：实际父进程退出见证

本步骤提供可复用的 `platform-update-witness` Rust 小库，只在 macOS 编译实际接口，不依赖 Tauri、core 运行时或签名凭据。它不启动 helper、不退出原应用、不安装或替换文件；未接入桌面运行路径，生产运行门禁保持拒绝。

## 为什么单独实现

既有 `native_process` 持有 Child、超时停止并 wait/reap 的规则适用于本程序创建的子进程；安装 helper 观察自己的父进程不能复用该规则，也不能通过数字 PID 存在性检查放行。共享进程组登记解决受管子孙及 I/O 的收尾，同样不是父进程退出证明。

本地 Tauri 2.12.4 的 `AppHandle::restart` 在主线程上可能直接执行 restart；`request_restart` 的请求失败也有直接重启回退，不能拿作本合同的退出握手。工程仍未依赖 `tauri-plugin-updater`；本步没有安装插件。此前核对的[官方 updater Mac 实现](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/updater.rs)在调用进程中替换 `.app`，不能直接连接当前 `ReadyForNativeExit`。

## 真实注册和一次性证据

`ProcessIdentity::current()` 从 libproc 读取本进程的 PID、开始秒／微秒和有效 UID。未来父程序应经可信启动通道把这些预期字段及单次 nonce 交给自己的 helper；本库不读取命令行、环境或磁盘配置来猜测它们。

`ParentExitWitness::register(expected, nonce)` 先单次认领 nonce，再要求实际 `getppid()` 等于预期 PID、父进程有效 UID 与 helper 相同，`proc_pidinfo(PROC_PIDTBSDINFO, arg=1)` 读取的完整 PID／开始时间／UID 与预期完全相符且不是 zombie。读取失败、PID 丢失、开始身份变化或非父进程一律拒绝。

随后创建专用 kqueue，设置 `FD_CLOEXEC`，用 `EVFILT_PROC + NOTE_EXIT + EV_ONESHOT + EV_RECEIPT` 注册。必须收到同一 PID／filter 的成功注册回执（EV_ERROR 标志、data=0），并再次确认相同的存活父进程身份，才可借用 `RegistrationReceipt` 发送“已注册”握手。注册回执只证明观察已建立，不证明父进程退出。

`wait(budget)` 在同一私有队列上有界等待；零事件或 EINTR 返回 `Pending`，保留原 FD、预期身份和 nonce，下一次可继续观察。预算最多 60 秒，超预算仅拒绝该等待。其他内核错误／意外事件使该 witness 保持失败，不能随后靠 PID 消失变为成功。只有该队列返回正确 PID／filter 的无错误 `NOTE_EXIT` 才产生一次 `ParentExitEvidence`；之后等待或读取注册回执均拒绝重放。

退出证据字段私有，没有 Clone 或 Deserialize，也没有从 JSON／文本构造证据的入口。Drop 仅关闭自有观察 FD；不发送信号、不强制退出、不制造证据。[Apple kqueue 文档](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kqueue.2.html)区分成功注册回执与 `NOTE_EXIT`，后者表示被观察进程已经退出。

nonce 只做严格的 64 个小写十六进制字符格式检查，不生成随机数、密钥或凭据，也不证明输入熵。每个 helper 进程内认领不可撤销，失败尝试也烧掉该 nonce，最多保留 256 个；并发认领仅一个成功。**此原语没有跨 helper 重启的持久 nonce 账本，不承诺跨进程重放保护或启动通道认证。** 未来 helper 仍须实现可信启动绑定和对应重放策略，不能用日志中的 nonce 代替授权。

## 实际验证（独立源码副本及主工作区，2026-10-25）

在当前 macOS 27.0.1 arm64 上：4 项单元测试、4 项实际进程集成测试、2 项编译失败接口测试均通过，0 失败／忽略。严格该 crate 全目标 Clippy（包含 fixture feature）和格式检查通过。运行入口：

```sh
CARGO_INCREMENTAL=0 node scripts/rust.mjs test --locked -p platform-update-witness --features test-process-fixture
CARGO_INCREMENTAL=0 node scripts/rust.mjs clippy --locked -p platform-update-witness --all-targets --features test-process-fixture -- -D warnings
```

测试 feature 才编译 `tests/fixtures/witness.rs` 中的人工 parent/helper 二进制，不进入 `.app` 资源。实际 parent 由测试驱动持有 Child，启动自己的 helper 并等待真实注册握手；存活时多次 wait 均 Pending，即使父端写入伪造 Ready 文本并关闭控制 pipe，仍 Pending。驱动随后明确要求这个人工 parent 自行退出，helper 的原 kqueue 收到退出事件；证据中的 PID、开始时间及 nonce 精确匹配，再次消费及 nonce 重用均拒绝。

其他实际进程测试覆盖开始身份错误、非父 PID、父进程在注册前已退出；这些情况均拒绝且不产生退出证据。另以真实 kqueue 验证 CLOEXEC 和内核拒绝不存在 PID 的注册，验证同进程并发 nonce 认领；编译失败测试确认调用方不能 clone 或按字段构造退出证据。人工 helper 按测试自行结束，不向未知数字 PID 发信号；没有操作开发应用、真实服务、业务资料、钥匙串或证书。

结果只证明本机内核退出见证和进程内接口边界。它不证明父进程优雅退出、manager／全部业务窗 Destroyed、WebKit／子孙／文件工作关闭，也不证明父程序代码签名、更新包验签、安装锁、文件交换、重启或恢复。`ReadyForNativeExit`、管道 EOF、PID 查不到和超时继续不是退出证据；这个新的真实证据本身也不是安装许可。最低 macOS 15、签名应用、真实退出后安装和全更新流程仍待后续集成与验收。
