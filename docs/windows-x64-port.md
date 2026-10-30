# 熊猫小抽屉 Windows x64 移植：执行标准

状态：已确认（2026-10-31）。本文取代 `docs/windows-support-plan.md` 与 `docs/windows-w1-verification.md`；后两者只作历史参考，其中关于备份、回收站、安装副本、源码候选、Git 取回的内容均已失效。旧分支 `codex/windows-compatibility` 已归档为标签 `archive/windows-compatibility`，**不合并、不沿用其受限架构**（`platform-host`、`platform-node`），只可从中参考个别实现。

本文写给在 Windows 机器上工作的 AI。目标是**一次性完成 Windows x64 支持**：按第 6 节的里程碑自主推进，每个里程碑完成并通过检查后提交、推送、合并回 main，直到第 7 节验收全部通过。只有第 9 节列出的情况需要停下来问用户。

---

## 1 已确定的范围

| 事项 | 决定 |
|---|---|
| 系统 | Windows 10 22H2 x64、Windows 11 x64。不支持 ARM64、32 位、LTSC 特殊版本 |
| macOS | 不变：macOS 15+、Apple Silicon。Mac 现有行为和测试必须保持通过 |
| 功能范围 | 与当前 main 上 Mac 的功能**完全一致**（见第 3 节）。不做 Mac 没有的功能 |
| 安装包 | Tauri NSIS，当前用户安装（不需要管理员权限） |
| WebView2 | Evergreen；安装包内置在线引导安装（downloadBootstrapper） |
| 签名 | 本轮不签名。SmartScreen 警告可接受，在验收记录里注明 |
| 自动更新 | Windows 本轮不做。更新入口在 Windows 上隐藏或提示"请手动下载新版本"，后端门禁关闭 |
| 工作区 | 默认 `%USERPROFILE%\PandaDrawer`（开发版 `PandaDrawer-Dev`），平台数据在 `%LOCALAPPDATA%` 下。只支持本机内置 NTFS 磁盘；迁移目标是外置盘、网络盘、映射盘或同步目录时，拒绝并说明原因 |
| 旧数据 | 不兼容任何旧 Windows 实验数据，不做迁移 |

## 2 总原则

1. **一个 main，平台差异写在代码里。** 不建立长期平台分支。平台相关代码集中在适配层（第 4 节），业务代码不直接写 `cfg(target_os)`，也不直接调用 Unix 或 Win32 API。
2. **Windows 的保证与 Mac 相同，用 Windows 自然的方式实现。** 不额外提高安全或校验要求。旧分支就是因为自加了大量比 Mac 更严的准入条件（例如系统父目录 ACL 检查），最后连真实界面都没跑通。确需更严格处理时，先问用户。
3. **能力未完成时明确拒绝。** 未实现的能力返回明确错误（如 `PLATFORM_FEATURE_UNAVAILABLE`），界面显示"Windows 暂不支持此功能"。不能崩溃，不能静默跳过，也不能假装成功。
4. **不破坏 Mac。** 每次合并前，macOS 与 Windows 两套 CI 都必须通过。Mac 上无法本地验证的改动，以 CI 结果为准。CI 只使用**自托管运行器**（用户的 Mac 和 Windows 开发机），不使用 GitHub 云端运行器（私有仓库的云端 macOS 额度很贵）。
5. **测试按平台跳过，不删除。** Mac 专属测试在 Windows 上用 `cfg` 跳过；为 Windows 新增对应测试。

## 3 当前 main 上需要在 Windows 实现的功能

以 main 的实际代码和 `docs/` 中已确认的契约为准，主要包括：

