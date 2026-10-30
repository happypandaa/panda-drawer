# Windows x64 移植验证记录

执行标准：[windows-x64-port.md](windows-x64-port.md)。开始日期：2026-10-31（Asia/Shanghai）。未完成、排队和未执行均不算通过。

## 起点与环境

- 原始 main：`ac3b5d72ab79e132fcedd149c73e1201a2896576`，开始前已 fetch 并快进同步。旧 Windows 分支不合并、不沿用旧受限架构。
- 已通读执行标准、AGENTS、完整删除和 Node 单目录运行契约。
- 当前主线没有 `.github/workflows`，没有可引用的原始 macOS 绿色检查；不能声称已确认 Mac 基线通过。先提交只涉及 CI/文档的步骤，再获取真实 macOS 检查结果；原有失败必须按测试名称记录，不能通过删除或跳过测试消除。
- 本机 Windows x64，Node `24.14.0`、Rust MSVC `1.99.1`、VS Build Tools 2022 `17.14.37806.7` / MSVC `14.44.35207`、Windows SDK `10.0.26100.0`、WebView2 `156.0.4314.50` 已存在。先前 `env:windows` 和独立 C/Rust x64 编译、链接、运行探针通过；不是本轮产品兼容性或安装验收证据。起点无需补装这些组件；后续补装的 Rust 目标标准库见 W0.9。
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

### W0.3 — core 文件身份接入（局部验证通过，W0 未完成）

`file_identity.rs` 作为现有文件身份适配边界，Windows 捕获调用原生适配 crate，持久记录增加 `windowsFileId`（16 字节），`device` 保存卷序列号；完整比较使用卷与全部 128 位，不能仅比较原有 `inode` 展示/内部兼容字段。未知旧 Windows pair 不走 Mac 身份修复，也不兼容旧实验数据。严格恢复记录的身份读取同步接受这一 Windows 字段。Mac 捕获仍采用原有 volume UUID / device / inode；新字段为 None 时不序列化，原有 Mac 格式与符号链接测试保留，Mac 实际 CI 仍待运行器。

因 core 的其他模块尚不能在 Windows 编译，使用忽略目录内的独立 Cargo harness **直接引用实际 `file_identity.rs`**，运行其两个 Windows 测试：真实临时文件捕获/序列化往返、高 64 位变更拒绝、旧 pair 拒绝、路径替换、缺失目标的预期卷核对，**2/2 通过**；该源文件的严格 Clippy 与格式检查通过。harness 对未移植的 Mac legacy repair 设为一旦调用即 panic，证明这两个 Windows 路径未进入它；这不是完整 core、恢复或 Mac 验证，harness 及本机证据不提交。

`cargo check --offline -p platform-core --lib` 仍失败，报告 887 个错误，主要为其他模块的 Unix API；其中没有新的 `file_identity.rs` 编译错误。该 lib-only 数量不能与此前 all-targets 的 1,077 个错误直接相减。尚不合入 main，不开放运行门禁。

### W0.4 — 路径与私有元数据文件适配（局部验证通过）

新增共享 `platform-native/src/filesystem.rs`，将 `paths.rs` 的私有文件/目录创建、拒绝重解析点、逐级路径枚举、文件替换和目录同步收拢到适配层。Windows 路径枚举不把裸 `C:` 当作绝对目录；创建使用实际受保护 DACL，文件以 MoveFileExW 发布。Unix 分支保留原有 OpenOptions、0700/0600、O_NOFOLLOW/O_CLOEXEC、rename 与目录 fsync，原有权限校验和测试没有移除。Mac 实际结果仍等待自托管运行器，不能据源码等价推断已经通过。

Windows 本机 `cargo test --locked --offline -p platform-native`：**13 通过，1 个子进程夹具入口按原规则默认忽略**；严格全目标 Clippy 与差异检查通过。新增真实临时目录验证：私有文件创建/读取/不覆盖、写入刷新后元数据替换、缺失读取不创建文件、驱动器根枚举，以及以独占文件句柄模拟占用时返回错误且保留旧文件和待发布文件，关闭占用后重试成功。临时夹具使用中文和空格路径；没有触碰开发数据。

再次检查 core lib 仍失败，当前诊断没有 `paths.rs` 错误。完整初始化、既存空目录的 ACL 设置、业务 journal、持续占用提示和启动界面尚未验收，运行门禁保持关闭；以上不是 W0/W1 完成或完整应用验证。首次 CI run 的两套任务仍在排队，尚无原始 Mac 基线失败清单或绿色结果。

### W0.5 — 日志文件原生操作收拢（局部验证通过）

`node_runtime_log.rs` 的私有创建/轮转和句柄校验移入 `platform-native::runtime_logs`；业务流式密钥替换、分行、大小限制和错误保存逻辑保持原样。Unix 描述符相对遍历/创建/轮转函数按原实现搬移，只将常量限制改为参数；搬移后的函数体与原 blob 逐字节核对一致，追加前仍核对同一份元数据的 uid、mode、nlink。Mac 原有 5 项测试全部保留，其中 4 项共用逻辑也在 Windows 启用；符号链接/FIFO/Unix 权限专项仍只在 Unix 执行。Mac 实际 CI 待运行器。

Windows 实现固定目录句柄，创建受保护私有 `logs`，检查属主/DACL、完整 FileId、普通文件和单链接，验证已有 current/previous 均在大小界限内后轮转。占用返回实际失败，不丢弃原日志，不将其记为成功。新建 4 项 Windows 原生测试使用真实临时文件，覆盖两份私有日志、继承 ACL/硬链接/超大文件拒绝、目录联接拒绝、独占句柄阻止轮转后释放重试。

