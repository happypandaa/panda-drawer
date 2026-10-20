# Windows Node 兼容审查

更新：2026-10-20。范围是将 `origin/main` 的品牌、统一项目入口与应用 Node 后端代码同步到 `codex/windows-compatibility` 时的源码审查与集成检查。本轮不表示 Windows Node 已实现或 NR/W 阶段已通过。实际 Windows 基线仍见 [W2 记录](windows-w2-verification.md)、[共用管理界面记录](shared-manager-ui-verification.md)与[接续清单](windows-execution-checklist.md)。

用户已确认平台携带私有 Node、统一管理应用后端、可视化配置和一个应用一个原生窗口，首版兼容以 Radio 类为基线。Mac arm64 独立源码槽与 Radio 的实际记录见[本机配置与运行](node-native-configuration.md)。这些子集没有完成公开安装或 Node 槽备份、导出、复制、回收、更新与迁移，也不自动成为 Windows 验收。下列适配顺序和具体 Windows 实现选择均为建议，未确认参数不在本文冻结。

## 当前边界

| 用途 | 当前实现 | Windows 口径 |
| --- | --- | --- |
| 平台开发与资源准备 | 开发机的系统 Node/npm，Rust 与平台工具链 | 继续准备并内嵌匹配的初始源码、SDK 和预编译网页 |
| project-v1 用户源码构建 | 系统 Node/npm；现有最低 22.12.0、支持 22/24、推荐 24 LTS 的约定 | W2.4/W2.5 尚未开放，不因新增私有 Node 改为已支持 |
| 成品打开静态网页项目 | 预编译网页与受限本地服务 | 当前不调用 Node；保留 W2.1 新样本范围和全部后端门禁 |
| 应用自带 Node 后端 | 包内固定私有 Node；作者交付前端产物、可执行后端和运行依赖 | 当前未开放；不回退系统 PATH、不执行用户端 npm 安装或源码构建 |

Windows 完整 core/CLI 仍被隔离，桌面使用 `platform-host` 的受限样本服务。当前公开 SQL、附件、构建、维护及生命周期门禁继续有效。主线的统一卡片和配置组件可以共用，能力须由 Windows 真实后端提供；不能为显示新界面而初始化完整管理器、伪造空 Node 列表或发送未开放 IPC。

## 源码审查结果

| 位置 | 当前代码事实 | Windows 所缺能力 |
| --- | --- | --- |
| `scripts/node-runtime-release.json`、`scripts/package-node-runtime.mjs` | 固定 Node 24.22.0、`darwin-arm64` 官方 tar.gz、`bin/node` 和摘要；准备脚本还使用 Unix 路径与文件身份规则 | 独立固定的 `win-x64` 元数据、官方归档与 `node.exe` 校验、许可、Windows 安全资源准备及包布局；不能把 Mac 摘要用于 Windows |
| `node_runtime_trial.rs` | `prepare_launch` 明确只接受 macOS arm64；受管所有权、信号和进程组回收使用 Mac/Unix 实现 | Windows 真实所有权句柄、启动到收尾的进程树约束、退出等待、异常保留与重开门禁 |
| `node_app_configuration.rs`、`node_apps.rs` | 本地秘密文件依赖 Unix 用户、`0700`/`0600`、单链接及 `dev/ino`；`LocalFileVault::new` 在非 Unix 明确拒绝 | 当前用户私有 ACL、卷与完整对象身份、重解析点/硬链接拒绝、相对 HANDLE 读写与安全提交；POSIX mode 不能替代 ACL |
| `node_app_proxy.rs` | 静态目录通过逐级 `openat`、`O_NOFOLLOW` 和保留文件描述符访问；非 Unix 的 `open_root`/`open_file` 返回不支持 | 保留根与目录链句柄、核对卷/ID128 的安全静态资源访问；保留路径、会话、SSE、Range 和跨源边界 |
| `node_apps.rs`、`platform-host/src/windows_projects.rs` | Mac Node 槽位于固定 `data_root/node-apps/`；Windows `validate_root` 精确接受既有六个根条目，没有 `node-apps` | 新槽格式与固定根准入的显式演进；直接新增目录会被既有 W2.1 视为未知现场，不能绕过该保护或自动接管已有目录 |
| `apps/desktop/src-tauri/src/node_windows.rs`、桌面入口及前端控制器 | 此处 `node_windows` 指 Node 应用的原生窗口管理，文件名不表示 Windows OS 实现；主线运行路径依赖 Mac 的 `platform-core` | Windows 受限服务到原生窗口的专门接入；来源隔离、唯一窗口、重复打开聚焦、启动中关闭和平台 X 收尾逐项验证 |

