# Windows x64 本机验证

## 2026-11-21：正式 0.1.0 打包入口

本轮按用户要求在 `D:\dev\postgresUI` 的 `develop` 直接实现。开始时同步到 `33b9ff8546f2546cb7a901ccd65c4599c3481d65`，确认 GitHub/Gitee 两端同一提交。正式 Windows profile 固定 `com.happypandaa.pandadrawer` / `0.1.0` / `x86_64-pc-windows-msvc`；新增 NSIS 候选入口、完整 Node/Git 文件树资源锚和桌面程序编译绑定的 CLI 字节锚。Mac 原 profile、签名和运行准入保留。

实际验证主机：Windows 11 Pro 26H2，`10.0.26300.9974` x64，普通用户。使用固定 pnpm 12.16.1；Node 测试串行，Rust 构建作业为 2，开发/测试 debug 与 incremental 输出关闭，未重叠运行构建测试套件。

- `pnpm install --frozen-lockfile`、`pnpm env:windows`、`pnpm check:windows:toolchain` 依次成功；工具链实际编译、链接并运行 C/Rust/SDK 探针。
- 新 Windows 资源协议 5 项、profile 5 项、CLI 摘要与持句柄防替换 1 项针对性 Rust 测试通过。
- Windows 候选和资源脚本及共享资源准备回归：11 项通过、1 项 Mac 专属跳过。
- `node scripts/ci.mjs` 完整通过，最终 `failures: []`；包含 TypeScript/Vite、377 项界面测试、16 项 SDK 测试、脚本测试、完整 Rust 工作区检查与测试，以及真实更新父进程交接测试。
- 候选 `check` 通过：固定正式标识、版本、公钥和 GitHub/国内 OSS 地址均匹配。正式更新公钥 packet SHA-256 为 `17b2747e00cd70aec60e5e36d0b6e3e295e0ace15920516304a88c60fec5947d`。

此提交阶段尚未完成正式 NSIS 构建、安装 GUI、数据保留、三种 WebView2 情况、隔离升级和 SmartScreen 验收。已准备 Windows 11 Enterprise `10.0.26100.9974` 的一次性 Sandbox；标准账户实际为 Medium token、非管理员，未安装系统 Node/Git，但客户端显示黑屏尚待解决。不能将该准备或上述 CI 记作完整安装验收通过。

本机尚未提供匹配的正式 Tauri 更新私钥/解锁输入；代码签名证书盘点也为空。候选入口会保留 EXE 并明确记录更新签名缺项，不生成替代密钥或伪造 `.sig`。实际产物、系统操作结果、Authenticode 与 SmartScreen 表现继续记录在本机交付报告；私钥、密码、生成应用及原始测试证据不提交。此步骤不创建正式标签、Release 或 OSS 清单。