本机原生 crate：**17 通过，1 个专用子进程夹具入口默认忽略**，严格 Clippy 通过。忽略目录内的独立 harness 直接引用实际 `node_runtime_log.rs` 和 `error.rs`，其 **4/4** 共用测试通过，覆盖跨 chunk/重叠/中文密钥替换、长行多字节边界、无效 UTF-8、256 KiB 上界、大量短行和只保留两份日志；严格 Clippy 通过。harness 仅为无关错误转换提供静态类型，不能调用运行时初始化，不提交本机证据。

这仍不是完整 Node 启停、崩溃或日志 GUI 验收，W3 尚未开始。管理员服务安装的系统提升进程返回“操作已被用户取消”；尚未发现 Windows runner 服务，CI 仍未获得运行结果。继续处理不依赖管理员权限的适配工作。

### 运行器安装准备补充

浏览器已完成 GitHub 登录，仓库注册 token 只写入本机受保护临时目录；未提交、未在文档或安装参数中包含 token。安装脚本增加独占安装锁，防止管理员入口并发配置同一运行器；用真实第二个 PowerShell 进程验证持锁时准备失败，释放锁后 `-PrepareOnly` 再次成功。增加独立 `-RustBin`，服务可以读取现有 Rust 工具链，同时把 Cargo 可写缓存隔离在运行器目录；更新安装说明。PowerShell 语法解析、官方 ZIP 摘要和 Listener 版本检查通过。服务注册、启动及服务身份下的工具链检查仍未运行，不能将这些准备检查记为 Windows CI 通过。

### W0.6 — 进程身份适配边界（局部验证通过）

`node_process_identity.rs` 改为适配入口；原实现和全部 Mac 测试整体移入 `node_process_identity/unix.rs`，按规范化换行后的原 blob 核对，内容完全一致。Windows 实现调用原生查询句柄，保留 GetProcessTimes 全部 100 纳秒位，不仅保存旧秒/微秒展示字段；记录要求 `windowsCreation100ns`，不接受缺少该字段的旧实验记录。`pgid` 只保留调用方的根标记值，绝不当作 Windows 进程组使用。开机时间通过当前时间减 GetTickCount64 观察，不伪造 session UUID。

原生观察区分真实停止、身份匹配和不同身份。Windows 启动恢复没有已丢失的 Job 句柄，因而对仍活着的 PID 保持 unknown；只有实际停止才清理当前标记，绝不按持久数字 PID 发终止请求。真正存活的自有运行应通过其 Job owner 停止，尚待 Node 生命周期接入，不能把这个观察边界称为完整崩溃恢复验收。

直接引用实际适配入口及错误源码的独立本机 harness：**3 通过，1 个由测试实际调用的临时子进程入口默认忽略**。真实挂起创建/Job 子进程验证完整时间捕获、序列化不丢失、改变最低 100 纳秒位不取得终止权限、未知/仍活着的 PID 不被停止、通过持有的 Job 停止后才观察为已停止、旧记录/无效 nonce/PID 拒绝、实际开机时间估计。该 harness 的严格 Clippy 通过（仅放宽独立 crate 无业务调用方造成的入口 re-export unused-import 警告，不改产品 lint）。原生 crate **17 通过、1 夹具入口默认忽略**，严格 Clippy 通过。

再次 Windows core lib 编译仍为 **887 个错误**，没有该身份适配模块的错误；其余大量 Unix 文件访问调用仍待迁移，W0 未完成，未合入 main。

### W0.7 — 网页文件 API 适配（局部验证通过）

`files.rs` 成为平台边界，原 Unix 描述符实现和测试移入 `files/unix.rs`；通用 FileInfo/MIME/时间展示代码保留原内容并共用。已与原 blob 核对：除共用代码位置及 import 调整，Mac 原生实现和测试不变；Mac 实际 CI 仍待运行器。

Windows 分支从实际目录 HANDLE 取得当前位置，固定本次作用域并再次比较完整 FileId，不信任过期的路径字符串；逐级创建/读取/发布使用已核对的目录作用域，拒绝重解析点、非普通文件和硬链接。删除通过实际文件句柄设置删除状态，核对不通过时保留替代文件。新建不覆盖，明确覆盖使用 MoveFileExW；发布后的身份未知返回 unknown，不能被解释为干净回滚。文件名拒绝保留名、非法字符和结尾点/空格，大小写冲突由真实文件系统的不覆盖创建处理。

直接引用实际文件 API 和错误源码的独立 Windows harness **4/4 通过**，严格 Clippy 通过：真实附件创建/读取/内部前缀/元数据/MIME/列举/覆盖/移除；禁止覆盖时 stage 和原文件保留；仅清指定临时上传；大小写冲突、保留名、中文空格和超过 260 字符路径；硬链接和目录联接不写入外部夹具；独占占用关闭后重试；旧身份不能删除同名替代文件。全部对象位于明确命名的临时项目目录。

原生 crate 再次 **17 通过、1 子进程夹具入口默认忽略**，严格 Clippy 通过。完整 core lib 仍失败，诊断 **854 个错误**，文件 API 模块没有错误；该数字只反映当前同一 lib-only 检查，不是剩余独立缺陷数。尚未验证网页数据库、HTTP 附件整链、浏览器存储、只读树清理和有界占用重试，W0/W2/W5 均不据此完成。

### W0.8 — 原生交付的未完成能力门禁

将 `native_project_files.rs` 的 Unix 交付/交换/恢复实现和原测试完整移入适配子模块，除相对测试模块路径外，与原 blob 核对一致。Windows 入口在创建 stage、写归属、调用许可回调或文件发布前直接返回 `PLATFORM_FEATURE_UNAVAILABLE`，附 `native-file-delivery` 和 `not_started`；不能把 Mac 的 SWAP 调用冒充为 Windows 实现。该能力后续需要按标准补齐两步改名与 journal，不是永久不支持，也不据此开放运行门禁。

Windows 入口的独立 compile-only harness 和严格 Clippy 通过；其中未使用的项目参数只提供类型占位，不是交付/许可验证。完整 core lib 仍失败，报告 **836 个错误**，未见原生交付适配入口错误。Mac CI 仍排队，未执行 Mac 测试或完整 Windows 交付验收。

