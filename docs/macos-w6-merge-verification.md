# W6 合并后的 Mac 本机构建记录

日期：2026-11-01（Asia/Shanghai）。

用户本轮明确要求将 `win/w6-installer` 合入 `main`，在这台 Mac 打包后交由用户手动测试。本地 `main` 从 `ac3b5d7` 快进到 W6 的 `67f9f7e`，包含全部 W0–W6 提交。本次按该授权合并，不将其写成 Windows W6 验收完成或双平台 CI 通过；Windows 原有未完成项仍见 `windows-x64-verification.md`。

## 环境和必要调整

- 本机 macOS 27.0.1（26A434）、Apple Silicon；构建 Node 24.22.0、pnpm 12.16.1、Rust 1.99.1。Rust 和构建 Node 放在仓库忽略的 `.tools/`，未改全局 Node 或 shell 配置。
- 用户确认以后本机所有 Node 依赖安装统一使用 pnpm，已写入本机全局和项目 `AGENTS.md`。新增 pnpm workspace 与锁文件，包含桌面依赖及根目录 UI 夹具所需依赖可见性；原 npm 锁文件保留。新旧锁中的 102 个包、版本和 integrity 一致。
- `pnpm run build:app` 的模板依赖也使用 pnpm：从模板原 npm 锁转换临时锁后冻结安装，移除本次临时生成的 pnpm 文件，保留构建前后严格源码摘要核对。原 npm 工作流及应用内受管 Node/npm 契约继续保留。
- 本机 Xcode 的 clang-2100.4.1.1／SDK 27.2／ld-27106.1 与此前固定 Git 资源的构建环境不同。按原配方从官方 Git 2.56.0 源码在两个独立目录重建，完整有效载荷逐字节一致后更新固定记录；原始源码、签名锚点及校验机制保持。详见 `development.md` 第 7.1 节，最低目标仍为 macOS 15 arm64。
- 修复 Windows 重构遗留的两处 Mac 测试问题：Git 设置大小上限移入子模块后未供原单测引用；durable writer 子进程夹具仍使用搬移前的测试名称。中断测试首次完整运行稳定失败，确认旧名称匹配零个测试后修正；单项真实中断回归和随后完整重跑均通过。

## 实际检查

| 检查 | 结果 |
| --- | --- |
| `pnpm install --frozen-lockfile` | 根和桌面工作区安装通过 |
| 桌面 TypeScript / Vite 构建 | 通过 |
| 全部 UI 回归 | 498 通过，0 失败 |
| SDK 回归 | 12 通过，0 失败 |
| CI 同集合脚本单测（Git 单独执行） | 166 通过，0 失败，10 项按条件跳过 |
| Git 打包与真实工具回归 | 16 通过，0 失败，4 项 Windows 原生测试在 Mac 跳过；包括源码重建、重定位、提交、分支、克隆及篡改拒绝 |
| 项目目录名和 AI 提示词生成检查 | 通过；目录名含 40,258 个固定映射及 4,120 个 golden cases |
| 平台策略测试 | 6 通过，0 失败 |
| pnpm 真实模板打包及命令入口检查 | 通过；原生／JS 入口与 Windows shell 拒绝检查通过，源码摘要保持 |
| `node scripts/rust.mjs check --locked --workspace --all-targets` | 通过 |
| `node scripts/rust.mjs test --locked --workspace` | 完整重跑退出 0；core 为 945 通过、0 失败、39 ignored，其余 workspace 与文档测试通过 |
| `pnpm run build:app` | 退出 0，生成并自动核验 `.app` |
| `git diff --check` | 通过 |

39 个 ignored 是未启用的专用夹具／可选真实工具检查，不作为已通过的场景。编译仍报告已有 unused import / dead code 警告，Vite 报告单块体积提示；这些不是本次失败或已完成的清理工作。

## 交付

产物：`target/release/bundle/macos/熊猫小抽屉.app`，约 334.59 MiB。

- 名称／版本：熊猫小抽屉 `0.0.0`，固定开发身份 `dev.happypandaa.pandadrawer`。
- 桌面和 CLI 二进制均为 Mach-O arm64，`LSMinimumSystemVersion` 为 `15.0`。
- 构建流程的严格 deep codesign、应用身份、内置开发 Node/npm 和 Git 完整树核验通过；打包事务锁已正常移除。
- 使用 ad-hoc 签名，属于内部开发包；没有执行 Developer ID 签名、公证或发布。
- 本次未启动该 `.app`、未操作已有业务项目；用户从本次路径自行执行原生界面测试。UI 自动测试使用 jsdom，不证明原生窗口、物理媒体设备或系统权限行为。完整 Windows 安装验收及双平台 CI 仍按原记录待完成。

若已有开发版正在运行，先通过菜单或 Cmd+Q 正常退出，再打开本次路径，避免单实例复用旧程序。构建产物、工具链和本地测试日志不进入 Git。