- 主界面"我的应用"、设置（存储与清理、运行环境、给 AI 的平台说明、关于、卸载前清除）、按需出现的任务入口
- 网页项目：新建、打开（独立窗口）、SQLite 数据库与附件、平台构建（`project build`）、只保留当前构建
- Node 应用：直接运行项目文件夹、配置与密钥注入、stdout 控制协议、运行日志（`logs/current.log`、`previous.log`）、停止与重启
- 添加项目：本机文件夹（空文件夹生成起始项目 / 已有项目登记）、从 Git 添加（GitHub、Gitee 公开 HTTPS）、依赖安装（`npm ci --omit=dev --ignore-scripts`）
- 停止并删除：持久删除任务、重启续删、重试、改名前放弃；完整删除（见 `docs/project-complete-deletion.md`）
- 每个项目独立的网页存储，删除项目时一并删除
- 工作区迁移（复制、核验、重开后自动删除旧目录）
- 复制给 AI 的交接与求助提示词（路径用 Windows 格式）
- 平台 CLI（`platform-cli`）

## 4 平台适配层：Mac 与 Windows 对照

先把下表各项收拢到适配模块（Mac 行为不变，以 macOS CI 全部测试通过为准），再实现 Windows 一侧。表中"现在位置"是主要涉及的文件，不是完整清单；开始前用 `grep -rE 'cfg\(unix|target_os = "macos"|libc::|std::os::unix'` 列出全部位置。