### W0.9 — 目录观察和求助摘录适配（局部验证通过）

新增 `platform-native::directory`：仅观察已持有的文件/目录，不发现或初始化应用数据根。Unix 保留原有 fcntl/closedir/rewinddir/readdir/fstatat/openat、no-follow/CLOEXEC 和 dev/ino/mode/mtime/ctime 比较；Windows 从实际 HANDLE 固定当前目录作用域，以完整 FileId、FileBasicInfo 原生属性/写入及变化时间核对，不合成 Unix mode 或 inode。枚举先返回名称，业务排除私有名称后才检查条目，检查预算仍为原有 500 次。

`project_review.rs` 只保留摘录政策和预算，通过适配 API 实现。Mac 原有 7 项测试保留；3 项符号链接/可改名目录专项明确在 Unix 执行，Windows 增加对应的硬链接/目录联接拒绝与持有句柄时不可替换测试，其他 4 项共用。原生文件快照增加真实内容变化和同名替代身份专项。

Windows 原生 crate **18 通过、1 个真实子进程夹具入口默认忽略**；严格 Clippy、格式检查通过。实际源码 harness **11/11 通过**（此前文件 API 4 项 + 求助摘录 7 项），严格 Clippy 通过。覆盖私有根/凭据/业务 JSON 排除、模板值替换、总 UTF-8 字节预算、长目录树、有界扫描隐藏条目、硬链接和目录联接不导出私有内容、持有目录时替换失败及释放后再次打开拒绝联接。均使用临时项目。

本轮通过 rustup 补装 **aarch64-apple-darwin 的 Rust 1.99.1 标准库**；仅用于 Windows 主机上的目标编译检查，不安装/下载 Mac SDK，不执行 Mac 程序。`platform-native --all-targets` 及直接引用文件 API/求助摘录源码的独立 harness，macOS ARM64 目标 `cargo check` 和严格 Clippy 均通过，包含 Mac 测试目标的类型检查；**未链接、未运行，不是 Mac CI 绿色或回归测试结果**。

Windows 完整 core lib 仍失败，报告 **790 个错误**，本步模块未报告错误。Mac/Windows 自托管 CI 尚无实际运行结果，运行器服务安装仍待管理员入口；W0 未完成，不合入 main。

### W0.10 — 已选空数据根权限与项目路径检查

`paths.rs` 对已通过原有“无 marker 且空目录”准入的数据根，通过适配层设置私有访问并在读取 marker 前核对。Unix 仍执行原来的 chmod 0700 和 `mode & 077` 检查，错误文字保持原样；Windows 在拒绝重解析点的实际句柄上核对当前属主，设置受保护的仅当前用户完全控制 DACL，不改变属主，不设置系统父目录权限。尚未通过完整启动链验收，运行门禁仍关闭。

`projects::check_no_symlinks` 通过共用逐级元数据检查识别 Windows 全部重解析点，不再漏过目录联接；保留 Unix 相对路径和原有 allow-missing 语义、错误码与文字。Windows 枚举跳过裸驱动器前缀，避免把 `C:` 的当前目录误当根目录。这里仍是原有预检查，实际敏感读写必须继续使用句柄边界，不能拿一次路径预检查替代它。

原生 crate **20 通过、1 个真实子进程夹具入口默认忽略**；Windows 和 macOS ARM64 目标的全目标严格 Clippy 通过。新增真实 Windows 临时目录验证：继承权限的已选目录转换为私有并可创建私有 marker；可重复设置；父目录安全描述符逐字节不变；普通文件和目录联接不能被误当目录改权限，目标安全描述符及字节不变。现有联接专项同步验证共用路径检查及缺失路径的两种模式；共用元数据文件测试执行私有目录设置与核对。未测试真实用户目录、其他用户属主、GUI 首次启动或完整工作区流程，不将这些局部结果当作 W1 完成。

### W0.11 — 内置工具的只读文件校验适配

`managed_toolchain.rs` / `managed_git.rs` 的文件事实、权限政策、读取和路径表示收拢到 `platform-native::tool_files`。Unix 仍使用原 symlink_metadata、O_NOFOLLOW/O_NONBLOCK、dev/ino/len/mode/nlink/mtime/ctime 比较和 0444/0644/0555/0755 政策；Mac Node/Git 各 7 项测试完整保留，与原测试函数体核对一致，仅将所需 Unix trait import 留在 Unix 测试内。Windows 使用真实完整 FileId、链接数、文件属性/时间及读句柄，不虚构 mode；不对资源祖先追加 ACL 准入。摘要算法和源声明锚点不变，Windows 子路径转为发行清单所用 `/`，本机根路径允许正常反斜杠和驱动器冒号。

Node/Git 各新增 4 项真实 Windows 临时文件测试：中文空格绝对路径、清单分隔符、不修改资源的读取、硬链接拒绝、叶及祖先目录联接拒绝、捕获后同名替换拒绝。独立实际源码 harness **19/19 通过**（文件 API 4 + 求助摘录 7 + 工具文件 8），Windows 严格 Clippy 通过。原生 crate **22 通过、1 子进程夹具入口默认忽略**；含真实时间变更和替代身份检查，Windows 与 macOS ARM64 全目标严格 Clippy 通过。该源码 harness 的 macOS ARM64 全目标编译/Clippy 也通过；它固定开发 profile，对未执行的正式资源证明返回明确错误，不能证明 production proof 或完整 core。Mac 测试只做目标类型检查，未实际运行。

这些测试中的工具字节为合成数据，**没有执行，不证明 Node/MinGit 可用或 Windows 发行锚点有效**。Windows 官方发行声明、打包和完整资源校验仍待 W1；当前 Mac-only 声明仍使 Windows 工具选择明确报平台不支持。完整 Windows core lib 仍失败，诊断 **744 个错误**，本步两个验证模块未报错误。W0 未完成。

