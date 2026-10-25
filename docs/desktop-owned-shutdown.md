# 桌面正常退出：owned coordinator 与业务窗口回执

本步接入菜单“退出”／Cmd+Q 的桌面收尾，沿用[窗口合同](app-window-runtime-contract.md)、[核心排空](phased-shutdown.md)、[受管进程](owned-process-drain.md)及[代理排空](node-proxy-owned-drain.md)。开发标识、数据根、业务资料与恢复现场不变。macOS 管理窗口红色 X 仍只隐藏；后台业务继续运行。没有新增自动安装、强制安全完成或 updater 许可。

## 正常退出的所有权

`AppState` 固定持有 `Arc<RuntimeService>`，不再先 take 服务再把关闭错误仅写日志后置 `exit_ready`。第一次退出同步封住项目／Node 窗口创建和普通 I/O，先登记两个实际 cleanup scope（业务窗和 Node／代理／会话），再启动单一 owned coordinator 并广播核心停止。协调器不持有全局 I/O lease，避免等待自己；普通调用方只发请求，丢失等待方不会取消已经接纳的工作。

若核心服务已启动而 Node 初始化随后失败，`StartupFailure` 同样保留这份 Arc 服务和退出 owner；错误管理界面通过 core-only backend 等待实际 quiesce，再进入同一 `ReadyForNativeExit`。未创建核心服务的早期启动失败可以直接退出；不拿 RuntimeService::Drop 的后台清理代替正常 Quit 的等待。

原生事件回调不依赖 Cocoa 线程已经进入 Tokio。入口显式注入 Tauri 的 Tokio Handle，使用该执行器 spawn；缺少执行器时报告 Blocked 并保留 scope。协调器、窗口工作、Node／代理工作、会话清理和最终核心确认的真实任务句柄均由 owner 保留。实际 JoinError／panic 是 sticky unknown，不以空槽、固定超时或 `is_finished` 代替成功 join；协调器自身异常也保留服务和 scope，不在 Drop 中调用可能再次异常的原生 UI。

- `Pending`：仍在等待真实窗口回执、工作或 I/O；不会提前退出。
- `Blocked`：保留错误、`resourceClosed`、`retryable` 和原 owner；管理窗口可见以供检查。再次从菜单退出只显式重试允许重试的工作，不重新接纳 scope、不重新启动已完成工作。Trial reader join／进程归属丢失、未知窗口或任务 panic 不可据此放行。
- `ReadyForNativeExit`：登记的业务窗、Node／代理／会话、巡检和核心都已满足本步关闭条件，且核心 coordinator 的实际完成已确认。仅正常 Quit 可据此设置 `exit_ready` 并请求 Tauri 最终退出。

`ReadyForNativeExit` **不是所有 WebKit 资源 Complete，不是自动安装许可**。诊断管理窗口保留到 Tauri 的最终原生退出才销毁。本步没有证明其最终 WebKit／系统进程销毁，也没有更新 helper 的交接协议；后续自动安装仍需独立闭合该证明。外部浏览器不属于平台原生业务窗口。

## 窗口、会话与并行收尾

创建路径在等待 project／Node 创建锁后重检停止状态；最终 builder 通过主线程操作队列，在真正执行原生创建前再次重检，并由已封闭的 generation registry 拒绝新登记。排队的旧打开请求不能在退出开始后补建窗口。

每次原生业务窗创建绑定唯一 generation receipt，事件闭包捕获该 receipt 与 Node session 的实际 Arc。只有这一次窗口的 `WindowEvent::Destroyed` 才确认销毁；`destroy()` 返回 Ok、原生窗口表里已不存在、导航到 `about:blank`、迟到的旧事件均不充当销毁证明。构建报错而未确认原生窗口状态会保留未确认 receipt，不假装已经销毁。旧代次事件不能清除新代次尺寸绑定或 Node session。

窗口工作等待既有创建锁完成后冻结 Node 导航会话、核对所有非 manager 原生窗口及其 receipt；在请求任何业务窗 Destroy 前，先等待已接纳 WebKit engine 下载的实际 Finished，再向全部业务窗发出销毁请求并等待各自实际 Destroyed 回执。Node／代理停止独立并行执行，不被第一个窗口回执阻塞。业务窗全部确认、Node／代理实际关闭之后才注销捕获的 Node session；代次变化或缺失回执保持 Blocked。窗口与 Node scope 分别只在其正面关闭结果后释放。

Node 后台巡检保留实际 JoinHandle 和停止 watch。退出打断三秒 idle sleep；已开始的巡检继续持自己的 tick IoLease 直到实际完成。Node／代理停止与巡检 join 并行驱动，避免巡检等待 Node 锁时抑制停止。丢失 join 等待方仍保留原句柄，任务异常永久保留。巡检任务本身不套全局 lease。

关闭开始前同步保存已有尺寸偏好；关闭后的 Destroyed 回调仅按当前代次移除内存尺寸绑定，不在 core seal 后新登记文件写入。没有清理用户资料、旧归档或恢复证据。

## 下载与强制退出边界

项目窗继续使用已登记的 `browser_download` handler，原生保存命令继续采用已有 IoLifetime 合同。Node 窗口和 manager 尚未支持通用 WebKit engine 下载；本步显式拒绝该入口，避免 wry 默认接受却没有登记真实文件工作。它不等于新增完整 Node 下载能力，不复用 project ID 的保护规则。

