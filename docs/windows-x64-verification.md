# Windows x64 移植验证记录

执行标准：[windows-x64-port.md](windows-x64-port.md)。开始日期：2026-10-31（Asia/Shanghai）。未完成、排队和未执行均不算通过。

## 起点与环境

- 原始 main：`ac3b5d72ab79e132fcedd149c73e1201a2896576`，开始前已 fetch 并快进同步。旧 Windows 分支不合并、不沿用旧受限架构。
- 已通读执行标准、AGENTS、完整删除和 Node 单目录运行契约。
- 当前主线没有 `.github/workflows`，没有可引用的原始 macOS 绿色检查；不能声称已确认 Mac 基线通过。先提交只涉及 CI/文档的步骤，再获取真实 macOS 检查结果；原有失败必须按测试名称记录，不能通过删除或跳过测试消除。
- 本机 Windows x64，Node `24.14.0`、Rust MSVC `1.99.1`、VS Build Tools 2022 `17.14.37806.7` / MSVC `14.44.35207`、Windows SDK `10.0.26100.0`、WebView2 `156.0.4314.50` 已存在。先前 `env:windows` 和独立 C/Rust x64 编译、链接、运行探针通过；不是本轮产品兼容性或安装验收证据。本轮暂未安装新组件。
- 不重置开发数据，不执行真实项目删除；CI 只使用独立 checkout 和临时测试目录。

## W0 — CI 先行（进行中）

首个步骤仅增加自托管双平台 workflow、隔离测试 harness、运行器安装说明和本记录，不改产品代码。任务标签固定为 `[self-hosted, macOS, ARM64]` 与 `[self-hosted, Windows, X64]`，不使用云端运行器。新增手动检查原始 SHA 的入口；不能调度时保留首个 CI 提交作为无产品重构的基线。

GitHub 连接器初次请求失败；本机没有 `gh`，Git 配置的 HTTPS credential helper 没有可用的非交互凭据（SSH fetch/push 可用）。尚未取得运行器注册 token，也尚未安装或注册 Windows 服务；不得记为已完成。Mac 安装步骤见 [ci-self-hosted-runners.md](ci-self-hosted-runners.md)，不等待用户安装才继续独立的 W0 工作。

连接器随后恢复，确认实际仓库为私有的 `happypandaa/panda-drawer`，原始 SHA 没有 commit status；运行器管理/注册不在连接器支持的接口中。浏览器也未登录 GitHub，未取得后台服务注册身份。

CI 步骤提交前检查：`node --check scripts/ci.mjs`、差异空白检查、现有 TypeScript/Vite 构建通过。原始 Windows 工作区 UI 测试 **479/480**：`The bundled platform requirements exactly match the maintained copyable document` 在 CRLF 检出文档上解析 LF 代码块失败；未改测试或产品逻辑。新增 `.gitattributes` 固定文本为 LF，并将对应三个参考文件按 Git 原始 blob 字节重写后，生成内容检查及完整 UI **480/480 通过**。这是 Windows 检出问题，不是已证实的 macOS 失败；Mac 原始结果仍待运行器。

原始 Windows workspace 编译首先在 Tauri build-script 失败：缺少 `binaries/local-projects-dev-x86_64-pc-windows-msvc.exe`；现有 `prepare:desktop` 仅支持 Mac，尚不能生成 Windows sidecar。core 的独立编译诊断继续采集，不把依赖编译成功当作产品通过。

W0 尚未完成，W1–W6 与第 7 节全部待执行。下一步：校验并推送 CI 步骤，准备官方 Windows runner，检查实际 Windows 编译错误，按第 4 节收拢适配层。

### W0.1 — 自托管配置已推送，运行器已准备

- `bc02ad8` 已推送到 `win/w0-ci`，先于任何产品重构。GitHub 确认已创建 [run 41092968343](https://github.com/happypandaa/panda-drawer/actions/runs/41092968343)，macos / windows 两项均排队，尚未运行，不算绿色基线。
- 已从官方 `actions/runner` release 下载 **2.338.2 Windows x64**，以官方 asset digest 核对 SHA256，解压到仓库忽略的 `.tools/actions-runner-panda/2.338.2`；实际 `Runner.Listener.exe --version` 返回 `2.338.2`。安装脚本的 PowerShell 语法解析及真实 `-PrepareOnly` 通过。
- 新增可重复准备、拒绝覆盖已有注册的安装脚本；服务注册从本机临时 token 文件读取秘密，经子进程环境传入。未取得 token 和管理员执行环境，因此没有注册、安装或启动服务，没有改称 Windows CI 已运行。用户的 SSH push 身份不能替代 Actions 注册 token。
- 原始 core 独立 `--all-targets` 编译失败，测试目标共报告 **1,077 个编译错误**，主要为 Unix imports、文件元数据/权限扩展及原生 API；这是诊断数量，重复错误不等于独立缺陷数。保留现有测试，尚未通过改动消除它们。

### W0.2 — Windows 原生适配原语（已自测，业务接入未完成）

新增 workspace 成员 `platform-native`，Windows API 只出现在其 `src/windows/` 内，未修改现有 Mac 产品执行路径，也未删除或跳过原有 Mac 测试。第 4 节对照表标出原语和仍待迁移的业务调用，不能把本步骤记为整个适配层重构完成。

本机 Windows x64 **25H2 / build 26200.9711**，实际执行 `cargo test --locked --offline -p platform-native`：**10 通过，1 个专用子进程夹具入口默认忽略**。该入口由进程测试用 `--ignored --exact` 在真实子进程中调用，不是未完成能力的验收替身。全目标严格 Clippy、rustfmt 和差异检查通过。

原生文件测试使用明确命名的 `panda-native-中文 space-*` 临时目录，覆盖：当前用户独占的受保护 DACL、属主校验、完整 128 位 FileId / 卷序列号 / 链接数、创建不覆盖、对象替换、真实目录联接及其祖先拒绝、超过 260 字符的中文/空格路径。没有审计或改变系统父目录 ACL。并发路径保护专项发现属性查询句柄不阻止现代 Windows 改名，改用目录读取句柄参与共享约束后，父目录在本次私有创建期间不能被改名，释放后恢复正常；没有保留额外父目录权限准入。

原生进程测试使用真实的临时测试宿主/子进程：挂起创建、加入 KILL_ON_JOB_CLOSE Job 后恢复、仅继承指定匿名 stdio 管道；stderr **200,000 字节**完全排空，stdin 的中文协议内容及退出码正确；核对 PID + 创建时间 + 可执行路径，错误身份不取得任何终止权限；关闭 Job 终止真实 root 和 descendant；**直接 TerminateProcess 结束临时宿主**后，其 Job 中的 root 和 descendant 都由内核结束。Windows 可产生额外 console host，根进程退出不等于整个 Job 已为空，收尾使用 Job 的真实活动计数而不是假定永远只有两个进程。

限制：这些原语尚未接入 Node 生命周期、持久删除、工作区和 GUI；未执行真实主应用的任务管理器验收，未验证日志轮转/密钥替换、WebView2 存储删除或安装包。`sync_directory` 在 Windows 按规范无 fsync 操作，文件使用真实刷新，命名空间崩溃恢复待业务 journal 接入。Rust stable 的 Windows `MetadataExt::file_index/volume_serial_number/number_of_links` 仍不稳定，不能通过启用 nightly 绕过；当前原语通过 Win32 HANDLE 读取实际身份。