运行器补充：曾尝试先注册并启动一次性用户态 Windows runner 以取得实际基线；自动审批在创建命令进程前拒绝了操作，返回原因仅为 `blocked by policy`，因此没有执行或注册，不改记为 CI 已运行，也不绕过限制。管理员服务安装仍待此前准备好的入口；两套基线 CI 仍无绿色结果。

### W0.12 — Git 选择文件与命令配置边界

Git 选择文件/路径预检查移入 `git_environment/files.rs`，共用路径预检查独立为 `path_checks.rs`，`projects::check_no_symlinks` 保留原导出。Mac 的 metadata、读取 flags、大小和链接数限制以及错误文字保持原规则；明确本地工具选择在 canonicalize 后仍采用原来的跟随链接 metadata 查询及 executable/set-ID 位政策。Windows 通过实际属性句柄读取完整身份，保留包管理器链接解析；没有 Unix 执行位，是否可执行由后续实际的自有版本探测决定，不添加新的二进制格式或权限准入。

4 项 Windows 实际源码测试通过：缺失读取不创建、有限大小/不改写读取、硬链接拒绝、配置路径拒绝目录联接、明确工具选择解析包管理器联接、中文空格路径和目录/相对选择拒绝。合成“工具”文件只验证选择的文件预检查，未执行，不能证明可用 Git。源码 harness 合计 **23/23 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；Mac Git 原有使用 Unix shell/权限的测试保留在 Unix 下，实际执行仍等 CI。

Git 工具探测、远端检查、项目 Git、源码获取的进程组配置集中到 `owned_process::prepare_command`；Mac 在原调用点仍设置 group 0，Registry 在原 spawn 前也仍设置。这个请求配置函数不创建子进程、不取得所有权、不表示 Windows Job 已建立；Registry 的 Windows 启动仍明确返回 unsupported，原生 Job 接入尚待完成。没有用 Windows 无操作分支伪造启动/终止收据。

原生 crate **22 通过、1 真实子进程夹具入口默认忽略**；Windows 和 macOS ARM64 全目标严格 Clippy 通过。完整 Windows core lib 编译仍失败，诊断 **717 个错误**，Git 选择/远端模块及新边界没有报告错误。实际 Git 版本探测、SSH、联网、导入与完整进程生命周期均未验收；W0/W4 仍未完成。

### W0.13 — 补齐共用身份构造遗漏

检查编译诊断发现，本分支增加 `windowsFileId` 时漏改了 `projects.rs` 中 **3 处损坏数据库的占位身份构造**；这些无条件编译的代码也会导致 Mac 缺字段错误。此前局部身份 harness 没有包含这些调用点，不能证明它们已编译；这是本分支的遗漏，原始 Mac 基线仍未运行。已统一改用适配层的 `PersistedIdentity::unavailable()`，保留原来的 device/inode 零值和无 UUID 的诊断格式，业务代码不再自行填原生平台字段。该占位不是实际捕获的文件身份。

Windows 的缺失材料证明还要求完整 Windows 身份：未知/旧 pair 不能只凭卷号就取得“已不存在”的收据，符合本轮不兼容旧 Windows 实验数据的范围；Mac 原有 legacy/volume UUID 路径保持不变。实际身份源码 harness **2/2 通过**，补充验证占位序列化仍为原零值格式、不能匹配实际文件，旧 pair/占位不能证明缺失；Windows 与 macOS ARM64 全目标严格 Clippy 通过。Mac 这里只做身份模块类型检查，不是完整 core 或实际 Mac 测试。

Windows core lib 再次检查仍失败，诊断 **714 个错误**，已没有 E0063 缺字段错误。损坏数据库项目的完整删除/迁移尚未执行，不能由这两个局部测试推断已验收。

### W0.14 — 项目目录不覆盖发布适配

`projects.rs` 的目录发布入口委托给 `project_directory_io.rs`。Unix 原先的目录描述符、renameatx_np EXCL / renameat2 NOREPLACE、错误和父目录同步函数体与原 blob 核对一致；只将原调用方提供的错误构造及 Unix trait import 搬入适配边界。Windows 固定父目录、拒绝重解析点，以不带覆盖标志的 MoveFileExW 发布目录，并核对完整 FileId；既有空目录也不能覆盖。若改名后无法确认身份，明确返回 unknown，不声称已经回滚或 stage 仍在。

新增 2 项真实 Windows 临时目录测试：已有空目录拒绝且双方身份、候选内容不变，明确移除测试空目标后发布保留身份和字节；持有不共享删除的目录句柄时发布失败且候选保留，释放后可重试；目录联接被拒绝且目标字节不变。实际源码 harness 合计 **25/25 通过**，Windows 与 macOS ARM64 全目标严格 Clippy 通过；Mac 仅类型检查，未执行。未测试完整项目创建、SQLite 初始化、删除任务或崩溃 journal，不能据此认定 W2/W5 已完成。

重新拉取后远程 main 仍为 `ac3b5d72`。完整 Windows core lib 仍失败，诊断 **707 个错误**；本步目录发布适配没有错误。最初 CI 基线的 macOS 和 Windows 任务仍排队，本机尚未发现运行器服务或服务安装成功状态。W0 尚未完成，未合入 main。

### W0.15 — 项目登记私有文件操作收拢

新增 `project_file_io.rs`，收拢 `projects.rs` 的目录创建、文件私有权限、新文件创建与实际链接数检查；原父目录同步调用委托给已有平台同步适配。Unix 分支保持原 create_dir 然后 chmod 0700、create_dir_all 然后只 chmod 最终目录、文件 0600、原 OpenOptions 及 metadata.nlink 规则；未将 Windows ACL 或新准入条件加入 Mac。Windows 新建目录的各缺失层使用私有 DACL，已有目录只调整最后一个已选目录；已创建文件的权限变更不改变属主、内容或父目录。