现有 Windows `platform-host` 的私有 ACL、句柄相对打开和 ID128 原语可作为适配基础，但目前仅覆盖既有样本和数据库操作。不能将它们的通过记录推广为 Node 槽、秘密文件、归档或静态代理已安全。新增资源也不能成为更换开发标识、重置固定根或手工搬工作区的理由。

## 建议实施顺序

以下是依赖顺序，不新增或重编号 W/NR 阶段。每步只用清楚命名的可丢弃样本，通过相关检查后独立提交、推送；开放能力仍需对应 W/NR 门槛和实际记录。

| 顺序 | 建议本步范围 | 开放前需取得的证据 |
| --- | --- | --- |
| 1 | 保持当前 Windows 静态宿主可构建；按平台分离 Node 资源选择与打包路径，Node 登记/执行保持关闭 | Mac 资源校验不变；Windows 不误装/执行 Mac Node，不因缺少尚未支持的私有 Node 阻塞静态功能；未知平台明确拒绝 |
| 2 | 确定 Windows Node 槽的格式与固定根准入，再实施仅供新样本的安全目录、登记、普通配置和私有秘密文件 | 原有样本继续可读；未知或中断记录保留；私有 ACL、身份变化、链接、占用与提交失败不泄漏秘密、不丢旧修订；不自动修复已有目录权限 |
| 3 | 准备固定 `win-x64` 官方 Node 资源，并单独实现受管进程所有权和回收 | 无系统 Node 的受控环境启动人工协议样本；版本/摘要错误明确拒绝；正常退出、启动失败和三代子进程收尾不影响独立 Node |
| 4 | 实施 Windows 静态目录代理和会话，再接唯一原生窗口及可视化配置 | HANDLE 身份与重解析点保护、跨槽隔离、SSE/音频 Range、会话撤销、保存不启动、明确重启、启动中关窗及迟到结果不复开通过 |
| 5 | 在 Win11 x64 使用可丢弃 Radio 类样本实机验收，再按目标补 Win10 x64 | 真正的包内 Node、WebView2 单窗口、平台 X/应用关窗收尾；需要真实账号或付费服务的动作另取得对应授权，人工协议测试不算真实 AI/TTS/音乐兼容 |
| 6 | 按原 NR/AP/W 保护链路补完整安装、数据生命周期与交付 | 不将独立源码登记当可分发安装；Node 槽的备份、恢复、复制、导出、回收、更新及迁移各有明确契约与失败证据；两系统与最终安装包分别验收 |

进程树管理建议评估 Windows Job Objects，保持当前的语义：只管理本应用拥有的进程，清理无法确认时保留所有权和现场、阻止不安全重开并取消普通退出。具体创建顺序、退出协议、Job 配置及异常宿主退出保证需在实现前落实并用实机证明；不以旧数字 PID 或 `taskkill /IM node.exe` 猜测归属，不将 Job Objects 等同于安全沙箱。

Windows ACL 方案应兑现用户已选择的“本机私有明文文件”边界，不擅自替换为其他凭据库，也不宣称加密。现有 Mac 目录/文件权限只是该平台的实现方式；Windows 需要独立的 ACL 与句柄验证。分享包、源码、Git、日志和诊断不能携带秘密值或可用绑定。

本轮合入后仍须分别记录自动化检查、原生编译、真实窗口、私有 Node 及外部服务结果。Mac 已有 Radio 播放进度不证明 Windows 解码、物理扬声器、浏览器、相机或麦克风；物理断网、硬崩溃、跨登录会话与断电也不由模拟测试代替。当前 W2.2 的安全初始化、串行执行器和 SQL API 可按原清单继续，不依赖先开放 Node。