| 能力 | Mac 现在的做法 | Windows 实现 | 现在位置（主要） |
|---|---|---|---|
| 子进程归属与停止 | setsid / 独立进程组；TERM → 等待 → KILL 整组 | **Job Object**：以挂起方式创建进程 → 加入 Job → 恢复；设置 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。停止时先发 stdout 协议的关闭消息，等待超时后 `TerminateJobObject`。Windows 没有 TERM，不要用 Ctrl+Break 代替 | Windows 原语：`crates/platform-native/src/windows/process.rs`；业务接入点：`owned_process*.rs`、`node_runtime_trial.rs`、`node_process_identity.rs`、`git_environment.rs`、`builds.rs` |
| 子进程管道 | `UnixStream::pair` | 匿名管道（tokio `process` 的 piped stdio 或 Win32 管道），协议内容不变 | `owned_process.rs` |
| 退出状态 | `waitid`、信号 | `GetExitCodeProcess`；没有信号，界面显示退出码 | `node_runtime_trial.rs` |
| 进程身份核对 | PID + 启动时间 + 进程组 | PID + `GetProcessTimes` 完整 100 纳秒创建时间 + `QueryFullProcessImageNameW` 路径。旧标记只观察，不取得 Job 终止权限 | 边界：`node_process_identity.rs`；Mac 原实现：`node_process_identity/unix.rs`；Windows：`node_process_identity/windows.rs` 与 `platform-native/src/windows/process.rs`；标记接入：`node_run_marker.rs` |
| 开机时间 | `sysctl KERN_BOOTTIME` | 当前时间减 `GetTickCount64` | `node_process_identity/unix.rs`、`node_process_identity/windows.rs` |
| 遗留运行 | 平台崩溃后应用进程可能存活，需要核对接管 | 因为设置了 KILL_ON_JOB_CLOSE，平台进程一退出，应用进程就随 Job 结束。启动时的标记核对仍要执行，通常会判定为已停止并清除 | `node_run_marker.rs` |
| 私有文件和目录 | 目录 0700、文件 0600、校验属主 uid | 创建时设置受保护的 DACL：只授予当前用户 SID 完全控制，不继承父目录权限。校验属主为当前用户；初始化已选空数据根时只设置该目录自己的私有访问，不改变属主或系统父目录 ACL | 共享文件适配：`crates/platform-native/src/filesystem.rs`；启动根已接入 `paths.rs`；日志原生操作：`crates/platform-native/src/runtime_logs/`；Windows 原语：`crates/platform-native/src/windows/files.rs`；待接入：`durable_writer.rs`、`node_app_configuration.rs` |
| 文件身份 | dev + ino、nlink | 卷序列号 + 完整 128 位 FileId（`GetFileInformationByHandleEx` / `FileIdInfo`）、链接数 | Windows 原语：`crates/platform-native/src/windows/files.rs`；业务接入点：`file_identity.rs`、`metadata.rs` |
| 只读目录观察与变化核对 | fdopendir / readdir / fstatat；dev/ino + mode/mtime/ctime | 固定实际目录 HANDLE 的作用域，按名称有界枚举；完整 FileId + FileBasicInfo 的原生属性/写入及变化时间，不伪造 Unix 元数据 | `platform-native/src/directory.rs` 及 `directory/unix.rs`、`directory/windows.rs`；求助摘录通过 `project_review.rs` 接入 |
| 符号链接 | `O_NOFOLLOW`，拒绝符号链接 | 以 `FILE_FLAG_OPEN_REPARSE_POINT` 打开，**拒绝所有重解析点**（符号链接、目录联接、挂载点），与 Mac 拒绝符号链接的位置一一对应 | 文件 API 边界：`files.rs` / `files/unix.rs` / `files/windows.rs`；原生句柄作用域：`platform-native/src/windows/files.rs`；其他接入：`file_identity.rs`、`native_project_files.rs`、`source_publication.rs` |
| 原子改名 | `rename`、`renameatx_np(RENAME_EXCL / RENAME_SWAP)` | 文件用 `MoveFileExW`（需要覆盖时加 `MOVEFILE_REPLACE_EXISTING`）；目录只在目标不存在时改名。没有 SWAP，用"日志 + 两步改名"实现，并覆盖中途崩溃 | 文件 API 已接入 `files/windows.rs`；原生交付的 Mac 实现在 `native_project_files/unix.rs`，Windows 入口在 journal 未实现前明确拒绝；其他待接入：`durable_writer.rs`、`project_git*.rs`、`object_layout.rs` |
| 持久化 | 文件和目录 fsync | 文件 `FlushFileBuffers`；目录无法 fsync，跳过，并在注释中说明 | `durable_writer.rs` |
| 单实例锁 | 文件锁 | `LockFileEx` | `paths.rs` |
| 网页存储隔离 | WebKit `data_store_identifier` | 每个项目一个 **WebView2 用户数据目录**（Tauri `data_directory`），放在平台数据目录下，以现有项目标识命名。管理器窗口使用自己的目录 | `webview_storage.rs`、`apps/desktop/src-tauri/src/native_stores.rs`、`main.rs`、`node_windows.rs` |
| 删除网页存储 | 等 WebKit 确认存储已移除 | 关闭该项目的所有窗口 → **等待该数据目录对应的 WebView2 浏览器进程退出** → 删除目录。进程未退出时目录被占用，会删除失败 | 同上 |
| 在文件管理器中打开 | `/usr/bin/open -a Finder` | `explorer.exe <路径>`。注意 explorer 的退出码不可靠，不能据此判断失败 | `apps/desktop/src-tauri/src/local_directory.rs` |
| 默认路径 | `~/PandaDrawer`、`~/Library/Application Support` | `%USERPROFILE%\PandaDrawer`、`%LOCALAPPDATA%`（`dirs` crate 已能返回） | `paths.rs` |
| 内置 Node | darwin-arm64 的 Node 24.22.0 | 官方 `node-v24.22.0-win-x64.zip`，固定 SHA256，写入 release json；直接执行 `node.exe npm-cli.js`，不走 `.cmd` 包装 | `scripts/node-runtime-release.json`、`development-node-release.json`、`package-*.mjs`、`managed_toolchain.rs`；只读文件事实适配：`platform-native/src/tool_files.rs`，Windows 发行资源待 W1 |
| 内置 Git | 源码构建的 Mac arm64 Git | Git for Windows 的 **MinGit 64 位**（官方便携版，专为嵌入设计）。实施时选当时最新稳定版，固定版本和 SHA256 | `scripts/development-git-release.json`、`package-development-git.mjs`、`managed_git.rs`、`git_environment.rs`；只读文件事实适配：`platform-native/src/tool_files.rs`，Windows 发行资源待 W1 |
| Git 子进程环境 | Unix PATH、`/dev/null`、`/usr/bin/false`、冒号分隔 | Windows PATH（分号分隔）、`NUL`；临时路径含 `D:\` 不能被拒；保持现有的禁用凭据助手、提示、hooks、filters、全局配置的策略；加 `-c core.longpaths=true` | `git_environment.rs`、`git_remote.rs` |
| 资源完整性锚点 | 编译时写入资源摘要（`signed_resources_unix.rs`） | 实现 Windows 对应版本，覆盖 Windows 资源树 | `crates/platform-core/build_support/` |
| 运行门禁 | — | `platform_support.rs` 当前对 Windows 返回 `PLATFORM_STAGE_UNAVAILABLE`。在 W1 完成时改为开放，未完成的能力用第 2 节第 3 条的方式单独关闭 | `platform_support.rs` |
| 更新 | `platform-update-handoff`、`platform-update-witness` 为 macOS 实现 | 本轮不实现，保证在 Windows 上能编译（用 `cfg` 隔离），门禁关闭 | `crates/platform-update-*` |

2026-10-31 实施状态：新增的 `platform-native` 提供 Windows 原生原语，文件身份、私有创建、拒绝重解析点及 Job/管道已有本机临时夹具测试；core 的 `file_identity.rs` 已接入完整 Windows 身份，`paths.rs` 的私有创建、逐级路径检查和元数据替换通过 `platform-native/src/filesystem.rs` 适配，Unix 分支保留原有 mode、no-follow、rename 和目录 fsync。其他业务仍待逐项接入，Mac 代码尚未全部收拢，不据此称 W0、W1 或 W3 完成。目录持久化在 Windows 明确无 fsync 等价操作，文件仍刷新，命名空间操作依靠后续 journal；不新增对系统父目录 ACL 的准入要求。实际结果和限制见 [windows-x64-verification.md](windows-x64-verification.md)。

## 5 Windows 特有的问题（必须处理）

1. **文件被占用时无法删除。** 打开中的文件、被杀毒软件（Defender）或索引服务短暂锁住的文件，删除会失败（`ERROR_SHARING_VIOLATION`、`ERROR_ACCESS_DENIED`）。
   - 删除和清理时，对这类错误做有限次数的短暂重试（例如间隔递增、总计数秒）。
   - 仍然失败时，按现有删除任务的失败语义处理：列出剩余路径，界面显示"有文件正在被使用，关闭相关程序后重试"，用户可以重试或放弃（改名前）。
2. **只读文件。** Git 对象文件（`.git/objects/**`）在 Windows 上带只读属性，直接删除会报"拒绝访问"。删除前先清除只读属性。`node_modules` 中也可能出现。
3. **长路径。** `node_modules` 的嵌套路径常超过 260 个字符。确认 Rust std 的文件操作、npm 和 Git（`core.longpaths`）在长路径下都能工作；不要依赖用户修改系统注册表。
4. **文件名规则。** 不区分大小写；保留名（`CON`、`PRN`、`AUX`、`NUL`、`COM1`–`COM9`、`LPT1`–`LPT9`，含扩展名时同样保留）；不允许 `< > : " / \ | ? *`；不能以点或空格结尾。项目目录名生成（`project_directory_name*`）在 Windows 上要避开这些情况，大小写不同的同名目录视为冲突。从 Git 导入时，仓库里有不合法文件名的，按"需要修改"失败并说明。
5. **换行符。** 按字节校验的文件（目录名参考数据、模板、AI 提示词生成结果、测试样本等）在 `.gitattributes` 中固定为不转换，避免 Windows 检出时被改成 CRLF 导致摘要不符。
6. **中文和空格路径。** 用户名、工作区路径中有中文或空格时，所有功能都要正常。所有子进程调用使用参数数组，不拼接命令行字符串。
7. **防火墙。** 平台服务和 Node 应用都只监听 `127.0.0.1`（Node 应用已通过 `APP_HOST=127.0.0.1` 注入），不能触发 Windows 防火墙弹窗。应用自己监听 `0.0.0.0` 的，属于应用问题，在求助提示词里说明。
8. **WebView2 缺失。** 启动时检测 WebView2 运行时；缺失时提示安装，不能白屏。

## 6 里程碑

每个里程碑在 `win/<里程碑>` 短分支上完成（例如 `win/w0-ci`），合并前先变基到最新 main，两套 CI 通过后快进合并到 main 并推送，然后开始下一个。

**全程无人值守。** 用户不会盯着进度，也不会在里程碑之间做验收：
- 推送后用 `gh run watch` / `gh run view` 查看 CI 结果。失败就自己修复、重新推送，直到通过。
- macOS 运行器离线（用户的 Mac 关机或睡眠）导致检查排队时，**不要干等**：在当前分支的基础上开下一个里程碑的分支继续工作，等检查通过后按顺序依次合并。排队超过 24 小时，在验证记录里注明，并在下次汇报时提醒用户。
- 每个里程碑合并后，只用几句话汇报结果，然后直接继续，不等待回复。

**W0：CI 与编译**
- 新增 GitHub Actions，两个任务都运行在自托管运行器上：macOS 任务用标签 `[self-hosted, macOS, ARM64]`，Windows 任务用 `[self-hosted, Windows, X64]`。每次推送和 PR 运行 Rust 编译与测试、`tsc -b`、界面测试（`npm run test:ui`）、脚本测试。
- 在 Windows 开发机上安装并注册 Windows 运行器（作为服务运行）。
- 写一份 Mac 运行器安装步骤（`docs/ci-self-hosted-runners.md`）：下载、注册、设为后台服务（`svc.sh install`）、关闭自动睡眠。交给用户在 Mac 上执行，然后继续做 W0 的其他部分，不等待；Mac 运行器上线后检查会自动开始。
- 运行器在用户的真实电脑上运行：测试只能使用临时目录，不能读写平台的开发数据目录和用户的工作区。
- **先提交 CI，再做重构。** W0 可以在 Windows 机器上完成：Mac 行为以 macOS CI 的结果为准，CI 没有覆盖到的 Mac 改动不要做。
- 完成第 4 节的适配层重构，Mac 行为不变。
- main 在 Windows 上编译通过。尚未实现的能力明确报错。
- 完成标准：两套 CI 绿色；Mac 全部现有测试通过。

**W1：平台能启动**
- Windows 内置 Node、MinGit 的打包脚本与 release json；资源完整性锚点。
- 默认路径、私有文件 ACL、单实例锁、文件身份、拒绝重解析点、持久写入。
- 打开运行门禁；主界面、设置、工作区目录、在资源管理器中打开。
- 管理器窗口使用自己的 WebView2 数据目录；WebView2 缺失检测。
- 完成标准：在 Windows 上 `npm run dev` 能打开主界面，设置各页可用。

**W2：网页项目**
- 新建、打开、数据库、附件、平台构建、只保留当前构建。
- 每个项目独立 WebView2 数据目录。
- 完成标准：两个网页项目的 localStorage 互不可见；构建三次后只剩当前一份产物。

**W3：Node 应用**
- Job Object 进程管理、匿名管道、stdout 控制协议、配置与密钥注入、运行日志（ACL 私有、轮转、密钥替换）、停止与重启、进程身份与开机时间、遗留运行核对。
- 完成标准：起始 Node 项目能运行；向 stderr 输出超过 64 KB 不被终止；在任务管理器里结束平台进程后，应用进程也随之结束，重启平台后状态正确。

**W4：添加项目**
- 本机文件夹（空文件夹生成起始项目、已有项目登记、同一文件夹不能重复添加）。
- 从 Git 添加：MinGit、`npm ci`、提交点前回滚 / 提交点后保留、取消与中断处理。
- 完成标准：从 GitHub 导入一个公开 Node 仓库并运行；导入中途取消，不留任何残留。

**W5：停止并删除、存储与迁移**
- 停止并删除完整流程（持久任务、续删、重试、放弃），加上第 5 节的文件占用、只读文件、长路径处理。
- 删除网页存储时等待 WebView2 浏览器进程退出。
- 存储与清理页、卸载前清除、工作区迁移（只允许本机 NTFS 内置磁盘）。
- 完成标准：第 7 节中删除和迁移相关项全部通过。

**W6：安装包**
- `tauri.conf.json` 增加 Windows NSIS 目标（当前用户安装、WebView2 在线引导）。Mac 的 `app`、`dmg` 目标不变（可用平台专属配置文件区分）。
- 更新入口在 Windows 上关闭。
- 完成标准：在干净的 Windows 10 22H2 与 Windows 11 上安装、使用、卸载，第 7 节全部通过。

## 7 最终验收（在真实 Windows 10 22H2 与 Windows 11 x64 上各做一遍）

用安装包安装，不用开发模式。所有删除测试只使用临时项目。

1. 安装：当前用户、无需管理员权限；SmartScreen 警告可以通过；缺少 WebView2 的系统能引导安装。
2. 主界面、设置各页正常；"在资源管理器中打开"打开正确目录。
3. 用户名或工作区路径含中文和空格时，以下各项同样通过。
4. 新建网页项目：增删改查数据、上传附件、构建、重新打开数据仍在。
5. 两个网页项目的 localStorage、IndexedDB 互相看不到。
6. 添加空文件夹：生成起始 Node 项目并能运行；修改配置后"立即重启"生效。
7. 从 GitHub 添加公开 Node 仓库（包含 `node_modules` 长路径）：能运行；日志文件存在，只有当前用户能访问，密钥已被替换为 `***`。
8. 运行中的 Node 应用：停止并删除成功；项目文件夹、实例目录、日志、网页存储全部不存在。
9. 在记事本中打开项目文件夹里的一个文件，再删除该项目：显示有文件被占用和剩余路径；关闭记事本后重试，删除干净。
10. 删除过程中在任务管理器里结束平台进程，重新打开：显示删除未完成，能继续完成。
11. 外部本地项目删除后，用户文件夹完好无损。
12. 在任务管理器结束平台进程：所有应用进程随之结束；重启平台后没有残留的"运行中"状态。
13. 工作区迁移到另一块本机 NTFS 磁盘：迁移成功，重开后旧目录被自动删除；选择网络盘或 U 盘时被拒绝并说明原因。
14. 卸载前清除：平台管理的数据和代码全部清除，外部原始文件夹保留。
15. 卸载后重新安装，平台能正常启动。

最后在 Mac 上做一次回归验收（由用户执行）：从最新 main 打包 Mac 开发版，打开主界面、运行一个 Node 应用、新建网页项目、停止并删除临时项目、设置各页。W6 完成后，在 `docs/windows-x64-verification.md` 里写好 Mac 打包命令和这份清单，通知用户。

验收结果记录在 `docs/windows-x64-verification.md`：每一项写明系统版本、实际操作、结果；不通过或没测的如实写出原因。CI 结果、Mac 测试结果不能当作 Windows 通过的证据。

## 8 Mac 侧的协作约定

Mac 和 Windows 会同时开发，Mac 一侧（包括 Mac 上工作的 AI）遵守：

- 新增或修改平台相关行为时，必须放进适配层，并更新第 4 节对照表。Windows 一侧可以先返回"暂不支持"，但**不能让 Windows 编译或 CI 失败**。
- 不在业务代码里新增 `cfg(target_os)`、`libc::` 或 Unix 专属 API 调用。
- 新增需要按字节校验的文件时，同步更新 `.gitattributes`。
- 合并到 main 前，两套 CI 都必须通过。

## 9 需要停下来问用户的情况

只有以下情况暂停并询问，其余按本文自行决定并在提交说明里写明：

1. 需要改变 Mac 的现有行为。
2. 需要比 Mac 更严格的安全或校验要求。
3. 某项功能在 Windows 上无法实现，打算永久不支持。
4. 需要引入本文没有列出的第三方二进制或下载来源。
5. 任何会删除临时测试项目以外数据的操作。
6. 第 7 节某项验收在两种系统中的一种上无法通过，且找不到修复办法。

## 10 工作方式

- 开始前通读：本文、`AGENTS.md`、`docs/project-complete-deletion.md`、`docs/node-single-directory.md`，以及与当前里程碑相关的契约文档。
- 每个里程碑完成后：提交、推送、合并回 main，并在 `docs/windows-x64-verification.md` 追加该里程碑的实际验证结果和限制。
- 遵守 `AGENTS.md` 的提交规则：不提交密钥、生成的应用、安装包和本地测试证据。