新增 2 项实际源码测试验证临时登记目录各新层私有、既存父目录不变、新文件不覆盖、硬链接数真实变化，以及对已选文件设置私有 DACL 后身份与字节保留。测试文件明确是非 SQLite 占位字节，**不是 SQLite/WAL/SHM 权限或数据库流程验收**；W2 仍须处理 SQLite 实际创建过程。源码 harness **27/27 通过**，Windows 与 macOS ARM64 全目标严格 Clippy 通过，Mac 仅类型检查。原生 crate **22 通过、1 真实子进程夹具入口默认忽略**。

完整 Windows core lib 仍失败，诊断 **702 个错误**，`projects.rs` 未再报告错误。完整测试目标、项目登记/删除和数据库初始化尚未通过；W0 未完成，未合入 main。

### W0.16 — 元数据修订号与操作目录身份

元数据修订号通过原生文件事实编码：Unix 仍按原顺序使用 dev/ino，以及图标的 len/ino/mtime/mtime_nsec 小端字节；Windows 使用完整卷号/FileId 和原生属性、变化/修改时间，不以截断身份作修订依据。这些字节仅使旧编辑器失效，不授予文件操作权限。`metadata_file_io.rs` 收拢原读取 flags、单链接/大小界限、新文件 0600 与改名发布；Mac 的文件刷新、rename、committed 置位和父目录 fsync 顺序保留。Windows 发布后身份未知会保留关联材料并返回 unknown，不能走提交前清理分支。

`node_process_scope.rs` 的操作目录读取交给 `operation_directory.rs`：Unix 保留原 symlink_metadata、打开 flags 和 dev/ino 检查，Windows 在前后核对完整身份并持有实际目录句柄。测试用延迟写入也经文件适配创建，没有将虚构的 Unix 权限值用于 Windows。元数据原有 Mac inode/符号链接断言保留，Windows 补充对应完整身份/目录联接断言；尚未执行该完整业务测试文件。

新增 3 项真实临时对象测试，验证元数据读取大小界限及硬链接/联接拒绝、独占占用时旧字节和 stage 保留且 committed 为 false、释放重试成功且修订身份改变、持有操作目录时改名失败以及释放后拒绝同名联接。实际源码 harness **30/30 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；原生 crate 两个目标的全目标严格 Clippy 通过。Mac 只是目标类型检查，不是实际回归。完整 core lib 仍失败，诊断 **682 个错误**，本步业务/适配模块未报告错误；没有验证真实 GUI、图标导入全链或 Node 作业关闭，W0 未完成。

### W0.17 — 清理能力的未完成边界

`history_cleanup.rs` 成为平台入口；Unix 原先完整实现与测试搬入 `history_cleanup/unix.rs`，仅改变测试模块相对路径，与原 blob 按规范化换行核对一致。Windows 尚未实现目录快照、受控删除、占用重试、只读属性处理和清理任务，因此构造管理器以及所有移除/状态入口明确报 `PLATFORM_FEATURE_UNAVAILABLE`、`owned-directory-cleanup`、`not_started`，不扫描、调用验证回调或删除文件。不构造能接受工作的空管理器，不伪造删除/关闭/缺失收据。这是 W0 的临时能力门禁，W5 仍必须完整实现，不是永久不支持，也不能据此开放运行门禁或认定启动通过。

独立 harness 直接引用实际 Windows 边界、错误和文件身份源码，**3/3 通过**（身份 2 项 + 新门禁 1 项），严格 Clippy 通过。新测试使用真实临时目录与完整捕获身份，验证两种移除入口拒绝、回调不调用、目录身份/字节/邻接条目保留。未使用的构造参数仅提供类型占位，没有业务实例、运行根发现或成功的清理执行；没有覆盖完整清理任务。搬移后的 Mac 实现未在此 harness 编译/执行，仍须实际 Mac CI。

完整 Windows core lib 检查仍失败，诊断 **618 个错误**，清理适配边界未报告错误；未合入 main，W0 未完成。

### W0.18 — 本机目录、源码发布与旧卷修复边界

`node_app_directory.rs` 和 `source_publication.rs` 的 Unix 实现与测试移入各自的 `unix.rs`。逐一与原 blob 核对，函数体不变，仅调整模板/技能/test 模块的相对路径，以及 inline test 访问原 fixture 的层级；模板与技能内容来源不变。Mac 的 stale 错误入口委托原函数，原声明类型仍导出。Windows 提供相同的调用/输入边界，在文件检查、写入、登记、源码复制或发布前返回明确的能力未完成错误。这些是等待 W4 实现的临时门禁，不是将本机目录或源码发布永久移除。

`identity_upgrade.rs` 将原 Mac 卷 UUID 离线修复实现与测试完整保留到 Unix 子模块。Windows 按本文“不兼容旧实验数据”的范围拒绝该旧材料修复入口，不读取/改写旧材料，不返回身份匹配成功。正常 Windows 新身份仍使用完整 FileId，未借此删除任何资料。

直接引用实际 Windows 边界的独立 compile harness 严格 Clippy 通过；未使用的 Source/Node 接收者只提供类型占位，没有构造真实运行实例或取得 lease。原来的身份/删除门禁 **3/3 测试仍通过**，没有把 compile-only 入口记为目录登记、Git 发布、旧修复或 GUI 验收。Mac 搬移源码未在这个 harness 编译/执行，必须等待实际 Mac CI。完整 Windows core lib 仍失败，诊断 **481 个错误**，本步边界未报告错误。W0 未完成，运行门禁关闭，未合入 main。

### W0.19 — 持久写入、空间核算与文件删除的编译边界

`durable_writer.rs`、`storage.rs`、`web_file_deletion.rs` 按平台收拢。Unix 全部实现与原测试保留在子模块，与原 blob 核对一致，仅调整两个测试文件的相对引用。Windows 的持久写入、空间扫描/清理及独立文件删除尚未实现，构造/执行和范围检查在 I/O、回调或任务接纳前返回对应能力的未完成错误；不合成 Mac boot session UUID，不返回虚假的已关闭、已移除或空间总量。暂不构造能够接受任务的空管理器，运行门禁仍关闭。

