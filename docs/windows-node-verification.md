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

## 文件、配置及应用边界

共用 Node 清单、配置、代理、运行协议和窗口参数移至独立 `platform-node` crate。`platform-core` 保留原公开模块的转导出，Mac 调用路径不变。Windows 桌面依赖 `platform-node` 与受限 `platform-host`，没有初始化完整 core、恢复、维护或生命周期管理器。

Node 状态从已由静态宿主持有并完成准入的根句柄创建，不再打开第二份固定根，不提供环境变量换根入口。尚未初始化的工作区需先通过现有“创建网页项目”流程创建明确命名的可丢弃样本；列表读取不会初始化 Node 根。显式登记才创建 `node-apps`，类型化根标记和槽证明绑定卷及完整 ID128。未知、中断或被替换的目录保留并拒绝接管。

源码读取使用完整目录链及叶文件 HANDLE；私有配置、凭据另外验证当前用户 ACL。重解析点、硬链接、写入占用与身份变化拒绝，不通过修改已有权限修复。Windows 配置使用连续不可变修订和不覆盖发布，API 保留修订冲突、秘密状态、明确解绑与重绑语义；秘密值不进入普通配置、前端返回或运行日志。仍是用户选择的本机私有明文文件，不能称为加密保险库。

发布与按身份删除拒绝文件写入占用。配置修订一旦进入提交尝试，失败时保留本次新凭据；若重命名已提交而后置校验失败，返回 `NODE_CONFIGURATION_COMMIT_UNKNOWN` 和 `operationOutcome: unknown`，界面要求重新读取状态，不重放保存。真实重命名后的故障注入已核对新旧凭据、已提交修订与启动快照均保留。

## 进程及桌面接入

Windows 使用 `CreateProcessW` 挂起创建，在加入私有 Job 后才恢复执行。只继承受控的标准输入/输出/错误句柄，环境显式组装；绝对私有 Node 路径和摘要在启动前检查。Job 不允许脱离，启用关闭最后一个 Job 句柄时终止全部成员；退出观察、强制收尾均依据持有的进程/Job HANDLE，不按进程名或旧 PID 杀进程。此机制用于生命周期管理，不是应用代码安全沙箱。

正常停止走协议并核对整棵树退出；无法确认时保留所有权，阻止重复启动并取消平台退出。强制停止或宿主异常后的运行标记保留，尚未实现自动恢复或根据旧 PID 重新绑定。主进程先退的测试会使用主动分离的后代，避免把 [libuv 默认随父进程退出的行为](https://github.com/libuv/libuv/blob/v1.x/src/win/process.c)误认为平台完成了收尾。

桌面重用现有 Node 原生窗口管理：打开启动、重复打开聚焦、关闭回收、明确重启更换会话。Windows 窗口另使用独立私有 WebView2 缓存，限制当前代理源、拒绝新窗口、下载和未声明设备权限；管理命令继续检查窗口标签及精确管理页 URL。平台 X 按 Windows 原约定收尾所有 Node 后再退出。窗口尺寸声明与本次宿主会话内的尺寸记忆可用；Windows 暂不写跨平台的窗口尺寸缓存文件。

目录选择框等待用户输入时不占用平台退出锁，返回后重新核验管理窗口及退出状态。退出收尾失败时保留宿主和运行证据，Node 进入停止／重试退出状态，不在同一宿主会话恢复新的启动；正常退出并重开后仍需通过残留运行标记检查。

## 自动化检查

以下使用 Win11 x64 本机执行，所有文件和运行样本均为新建、明确命名的可丢弃目录，未登记或执行现有业务项目，秘密值均为人工测试字符串。

| 检查 | 结果与范围 |
| --- | --- |
| Windows 资源及私有 Node 准备脚本 | 19 项通过；另实际核对官方归档、私有 exe 版本及输出摘要 |
| 共用 UI / Windows UI | 80 + 29 项通过；合成 DOM/IPC，包含配置、单次提交、迟到结果、未知结果不重放和显式重启 |
| `platform-host` | 105 项通过、1 项子进程辅助入口默认忽略；含新增 9 项 ACL、链接、写入占用、身份与静态根共存测试 |
| `platform-node` 默认测试 | 52 项通过、15 项真实运行测试默认忽略；含 Windows 私有配置重开、提交后校验失败保护、代理会话/SSE/Range、目录别名去重与缺失凭据不补建 |
| 真实私有 Node 运行测试 | 单独启用的 15 项全部通过；实际官方 exe、协议停止、超时强停、取消、三代后代、主进程先退、宿主无析构终止及独立 Node 不受影响；另串联登记/配置/保存不自动重启/明确重启/旧会话失效/双应用隔离/SSE/Range/停止/持久数据重开。音频为人工字节夹具，不证明解码或外部 TTS 服务 |
| desktop Rust 测试 | 9 项通过；精确管理页来源、帮助地址、导航及窗口代次状态测试，不代表原生窗口验收 |
| 三包 `clippy --all-targets -- -D warnings` | 通过 |
| `npm run build -- --debug -- --locked` | 通过；完整模板/SDK/Node 资源准备、TypeScript/Vite 和 Rust 编译，自动 `--no-bundle`；最终输出私有 Node 版本与 SHA-256 再核对通过 |

本轮修改文件格式化及 `git diff --check` 通过。Mac 抽取作过源码、依赖与条件编译复核，原模块公开路径和测试保留；Node 内联测试现由 `-p platform-node` 执行。没有 Mac 设备，未将只读复核写成 Mac 编译或原生回归通过。

复现默认检查：

```powershell
npm run test:windows:node-runtime
npm run test:ui
npm run test:windows:ui
node scripts/rust.mjs test --locked -p platform-host -p platform-node -p platform-desktop
node scripts/rust.mjs clippy --locked -p platform-host -p platform-node -p platform-desktop --all-targets -- -D warnings
npm run build -- --debug -- --locked
```

真实 Node 测试需要已核验的官方 Windows x64 Node 24.22.0。可在当前 PowerShell 会话设置 `NODE_RUNTIME_TRIAL_TEST_NODE` 为包内 `node.exe` 的绝对路径，再执行 `node scripts/rust.mjs test --locked -p platform-node --lib -- --ignored --test-threads=1`。测试再次核对固定摘要，不使用系统 PATH 中的 Node；这仍不等于已经在无系统 Node 的干净机器验收。

## 本机原生窗口阻塞

本机为 Windows 11 专业版 x64，构建号 26200。完整 `npm run build -- --debug -- --locked` 已生成可执行文件与私有 Node 资源。通过 Computer Use 真正启动该 exe 后，系统原生诊断返回 `SYSTEM_PARENT_ACL_REJECTED`：系统 API 返回的 `C:\Users\g\AppData\Local` 上，额外能力身份被授予 `0x001F01FF`。宿主在业务根准入前拒绝，未调整 ACL、固定根、工作区或已有项目。

因此本轮不能把原生单窗口、重复聚焦、原生关窗、平台 X、WebView2 播放及 GUI 重启记为通过。启动诊断已正常关闭。界面 IPC/状态测试和 Rust 生命周期测试将单独列出；它们不替代这组实际桌面验收。Win10 22H2 x64、Mac 原生回归、无开发工具干净机器、浏览器、真实 Radio 外部服务及物理媒体设备也需分别补验。未生成安装器，未合并 `main`。
