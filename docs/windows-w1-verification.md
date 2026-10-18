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
| MSVC/Windows SDK | Build Tools 2022 `17.14.42`（build `17.14.37806.7`）、VC Tools `14.44.35207`、SDK 目录版本 `10.0.26100.0` | 重试安装退出码 `0`，vswhere 为 complete/launchable 且不要求重启；独立原生小样通过，不代表平台或最低版本通过 |

Rust 官方 Windows 安装器下载后按同源 SHA-256 校验通过；组件安装结果通过本地 `rustup show`、`component list --installed` 和编译器版本查询复核。首次 Build Tools 安装器由 WinGet 校验哈希后启动，用户随后允许了 Windows 系统许可；安装仍以 `5008` 失败，日志显示 `Unexpected Installer Version`，没有登记为成功。原 bootstrapper 日志保留在忽略的验证目录中。

后续从微软官方 `https://aka.ms/vs/17/release/vs_buildtools.exe` 获取当前引导程序，实际文件版本 `17.14.37806.7`，SHA-256 `2a6aa7d94e38d457a4eb3930fc60056ecab06865b2eb925e5275b8473a017583`，Microsoft Corporation 签名为 Valid。使用同一 C++ 工具与 SDK 26100 组件范围重试，没有删除安装缓存、降低签名验证或操作平台数据；新引导程序产生的系统许可仍由用户处理。此文件版本是引导程序版本，不冒充已安装的 MSVC 或 SDK 版本。

首次直接启动新引导程序后用户反馈没有看到提示，复核发现该进程已经退出，不能继续声称权限窗口仍可操作。随后通过 Windows `RunAs` 以可见交互方式启动已验证的引导程序并等待完成；安装器自更新为 `4.11.24.302448642`，组件安装退出码 `0`。vswhere 的 `isComplete`、`isLaunchable` 均为 true，`isRebootRequired` 为 false；没有自动重启。原 `5008` 和安装前链接失败继续作为历史失败证据保留。

## 3 开发辅助脚本

- `scripts/windows-environment.mjs`：通过静态 PowerShell、注册表、.NET 和工具版本查询输出 JSON，区分 `available`、`missing`、`inaccessible`、`probeError`。不使用 CIM/WMI、不启动平台、不安装工具、不写平台状态。`available` 只表示本项探测得到结果，不表示编译或功能验收通过。
- `scripts/process.mjs`：识别 Windows 仓库私有 `cargo.exe`，正确保留 Windows PATH/Path，并仅向子进程提供 Cargo/Rustup 环境；保留原 Mac 选择规则。
- `npm run env:windows` 输出环境报告；需要纯 JSON 时直接运行 `node scripts/windows-environment.mjs --json`。`npm run test:process` 检查环境选择和不修改调用方环境的行为。
- `npm run check:windows:toolchain` 是单独的原生编译探针：选择完整 MSVC x64 实例和项目 Rust，只在新的 `.test-artifacts/` 子目录编译 C/SDK、创建静态库、由 Rust 链接运行并核对 x64 PE。与只读环境报告不同，这个命令会写入专用验证产物；不会启动平台、构建用户应用或更改全局环境。直接执行 `node scripts/windows-toolchain-smoke.mjs --json` 可取得纯 JSON。
- VsDevCmd 的完整环境仅在内存中传递，不写入命令日志或报告。调用前移除 `VSCMD_DEBUG`/`VSCMD_TEST` 调试开关，编译前移除 `CL`、`_CL_`、`LINK`、`_LINK_` 追加参数，避免环境快照落盘或输出路径被覆盖。原生编译的 TEMP/TMP 也指向本次唯一证据目录。

环境报告包含实际路径和机器版本，属于本机验证材料，不提交报告原件、完整安装日志、安装器或可执行探针。记录保存在 `.test-artifacts/w1-environment-2026-10-19/`。

## 4 实际检查

| 检查 | 实际结果 |
| --- | --- |
| 工程依赖 | `npm ci --no-audit --no-fund` 成功，安装 67 个包；未更新 npm 全局版本或修改依赖锁文件 |
| 管理界面前端 | `npm run build --workspace @local-projects/desktop` 的 TypeScript/Vite 构建通过；仅生成工程前端产物，没有构建或运行 Tauri 平台 |
| 固定浏览器 SDK | `npm run test:sdk`，12 项通过；不代表 Windows 原生数据库或桌面运行通过 |
| 子进程 Rust 环境 | `npm run test:process`，5 项通过，覆盖 Windows `.exe`、PATH 大小写、系统工具回退和模拟 Mac 规则；`node scripts/rust.mjs --version` 实际取得仓库私有 Cargo `1.99.1`。模拟 Mac 规则不是 Mac 实机回归 |
| 只读环境报告 | 脚本语法检查、真实 Windows 执行和 JSON 解析通过；安装前 MSVC/SDK 为 missing，安装后能读出完整实例与 SDK 文件；两份报告均保留，盘点脚本的 compilationVerified 仍为 false |
| 安装前 Rust 链接探针 | 在可丢弃目录调用本机 Win32 API 的小程序曾报 `linker link.exe not found`；错误日志保留，未覆盖为成功记录 |
| 安装后 C/Rust/SDK 混合探针 | vswhere、VsDevCmd、rustc 版本、cl、lib、rustc 链接及运行 7 步均退出 `0`；C 包含 `windows.h` 并调用 `GetCurrentProcessId`，Rust 实际读取静态库函数返回 `42`；固定输出及 AMD64 `0x8664` / PE32+ `0x020b` 检查通过 |
| 原生环境干扰回归 | 在不含真实凭据的最小环境中注入 `VSCMD_DEBUG=1`、`VSCMD_TEST=1` 及四项编译/链接器附加参数；真实编译运行仍通过，指定的 5 个旁路输出和 VsDevCmd 环境快照日志均未生成 |

基线与干扰回归分别保存在唯一的 `.test-artifacts/windows-w1-toolchain-smoke-*` 目录；干扰回执另在 `.test-artifacts/windows-w1-toolchain-flags-*`。原生小样确实编译了 C 和 Rust，但没有编译实际 `sqlite3.c` 或整个 Cargo 依赖树，因此不记为 bundled SQLite、Tauri 或平台编译通过。

完整平台的无条件 Unix API、Mac 打包入口、Windows 文件安全与宿主适配仍待后续小步完成。本轮不以绕过这些检查、关闭安全测试或运行平台初始化来代替工具链验证。

## 5 待完成与下一步

1. 本机工具安装、独立编译链接与环境干扰回归已完成；不再把系统许可列为当前阻塞。后续修改工具链或脚本时按影响重新验证，不能沿用旧版本证据。
2. Windows 10 22H2 x64 测试环境尚未落实，本轮 Mac 回归未执行。W1.1 的双系统环境安排未满足，故保留进行中；不能把当前机器检查推广为两个系统或 Mac 通过。
3. W1.2 的能力/启动门禁随后单独实施，W1.3 再实现实际安全文件行为。安装形式、WebView2 分发/最低版本和正式发行条件继续待定。

成品用户运行带有效网页产物的项目无需 Node、Rust 或 C++ 工具；修改业务源码后才需要系统 Node/npm 来构建。内置 Node、外部网页应用文件夹安装和资源限额讨论稿没有在本轮获得新确认。
