# 可排空关闭第二步：核心所有权与真实完成报告

本步只建立核心服务的关闭协议，不签发自动更新安装许可。桌面正常退出仍需单独接入；Node 进程树、原生窗口与 WebKit 下载取消完成也需保留各自的证明。开发身份、固定数据根和用户资料保持原合同。

## API 与所有权

`RuntimeService::begin_quiesce()` 关闭接纳并启动或复用单一 owned coordinator，立即返回报告；`quiesce(&self)` 等待本次报告，`shutdown_status()` 只读取状态。原有 `shutdown(self) -> Result<()>` 保留兼容，但只在报告 Complete、resourceClosed=true、无问题时返回成功。报告是诊断结果，不是 updater 许可。

调用方只等待结果，丢失等待 future 不会取消真实排空。协调器拥有 HTTP JoinHandle、停止信号、实例锁和迁移前后的 HTTP 状态。Pending 表示仍在等待；Blocked 保存问题及各项 resourceClosed/retryable。已取出的失败句柄不会在下一轮因 None 被解释为成功。Blocked 协调器继续持有资源和实例锁，即使消费式 shutdown 已返回 Err、外部 RuntimeService 已释放。成功的非消费式 quiesce 仍持有实例锁，直到服务 owner 被释放；这样桌面仍未关闭的资源不能提前丢失平台占用。

生命周期导入的 ZIP 前置 inspection 与历史回收站扫描先登记 owned_operation，再运行阻塞文件检查；丢失请求只丢接收端。导入检查结束后，仍在与 stop/take 相同的锁内检查 stopping，再创建任务和登记后续工作。maintenance、history、Node 开发提交的准入与 runner 登记也和停止共锁，不依赖外层请求恰好仍存活。

同一关闭任务接收重复请求。已知可重试的发现文件清理错误可以显式重试；任务/线程 JoinError、未知资源、部分清理或协调器 panic 为 sticky，不自动重试放行。Drop 只发起尚未开始的首次排空，不会偷偷重试已经返回的失败。没有运行时执行器时保留实例 owner，并报告 Blocked。

## 顺序与竞态

1. 关闭普通及 cleanup 接纳，设置 closing，并先广播所有管理器、SQL 接纳和 HTTP 会话的停止信号。
2. 迁移提交从 closing 检查到 runner 登记，与关闭 seal/take 共享 migration_submission 锁。等待全部既有及旧已完成迁移句柄，重新取得当前状态并再次广播，包含迁移安装的新状态。
3. 等待活动 HTTP 请求和真正持有文件/上传/下载/原生工作的 I/O 登记。
4. 同时启动各业务管理器的独立 owned drain，再逐一收集结果。任何一个 manager panic 都不会取消其他 manager 的 join。BuildManager 已先收到停止信号，但其 join/共享任务终态核对最后执行，避免在其他 runner 尚未收尾时提前标记其任务中断。Git/数据工作和全部管理器先完成，随后才关闭 SQLite worker。
5. 等待数据库实际线程 join，再等待 Axum graceful server 及实际连接子任务；不以固定超时、父任务 abort 或响应头返回作为证明。
6. 仅在此前所有步骤无问题时，核对本实例 ID 与凭据归属，再删除本实例拥有的发现文件；删除 helper 返回 Ok 后仍核对两文件实际均缺失。有效 foreign ID、凭据不匹配或 Ok 后仍有文件均为 sticky changed/unknown，不输出凭据值，保留现场及实例锁。connection 已删、token 删除失败属于部分清理，后续缺少 connection 不能变为成功；保留凭据文件及实例锁供检查。

HTTP/native 的实际 owned future 使用 task-local 标记。若操作自身请求关闭，它只得到 Pending 并归还响应与 lease，避免等待包括自己的排空；外部协调器继续等待真实工作结束。

各管理器的已完成 runner 保留到关闭时观察 JoinHandle 结果，不再用 is_finished 当成功证明。迁移旧句柄同样保留；SQLite 缓存替换/重建前在缓存锁外 join 退役 worker，并永久汇总异常。原生或响应 I/O panic 也记录 sticky unknown，不能仅转换为请求错误后在关闭时遗失。

意外协调器 panic 保持 Blocked/unknown；继续向独立状态发出停止并观察 HTTP 结束。它不会在 manager 寿命未知时关闭数据库或删除发现文件，也不会从已经 take 的空槽恢复为 Complete。

## 约束与限制

未知异常没有“强制安全完成”入口。故障场景下协调器会长时间保留实例 owner；操作系统直接终止进程无法由此协议保证成功关闭。大量已完成 runner 的句柄暂保留到服务关闭，换取不丢失异常证据；后续可独立实现有界 reaper，但必须永久保留失败。

