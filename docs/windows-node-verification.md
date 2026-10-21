# Windows Node 实现与验收

更新：2026-10-21。用户已授权实现 Windows 私有 Node、配置文件保护、进程树管理、代理及原生单窗口接入并测试。工作在 `codex/windows-compatibility`，主线同步至 `770ec1d`；固定应用身份、默认数据根和已有项目不变。此记录逐项区分实现、自动化与真实桌面验收，不表示 W1–W4 或完整 Node 安装生命周期通过。

## 私有运行时资源

新增独立的 `scripts/node-runtime-release-windows.json` 与 `scripts/package-windows-node-runtime.mjs`，固定官方 Node 24.22.0、Windows x64。Mac 的元数据和准备脚本保持独立。开发机资源准备可下载固定官方 ZIP 或使用已核验缓存；用户端只运行包内绝对路径，不寻找系统 Node，不安装 npm 依赖。

- ZIP SHA-256：`f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3`。
- `node.exe`：93,882,184 字节，SHA-256 `ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c`。
- `LICENSE` SHA-256：`ed34dd8e3f0a78dbaf00d0444ce8e285b015b765379c2e17880455f70370f8e9`。

归档、可执行文件和许可分别校验，再验证 PE x64 与私有绝对路径执行结果。ZIP 拒绝路径逃逸、链接、重名、文件/目录冲突、本地与中央记录不一致和超限。输出须为新目录，未知旧输出或坏缓存不覆盖；失败现场保留。工具仅用于开发资源准备，不是公开应用包导入器。

Tauri 将生成目录 `resources/windows/node-runtime/` 的三个文件映射至可执行文件旁的 `resources/node-runtime/`。Node 不进入内嵌模板的 JSON。分发内部程序时，Node 功能需要一起携带这三个资源文件；静态网页样本仍从 exe 内嵌资源创建和打开。

实测：`node --test scripts/package-windows-node-runtime-test.mjs scripts/windows-resources-test.mjs` 共 19 项通过。官方归档准备、真实私有 `node.exe --version` 和 Tauri 输出位置另已核对，版本为 `v24.22.0`，输出 exe 摘要与上述固定值一致。生成运行时、官方归档、缓存和本地证据均不入 Git。