## 本轮集成与检查

从 Windows 基线 `104c1bb` 合入主线 `88f9bf3`（18 个新提交），保留已发布历史，不 rebase、不改 `main`。固定 `dev.localprojects.platform.m0` 身份、系统 API 默认根、现有样本和工作区指向均不调整，也未运行真实项目或读取应用凭据。

- 保留 Windows 的资源快照、内嵌模板、Node 调用 Tauri JS 入口与 `--no-bundle`；同时接入 Mac 生成包校验和失败保留流程。Windows 的 `-b`/`--bundles` 仍明确拒绝安装器路径。
- 两平台继续复用 `ManagerShell`、工具栏、项目集合、创建表单、卡片、主题与样式。Windows 的 Node 添加、导入、空间和删除继续显示未开放原因；设置只读取现有受限状态，不挂载完整业务面板。
- 主线完整 Mac 宿主移入既有 `macos_host.rs`，保留 Node 初始化、单窗与退出逻辑；Windows 根入口、条件依赖及受限服务不接入完整 core。

当前 Win11 x64 开发机（系统 Node v24.14.0）的实际结果：

| 检查 | 结果与证明范围 |
| --- | --- |
| 桌面 TypeScript/Vite 生产构建 | 通过，两入口共享组件与样式；未运行真实 WebView2 |
| `npm run build -- --debug -- --locked` | 通过，重新准备匹配的初始源码/网页、SDK 与 skill，并生成 `target/debug/local-projects-desktop-dev.exe`；自动添加 `--no-bundle`，未生成安装器、未启动程序。首次沙箱内模板 npm 异常退出，获准在沙箱外重跑完整链路后通过 |
| `npm run test:ui`、`npm run test:windows:ui` | 76 + 19 项通过，合成 DOM/IPC；保留五命令白名单、未知结果不重放、只读详情、主题持久化与后置能力拒绝 |
| Windows 原生 Rust 测试 | host 96 项通过、另 1 项默认忽略；preflight 96、project-format 14、desktop 3 项通过；数据库连接仍是私有原语，不开放 SQL |
| desktop `check --all-targets`、上述四包 `clippy --all-targets -- -D warnings` | 通过；增量缓存无法硬链接时退回复制的工具链提示不影响结果 |
| 工程入口、进程环境及 Windows 资源测试 | 6 + 5 + 8 项通过；资源的链接拒绝夹具首次因沙箱禁止创建链接未完成，获准在沙箱外重跑全部 8 项通过 |
| Node 准备脚本的参数、固定元数据、不支持平台拒绝、tar 与架构检查 | 选定 5 项通过；不执行 Mac Node，不下载运行时 |
| Mac 生成包计划与身份/输出参数拒绝 | 选定 2 项通过；不执行生成包文件操作、签名或 DMG 工具 |
| 共享完整运行时门禁模块 | 5 项通过，Windows 完整 core 路径继续拒绝 |

本轮修改的七个 Rust 集成文件格式检查通过，差异空白检查通过。额外的全工作区 `cargo fmt --all -- --check` 未通过：`builds.rs`、`service.rs` 和 `platform-preflight/src/trash_tests.rs` 有合入前已存在的排版差异，本轮不扩展为全库格式整理。

Mac 宿主与主线完整入口仅作源码差异和模块作用域复核；本机无法完成 Mac 编译、原生 Node、文件权限、签名或 Radio 回归。Node 准备与 Mac 包事务的其余文件系统夹具未在 Windows 执行，不将选定检查写成完整套件通过。本轮未验真实窗口、Win10、无系统 Node 干净机器、物理断网、设备或安装器。

2026-10-21 用户授权完成上述 Windows Node 适配并测试。实施前再次同步主线至 `770ec1d`，保留最新启动器、窗口尺寸契约和 Mac 管理窗口关闭语义；Windows 受限宿主仍保持原边界。此次同步的 UI 80 项、Windows UI 19 项、TypeScript/Vite 及 Windows host/desktop all-targets 编译检查通过，未据此开放 Node。后续按资源、文件与配置、受管进程、代理和窗口分别记录实际实现与检查。
