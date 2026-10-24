# 源码获取的受管进程与候选保全

本步将 `source_acquisition` 的 execute 和 raw cat-file materialize 接入 `ProjectStore::processes()` 的同一个 owned process registry，移除原 ChildGroup 的 kill/wait 超时后清空。只修改源码获取、配套测试与本文；没有修改共享 service/store/registry、ProjectGit、构建环境、NodeInstall 或桌面退出。开发身份和固定数据根不变，不签发 updater 安装许可。

实现基于 `2bdb1277dca69a7f46de976e0adce83e5e4a9a52` 后根工作区尚未提交的 foundation、store/service wiring、Trial、关闭顺序和参数修正快照，完整文件哈希及实际 status 记录于独立副本的 `ledger/baseline.json`。随后仅在该副本应用 Git probes 前置补丁 `86b936d5a0eed9a423ad3919e75bf37bdf90ea0181e23ee7a8bc6459cdf3f1ea`。交付补丁不包含这些共享前置改动，也不包含冻结的 proxy 步骤。

## 实际所有权

`SourceScope` 实际持有 source 的 ProjectTaskLease、active 忙门、任务管理器和新候选 slot 的目录 File 锚。每个进程的 G 持有同一个 Arc；materializer 的 blocking control 也保留该 scope。外层 runner 保留到发布、业务结果及日志收尾之后。scope 的 Drop 只解除本任务的内存忙门和租约，不写记录、删除目录或自动重试；registry 未确认的 G 不释放，丢业务 runner 也不能提前解除 source 占用。

execute 的 stdout 和独立 stderr drain 交由 IoPlan 拥有。materialize 的 registered session 创建 blocking 写者并真实 await 其 JoinHandle；写者用 runtime Handle 读取已登记的 Duplex facade，所有新文件写入与 sync 使用 std File，在这个实际会话内完成。取消只在协议/文件 chunk 边界检查；阻塞 write/sync 仍须真正返回。没有可因 supervise future 丢失而被遗弃的 tokio::fs 写包装，也没有 try_join 先失败就丢另一条实际 reader。

blocking 写者 JoinError 会成为 registry 可观察的 session JoinError，不能转为普通业务错误后把 G 释放。标准输出/协议、文件预算、无 checkout/filter/script 执行、匿名 HTTPS、拒绝凭据/重定向/危险路径等原合同保持。

## Pending、unknown 与发布

监督只借用 ProcessRun 的回复，一次回复完成后不再次 poll oneshot。取消、超时或磁盘检查失败会请求实际进程停止，再等其 leader/group/session/pump 证明闭合。CleanupPending 时保留 acquiring、显式 process-cleanup-pending、active、任务租约和全部候选文件；不发布 repository、不结束业务任务。状态记录写入失败也不改变实际保留的 owner。

只有 ProcessPhase::Closed 且 leader_reaped/group_exited/io_joined 全真、无 unresolved issue，才能继续原业务结果、发布或结束失败。初次 cleanup 超时后来闭合仍保留原次失败，不自动变成获取成功；回执可明确 processResourceClosed=true。driver 的最终 JoinHandle 证明由共享 registry 的最终 shutdown 单独核对，这里不把 phase Closed 扩大为完整应用退出。

Unconfirmed 则业务任务返回 operationOutcome=unknown、processResourceClosed=false；候选状态 failed/process-unconfirmed，无可用 sourcePath。active、实际 ProjectTaskLease、slot 锚和文件仍由 registry G 持有，新获取被拒绝。不能附加 not_committed/not_registered 覆盖这个未知资源结果，也不会清理未知目录。manager 业务 runner 可以结束，独立 registry 仍报告 Blocked；必须同时检查两层。

候选 slot 在检查、阶段推进、发布和错误记录收尾时与原 File 锚核对。路径被替换时，原目录和替换目录均不由错误收尾写入；报告 SOURCE_SLOT_CHANGED/unknown。该检查没有声称能阻止所有恶意瞬时祖先替换，也没有宣称是完整 FD-relative 文件系统协议。

## Git 前置与限制

产品 service 必须给 GitEnvironmentManager 注入同一 store registry；本步测试 fixture 已如此构造，产品共享 wiring 由根步骤负责。Git resolve 使用前置补丁的 resolve_with_keep_alive，独有 probe hold 包含真实 source scope。完整 verify/probe future 先实际 await，再检查 source 的取消状态，避免丢弃 managed verify 的 blocking 前置。遇 probe CleanupPending 时等 registry 释放该独有 hold；unknown 持有时继续保持 source Pending/忙门，根级前置 registry 可先报告 Blocked。没有逐 probe 公共 recordId，因此没有伪造逐 probe 的 driver-joined 回执。及时取消版本检查/verify 的阶段拆分可由后续前置 API 提供；当前取消可能等该 probe 自身预算后才回执。

本步没有候选恢复/强制释放接口，没有删除、自动重试或发布未知 slot。宿主强制终止不由此协议保证；重启沿既有 source 记录恢复规则保留候选，不执行原路径、不复用旧 slot。源码获取成功也不等于项目注册、程序安装、构建或运行成功。

## 实际验证

全部在新独立源码副本和 target，使用 TempDir、人工 shell Git 协议、实际人工子进程/pipe/文件，以及复制到该副本的官方 disposable Git 2.56.0。没有读取真实项目/开发资料、访问公网业务、运行 GUI 或签名安装。原始日志、时间/命令/退出码、源码哈希保存在忽略目录 ledger，不提交本地证据。

- source_acquisition 默认全集：**30 通过，0 失败，2 忽略**（13.48 秒）。新增 6 项覆盖真实 pipeline 闭合后发布、慢文件写者 Pending/取消、实际写者 panic 后 unknown 与 G 保全、丢业务 runner 后真实写者/忙门/租约保留、slot 替换不改写两份现场、取消版本 probe 仍等真实结束。
- 既有官方 Git 离线 blob/index/filter 单项：**1 通过**（0.13 秒），核对字节和可执行位、无 filter/project script 执行，并实际等待最终 registry driver join。
- 核心全目标严格 Clippy、全工作区格式检查：通过。

两项默认忽略分别为离线官方 Git 和真实公网 HTTPS 获取；只显式运行了前者。没有重跑完整核心库或真实公网下载，未把人工协议证明成公开仓库完整获取/安装验收。初期新增测试错误码断言与 slot changed 分类失败、Clippy 写法失败均保留原日志；上述结果属于修正后的最终源码。

主线在 `5e5084e` 上整合本步，使用已接入的同一 store/Git registry：源码获取全集 30 通过、2 忽略（13.18 秒），官方 Git 离线 raw materialize/index/filter 精确专项 1 通过。全工作区全目标严格 Clippy、格式及差异检查通过。新的 prepare/probe API 已在主线提供，本步继续使用保留兼容的完整等待 API；因此上述取消延迟边界仍然适用。
