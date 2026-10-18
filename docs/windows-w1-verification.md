# Windows W1 环境与工程验证记录

日期：2026-10-19（Asia/Shanghai）。当前仅推进 W1.1 的本机环境准备和开发辅助脚本；W1.2–W1.4 未开始，平台 Windows 编译、启动和安装未验收，W1.1 也未整体通过。

## 1 本轮授权与范围

用户已确认按推荐开始并继续：初期使用本机内置 NTFS、由系统 API 解析的固定默认工作区，允许补齐 Rust MSVC、C++ Build Tools 和 Windows SDK。开发应用身份继续为 `dev.localprojects.platform.m0`。外置盘、网络/映射盘、同步目录及后续迁移目标不在本轮起步范围内。

本轮不初始化平台数据根、不启动桌面服务、不连接业务数据库，也不调用可能生成工作区的 CLI。工具、依赖和独立探针只放在工程 `.tools/`、`node_modules/` 和忽略的 `.test-artifacts/` 中。MSVC/SDK 属于经授权的系统开发工具安装；不是运行成品项目的用户依赖。

## 2 本机盘点

| 项目 | 实际观察 | 结论边界 |
| --- | --- | --- |
| 系统 | x64，DisplayVersion `25H2`，build `26200.9711`，Professional，普通用户进程 | 当前机器的系统事实，不代表 Windows 10 或全部 Windows 11 通过 |
| 卷 | 仓库所在 D 盘和系统 LocalAppData 所在 C 盘均报告 `NTFS`、`Fixed` | 只读卷信息；未验收重解析点、物理连接类型、ACL、长路径或持久操作安全 |
| 数据目录 | 主环境的系统 LocalAppData 与进程变量一致；探针另报告注册表位置与 Known Folder 的差异 | 不把子代理沙箱重定向目录当正式根，也不由探针创建或选择平台根 |
| Node/npm | Node `24.14.0`、npm `11.9.0` | 本轮开发版本，未借此改变 Node 22/24 支持约定 |
| WebView2 | 精确 Runtime 注册项版本 `155.0.4283.62` | 不是只检查 Edge 浏览器；未实际创建 WebView2，也未确定最低 Runtime 版本 |
| 仓库私有 Rust | rustup `1.29.1`、rustc/cargo `1.99.1`，host/target `x86_64-pc-windows-msvc`；rustfmt、clippy 组件已安装 | 安装在 `.tools/cargo`、`.tools/rustup`，未修改全局 PATH；不是已验证的最低 Rust 版本 |
| MSVC/Windows SDK | 已发起 Build Tools 2022 `17.14.42` 安装，选择 C++ 工具及 Windows 11 SDK 26100；Windows 权限确认仍在等待 | 尚无可用 MSVC/SDK 通过证据；不将下载和启动安装器记为安装成功 |

Rust 官方 Windows 安装器下载后按同源 SHA-256 校验通过；组件安装结果通过本地 `rustup show`、`component list --installed` 和编译器版本查询复核。Build Tools 安装器由 WinGet 校验哈希后启动，目前仍停在 Windows 系统许可交互。没有通过其他工具代替用户处理系统权限确认。

## 3 开发辅助脚本

- `scripts/windows-environment.mjs`：通过静态 PowerShell、注册表、.NET 和工具版本查询输出 JSON，区分 `available`、`missing`、`inaccessible`、`probeError`。不使用 CIM/WMI、不启动平台、不安装工具、不写平台状态。`available` 只表示本项探测得到结果，不表示编译或功能验收通过。
- `scripts/process.mjs`：识别 Windows 仓库私有 `cargo.exe`，正确保留 Windows PATH/Path，并仅向子进程提供 Cargo/Rustup 环境；保留原 Mac 选择规则。
- `npm run env:windows` 输出环境报告；需要纯 JSON 时直接运行 `node scripts/windows-environment.mjs --json`。`npm run test:process` 检查环境选择和不修改调用方环境的行为。

环境报告包含实际路径和机器版本，属于本机验证材料，不提交报告原件、完整安装日志、安装器或可执行探针。记录保存在 `.test-artifacts/w1-environment-2026-10-19/`。

## 4 实际检查

| 检查 | 实际结果 |
| --- | --- |
| 工程依赖 | `npm ci --no-audit --no-fund` 成功，安装 67 个包；未更新 npm 全局版本或修改依赖锁文件 |
| 管理界面前端 | `npm run build --workspace @local-projects/desktop` 的 TypeScript/Vite 构建通过；仅生成工程前端产物，没有构建或运行 Tauri 平台 |
| 固定浏览器 SDK | `npm run test:sdk`，12 项通过；不代表 Windows 原生数据库或桌面运行通过 |
| 子进程 Rust 环境 | `npm run test:process`，5 项通过，覆盖 Windows `.exe`、PATH 大小写、系统工具回退和模拟 Mac 规则；`node scripts/rust.mjs --version` 实际取得仓库私有 Cargo `1.99.1`。模拟 Mac 规则不是 Mac 实机回归 |
| 只读环境报告 | 脚本语法检查、真实 Windows 执行和 JSON 解析通过；报告正确区分已安装 Rust/WebView2 与尚缺 MSVC/SDK，未把盘点成功写成工具链或平台通过 |
| 独立 Rust 链接探针 | 在可丢弃目录调用本机 Win32 API 的小程序编译失败：`linker link.exe not found`；错误日志保留。这准确说明 Rust 已安装而本机链接工具尚未就绪 |

完整平台的无条件 Unix API、Mac 打包入口、Windows 文件安全与宿主适配仍待后续小步完成。本轮不以绕过这些检查、关闭安全测试或运行平台初始化来代替工具链验证。

## 5 待完成与下一步

1. 用户处理 Windows 的 Visual Studio Installer 系统许可后，核对实际安装的 MSVC、Windows SDK、组件版本和安装结果；不能仅按计划版本写成成功。
2. 在独立可丢弃目录编译 C 文件（含 Windows SDK 头文件和 API）、生成静态库，由 Rust 链接并运行，检查 x64 架构和确定输出。它只能证明开发工具链，不证明 bundled SQLite、Tauri 或平台功能。
3. Windows 10 22H2 x64 测试环境尚未落实，本轮 Mac 回归未执行。W1.1 的双系统环境安排未满足，故保留进行中；不能把当前机器检查推广为两个系统或 Mac 通过。
4. W1.2 的能力/启动门禁随后单独实施，W1.3 再实现实际安全文件行为。安装形式、WebView2 分发/最低版本和正式发行条件继续待定。

成品用户运行带有效网页产物的项目无需 Node、Rust 或 C++ 工具；修改业务源码后才需要系统 Node/npm 来构建。内置 Node、外部网页应用文件夹安装和资源限额讨论稿没有在本轮获得新确认。