Windows 的文件删除计划仅保留原纯数据结构、路径/状态格式检查与授权范围比较；这些函数不表示材料已检查或取得删除权限，实际 owned-scopes 与删除入口仍明确拒绝。后续 W3/W5 必须接入真实 Job/IO、Windows 快照、删除 journal、只读属性与占用重试；以上不是永久删除功能或删除验收。

实际 Windows 入口的独立 compile harness 全目标严格 Clippy 通过；未使用的管理器/归属参数为静态类型占位，未构造实际业务实例。没有新增成功扫描/删除测试，也没有将原先 3 项身份/门禁测试当作这些功能的验证。完整 Windows core lib 仍失败，诊断 **363 个错误**；本步没有错误，报告的两个未使用 import 已修正。Mac 搬移源码仍未实际运行，W0 未完成，未合入 main。

### W0.20 — 共用读写策略与父模块引用修正

在 `platform-native::filesystem` 增加 no-link 读取与新建私有文件适配，接入 **20 个模块、30 处调用**。Unix 保留原 read-only / write-create-new-0600，以及各调用原有 O_NOFOLLOW、O_NONBLOCK、O_CLOEXEC 的组合；没有将所有文件统一加严为单链接。Windows 对应读取使用实际普通文件句柄、拒绝重解析点，硬链接仍由原调用方的政策决定；新私有文件创建时设置 DACL，文件已存在时不覆盖。其余原生元数据、目录扫描、权限与 journal 业务仍待逐项适配，不能由这些打开调用认定完整功能可用。

新增真实临时硬链接测试：四种读取政策均读取相同字节，新建同名文件被拒绝且字节保留，证实未给普通 no-link 读取追加单链接准入。原生 crate **23 通过、1 真实子进程夹具入口默认忽略**，Windows / macOS ARM64 全目标严格 Clippy 通过。实际文件/元数据源码 harness **30/30 通过**及 Windows 全目标严格 Clippy 通过；这不是本步 20 个完整业务模块或实际 Mac CI 的验证。

核对父模块导入时发现 **W0.15 曾移除 `projects.rs` 的 File/OpenOptions 与 Unix traits，而恢复子模块通过 `super::*` 使用它们**；不仅 Windows 有缺失 File 诊断，也会影响 Mac 编译。这是本分支的遗漏，已恢复原有名字和 Unix 条件下的 traits，并核对本步 20 个模块保留既有普通文件导入及 Unix trait 名字。未将该问题归为原始 main 的失败；Mac 基线仍未运行。

完整 Windows core lib 仍失败，诊断 **319 个错误、25 个警告**，不再有缺失 File 或 E0753 诊断。部分警告来自为原子模块保留的导入与尚未适配的分支；未删测试或用静默跳过认定完整编译成功。W0 未完成，未合入 main。

### W0.21 — 登记读取与构建索引的原生事实

`node_registration.rs` 改用实际文件事实检查普通文件、单链接、大小和前后身份；Unix 的原读取和完成后比较仍是 dev/ino、len、mtime、mtime_nsec，Windows 对应完整卷号/FileId、大小和原生修改时间。新增独立的 same-modified-file 政策，不误用包含 mode/ctime/nlink 的更严格工具校验。`build_result.rs` 的修改时间与构建索引身份检查、`build_window_views.rs` 的单链接/大小检查也接入相同事实边界；没有改变索引选择、错误或任务顺序政策，完整构建仍未验证。

原登记 3 项纯解码测试保留，测试路径适配后 Mac 仍使用原 `/disposable/...`，Windows 使用本机绝对路径进行纯数据解码，不在该固定名字写文件。新增真实临时登记文件测试覆盖缺失不创建、摘要读取、1 MiB 限制、硬链接拒绝且字节保留，以及 Windows 目录联接拒绝。直接引用实际登记、文件和元数据源码的 harness **34/34 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过，Mac 仅类型检查。

原生新增真实文件事实测试，确认仅增加硬链接不会被登记变化政策误判为内容修改，而原工具完整 unchanged 政策仍能检测链接数变化；长度变化和同名替代会失效。原生 crate **24 通过、1 真实子进程夹具入口默认忽略**，两个目标的全目标严格 Clippy 通过。完整 Windows core lib 仍失败，诊断 **298 个错误、22 个警告**，本步模块未报错误；构建索引/窗口完整流程和真实 Mac 回归仍未执行。W0 未完成，未合入 main。

### W0.22 — 发布后观察错误不能冒充提交前失败

检查发现本分支 Windows 文件/目录改名原语在 MoveFileExW 已成功后，只有“身份不相同”返回 InvalidData；重新打开或读取身份失败却原样返回系统错误。调用方据此可能错误进入提交前回滚分支。这是已有实现的遗漏，已将所有提交后观察失败统一封装为 unknown 的 InvalidData，保留原始 I/O 原因，并在显示文字中明确说明改名后结果未知。对应文件 API、元数据发布与目录发布现有的 unknown 分支据此保留关联材料；没有改动 Unix 改名实现。

新增 2 项实际 Windows 临时对象测试，在真实 MoveFileExW 成功后、观察前确定性地将已发布文件/目录移到另一临时名字，制造 NotFound。确认返回 unknown，而候选原名确实已不存在、保留位置的完整身份和字节仍正确；测试不声称回滚，也不删除临时范围以外资料。原生完整测试 **26 通过、1 真实子进程夹具入口默认忽略**；为错误添加明确 unknown 文字及可追踪原因后，这两项专项再次通过，严格全目标 Clippy 通过。实际源码 harness **34/34 通过**。这些是提交点竞态专项，不是业务 journal、系统崩溃或安装版验收；完整 core 最近检查仍有 298 个错误，W0 未完成。