项目 engine 下载由独立 ticket registry 跟踪，Requested 取得原 IoLease 和登记 ticket 与 close 共享锁。退出先封住新 ticket，再封 core；窗口工作只等待该 registry 的真实 Finished，不等待含有自身 aggregate scope 的全 IoLifetime。stage 创建／请求校验失败且尚未接受引擎写入时可以归还 ticket；已接受后仅 Finished（含失败／取消回执）归还。Finished 后的保存对话框撤销及后台复制仍由原 IoLease 合同保护。

已接纳的 WebKit 下载若回调被释放但未收到 `Finished`，继续保留 stage、busy lease 和 engine ticket；callback map 消失不算完成。Quit 不再先 Destroy 仍下载中的项目窗，避免自己丢掉正常在途下载的回调；单个业务窗被提前关闭或引擎异常等仍可能产生未知状态，此时正常退出会持续 Pending。需要真实引擎完成回执或后续独立处理方案，不能为退出丢弃保留现场。

操作系统强制结束、崩溃或绕过可阻止退出的最终事件只有 best-effort owned 请求；不能阻塞主线程等待需该线程投递的 Destroyed 回执，也不宣称这类退出已安全完成。`ReadyForNativeExit` 不扩大 Node 现有普通进程组归属证明的范围，不保证主动逃离受管组的后代。

## 验证（隔离源码副本，2026-10-25）

- `cargo test -p platform-desktop -- --test-threads=1`：最终源码 **44 通过、0 失败**。其中本步新增 26 项；使用同一生产 coordinator／monitor、真实 Tokio 任务与 Join、IoLifetime、TempDir 人工文件和 generation registry。覆盖独立无 current Tokio 的 std 线程入口、显式重试、丢失 Quit／monitor 等待者、scope 阻断真实 I/O drain、并行 Node 停止、worker／coordinator panic、最终核心确认、实际监控句柄保留、旧代次迟到事件、engine close/admit 竞态、未 Finished 不 Destroy、Finished 后只释放 engine barrier 与丢失回调保留人工 stage／I/O。
- core-only 启动失败专项另 **2 通过、0 失败**，使用真实 RuntimeService／回环 HTTP、TempDir 私有 RuntimePaths、人工文件 IoLease 和有效 foreign discovery：等待方丢失仍保留 runtime／实例锁，实际 I/O 释放后才完成；关闭 Blocked 保留发现文件、凭据文件及实例锁，不能退出。RuntimePaths 没有新增生产 custom-root 或公开测试入口；独立副本通过临时 cfg(test) include 调用 core 私有 TempDir 工厂，运行后 lib.rs 逐字节恢复，该 instrumentation 不进交付补丁。首轮人工 foreign ID 非法落在可重试解析错误分支，修为有效不同 UUID 后覆盖真实 sticky ownership 分支通过；两轮日志均保留。该 core test 依赖快照含既有未使用 test-only reader fixture 警告，桌面严格检查的正常 core 依赖无此警告。
- `cargo clippy -p platform-desktop --all-targets -- -D warnings` 与格式检查：通过。
- 测试中的窗口回执由人工触发；**未创建 Cocoa／WebKit 窗口，不证明实际 Destroyed 投递、物理音视频释放、真实保存对话框或系统强制退出**。原生 Mac 验收仍待解锁后另记。
- 构建只使用忽略目录内的非可执行文本 CLI／资源占位，供 Tauri build script 检查存在性；未打包或启动应用，未读取真实开发资料根、密钥或业务下载。它不是发行资源验证。

集成后仍须独立验收：管理红 X 隐藏与菜单退出、多个网页／Node 业务窗销毁及全部后端停止、退出同时排队打开／重启、活动上传／响应体／文件读取及 WebKit 下载未 Finished、可重试关闭故障与 sticky unknown、管理窗最后退出、前后已有项目保全。任何未完成回执都不能替换为安装准入。

## 主线整合验证（2026-10-25）

应用到主线时保留已接入的共享进程 registry：原生启动仍先创建 RuntimeService，再用 `NodeApps::with_runtime` 绑定同一 RuntimeHandle。主线桌面测试 44 通过、0 失败；工作区全部目标的严格 Clippy 通过，补丁空白检查通过。该轮仍未打开真实窗口、构建新安装包或执行自动安装，不扩大上述原生验收边界。

## 本机真实窗口补验（2026-10-25）

用户解锁后，以 `d4cc854` 的 debug 开发包在 macOS 27.0.1 arm64 完成[三场景 E24](three-scenario-verification.md#e24原生数据版本往返正常退出与整个平台重开2026-10-25)所列正常路径。网页及 Node 业务窗已打开时，管理页红 X 只隐藏，同一 Node 窗仍可读回记录且服务 instanceId 不变；“窗口 → 项目管理”可恢复。先 Cmd+Q 正常退出，重开同一包并核对网页／Node 数据和新服务实例，再用菜单“退出开发版”正常退出。

两轮已记录主进程、实际子 Node 的 PID／启动时间和监听端口。正常退出后，两 PID 均消失，各轮三个监听端口拒绝连接，原生应用清单停止，服务发现及控制凭据文件不存在，CLI 为 `SERVICE_NOT_RUNNING`；未强杀进程。数据版本往返的原件和已完成现场保留，最终离线保全另见 E24，不以服务离线代替资料核对。

本次未捕获标准输出中的 `ReadyForNativeExit` 文本或 OS exit code，也未逐个追踪最终 WebKit 系统资源。自动检查中的回执／故障矩阵不能扩写为本次原生已执行：活动 WebKit 下载／响应体／文件工作中退出、退出同时排队打开、关闭重试与 sticky unknown 等仍待独立实机验证。结果仅覆盖本机开发包的正常退出，不是最低系统、正式发行或 updater 安装准入证据。
