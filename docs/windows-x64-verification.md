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