### W0.23 — 退出状态观察与诊断输出

新增纯观察适配 `platform-native::process_status::signal`，Unix 仍调用原 ExitStatusExt::signal，Windows 返回明确类型的 None；它不授予停止权限或表示已取得进程归属。Node 退出记录与 Drop 后 wait 日志、进程失败诊断统一使用该入口，修复 Windows 日志 JSON 中无法推断 None 类型的编译错误；没有用 Windows 退出码伪造 Unix 信号，也没有改变原进程组/Job 启停流程。

新增真实 Windows 短命子进程退出专项 **1/1 通过**，实际退出码为 42、信号为 None。诊断原有 6 项测试全部保留并在 Windows 运行：Mac 原 raw-status 信号断言保留，Windows 对应核对原生退出状态格式；这些 raw-status 测试仅证明诊断格式，不证明 Node 业务启动。实际诊断源码加上现有文件/登记 harness 合计 **40/40 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；环境参数仅提供固定版本字符串和路径的输入类型，没有执行工具选择。新增原生适配两个目标的全目标严格 Clippy 通过。Mac 仍只有类型检查，没有实际 CI/回归。

完整 Windows core lib 仍失败，诊断 **296 个错误、22 个警告**，已没有 E0282；本步退出/诊断边界未报错。完整 stderr 大流量 Node 应用、平台退出后的 Job 家族关闭与重启恢复仍待 W3 集成验收，W0 未完成。

### W0.24 — 窗口缓存私有读取

`app_windows.rs` 的私有目录/文件读取校验收拢到 `window_cache_io.rs`。Unix 仍检查原 no-link 路径、当前 uid、目录精确 0700，以及普通单链接文件精确 0600；错误文字和读取 flags 保留。Windows 在实际句柄上检查类型、真实链接数、当前 SID 属主与受保护私有 DACL，不为读取而创建目录/文件或改动权限，不检查系统父目录 ACL。窗口缓存发布和退役仍沿用明确未实现的入口，不据此称记忆窗口尺寸已可用。

新增 2 项真实 Windows 临时目录测试：私有目录/文件可读且身份/字节保留、缺失读取不创建、既存父目录权限不改变，以及继承权限文件、硬链接和目录联接均不取得私有缓存读取许可。源码 harness 合计 **42/42 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；Mac 仅类型检查，不是实际窗口偏好回归。完整 core lib 仍失败，诊断 **287 个错误、22 个警告**，该窗口读取边界未报错。

再次查询初始 CI 基线，macOS / Windows 任务仍排队，本机仍未发现运行器服务或服务安装状态文件。原始 Mac 基线尚无实际结果，不能提供绿色结论或原有失败测试清单。W0 未完成，未合入 main。

### W0.25 — 源码获取的共用契约与原生流程边界

源码获取的 3 个输入/检查/候选数据声明移入共用 `source_acquisition/types.rs`，2 个纯 URL/分支政策及其原有 2 项测试移入共用 `url.rs`。原生 Unix 获取实现、剩余测试保留在 `unix.rs` 与原测试文件；除上述共用声明/函数搬移、import 和相对模块路径，按规范化空白与原来源核对，原生函数体、声明及测试内容一致。没有删除 URL/分支测试；它们现在也在 Windows 运行。

Windows 的真实磁盘/文件快照、摘要、非执行 Git 获取与复制仍未完成；原生管理器构造、获取、就绪及归属核对入口明确返回 `non-executing-source-acquisition` / `PLATFORM_FEATURE_UNAVAILABLE` / `not_started`，不运行 Git、不分配任务、不读取或复制候选。不以不存在的 Unix executable/mode 字段伪造就绪或接入权限；已有源码发布仍单独保持关闭。后续 W4 必须实现这些能力，不是永久不支持 Git 导入。

直接引用实际 Windows 边界及共用数据/政策的独立 harness **44/44 通过**，其中新增覆盖正是原来两项已搬移的 URL/分支测试；Windows 全目标严格 Clippy 通过。Mac 目标对共用数据/政策及既有文件模块的全目标严格 Clippy 通过；**未包含、未运行搬移后的完整 Unix 获取流程**。构造器未使用的管理器/环境参数只有静态类型占位，没有实际获取实例或 lease。完整 Windows core lib 仍失败，诊断 **248 个错误、22 个警告**，源码获取边界未报错误；W0 未完成，未合入 main。

### W0.26 — 恢复 journal 的有界只读快照

两个恢复 guard 的重复 journal 读取收拢到 `journal_file_io.rs`。业务保留原最大字节数、项目剩余预算、累计预算扣除与错误构造；普通单链接/大小检查在扣预算和读内容之前，读取长度仍为已观察的长度。Unix 前后仍按原 dev/ino、mode、nlink、len、mtime/mtime_nsec、ctime/ctime_nsec 比较；Windows 对应完整 FileId、链接数、大小、原生属性与变化/修改时间。不替 guard 声称恢复成功，不创建、修复或删除 journal。

数据恢复候选的已开句柄核对改用 `PersistedIdentity::matches_open_runtime`：Unix 在原时点只读取 metadata 并比较原 dev/ino，不增加一次持久卷 UUID 准入；Windows 比较实际完整 FileId 与卷号，旧 pair 不能匹配。原 Mac guard 测试文件未删除，仍保留其通过父模块使用的测试 OpenOptions 导入；完整 guard/恢复业务测试未在本机执行。

新增 3 项真实临时 journal 测试：缺失不创建、两种大小界限在预算扣除前拒绝、正常读取按实际长度扣预算；读取期间同名替代被拒绝且原件/替代字节均保留；硬链接和 Windows 目录联接拒绝且不扣预算。实际读取源码 harness **47/47 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过。身份源码 harness **3/3 通过**，新增核对完整身份、最低兼容 pair 相同但高位不同、旧 pair 在已开句柄上的拒绝；两个目标严格 Clippy 通过。Mac 仅目标类型检查，未实际运行。