`resourceClosed` 在本阶段报告登记的任务、SQLite 线程、I/O 与 HTTP 连接关闭情况。既有 npm 构建/部分 Git helper 的进程组终止仍有未确认边界：发送 kill 或忽略 child.wait 超时不能证明所有后代已退出；本阶段没有改写这些 helper，也不能将 core Complete 扩大为后代已全部消失。接自动安装前必须独立登记这些进程并保留未确认结果。Node 应用服务的既有进程树检查仍由桌面下一步处理。

第一步中 WebKit 未收到 Finished 即销毁回调的情况仍保留 busy lease，可能持续 Pending；本步没有伪造浏览器取消完成。核心 Complete 也不能单独证明 Node 进程树及原生 WebView 已完全关闭。桌面旧错误后 exit_ready 行为属于下一独立步骤，本步不把该旧行为称为安全退出。

验证仅使用独立源码副本、独立 target、TempDir、人工 SQLite/文件和回环 HTTP。没有读取真实开发资料根、业务内容，未运行 GUI、下载公网业务或签名/安装软件。

- 最终源码 `cargo test -p platform-core service::shutdown_coordinator::tests:: -- --test-threads=1`：20 项通过，0 失败（7.39 秒），包含真实 HTTP 子连接、SQLite 线程 panic/缓存重建、迁移登记暂停、丢失导入请求、有效 foreign ID/凭据不符、部分 unlink、任务终态尾序等情况。
- 最终源码 `cargo clippy -p platform-core --all-targets -- -D warnings`、`cargo fmt --all -- --check`：通过。
- 独立副本的 18 项版完整串行回归为 659 通过、0 失败、40 忽略（221.26 秒），登记集成另 10 项通过。最终 20 项版完整回归为 660 通过、1 失败、40 忽略；失败发生于既有人工 Node 解释器版本探测的三秒超时，尚未执行该测试的关闭断言，单项重跑通过（1.13 秒）。失败日志和此前 13、17、18 项版日志均保留，不能将重跑称为该轮全量通过。

## 主线整合验证（2026-10-25）

将本步整合到包含数据 Git 取回、原生入口与正式资源锚的开发分支后，两处数据恢复关闭数据库调用传播实际关闭错误，测试清理明确检查管理器及数据库关闭结果。主线完整串行核心回归为 **679 通过、0 失败、53 忽略**（224.72 秒），Node 登记集成另 **10 项通过**；CLI 为 **23 项单元测试及 1 项离线元信息测试通过**，桌面为 **18 项通过**。全工作区全目标严格 Clippy、格式检查通过。这些结果覆盖代码与人工资源，不代表真实窗口、完整子进程树或自动安装已验收。

随后增加仅用于测试的真实 SQLite 线程 panic 回归，在数据库已移出但尚未提交、数据已提交两个状态下，核对恢复任务的 `DATABASE_CLOSE_FAILED` 回执仍报告实际提交状态、原恢复任务及保留路径，plan/journal/slots/数据字节不被关闭失败改变。显式继续可完成恢复，而历史线程 join 异常仍保留于最终关闭报告。包含该项的真实 Git／SQLite／HTTP 取回短集 **10 项通过、0 失败**（182.67 秒）；这是本轮选定的短集，不重复声称已重跑此前完整故障边界矩阵。新增测试默认忽略，需显式指定独立 Git 资源，未连接真实开发资料。

## 待单独闭合的进程调用链

- `builds.rs`：BuildManager::run → npm ci / npm run build → command。当前路径向组发送 TERM/KILL，错误分支忽略 child.wait 结果，随后直接清空 BuildProcessGroup；输出 reader 分别等待两秒，超时则 abort 并 await reader。已等待 reader 任务结束仍不能证明逃出/残留的后代已消失，组清空也不是退出证明。需要独立 owned process 身份、真实 child wait 与后代核验、reader 完成结果，以及未知结果的 sticky ledger。
- `project_git.rs`：源码状态/保存/取回、data_objects、build_guard 等经 Context::run；引用提交经 Context::commit_transaction。两个方法都创建 Process，在取消/错误时丢弃 try_join 内的 stdin/stdout/stderr/child.wait future，然后调用 finish。finish 的 stop 先 take group，start_kill 后仅等待 250ms 并忽略结果；Drop 不能再对已清空的 group 提供证明。这里 reader 是同一个任务内的 future，需在后续步骤保留其资源和真实 child/后代结果，不能以 manager runner 已 join 代替进程关闭。
