# Node 源码版本服务的实际归属

此增量接通 `RuntimeHandle::node_development_git`，保留原有 `&NodeApps` 公开接口。`NodeDevelopmentLease` 的任务租约由 Arc 实际拥有，`keep_alive()` 仅组合本次任务 Arc 与 `NodeDevelopmentSourceLease::keep_alive()` 的实际源码 active token。所有 NodeGit 操作接收当前 store 的同一进程 registry 和此 owner；不捕获 HttpState 的 Active 或全局 I/O lease，避免退出前序等待与进程排空形成循环。

调用者收到 Pending/unknown 或失去回复后，进程登记仍持有该 owner。源码开发、启动和删除继续被实例令牌阻止；公开工作区迁移真实等待 transient task lease，直到进程组与已登记 I/O 真正完成。底层永久 unknown/sticky 仍沿原 registry 门禁保留，不因服务方法返回而核销。本增量不单独许可桌面退出、软件更新安装或重启。

Git/源码操作成功后仍执行原有租约校验。若校验失败，保留原错误 code/message 并附上刚取得的实际业务 result；只有该实际回执自身明确携带 `operationOutcome=committed`，才在错误中保留 committed。不会根据 action 类型、停止原因或 status/preview 读取结果猜测已经提交。底层本来返回的 Pending/unknown 错误继续原样透传。

验证只使用新 disposable 源码、NodeApps、RuntimeService 和完整真实任务租约。版本探针夹具给出人工版本文本，但确实执行 OS 进程并保持实际 stdout/stderr pipe；另一显式测试使用复制的官方 Git，经过真实初始化及元数据命令。pipe holder 是测试自建、独立会话且自结束的进程，仅用私有文件请求其退出，不按陈旧 PID 发信号，不作为恶意逃逸追踪证明。`nativeIoLease.run` 路径验证丢回复不取消实际请求，且 native I/O 排空后进程 owner 仍保留源码和任务保护。没有 GUI、签名账号、现有项目或真实业务数据操作。

独立增量基线为精确 `c823524` 源码导出加已冻结 Node 16 文件补丁 `3b691d…`，没有复制桌面未提交修改。最终新增命名选集显式包含 opt-in，**4 项全部通过**（其中真实 Git 命令用例内部包含 Pending 与丢调用者两轮）；既有 Node 开发服务默认 **2 通过、1 忽略**。实际测试核对公开迁移停在 waiting-for-project-tasks、私有释放后迁移成功、同一 store 的 git-environment/project-git records，以及同一个真实服务 lease 所取得的 Git 初始化回执在确定 stopping 边界仍保留 committed，实际 status 读取回执不虚构提交。严格 core 全目标 Clippy 与格式检查通过，没有在此独立副本重复全核心、整个桌面工作区、原生 GUI 或最低系统版本验证。原始日志与一次早期 fixture 编译失败均留在独立 ledger，没有以失败或零选中项充当通过。