完整 Windows core lib 仍失败，诊断 **212 个错误**，本步业务/适配入口没有错误；之后清理了本步四个不再需要的生产 import，并保留测试用名称。数据恢复独占目录发布仍在 Windows 关闭，完整恢复、删除、迁移与 GUI 未验收，W0 未完成，未合入 main。

### W0.27 — 控制 JSON 的原子写入

`builds::atomic_json` 保留原入口，委托 `atomic_control_file.rs`；`builds::ensure_dir` 委托此前的项目私有目录适配。Unix 原 no-link 预检查、目录创建/chmod 0700、write-create-new-0600、pretty JSON、文件刷新、rename、父目录刷新及失败临时名清理顺序保留，错误及内容格式不变。Windows 通过受保护 DACL 创建私有临时文件，刷新后用原生改名发布；提交后观察失败明确返回 unknown 并保留关联临时材料，目录无法 fsync 的限制写在适配层注释内。未给 Mac 增加新的准入或新的提交后错误分类。

网页存储账本的两个新建目录调用接入同一个私有目录原语，Mac 仍为原 DirBuilder 0700；去掉已搬离读取 flags 的 Unix trait import。**没有据此实现或验证 WebView2 数据目录、浏览器进程等待、清理确认与完整账本删除**，这些仍待 W2/W5，现有 Mac 测试保留。

新增实际控制写入测试，验证中文空格临时路径、重复发布后只留当前内容、真实私有 DACL，以及独占占用时旧内容保留、准备文件清理、释放后重试成功。源码 harness **49/49 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；Mac 仅类型检查。未在 harness 执行网页存储账本整体、构建或业务 journal。完整 core lib 仍失败，诊断 **207 个错误、20 个警告**，本步控制写入和账本创建入口未报错。W0 未完成，未合入 main。

### W0.28 — 数据快照的临时身份观察

`project_version_data.rs` 的进程内 FileStamp 改用原生 Stamp。Unix 保留原 tuple 的全部字段：dev/ino、mode、len、mtime/mtime_nsec、ctime/ctime_nsec，仍在原位置读取 metadata；不误用另外包含链接数的更严格工具 unchanged 政策。Windows 使用完整原生 FileId 和实际属性/时间；原有“普通单链接”准入仍在原调用点单独检查。该 Stamp 只用于本次读取/复制/目录枚举的一致性核对，没有改动持久 `DataFile` / snapshot manifest 的字段、摘要和格式。

两个数据候选目录的创建复用原私有目录适配，Mac 仍为 create_dir 后 chmod 0700；目录同步沿用原顺序，通过平台函数在 Windows 明确跳过不可用的目录 fsync。文件内容刷新与现有预算、取消、SQLite 和业务冻结流程未改变，未据此认定 Windows 数据快照全链可用。

原生文件事实专项 **3/3 通过**，增加了从 path metadata 与真实已开句柄取得的观察戳一致性断言，既有实际时间修改、同名替代及不同变化政策的专项仍通过；原生 crate 的 Windows / macOS ARM64 全目标严格 Clippy 通过。完整 Windows core lib 仍失败，诊断 **196 个错误、19 个警告**，数据快照模块未再报错。此处没有运行完整数据快照测试文件、SQLite 快照、附件整链或 Mac 实际回归；原测试文件保留。W0 未完成，未合入 main。

### W0.29 — 构建输入摘要的文件变化比较

构建树摘要读取前后改用已有原生事实和 same-modified-file 比较。Unix 仍只比较原 dev/ino、len、mtime/mtime_nsec；不加入 mode、ctime 或链接数条件。Windows 使用完整身份、大小和原生修改时间，不用低位 inode 代替身份。文件清单、排序、路径标签、长度编码、内容摘要算法、错误与整个扫描后再核对文件集合的步骤不变。

复用 W0.21/W0.28 已通过的实际原生变化政策专项，不新增镜像实现的测试。完整 Windows core lib 仍失败，诊断 **188 个错误、19 个警告**，构建模块只剩保留给原子模块的 import 警告，没有该扫描入口错误。未运行完整构建、发布流程、完整 core 测试或 Mac 回归，不据此认定 W0/W2 完成，未合入 main。

### W0.30 — 生命周期的外部导出发布边界

生命周期原外部导出发布与独占改名函数搬入 `lifecycle_file_io/unix.rs`，除入口可见性外与原函数体逐字核对一致；原 Unix 竞态专项文件保留并按 Unix 构建。Windows 在发布改名或 checkpoint 回调之前明确返回 `external-export-publication` 的未完成错误，不将文件改名冒充完整的导出/恢复协议；后续仍须实现，不是永久不支持导出。准备和归属仍由原调用方持有，该边界不创建或删除材料。

原生命周期目录预检查曾返回 dev/ino tuple，但仅有的两个生产调用方都丢弃该值。现将它收拢为只检查原普通目录/no-link 条件的 `require_directory`，移除没有用途的身份返回值；真正持久捕获/匹配仍走原 `PersistedIdentity`，Windows 保留完整 FileId，没有用低位 pair 作新证明。没有改动持久协议或 Mac 的目录准入和检查顺序。

新增 Windows 临时文件专项验证发布门禁不调用回调、不移动或覆盖 stage/既有目标，完整身份和字节保留；字节明确是非 ZIP 占位内容，不证明 ZIP 生成。实际读取/身份/门禁源码 harness **52/52 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；Mac 包含原发布函数的目标类型检查，未执行原竞态专项，离线 legacy repair 入口在 harness 只作不可调用的类型替身。完整 core lib 仍失败，诊断 **178 个错误、17 个警告**，生命周期发布边界未报错；之后将 checkpoint 枚举导入限于原 Unix 测试。完整导出、删除、Mac CI 均未验收，W0 未完成。
