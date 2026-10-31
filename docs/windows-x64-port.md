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
| 子进程归属与停止 | setsid / 独立进程组；TERM → 等待 → KILL 整组 | **Job Object**：挂起创建→实际 owner/匿名管道登记→加入 Job→恢复，KILL_ON_JOB_CLOSE；创建后错误保留根/主线程/Job。协议 stop watch 先发，宽限后 TerminateJobObject，无 TERM/Ctrl+Break。env_clear 由明确 sidecar 保留，未知在创建前拒绝 | `owned_process/{command,windows}.rs`、共享 `owned_process.rs`；原语 `platform-native/src/windows/process.rs`，环境 UTF-16 编码 `process/environment.rs`；网页 npm/环境探测与长驻 Node 已接线；packager 未确认结果保留 `tools/native-packager/src/windows/{payload,git}.rs` |
| 子进程管道 | `UnixStream::pair` | 原生匿名管道，创建前登记三个实际 std thread；64 KiB duplex 仅为异步接口，消费者退出后仍读到实际 EOF；stdin 的 BrokenPipe/109/232/233 只正常结束对应 worker，其它 I/O 错误保留。逐个 join 加根 HANDLE/Job 空及 driver join 才闭合，协议内容不变 | `owned_process/windows/pipe_bridge.rs`；`owned_process.rs` / `owned_process/windows.rs`；原语 `platform-native/src/windows/process.rs` |
| 退出状态 | `waitid`、信号 | `GetExitCodeProcess`；没有 Unix 信号，界面显示退出码 | 原生 Job 查询：`platform-native/src/windows/process.rs`；退出信号观察：`platform-native/src/process_status.rs`，接入 `node_runtime_trial.rs`、`process_diagnostics.rs` |
| 进程身份核对 | PID + 启动时间 + 进程组 | PID + `GetProcessTimes` 完整 100 纳秒创建时间 + `QueryFullProcessImageNameW` 路径。旧标记只观察，不取得 Job 终止权限 | 边界：`node_process_identity.rs`；Mac 原实现：`node_process_identity/unix.rs`；Windows：`node_process_identity/windows.rs` 与 `platform-native/src/windows/process.rs`；标记接入：`node_run_marker.rs` |
| 开机时间 | `sysctl KERN_BOOTTIME` | 当前时间减 `GetTickCount64` | `node_process_identity/unix.rs`、`node_process_identity/windows.rs` |
| 遗留运行 | 平台崩溃后应用进程可能存活，需要核对接管 | 因为设置了 KILL_ON_JOB_CLOSE，平台进程一退出，应用进程就随 Job 结束。启动时的标记核对仍要执行，通常会判定为已停止并清除 | `node_run_marker.rs` |
| 私有文件和目录 | 目录 0700、文件 0600、校验属主 uid | 新目录 protected、唯一当前用户 FA 并以 OI/CI 让普通工具创建的子项保留同一用户访问；显式文件/凭据仍不继承。既有目录修复仅用原 WRITE_DAC HANDLE 的 NtSetSecurityObject 更新所选 leaf，已私有目录幂等，不传播到子项，不改变属主或系统父 ACL | 共享创建与分策略 no-link 读取：`crates/platform-native/src/filesystem.rs`（原 Unix flags 保留；硬链接沿用调用方规则）；`project_file_io.rs`、`paths.rs`、`runtime_logs/`；Windows 原语与真实子进程/原始 SD 保持测试：`platform-native/src/windows/files.rs` / `files/private_creation_tests.rs`；其他私有文件政策仍待逐项接入 |
| Node 配置与本地密钥槽 | 原明文配置/凭据，0700/0600；普通配置可修权限，异常既有 credential 拒绝；新 secret 仅在提交前失败时回滚 | 当前 SID 私有 DACL、完整 FileId/no-reparse/单链接；只修已核验的当前用户配置 leaf，凭据不修复。配置以已绑定源 HANDLE 的原生 rename 成功为提交点，后续不返回错误误触 secret 回滚；锁 guard 在关闭前显式 UnlockFile，避免只依赖最终 HANDLE 关闭 | `node_app_configuration/{unix,windows,windows_tests}.rs`；原共用 save 业务正文与 Mac 实现保持；原生创建/私有打开/提交：`platform-native/src/windows/files.rs` |
| 文件身份 | dev + ino、nlink | 卷序列号 + 完整 128 位 FileId（`GetFileInformationByHandleEx` / `FileIdInfo`）、链接数；缺失证明也要求完整 Windows 身份，旧 pair 和诊断占位不取得权限 | Windows 原语：`crates/platform-native/src/windows/files.rs`；身份/占位构造：`file_identity.rs`；元数据与选择性变化比较：`platform-native/src/tool_files.rs`，接入 `metadata.rs`、`node_registration.rs`、`build_result.rs`、`build_window_views.rs`、`builds.rs` 的输入摘要扫描；元数据文件发布：`metadata_file_io.rs`；操作目录：`operation_directory.rs` / `node_process_scope.rs` |
| 只读目录观察与变化核对 | fdopendir / readdir / fstatat；dev/ino + mode/mtime/ctime | 固定实际目录 HANDLE 的作用域，按名称有界枚举；完整 FileId + FileBasicInfo 的原生属性/写入及变化时间，不伪造 Unix 元数据 | `platform-native/src/directory.rs` 及 `directory/unix.rs`、`directory/windows.rs`；求助摘录通过 `project_review.rs` 接入 |
| 数据快照临时观察戳 | dev/ino、mode、len、mtime/ctime 的进程内 tuple；链接数另按原规则检查 | 完整原生身份与 FileBasicInfo 的进程内观察戳；不合成 Unix tuple、不增加比较字段，不进入持久快照格式 | `project_version_data.rs` / `platform-native::tool_files::Facts::stamp`；目录私有创建与同步复用已有适配 |
| 恢复 journal 的有界只读快照 | 原 no-follow/nonblocking 读取；身份、mode、链接数、大小、mtime/ctime 前后核对 | 实际完整 FileId、链接数、大小与原生属性/时间前后核对；沿用业务预算及错误政策，不创建或修复 journal | `journal_file_io.rs`，接入 `source_restore_guard.rs`、`data_restore_guard.rs`；已开句柄的运行身份比较在 `file_identity.rs` |
| 项目窗口尺寸缓存 | 原私有读取；RENAME_SWAP/EXCL 发布与绑定身份/可选字节后的退役 | 读取沿用实际私有 DACL/独立文件规则；发布/退役使用私有不可变 journal、固定父 HANDLE、两步不覆盖改名与确定状态恢复；退役按 Mac 不额外要求 leaf 私有或单链接 | `window_cache_io.rs` 及 `window_cache_io/{macos,windows}.rs`；`app_windows.rs` 委托，原 Mac 业务/测试保持 |
| 符号链接 | `O_NOFOLLOW`，拒绝符号链接；明确选择本地工具时保留包管理器链接解析 | 以 `FILE_FLAG_OPEN_REPARSE_POINT` 打开，**拒绝所有重解析点**（符号链接、目录联接、挂载点），与 Mac 拒绝符号链接的位置一一对应；明确工具选择按原策略解析链接 | 共用预检查：`path_checks.rs`；文件 API：`files.rs` / `files/unix.rs` / `files/windows.rs`；原生句柄：`platform-native/src/windows/files.rs`；Git 选择文件边界：`git_environment/files.rs`；其他接入：`file_identity.rs`、`native_project_files.rs`、`source_publication.rs` |
| 原生路径与扩展命名空间 | 原 Unix 路径/字节及描述符遍历 | 在 UTF-16 层先统一 Windows 原本接受的两种分隔符，再分析祖先及添加扩展路径前缀；保留未配对 surrogate，不遗漏夹在混合分隔符中的重解析点 | `platform-native/src/windows/mod.rs`、`windows/files.rs`；共用 `filesystem::path_components` 仅 Windows 分支使用该分隔符转换 |
| 传给子程序/引擎的目录路径表示 | 原本地绝对路径 | 共用 UTF-16 conventional_path，标准扩展盘符/UNC 仅在 GetFullPathNameW 逐单元保持原路径时去前缀，特殊名字/命名空间保留；文件操作仍用原扩展路径，不由转换取得目录归属 | `platform-native/src/windows/api_paths.rs`；子进程 cwd 与 WebView2 接线分别验证，普通 API 路径不等于放宽 no-reparse/完整身份规则 |
| 系统工具目录 | 固定系统 Unix 位置 | 由 GetSystemDirectoryW / GetSystemWindowsDirectoryW 实际查询，不读取环境变量猜 C:\\Windows；按原生容量返回扩容、UTF-16 保留，失败明确拒绝 | `platform-native/src/windows/system_paths.rs`；供隔离 Node/npm 探测环境适配使用 |
| Git 离线工具选择与探测 | 原本地路径/Apple 开发工具查询、Unix PATH、清空配置的 version 探测 | MinGit 原路径、原生分号 PATH 和实际 Win32 系统目录；cleared Job 探测显式 SystemRoot，配置空设备 NUL，不读取宿主 Git 设置或凭据，不因缺少内置 Git 偷选 PATH 工具；版本保留 Windows vendor suffix | `git_environment/{unix,windows}.rs`；Mac 原正文保持，远程与源码业务另验 |
| 匿名 Git HTTPS HEAD 检查 | 原严格 GitHub/Gitee URL、版本范围、私有临时 scope 和禁配置/提示环境 | 同一 URL/输出/预算，Windows 盘符 cwd、对应 .windows.N vendor 版本、原生 API 路径、SystemRoot/本次 TEMP/TMP 和 NUL 配置；显式 cleared Job 及 core.longpaths，不继承宿主配置/代理/凭据 | `git_remote/{unix,windows}.rs`；共享取消/真实进程与 reader 闭合保持，Mac 原 Command/version 正文机械移动 |
| Node/npm 探测与参数 | 原 candidates、冒号 PATH、清空后的 managed probe 环境 | PATH 中 node.exe、本机 join_paths 与真实系统目录；隔离 probe 显式 Windows home/temp/SystemRoot，关闭 compile cache/npm 日志；npm 脚本/prefix/cwd/outDir 使用共用精确 API 路径表示，文件身份政策不变 | `environment/{unix,windows}.rs`，`node_tool_layout::command_argument`；Mac 原 candidates/tool_path/managed Command 正文保留；接入 `builds.rs` |
| Node 声明和静态网页代理的文件边界 | 声明 dev/ino、单链接、len/mtime 前后核对；静态根逐组件 openat/no-follow，拒绝文件硬链接 | 声明完整 FileId 与原 len/mtime/单链接政策，不新增 DACL/owner/ctime 限制；按原位置拒绝 reparse。代理根与每级目录固定 HANDLE 并持 pin 到实际叶文件打开；沿用原 HTTP、认证、CSP 与排空协议 | `node_app_manifest/{unix_files,windows_files}.rs`、`node_app_proxy/{unix_files,windows_files}.rs`；Mac 原函数正文机械比对保持，Windows 真实 HTTP/长路径/junction/硬链接与共享停止测试 |
| 长驻 Node 的实际 owner 与环境 | 原直接 Child/waitid/进程组与协议关闭；原明确清空环境和私有运行目录 | 复用共享匿名管道桥；挂起→Job→实际 owner/三泵/两 reader/Self 登记→resume，创建后错误继续持有。协议宽限后实际 Job 强停；root+Job 与所有原生/协议 I/O join 后才退役。应用显式变量加 Win32 查得的 SystemRoot 和本次 TEMP/TMP，保留 APP_TEMP_DIR/TMPDIR，不继承宿主 PATH/密钥 | `node_runtime_trial/{macos,windows}.rs`；共享 `owned_process/windows/pipe_bridge.rs`；只读属主 helper `platform-native/src/windows/files.rs` 不要求私有 DACL；管理器/桌面崩溃整链另验 |
| Node 启动标记与退役 | 原 inode/字节绑定、原文件就地更新保留 birth time，独占 capture 后 unlink，原 boot 门禁 | 原 HANDLE/完整 FileId/字节绑定与原地写回；闭合后在 slot 发布私有不可变 receipt，记录原 parent/gate/source/journal 身份、UTF-16 leaf 和原字节，先刷盘再 capture；实际 marker→gate→receipt 依次缺失，自己的 HANDLE 先关闭。丢 owner/重开可按 receipt 的已确认闭合事实续退，不从 PID/boot 猜；未知 preboot 材料仍原 Mac skip、保留不删 | `node_apps/{unix_files,windows_files}.rs`；`node_run_marker::cleanup_pending` 只委托平台续退，Unix no-op；Windows 普通记录不加 DACL 准入，原 256 KiB 预算保持；receipt 预算仅为该源字节的编码上界 |
| Node 桌面浏览器存储和关闭 | 原 Node data_store_identifier、导航政策和 Destroyed/后端停止顺序 | 与网页项目复用已绑定 UDF 的 Profile；Node kind/UUID/node-app label 精确相配，about:blank 创建后实际 Env7 核验/Env5 登记才显示导航。所有普通关闭/失败/monitor 路径先 UI thread seal/destroy，Destroyed 后还等实际 browser exit/retirement；不把默认 WebView2 目录当独立存储 | `node_windows.rs` 委托 `project_webview_profile.rs` / `windows/{binding,owner}.rs`；Mac facade 保持原 builder/事件/直接 destroy，Windows kind 同源真实引擎测试另含原 Web 回归 |
| 原子改名 | `rename`、`renameatx_np(RENAME_EXCL / RENAME_SWAP)` | 文件用 `MoveFileExW`（需要覆盖时加 `MOVEFILE_REPLACE_EXISTING`）；目录只在目标不存在时改名，发布后核对完整身份。改名成功后的任何观察失败均为 unknown，保留原始错误原因，不能冒充提交前失败。没有 SWAP，用"日志 + 两步改名"实现，并覆盖中途崩溃 | 文件 API 已接入 `files/windows.rs`；项目目录发布：`project_directory_io.rs` / `platform-native/src/windows/files.rs`；原生交付的 Mac 实现在 `native_project_files/unix.rs`，Windows 入口在 journal 未实现前明确拒绝；其他待接入：`durable_writer.rs`、`project_git*.rs`、`object_layout.rs` |
| 已归属名字的原生移除 | 按已绑定父目录 unlinkat；允许源码硬链接，只移除所选名字 | 固定父 HANDLE，打开实际 leaf 并匹配完整 FileId，以原 DELETE HANDLE 设置 FileDispositionInfoEx 的 DELETE/IGNORE_READONLY_ATTRIBUTE；不改只读位，不用 POSIX 提前移除打开名字。重解析 leaf 只移除自身，实际缺失才完成 | `platform-native/src/windows/removal.rs`；core `history_cleanup/windows/removal.rs` 实际 inventory、父 scope 持续 pin、占用短暂重试及剩余路径；迁移内容快照另待 W5 |
| 本机目录登记与可编辑源码发布 | 描述符绑定的只读检查、空目录初始化、候选复制与发布恢复 | 本机四入口用实际 HANDLE/完整 FileId 和 UTF-16 名称检查，已有外部目录只登记；对外响应回显已验证请求的原表示，内部 canonical/身份/revision 不变，保留明确 verbatim 命名空间。空目录先持久归属 UUID 再创建，失败只回滚原身份/字节。Git 发布固定父 HANDLE、原 payload 全身份与不可变计划，排他复制/不覆盖改名；已发布接受后改不二次复制，未知保留 reservation/只读核对。组件按 Windows UTF-16 自然限制，附件 API 不变 | `node_app_directory/windows.rs`、`source_publication/windows.rs`；Mac 原实现分别保留在各自 `unix.rs` 子模块 |
| 外部 ZIP 导出发布 | 原子不覆盖改名、目录/文件身份复核、替代材料保留或独占归还 | 发布和中断恢复未完成时，在改名或验证回调前明确拒绝，保留已准备的临时材料；待后续补齐 | `lifecycle_file_io.rs`；原 Unix 发布实现在 `lifecycle_file_io/unix.rs`，生命周期目录类型预检查也经该边界 |
| 受控 Git 源码获取 | 原生磁盘预算、文件树/摘要核对、非执行复制 | 同一 clone --no-checkout→分支证明→fsck→ls-tree→cat-file 排他物化→read-tree，不运行 checkout/filter/构建；原预算与实际 blocking writer/Job/I/O 闭合。完整 FileId/原 len/mtime/readonly，不加旧目录 ACL 准入；Windows 名称/真实 ordinal 碰撞和最终排他冲突返回需要修改，摘要执行位固定 0，Git index 保留 commit mode | `source_acquisition/windows{,_files,_policy}.rs`、`source_acquisition_process_windows.rs`；共用 types/url，Mac acquisition/process 原文件零修改 |
| 网页构建的 Git 准入与恢复 inventory | 无仓库不选择 Git；有仓库实际核对索引；逐项目检查恢复记录 | 无仓库实际扫描前后 marker/完整目录身份，不读工具配置或 spawn；保持实际读许可到扫描返回。有仓库索引与任意保留恢复记录在完成原生事务前明确拒绝；真实 Missing/Empty 可通行，沿用 Mac 1000 项目目录预算 | `project_git/windows/{build_guard,data_inventory}.rs`；Mac 原 `unix.rs` / `project_git_data.rs` 保持；不是 Git 导入或恢复完成 |
| Git 添加任务接纳与发布恢复 | 原任务去重/状态、接纳与中断清理顺序 | 保持实际源码错误与单 UUID；publishing 未收到路径时先只读对账原 receipt/payload/author 归属，确定发布才记录原路径，未知保留并列剩余路径，不因 registration_attempted=false 清现场。临时目录同步走平台 facade，恢复仍核实实际 writer/登记/归属 | `node_git_import/capability.rs`；共享业务只委托，Mac 新 hook no-op；实际 Git 整链和限制见验证记录 |
| npm 准备的输入与私有临时文件 | 原 no-follow 读取，单链接/大小准入，dev/ino、len、mtime 与 mode 变化核对；0700/0600 创建 | 实际完整身份、大小、修改时间和只读权限变化；当前用户私有 DACL，独占新建且刷新；不合成执行位，不增加 ctime/链接数变化要求。真实 cleared Job，原 npm 参数与 --prefix 目标保持；超过 Win32 cwd 限制仅将启动目录换到系统目录。显式私有 home/temp 和 SystemRoot，不继承宿主 npm 配置 | `dependency_file_io.rs` / `platform-native::tool_files::Facts::same_modified_permissions`；`node_dependency_prepare/{unix,windows}.rs`，原共享预算/扫描/收尾保持 |
| 构建输入文件复制 | 原前后 dev/ino、len、mtime 与 mode 核对；副本私有 0600，原可执行文件副本 0700 | 完整原生身份、大小、修改时间与只读属性核对；副本独立私有可写，不伪造执行位，不额外拒绝原来允许的源硬链接 | `build_source_io.rs`，由 `build_candidate.rs` 调用；完整构建运行与发布仍待 W2 |
| 已复制产物的刷新 | 原逐文件二次 fsync、各级目录 fsync | copy_tree 每个新私有可写输出 HANDLE 已实际 sync_all；完成时观察目标目录，不再用只读 HANDLE 重复调用需写权限的 FlushFileBuffers。源只读属性/硬链接不变，不伪造目录 fsync | `build_copy_io.rs`；`builds::copy_tree` 委托，Mac 原二次刷新正文保持；原始 npm staging 的发布另由 publication 适配处理 |
| 构建树摘要中的相对路径 | 原 UTF-8 路径字节，反斜杠文件名保持原义 | 按原生路径组件以 `/` 连接，与现有 Node 资源打包规则一致；不改变摘要 v1 格式、遍历或源码准入 | `build_digest_path_io.rs`；`builds::hash_tree` 委托；真实新准备的 template 源码及 starter 摘要已核验 |
| 构建产物目录交换与提交 | 原缺失目标 rename、既有目录 RENAME_SWAP、两父 fsync；provenance 为提交点 | 固定两父/完整 FileId、私有不可变 journal 与四种确定状态；Windows staging 复制成独立已刷新 generation，再交换 publication。inner pending 或 provenance rename unknown 禁止 outer rollback，保留 journal；恢复只 resume，false 不重新 swap，随后按原 digest/provenance 对账 | `build_directory_exchange.rs`、`build_publication_io.rs`；Mac publisher 原正文机械保持；实际三代产物/旧窗口租约/恢复及提交不确定测试 |
| 导入临时目录与归档只读观察 | 原目录类型/no-link/chmod；摘要按 dev/ino、len、mtime 复核；真实单链接检查 | 只调整已选目录自己的私有 DACL；摘要以完整身份、大小和修改时间复核，链接数来自实际句柄；归档权限数据见下项 | `import_directory_io.rs`、`project_file_io.rs` 的已观察链接数入口；接入 `node_git_import.rs`、`service.rs`、`backups.rs` |
| 归档中的文件权限数据 | `mode` 保留原 0–0777 数字，缺失默认 0600；恢复原 chmod，ZIP 写原 Unix 权限 | `mode` 使用明确的 `{windowsReadOnly: bool}` 原生数据，不伪造 Unix 位；新副本先为私有 DACL，再通过属性写入句柄设置真实只读位。跨系统导入映射为目标系统的只读/可写私有文件 | `archive_permissions.rs`（FileEntry 及权限数据/应用），`backups.rs` 保留现有入口；实际句柄属性更新在 `platform-native/src/windows/files.rs`；完整归档/恢复仍待验证 |
| 迁移证明只读输入 | 原已开文件的 dev/ino、len、mtime/ctime 前后核对和独立文件/大小界限；不按路径重新认领 | 同一实际句柄的完整身份、大小及原生写入/变化时间，沿用单链接和大小政策；不由读取产生清理权限 | `migration_metadata_io.rs` / `platform-native::tool_files::Facts::same_modified_and_changed`，接入 `workspace_migration_cleanup.rs` |
| 卸载范围的已绑定文件名字 | 先核对完整范围，再持有父目录并 fstatat / unlinkat；没有单链接准入 | 固定实际父目录，核对完整父目录/文件身份，经 DELETE 句柄移除该名字；保留原来允许的硬链接政策，不删除其他名字 | `cleanup_file_io.rs`，由 `local_cleanup.rs` 在原范围复核后调用；Windows 原语 `remove_file_allowing_hard_links`；完整卸载流程仍待 W5 |
| 工作区卷检查、清单与复制 | 原 APFS/local 与同步目录政策、statfs、Unix 权限/链接清单、复制与阶段日志 | 实际目录 HANDLE 查询卷及容量；全部 disk extents 绑定设备号与 PnP 本机容器属性观察内置性，fixed/NVMe/无 hotplug 不作证明。首启创建前后及已有选择核实本机内置 NTFS。同步观察独立使用 Cloud Files 与系统注册根，按实际 HANDLE/GUID 路径比较；不增加首启同步限制。原生复制与恢复未完成，迁移准入仍拒绝 | `paths/workspace_admission.rs`；Windows 原语 `platform-native/src/windows/{volume,workspace}.rs` 及 `volume/internal.rs`；`workspace_file_io.rs` 的原 Unix 实现不变，完整迁移待 W5 |
| 迁移启动恢复与初始化前检查 | 原遍历 journal、提交标记/epochs/任务/旧根退休处理 | 初始化目录前和恢复入口只读核实固定 migrations；真实 Missing/Empty 且 settings 无迁移引用才继续，已有/未知材料保留并明确拒绝；引用缺失 journal 报不一致，不清设置。沿用 settings 原私有/大小政策，允许原来允许的硬链接 | `workspace/startup.rs` / `startup/windows.rs`，`paths.rs` 只委托最早目录检查；Mac 恢复正文保持，实际迁移恢复待 W5 |
| 延后迁移原目录清理 | 原选中 journal 后提交任务，错误时保留 reopen failure；公开 retry 同一事务 | 真实无选中清理仍无工作；Windows 有选中清理时在自动提交及错误记录闭包前拒绝，公开 retry 在原 ID 授权后、写任务/journal 前同样拒绝，不凭早期空快照启动删除 | `service_workspace_migration/cleanup_admission.rs`；共享流程委托，Mac 原提交及错误保留正文保持，实际原目录收尾待 W5 |
| 构建任务与发布的启动恢复 | 原任务中断写回、发布恢复、任务核对和 best-effort 旧产物清理 | 固定实际 task scopes，以 no-reparse HANDLE 读取原记录，保留 Mac skip/硬链接/mtime 标签政策；非终态变 Interrupted、不自动执行；先恢复 inner/outer 发布及任务对账，再按原租约/恢复核验清理旧代和完成的中间材料 | `builds/startup.rs` / `windows_startup.rs` 与 11 项真实启动测试；`builds.rs` 只委托，共享 publication recovery 调用平台 exchange 恢复，Mac 原恢复语句保持 |
| 本机 Git 版本、构建 Git 准入与源码/数据恢复 | 原私有索引副本、仓库/文件观察、描述符交换与可恢复事务 | 共用输入/输出契约；原生仓库快照、进程、发布与恢复未完成时明确拒绝，不生成干净仓库或恢复/关闭收据；不等同于 MinGit 的远程克隆能力 | `project_git.rs`，契约 `project_git/types.rs`，Unix 实现经 `project_git/unix.rs` 保持原模块层级；Windows 临时边界 `project_git/windows.rs`。构建准入待 W2，本机版本/恢复必须后续补齐 |
| 数据恢复登记的不可变证明捕获 | 原已开文件的完整 Unix metadata tuple、摘要、稳定身份及 mode；原业务错误政策 | 原生不可变证明未实现时在读取或调用错误策略之前明确拒绝；不为 Windows 构造 Unix mode 证明 | `data_restore_proof_io.rs`，Unix 原实现在 `data_restore_proof_io/unix.rs`；`project_data_restore_registration.rs` 转交原错误函数；后续恢复实现必须补齐 |
| 旧卷身份离线修复 | 已确认旧 Mac 迁移材料的卷 UUID 绑定 | 本轮不兼容旧 Windows 实验数据，不能用 Mac 卷身份修复取得 Windows 归属；直接明确拒绝 | `identity_upgrade.rs`；Mac 原实现与测试：`identity_upgrade/unix.rs` |
| 持久化 | 文件和目录 fsync | 文件 `FlushFileBuffers`；目录无法 fsync，跳过，并在注释中说明。控制 JSON 在发布后观察失败时保留 unknown，不走提交前清理 | `atomic_control_file.rs`（`builds::atomic_json` 保留原入口）、`platform-native/src/filesystem.rs`；持久进程写入者仍在 `durable_writer.rs` 的未完成边界内 |
| 历史清理与已归属目录移除 | 描述符相对快照、内容/身份核对、unlinkat | Windows 完整身份、类型、大小、只读位与内容摘要快照；原计划 TTL/确认/任务保护/forget 保持。核验回调后及逐项删除前重查原对象，保留新内容与替代物；只读/长路径/占用有界重试，实际缺失才闭合，不跟随 junction | `history_cleanup/windows/{manager,plan_io,snapshot,removal}.rs`；Mac 原 Unix 实现和测试不改；迁移公有整链另验 |
| 持久写入者与空间核算 | 私有持久标记、Mac boot session UUID、描述符扫描/核算与任务清理 | preparing 原 HANDLE 刷新/发布/精确 finish；v2 标记记原生 ProcessTelemetryIdInformation BootId 和来源。当前 boot、旧无 boot 记录保持 unknown；不同 boot 先完整核验全组再精确回收。空间统计/分页/缓存失效和白名单清理用实际完整身份、逻辑硬链接字节及实际任务/reader join；无法定位开发程序范围时报告 unknown | `durable_writer/windows/`、原语 `platform-native/src/windows/boot_session.rs`；`storage/windows/{accounting,cleanup,coordinator}.rs`；不按 PID、时间差或任务终态清 marker；冷启动/休眠物理验收另记 |
| 网页项目独立文件删除 | 同卷私有暂存、原子不覆盖改名、状态 journal、身份核对后 unlink | 同父卷 private stage、完整父/文件 FileId 与原 owner 持久计划；不覆盖绑定 HANDLE 改名后移除原名字。普通原文件保留 Mac 允许硬链接政策，IGNORE_READONLY 不改变其它名字属性。delete-pending/占用有界等实际缺失，未知或替代内容保留且可重试 | `web_file_deletion/windows/{backend,io}.rs`；原语 `files/hardlink_rename.rs` 独立 helper，不放宽原单链接 helper；Mac Unix 文件不改 |
| 单实例锁 | 文件锁 | `LockFileEx` | `paths.rs` |
| 网页存储隔离 | WebKit `data_store_identifier` | 管理器固定 `webview-manager`；Web 与 Node 项目各自绑定独立目录及完整 FileId/pins。均先不可见 blank 创建、实读 Env7.UserDataFolder 核验才导航显示，项目另登记 Env5/销毁回执。环境/策略覆盖明确拒绝，启动前真实查询 Evergreen loader，缺失显示原生安装提示；Windows bundle origin 为 `http://tauri.localhost` 默认端口 | 管理器 `desktop_webview_profile/` / `windows/actual.rs`；项目 `project_webview_profile`、`webview_storage.rs`，Web/Node 分别接 `main.rs` / `node_windows.rs`；Mac 原默认清理及 identifier 行为保持 |
| 删除网页存储 | 等 WebKit 确认存储已移除 | 关闭该项目的所有窗口 → **等待该数据目录对应的 WebView2 浏览器进程退出** → 删除目录。进程未退出时目录被占用，会删除失败 | 同上 |
| 项目浏览器目录的持久身份绑定 | 原 StoreLedger 记录、项目标识与目录身份 | 仅按既有持久记录核对并 pin 独立项目目录；从 about:blank 创建后读回实际 Env7 UDF/full FileId，再开放原导航。sealed generation、Destroyed 与最后实际浏览器 PID 的 Env5 退出均到齐后，下一主队列才释放 profile/产物租约 | `webview_storage/windows.rs`；桌面 `project_webview_profile` 适配已接网页窗口/清理/关闭，Mac 原正文保持；磁盘 purge/forget 仍待 W5 |
| 引擎所选私有目录的子项权限 | 同用户在私有目录中创建引擎子项 | 在原持久身份及 pins 授权的 selected profile root 上，为唯一当前用户 FA 添加 OI/CI；已具备时不写，保持 protected，不修改 owner/SACL/祖先或修复普通未知目录 | `platform-native/src/windows/files/private_inheritance.rs`；不改变一般文件/凭据私有策略，不放宽 sandbox；真实 WebView2 子项及 Cookie 持久化已验证 |
| 网页归属索引目录的私有创建 | 对已选 ownership 根及项目 leaf 创建后 chmod 0700，既有 leaf 同样修权限 | 两个已选目录用原生私有 DACL 创建；既有普通已选目录只修自身权限，拒绝 reparse，不递归创建数据根或修改系统父 ACL | `web_project_ownership_io.rs`；`WebProjectOwnership::update` 仅委托原目录循环，索引写入仍用原 atomic_json |
| 网页下载与外部保存 | 原 WKWebView stage、写入归属、Finished 与用户所选位置发布 | WebView2 写入归属与外部发布未完成时，在 Requested 可见报告并拒绝；不修改 destination、不取得 writer，未受理的 Finished 不构成交付 | `browser_download.rs`；原 Unix 正文与 8 个测试在 `browser_download/unix.rs` 保持原模块层级；Windows 边界在 `browser_download/windows.rs` |
| 编辑器确认的目录身份 | 求助摘录前后核对路径与 dev/ino，不按目录内容变化撤销确认 | 核对卷序列号与完整 128 位 FileId；不新增时间、权限或内容变化比较 | `desktop_path_io.rs`，由 `project_onboarding.rs` 调用；原生观察复用 `platform-native::tool_files::Facts` |
| 管理器关闭与应用事件 | 关闭管理器隐藏窗口，Dock Reopen 显示；退出先等待受管服务排空 | 保留已有非 Mac 关闭后退出政策，退出同样等待排空；没有 Cocoa Reopen 事件 | `desktop_app_events.rs`；`main.rs` 仅注册两个事件处理入口 |
| 界面平台文案与路径提示 | 原 Finder、Dock/Cmd+Q、Mac 路径示例及 `/` 开头提示 | 依据实际编译 target 集中选择资源管理器、关闭退出、Windows 路径提示；本机生成/成功/复查文案按实际 ownership 说明生成目录随删、外部原始目录保留，删除业务不变。缺失 target 用原中性文案，实际路径/卷/权限仍由原生核验 | `apps/desktop/src/frontend-platform.tsx`；统一 provider 供设置、Node/Git 工具、WorkspaceLocation、LocalDirectoryPanel 使用；Mac 原文案和词法规则保持 |
| 桌面平台图标 | 现有熊猫 PNG / ICNS | 从同一已跟踪 PNG 通过仓库已安装 Tauri CLI 转换 ICO，保留原 Mac 素材 | `apps/desktop/src-tauri/icons/icon.ico`；图标不是应用或安装包产物 |
| 在文件管理器中打开 | `/usr/bin/open -a Finder` | SystemRoot 下的 `explorer.exe <路径>`；选文件为独立 `/select,<路径>` 参数。只确认创建派发进程，不等待不可靠的退出码或终止用户的 Explorer；真实窗口定位待验收 | `apps/desktop/src-tauri/src/local_directory.rs` |
| 自动打开已安装编辑器 | 原 Mac bundle ID、Apple 签名验证及 workspace URL 派发 | 安装验证与自动打开暂未完成，入口明确拒绝；界面可继续提供手动选择目录与复制接手说明，不伪造已安装路径 | `codex_handoff.rs`；原 Mac 全文/测试在 `codex_handoff/macos.rs` 同层保留，临时边界在 `unavailable.rs` |
| Node 普通路径观察与目录同步 | 原逐组件 symlink_metadata；明确目录 File::open / sync_all，文件刷新另保留 | 跳过孤立盘符，观察实际路径并拒绝所有重解析点；真实观察目录后跳过无等价的目录 fsync，文件仍真实刷新。Node 删除 journal 已实现持久 CAS intent、完整身份 staging/移除/重开恢复/改名前放弃；原同卷及替代物保留政策不变 | `node_apps.rs` 委托 `platform-native::filesystem::{path_components,is_reparse,sync_directory}`；`node_deletion_journal/windows/{backend,control,paths}.rs`；完整产品停止并删除与 GUI 续删另验 |
| 默认路径 | `~/PandaDrawer`、`~/Library/Application Support` | `%USERPROFILE%\PandaDrawer`、`%LOCALAPPDATA%`（`dirs` crate 已能返回） | `paths.rs` |
| 生成源码路径的词法检查 | 绝对、非根目录，components 仅 RootDir/Normal；沿用 Rust 的当前组件归一化 | 额外接受原生 Prefix（含 canonicalize 的 VerbatimDisk）；此处不读取目录、不额外检查 ACL/卷类型或授予归属 | `platform-native::filesystem::is_explicit_non_root_absolute_path`，由 `node_project_ownership.rs` 委托；后续完整身份与本机卷政策保留 |
| 已登记项目路径的保留名字 | 原无越界绝对路径末尾 projects/安全名字，只用于分配占名，不访问或重绑历史目标 | 接受盘符、UNC、扩展命名空间及两种分隔符的原生绝对路径；在 Rust 归一化前拒绝原始空、点及父组件。占名仍不授予目录身份或修复旧绑定 | `project_directory_io::retained_directory_leaf` 及 `project_directory_io/windows_claims.rs`；`projects::retain_directory_claims` 只委托，实际项目定位原严格绑定保持 |
| 内置 Node | darwin-arm64 的 Node 24.22.0 | 官方 `node-v24.22.0-win-x64.zip` 固定 ZIP/PE/许可证及完整 npm 树摘要；源码 helper 校验后用真实 Job 执行 Node/npm/npx，再以绑定源 HANDLE 不覆盖发布；直接执行 `node.exe npm-cli.js` | Mac 原 JSON/入口保持；Windows：两份 `*node*-release.windows-x64.json`、`package-node-runtime-windows.mjs`、`package-development-node-windows.mjs`、`tools/native-packager`。core 锚点/路径经 `node_tool_layout.rs` 接入 `managed_toolchain`、`node_apps` 与 resources API；正常桌面准备与进程调用仍待 W1/W3 |
| SDK 与起始模板打包的 npm | 原 `spawnSync('npm', args)` | 使用当前 Node 直接执行实际 `npm-cli.js`；参数保持数组，找不到入口时报错，不回退到 `.cmd` | `scripts/npm-command.mjs`，接入 `package-sdk.mjs`、`package-template.mjs`；Mac 原 npm 调用保持 |
| 开发验证脚本的父路径组件 | 按 `/` 分割，仅拒绝 `..`，`.` 继续交给 normalize | 同时按 `/` 与 `\` 分割，只拒绝同样的 `..`；不改变 Mac 反斜杠普通文件名语义，不启用 Windows 更新 | `scripts/native-path.mjs`，接入 `update-artifact-verification.mjs`、`update-manifest.mjs` |
| 内置 Git | 源码构建的 Mac arm64 Git | 官方固定 **MinGit 2.56.0.windows.4 64 位**，完整 ZIP/许可证/文件树摘要，保留 UCRT64 布局；校验后用真实 Job 探测 cmd/git.exe，再以绑定源 HANDLE 不覆盖发布 | Mac 原 release/源码构建入口保持；Windows打包：`development-git-release.windows-x64.json`、`package-development-git-windows.mjs`、`tools/native-packager/src/windows/git.rs`；core 完整树锚点/原生 schema 与路径：`managed_git/layout.rs`，Git 实际业务进程/远程导入待 W3/W4 |
| 安装资源与平台 CLI 定位 | Tauri 资源目录；debug 原源码资源/带 Mac triple sidecar fallback | 仅 Tauri resource_dir 下的 resources，以及桌面 exe 旁已复制的 local-projects-dev.exe；缺失如实报告，不回退旧源码资源或带 triple 的打包源文件名 | `apps/desktop/src-tauri/src/desktop_installation/{macos,windows}.rs`；main 委托，原 Mac 函数正文保持 |
| 正常桌面资源准备与 Tauri 调用 | 原固定 Mac resources / sidecar 与标准 Mac 输出流程 | 每次新建 Windows 准备目录和独立 Cargo target，准备固定 Node/MinGit、SDK/模板和实际 CLI；以当前 Node 调用 Tauri JS，最后合并资源 reset/map，避免旧成员混入。Windows 平台配置选择 currentUser NSIS / downloadBootstrapper | `scripts/prepare-desktop-windows.mjs`、`desktop-host.mjs`；共享入口仅委托，Mac 原流程保持；`tauri.windows.conf.json`，尚未做安装验收 |
| Git 子进程环境 | Unix PATH、`/dev/null`、`/usr/bin/false`、冒号分隔 | Windows PATH（分号分隔）、`NUL`；临时路径含 `D:\` 不能被拒；保持现有的禁用凭据助手、提示、hooks、filters、全局配置的策略；加 `-c core.longpaths=true` | `git_environment.rs`、`git_remote.rs` |
| 资源完整性锚点 | 编译时写入资源摘要（`signed_resources_unix.rs`） | 实现 Windows 对应版本，覆盖 Windows 资源树 | `crates/platform-core/build_support/` |
| 运行门禁 | — | Windows x64 开发版运行入口已开放；迁移/构建历史恢复、Git 添加恢复等未实现能力在自己的适配入口明确拒绝。非 x64 Windows 仍不接纳，正式版签名/资源证明门禁保持原规则；策略开放不代表 GUI 或完整产品验收 | `platform_support.rs`；真实编译 policy 检查由 `scripts/platform-support-test.mjs` 委托 core 单测 |
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

2026-10-31 用户执行补充：CI 和运行器注册后置，先完成其余 Windows 功能、安装包与真机验收。继续按未验收栈分支推进，已完成的小步检查后提交并推送；未运行 CI 不记绿色，也不提前合入 main。Mac 行为保持要求不变。
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
