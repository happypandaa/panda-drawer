# Node 应用代理的实际排空

本步只修复私有回环网页代理及 NodeApps 的代理关闭接点。基线为 `2bdb1277dca69a7f46de976e0adce83e5e4a9a52` 的干净源码快照；验证使用独立源码副本、独立 target、TempDir、人工文件、回环 HTTP 和人工解释器进程。没有修改真实项目、固定开发数据根、开发身份、窗口或签名配置。本步不授予自动安装许可，也没有改变桌面 `exit_ready`。

## 所有权与结果

`NodeAppProxy::start_retaining_failed` 返回成功代理，或同时返回原始错误与失败代理 owner。产品调用方必须保留失败 owner，直到其代理与运行时分别确认完成关闭。模块测试保留一个裸 start 便捷包装，只用来验证原错误及关闭问题的汇总；产品不能用丢弃失败 owner 的包装替代上述合同。

代理启动的真实根目录/入口检查、每次已接入的 request、静态文件 open/read、响应 producer、上游 HTTP/1 connection driver、Axum graceful serve 均登记实际任务。普通请求接纳、停止 seal 和登记共享提交锁。已接入请求把原有 IoLease 转交给衍生 I/O；关闭后不再尝试第二次普通接纳。返回响应头、客户端丢 Body、丢等待 future 或 oneshot send 失败都不算实际工作已结束。

静态文件由登记的 blocking worker 持有真实 File。响应 Body 只接收有界通道；未消费/丢弃 Body 不会丢失文件 worker 的 JoinHandle。停止后阻塞 read 若尚未返回，结果保持 Pending，没有“两秒后 abort 即完成”。

上游使用锁定版本的 hyper HTTP/1 和 TokioIo，直接连接 `127.0.0.1` 的既有后端端口；不使用代理、重定向跟随或隐藏连接池。敏感内部 Bearer 只在 Rust 注入，浏览器 Cookie/Origin/任意头不传后端；请求体限制、响应头白名单、跳转与升级拒绝、CSP 保持原合同。原响应 producer 持有 Incoming 和取消 guard。停止或 Body 接收端消失会使实际 driver 释放其 TcpStream；仍须观察该 driver 的真实 join，关闭通知本身不算完成。这里只增加现有锁版本的直接依赖，没有升级网络依赖或自行解析生产 HTTP。

`request_stop` 同步关闭接纳、撤销网页授权并发停止信号。`stop(&self) -> Result<(), ApiError>` 启动或复用单一 owned coordinator，等待实际任务、I/O lease、serve 及其 graceful 连接完成，再等待 coordinator 的 done 标记。调用方丢 stop future 不取消 coordinator。正在 join 的队列与 server 句柄一直保存在 owner 内，协调器意外 panic 不会因局部 Vec 被丢而失去其余句柄。

普通后端 reset 等传输错误在实际 socket 已关闭、driver 正常 join 时，作为请求/Body 的业务错误回执，不因这类已知关闭永久阻止退出；原传输错误可能包含敏感 URL，不直接输出。JoinError、worker/serve/driver panic 或协调器 panic 为 sticky Blocked/unknown；重复 stop 不把空槽或已发送停止信号解释为成功。失败协调器保留 owner；无法取得执行器同样保留 owner 并报告 Blocked。这是有意的失败留存，可能一直阻止退出，不能称作资源已关闭。

`NodeApps::stop_all` 先为所有代理发停止，然后并行驱动每个应用的 runtime 与 proxy 关闭。第一个代理文件繁忙时，后续应用的 runtime 仍会被实际 poll 并停止。每个 Running 保留在受管 map 中；仅在该 runtime 的既有进程证明及 proxy 排空均成功后移除。runtime 已停、proxy 未确认关闭时，Running 和 active marker 保留，新启动被拒绝。代理关闭失败信息不再被裸 unwrap 转成 panic。

## 明确边界

- 代理的 Complete 只证明本模块登记的文件、producer、直接 HTTP 连接和 serve/连接任务已实际结束；它不证明 Node 后代、Git/npm 进程、原生窗口、WebKit、物理媒体或系统下载已经关闭。
- 本步没有修改 NodeRuntimeTrial 既有进程证明。人工解释器测试证明真实进程/HTTP 排空顺序，不证明配套 Node 版本兼容；该部分需由后续共享进程 registry 及原生验收证明。
- 代理启动的前置 blocking 工作即使丢调用方也有 owner，并在停止后拒绝发布新 listener。更外层公开 Rust `NodeApps::start` 在 Running 插入前仍由调用 future 驱动；本步不承诺裸 crate 调用任意丢 future 的通用启动安全。既有产品 native 命令通过 IoLease.run 持有整个 Node 窗口打开/启动工作；新的桌面 owner 接入仍需审查该合同。
- 已完成任务句柄保留到 stop 观察，避免遗失 JoinError；有界 reaper 可另做，但必须观察真实 join 并永久保留异常。
- 桌面窗口的真实 Destroyed 确认、关闭接纳与窗口创建竞态、浏览器下载 Finished/取消证明、RuntimeService 保留和 `exit_ready` 的组合许可仍是后续工作。操作系统强制终止不能由本协议保证。

## 验证

最终检查的命令、时间和退出码保存在独立副本 `proxy-freeze-checks.json`、`proxy-closed-transport-checks.json`，完整索引见 `node-proxy-verification-summary.json`，原始日志未提交。真实回环测试包括：超过旧两秒阈值的文件 read、Body 未消费/丢弃、stop 等待者丢失、seal 后衍生工作、前置 open 丢启动等待者、worker/serve/driver panic、实际上游 socket EOF 与 driver join 分离、丢响应接收端后主动释放上游。NodeApps 回归包含两个真实人工 runtime 的停止顺序、runtime 已停但 proxy 失败时保留、丢 stop 等待者后的 Running/marker 保留。人工项目的源文件与数据哨兵未被清理。

最终代理专项 **24 通过、0 失败**（2.34 秒）；NodeApps 全集 **54 通过、0 失败**（21.18 秒，包含上述 9 项生命周期测试）；Node 登记集成 **10 通过、0 失败**。核心全目标严格 Clippy 与全工作区格式检查通过。未重跑完整核心全库；主线合并后仍需整合验证。未执行 GUI、真实业务下载或签名安装，不把这些后续项目计入本步通过。

主线在 `bc550aa` 上整合：代理专项 24 通过（2.20 秒），NodeApps 全集 54 通过（10.35 秒），Node 登记集成 10 通过；全工作区全目标严格 Clippy、格式及差异检查通过。此处仍是人工项目与回环网络验收，原生窗口和系统下载的关闭证明尚未接入。
