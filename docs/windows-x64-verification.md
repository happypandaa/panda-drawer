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


### W0.31 — npm 准备的真实文件输入与私有配置

`node_dependency_prepare.rs` 的目录、输入读取与独占写入收拢到 `dependency_file_io.rs`。Unix 原绝对路径/no-link 检查、create_dir_all 后 chmod 0700、文件新建 0600/0700、O_NOFOLLOW、文件刷新与错误政策保留；输入读取仍在原时点只比较 dev/ino、len、mtime/mtime_nsec 和 mode，不追加 ctime 或完成后的链接数条件。Windows 对应完整身份、大小、原生修改时间与真实只读权限；创建使用受保护当前用户 DACL，不以虚构执行位宣称二进制可执行。

依赖树与空间扫描通过原生重解析点观察区分目录，不将 Windows junction 当普通目录继续递归。依赖最终链接仍沿用原来的“相对目标且解析后在项目内”政策；普通文件的真实链接数由适配器读取，Mac 直接使用原已读 metadata 的 nlink。npm 参数、脚本禁用、预算、任务归属与中断收尾逻辑未改变。原 `/bin/sh` 与 Unix 链接夹具测试文件完整保留并按 Unix 构建；实际 Windows npm 运行、取消清理和导入仍未验收，Job 集成尚待完成。

新增真实 Windows 临时文件测试验证读取缺失不创建、大小界限、硬链接拒绝且内容/身份保留，以及多级私有 npm 目录、独占文件新建、已存在不覆盖、既存父目录权限不变；`.exe` 名字里的内容只是非程序字节，未执行。实际文件源码 harness **54/54 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；未在该 harness 包含完整 npm 协调器。原生完整测试 **28 通过、1 真实子进程夹具入口默认忽略**，新增实际只读属性变化与未追加 ctime/链接数规则的断言；两目标全目标严格 Clippy 通过，Mac 仅类型检查。

完整 Windows core lib 仍失败，诊断 **159 个错误、16 个警告**，本步输入适配与依赖准备模块无错误。初始 CI 基线两任务仍排队，本机未发现运行器服务；Mac 原始基线未运行，不能声称绿色或列出原始失败测试。W0 未完成，未合入 main。


### W0.32 — 构建输入的原生文件复制

`build_candidate.rs` 的文件复制收拢到 `build_source_io.rs`，保留原调用处已经观察的 metadata。Unix 原 no-link 打开、三次身份/len/mtime/mode 比较、新建时按原执行位选择 0600/0700、复制、设置同一副本权限、刷新与错误顺序保留；没有对 Mac 构建输入增加单链接限制。Windows 使用实际完整文件身份与已适配的变化政策，创建独立私有可写副本，原文件可只读或有硬链接；没有按扩展名宣称程序可执行。候选目录、清单、摘要和 npm 业务未改变。

新增两个真实 Windows 临时文件专项：只读且有硬链接的源码能复制成独立私有单链接文件，源字节保留，既有目标不覆盖；经过目录联接的来源被拒绝，不创建候选且外部字节保留。专项只复制非程序占位字节，没有运行源码、构建脚本或真实应用。实际文件源码 harness **56/56 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；Mac 仅类型检查，完整构建测试未执行。

完整 Windows core lib 仍失败，诊断 **146 个错误、16 个警告**，构建候选复制及新增适配没有错误。W0 未完成，未合入 main。


### W0.33 — 导入目录、下载目标与归档摘要观察

Git 导入的私有临时目录创建委托 `import_directory_io.rs`；Unix 保留原缺失时 create_dir_all、普通目录检查、chmod 0700 与原错误。Windows 创建缺失层为私有目录，仅调整已经选中的最终目录 DACL，不改变系统父目录。已观察链接数政策收拢至 `project_file_io.rs`：Unix 继续使用调用方原 metadata 的 nlink，Windows 使用实际 no-reparse 句柄。服务的下载目标独立文件检查与 npm 最终依赖检查复用该入口，没有改变任务、下载覆盖或删除政策。

归档摘要前后读取改用原生文件事实，Unix 仍只比较原 dev/ino、len、mtime/mtime_nsec，以及总读取长度与非符号链接；不追加 mode、ctime、链接数变化要求。SQLite 源单链接检查读取同一已开文件的真实事实；归档目录创建复用原私有目录政策。**尚未适配归档持久权限字段，完整归档仍不能在 Windows 编译/使用**；也未执行 SQLite 快照、导入或下载整链。

新增真实临时导入目录专项，验证多级私有目录及重复操作、父目录权限保留、普通文件原字节保留并报告原错误、目录联接拒绝且目标身份/权限不变。已有真实硬链接专项同时检查新的观察入口在一条/两条链接时返回实际计数。源码 harness **57/57 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；其中未包含完整服务、归档或导入管理器，Mac 仅类型检查。修正了独立 harness 不使用的中间 reexport，未用抑制警告处理。

完整 Windows core lib 仍失败，诊断 **131 个错误、16 个警告**，服务、Git 导入与新增适配无错误；归档还有 3 个持久权限错误。随后清理归档中已无使用者的 OpenOptions 和两个 Unix trait import。运行器服务仍未发现，W0 未完成，未合入 main。


### W0.34 — 迁移证明的原生只读输入

`workspace_migration_cleanup.rs` 的证明读取收拢到 `migration_metadata_io.rs`。原大小界限、已开句柄前后独立文件核对、JSON 解码及错误文字/代码保留；Unix 仍只比较 dev/ino、len、mtime/mtime_nsec、ctime/ctime_nsec，前后单链接检查仍在原时点，不加入 mode、路径重开或额外准入。Windows 比较同一实际文件句柄的完整身份、大小和原生修改/变化时间，不将读取当作迁移提交或删除凭据。

新增真实 Windows 临时证明读取专项：缺失不创建，合法 JSON 读取后字节和身份不变，硬链接拒绝，超限临时稀疏文件拒绝且大小保持；没有删除工作区或生成迁移完成回执。实际源码 harness **58/58 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；原生文件事实专项 **4/4 通过**，两个目标全目标严格 Clippy 通过。Mac 仅类型检查，完整迁移/清理协调器与其原测试文件仍未执行。

完整 Windows core lib 仍失败，诊断 **117 个错误、15 个警告**，迁移证明适配无错误；随后去掉旧调用方已经不再使用的预检查导入。再次拉取全部远程引用，main 仍为 `ac3b5d7`，没有合入本分支。W0 未完成。


### W0.35 — 已绑定清理文件名字的原生移除

`local_cleanup.rs` 继续先执行原 BoundPath 完整范围复核，之后将单文件移除委托 `cleanup_file_io.rs`。Unix 持有父目录、原父目录身份检查、fstatat、普通文件/dev/ino 检查、unlinkat、父目录刷新顺序保留；只把已验证输入改为参数，没有改变准入。Windows 固定实际父目录作用域，核对完整父目录和文件身份，使用 DELETE 句柄再次核对后移除，不按旧 PID、低位 inode 或路径字符串取得权限。

原卸载的这一步并无单链接限制，故新增原生“允许其他硬链接名字”的明确移除政策；原 `remove_file` 的独立单链接政策不变。不会为了 Windows 的方便额外拒绝 Mac 原来允许的硬链接，也不认领或删除它的其他名字。只读属性、占用重试与整个卸载协调器仍待 W5，未在本步宣称完整卸载或删除成功。

新增原生真实临时硬链接专项 **1/1 通过**：原独立文件政策拒绝并保留，明确已绑定名字的政策仅移除所选名字，另一名字及字节保留。新增业务适配专项覆盖同样的实际移除，以及叶对象替代、父目录身份不匹配均拒绝且两份字节保留。源码 harness **60/60 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；原生两个目标全目标严格 Clippy 通过。完整原生组本步未重复运行。原卸载测试文件保留并按 Unix 构建，把其依赖的 CString/OsStrExt 显式导入原测试，避免父模块搬移后的漏导入；完整卸载/Mac 测试均未执行。

完整 Windows core lib 仍失败，诊断 **102 个错误、14 个警告**，本步卸载文件适配无错误。所有移除只发生在明确命名的临时测试目录，未操作真实开发项目、数据根、原始目录或导出。W0 未完成，未合入 main。


### W0.36 — 归档的原生权限数据

归档 `FileEntry` 与权限操作收拢到 `archive_permissions.rs`。Mac 的 `mode` 仍序列化为原 0–0777 数字，缺失仍默认 0600，显式 null/非法值仍拒绝，恢复仍执行原路径 chmod 并保留原 0777 掩码；原 ZIP 权限数字及压缩选项不变。Windows 记录明确的 `mode: {"windowsReadOnly": bool}`，值来自实际 metadata，不伪造 Unix 执行位、mode 或 ACL 权限证明。Windows ZIP 数据项不写伪造的观测 Unix mode；归档清单保留真实只读状态。新副本仍以受保护 DACL 创建，再通过原生属性访问句柄调整只读位，不因此创建缺失文件。

跨系统读取当前格式时，Windows 将 Unix 文件没有任何写位映射为只读属性；Mac 对 Windows 文件使用私有 0600/0400，不凭空补执行位。该支持只新增明确 Windows 变体，不改变既有 Mac 数字格式。五处归档目录刷新接入原同步边界：Unix 原 File::open/fsync 不变，Windows 按文档跳过目录 fsync；SQLite 内容文件的实际同步保留。原备份测试文件与其余逻辑保留，仅为一个 FileEntry 夹具显式注明 Unix 权限数据。

新增共用格式专项，直接使用实际 FileEntry 验证原 JSON 字节形状、缺失默认值、null 拒绝，及数字模式/Windows 变体格式；这些只是格式检查，不证明完整归档。新增实际 Windows 私有文件只读往返专项，验证属性可设置/清除、完整身份和字节不变、私有 DACL 保留、旧数字只读模式可映射、缺失不创建。源码 harness **63/63 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；原生两目标全目标严格 Clippy 通过。Mac 仅类型检查，未执行完整归档、SQLite、ZIP 导入/导出或恢复。

完整 Windows core lib 仍失败，诊断 **99 个错误、14 个警告**，归档与权限适配无错误。初始 CI 基线 macOS/Windows 仍排队，Mac 原始基线没有结果；不得据此声称绿色。生命周期外部导出发布仍关闭，W0 未完成，未合入 main。


### W0.37 — 工作区原生卷与复制边界

工作区原生卷信息、同步目录政策、清单收集、复制与阶段日志追加收拢到 `workspace_file_io.rs`，Mac 原实现保留在 `workspace_file_io/unix.rs`。collect/copy_entries/synced_path/filesystem 四个函数与上个提交逐项按可见性/空白规范化核对一致；原 APFS/local 的正式/测试准入条件和拒绝文案搬入同一适配层，阶段日志仍按原 create+append+0600 写入并刷新。原 Entry 声明保持原字段与 JSON，改为适配层内部共享数据，没有据此生成任何 Windows mode。原 workspace 测试保留并按 Unix 构建，将其 symlink 依赖改为测试本身显式导入。

Windows 的实际 NTFS/内置磁盘与同步目录核对、迁移快照/复制/恢复和阶段日志原生处理尚未完成，入口明确返回 `workspace-native-migration` 的未完成错误，不返回虚构的本机卷类型、余量、复制收据或干净工作区。卷信息影响设置显示，必须在 W1 补齐；完整公开迁移与旧根清理必须在 W5 补齐，不是永久不支持迁移。

新增真实 Windows 临时材料门禁专项：失败时源目录完整身份/字节保留，候选不创建，既有日志不追加、缺失日志不创建；入口拒绝发生在本模块 I/O 前，未运行真实迁移。实际源码 harness **64/64 通过**及 Windows 全目标严格 Clippy 通过；Mac ARM64 严格 Clippy 首次发现独立 include 缺少显式 Unix 模块路径，补齐后全部通过。Mac 包含搬移后原生函数的目标类型检查，但未运行原工作区业务测试。

完整 Windows core lib 仍失败，诊断 **90 个错误、13 个警告**，工作区调用及新适配无错误。W0 未完成，未合入 main。


### W0.38 — 本机 Git 版本与恢复的原生事务边界

本机 Git 输入/输出声明收拢到共用 `project_git/types.rs`，包含原 serde 字段、deny_unknown_fields、大小写格式、测试用必填 expectedHead 的反序列化区别及 From 转换。原主实现搬到 `project_git/unix.rs`，在同一个 project_git 模块层级 include，以保留子模块可见性、公开类型入口、原测试名字和真实子进程夹具的精确过滤路径。其余四个实现文件仅移走共用声明，源码恢复请求的 task_kind 原方法仍留在原模块。按规定搬移、相对路径和空白规范化后，五个原实现及所有搬移声明均与上个提交逐项核对一致；未删除或改写原测试体。

Windows 本机版本、源码/数据恢复、恢复证明读取与构建 Git 准入在原生仓库观察、Job 进程、发布与恢复未完成前明确返回 `project-native-git-versions` / not_started。管理器和构建准入收据均为不可构造类型，不能接收任务或合成已停止、干净仓库、恢复成功、元数据用量。此处不是远程 Git 添加的实现，也不是永久取消 Mac 已有的 CLI 能力；构建准入须 W2 补齐，本机版本/恢复仍须后续完整实现。

新增两项共用 wire 专项，验证恢复 action 及未知字段政策、显式 null 与省略 expectedHead 的原区别；数据为纯格式占位，不代表有效摘要或恢复许可。新增 Windows 真实临时现场门禁专项，验证现有字节/完整身份保留、不创建缺失根、不生成其他文件。源码 harness **67/67 通过**及 Windows 全目标严格 Clippy 通过；Mac ARM64 对共用 DTO 及既有原生文件模块的严格 Clippy 通过，**不包含完整 Unix Git 实现的类型检查/执行**。未使用的项目、任务、进程、关闭结果和恢复状态只有静态参数/结果类型，未构造成功对象或回执。另用独立 Rust include 小型夹具实际确认原模块和测试名字的路径规则，不能代替真实 Mac 流程。

完整 Windows core lib 仍失败，诊断 **9 个错误、13 个警告**，剩余错误均在数据恢复登记的 Unix 元数据证明中。本步实际 Windows Git API/共用 DTO 无错误，Mac CI 及完整旧 Git/恢复测试尚未运行，W0 未完成，未合入 main。


### W0.39 — 数据恢复登记证明边界；Windows core lib 首次编译通过

数据恢复登记的 read_proof 与 metadata_stamp 搬入 `data_restore_proof_io/unix.rs`，原调用入口委托适配层并传入原 invalid/changed 错误函数。Unix 函数体与原 tuple 逐项比对，除两个错误函数参数、可见性与格式外一致；原 FileProof 数据、严格身份反序列化与其余登记/恢复流程未改写。没有把持久的 Unix mode 当作可随意替换的临时数字。

Windows 原生不可变证明仍未实现，入口在 I/O 或错误策略调用之前明确拒绝 `data-restore-registration-proof`，不构造虚假的 FileProof/mode。新增真实临时现场门禁专项覆盖两种 keep 策略、缺失不创建、错误策略不调用、原字节/完整身份保留；这不是完成了原生证明、恢复或删除。原登记测试文件保留，未实际运行完整恢复流程。

实际源码 harness **68/68 通过**，Windows / macOS ARM64 全目标严格 Clippy 通过；其中 FileProof 只是匹配原字段的静态返回类型，Windows 分支不能构造，Mac 仅原函数的目标类型检查。原 Unix 源码比对通过。

`cargo check --locked --offline -p platform-core --lib` 在本机 Windows **首次通过**，仍有 **79 个警告**（类型错误消除后完整报告了尚未使用的能力/导入）。这是核心库的生产 lib 检查，不包含完整 workspace、CLI、桌面、测试或安装包。尚未开放总运行门禁，Windows 运行器服务与 Mac 基线仍未完成；W0 仍未完成，未合入 main。


### W0.40 — Windows 测试目标编译与 Unix 原生夹具范围

Windows `platform-cli` 生产目标检查通过，核心库测试目标最初有 76 个错误，均来自 Unix 权限/链接、FIFO/信号、shell 进程夹具或已按 Unix 构建的 proxy 夹具被跨平台引用。将 12 个依赖这些原生后端的测试模块限制为 Unix：Node Git 导入、环境受管进程、原生交付 I/O、迁移旧根清理、生命周期、数据恢复登记、构建完整流程及 npm 日志、HTTP Git 远程/版本、proxy 关闭、可读目录映射。**这些模块的 Mac 测试体和执行条件没有删除或弱化**，Mac 上 Unix 条件为真；Windows 对应完整业务流程仍须 W1–W5 补齐，现有原生/门禁专项不能当作相同业务覆盖。

对混合测试模块只为 Unix 符号链接、执行权限、POSIX probe 等 16 个单项添加 Unix 条件，其余共用的版本政策、配置缺失、输入/恢复格式、预算和路径政策继续在 Windows 编译。没有因为 Mac 测试失败而删除或跳过 Mac 用例，原始 Mac 基线仍未实际运行。

检查还发现 W0.32 移走父模块 Unix traits 后，原构建复制测试通过 super::* 依赖的 MetadataExt/PermissionsExt 未显式导入。这是本分支的测试编译遗漏，现与 symlink 一起放在该 Unix 夹具自己的 import 中；未把它归为原始 main 的失败。修改只涉及测试编译范围、依赖导入及格式，没有改变业务实现或原测试断言。

`cargo check --locked --offline -p platform-core --tests` **通过**：lib 仍有 79 个警告，lib test 有 67 个警告。随后首次使用完整真实 platform-core 测试二进制（非静态类型 harness）运行本机 Git 门禁和数据证明门禁，两项均 **1/1 通过**；分别过滤其余 371 个库测试和 9 个集成测试，未把未运行者当作通过。没有运行完整 Windows 测试组或桌面，W0 仍未完成，未合入 main。

### W0.41 — 桌面原生边界、源码编译与首次完整本机测试结果

`project_onboarding.rs` 的目录确认改为委托 `desktop_path_io.rs`：Mac 仍在相同位置使用 symlink_metadata 的 dev/ino 和目录/no-link 判断；Windows 使用实际完整原生文件身份。确认只比较对象身份，不新增目录时间、权限或内容比较。临时目录测试实际验证内容编辑不撤销确认、同名替换会撤销、文件不是目录、缺失不创建；专项 **2/2 通过**，对应真实源码的 Windows / Mac ARM64 严格 Clippy 通过。后者只检查类型，不代表 Mac 桌面执行或 CI。

管理器窗口关闭和应用退出处理移入 `desktop_app_events.rs`；原 Mac 关闭隐藏、Dock Reopen、退出排空和 forced-exit 处理不变，Cocoa 专属事件只在该适配层编译。Windows 保留此前的非 Mac 关闭后退出分支。Mac 事件执行仍待真实 CI/回归，不用源码比较冒充运行证明。

网页下载原 Unix 正文（含 8 个测试）通过同层 include 保留，除搬移说明外逐字核对；Windows 尚未实现下载写入归属及外部发布，Requested 可见报告 PLATFORM_FEATURE_UNAVAILABLE 并拒绝，既不绑定 engine/writer，也不创建 stage 或重写 destination。未受理的 Finished 无论 success/path 一律拒绝，不收养或删除外部文件。使用真实 Tauri DownloadEvent 和 ApiError 的临时现场专项 **2/2 通过**及严格 Clippy 通过；尚未实际启动 WebView2 下载或原生对话框，这不是完成下载功能。

通过已安装的 Tauri CLI 从现有熊猫 `128x128@2x.png` 转换并纳入 Windows 所需 `icon.ico`，核对 16/24/32/48/64/256 像素条目边界；Mac PNG/ICNS 没有改动，没有引入新下载来源或应用产物。桌面测试中只适配原生夹具：Mac 原权限设置保留，Windows 创建实际私有 DACL；Windows 文件名避开不合法的问号，Mac 原文件名保持；Unix 符号链接夹具保留，其余私有目录拒绝检查在两端继续执行，Windows 链接覆盖仍由原生目录联接专项提供。

本机 `platform-cli` **实际 build 通过**，未运行平台服务。Windows 桌面生产 bin、桌面 all-targets 和完整 workspace all-targets **源码 check 通过**（core lib 仍有 79 个警告，桌面生产目标 9 个、测试目标 4 个警告）。这三次桌面相关检查均用进程内 TAURI_CONFIG 暂时移除 bundle.resources / externalBin，以诊断源码；**正常资源准备、sidecar 嵌入、桌面打包/启动仍未通过**。没有删除或复用旧 `resources/windows*`、`.windows-preparing-*` 树作为证据。

首次在隔离 HOME/USERPROFILE/APPDATA/LOCALAPPDATA/TEMP 的真实 Windows 测试进程运行完整 core 库：**184 通过、186 失败、2 ignored**，失败后 9 个集成测试未执行。失败主要来自仍关闭的运行门禁、Windows 配置目录前缀/权限与密钥后端、原目录 fsync、生成目录路径校验和待实现的能力；不能据此声称 Mac 原基线失败。没有删除或跳过这些失败用例来取得绿色。

同样隔离运行完整桌面测试二进制：**73 通过、8 失败**，本步目录身份、project_onboarding 共用检查及下载拒绝专项均通过。8 项失败实际为 codex_handoff 的绝对路径/安装判定 2 项、local_directory 的 Finder 参数夹具 1 项、native_process 的 POSIX shell 夹具 2 项、Node 生命周期夹具目录 fsync 1 项、尚关闭的窗口尺寸缓存发布 2 项。保留这些失败，后续分别补平台夹具与真实后端；没有将它们记为通过。两次完整测试均仅使用新建临时现场，没有运行开发数据重置、真实项目删除或实际平台服务。

再次 fetch 后 main 仍为 `ac3b5d72ab79e132fcedd149c73e1201a2896576`。原始 CI 基线 run 41092968343 的 Mac / Windows 作业仍排队；Windows 服务未就绪，Mac 测试尚未实际运行。W0 仍未完成，未合入 main。对外整体百分比只是七个里程碑等权、W0 暂估完成一半所得约 7%，正式验收完成仍为 0/7，不按小步数量或编译错误减少数计算完成率。

### W0.42 — Windows SDK / 模板打包 npm 执行方式

新增脚本适配 `npm-command.mjs`，只在 Windows 将 SDK 与起始模板打包改为当前 Node 加实际 npm-cli.js 的参数数组，优先有效 npm_execpath，其次 Node 旁及 PATH 中的安装。找不到可用文件时明确报错，不执行 npm.cmd 或 shell。Mac 返回原 npm 命令和原参数值，两处打包调用的其余行为保持。此处仅适配开发打包工具，不代替 W1 内置 Node 发行资源或 W3/W4 项目 npm 管理。

新增 6 项脚本测试及原 process 的 5 项回归共 **11/11 通过**，包含真实 Node 子进程逐值收到中文、空格、引号、&、美元符和反引号，缺失/目录伪装入口报错，Mac 仍选择原命令。实际安装的 npm 另经该 helper 执行 `--version`，结果 **11.9.0**。语法检查通过。未运行现有 Mac prepareDesktop，未触及旧资源目录；完整 SDK/template 打包和 Mac 实际执行仍待后续验证。既有完整 core/桌面测试失败与 CI 排队状态没有因此变为通过，W0 未完成。

### W0.43 — 生成源码归属的 Windows 路径前缀

完整 core 测试中两项生成源码归属用例，在进入完整身份绑定之前被原 Unix components 白名单拒绝了 Windows 的 canonical VerbatimDisk 前缀。将纯词法判断委托 `platform-native::filesystem::is_explicit_non_root_absolute_path`：Mac 的绝对路径、非根目录和 RootDir/Normal 判断与原源码比较一致；Windows 仅额外接纳 Prefix。不引入存在性、UTF-8、ACL、canonicalize 相等或盘型准入；网络/内置 NTFS 限制仍属于工作区层，语法通过不等于获得目录归属或删除许可。

新增原生 Windows 专项 **2/2 通过**，覆盖中文空格 canonical 缺失子路径、不创建材料、Disk/VerbatimDisk/UNC 表示，以及相对、盘根、ParentDir 拒绝。Windows / Mac ARM64 全目标严格 Clippy 通过，Mac 仅类型检查。随后在真实完整 core 二进制分别执行原 `generated_roots_are_explicit_and_identity_bound` 与 `missing_known_generated_material_on_another_volume_is_not_deleted`，均 **1/1 通过**；保留原测试体、对象替代拒绝及缺失身份拒绝断言，没有启用任何删除流程。其余 371 个库测试及 9 个集成测试分别过滤，尚未重跑完整 core，不能据此换算完整测试新通过数。W0 未完成，未合入 main。

### W0.44 — 原生桌面派发与 Node 目录观察；桌面仍有 3 项失败

`local_directory.rs` 的 Mac Finder 命令正文与错误政策保持。Windows 改为当前 SystemRoot 下的 explorer.exe 加独立路径参数，选择文件也保持一个 OsString 参数，不经 cmd、不做有损 UTF-8 转换。Explorer 会把请求转交已有 shell，退出码不可靠，因此只报告派发创建失败，不等待退出或 kill 用户的 Explorer。实际命令构造专项 **1/1 通过**，覆盖中文空格及 shell 字符、逗号和相对路径拒绝；尚未实测真实 Explorer 窗口定位、选中文件或长路径。

`codex_handoff` 原 Mac 全文及 3 个测试同层保留，Unix 命令没有在 Windows 上尝试执行。Windows 安装验证/自动打开在完成前立即返回 PLATFORM_FEATURE_UNAVAILABLE / not_started，不创建应用或启动 fallback。真实桌面二进制的两项临时现场门禁测试 **2/2 通过**；这不是完成 Windows 自动打开编辑器，也不是永久不支持，手动复制接手说明继续由现有界面提供。

`native_process` 的 finish/status/output 生产正文逐字不变，Mac 两个原命令与断言保留。Windows 固定系统 cmd 输出夹具实际写出准确字节；超时夹具精确启动当前测试 exe 的 ignored 子测试，等待临时 ready 文件后才调用真实 finish 的 kill 与 wait，验证 child 被回收。实际桌面专项 **2 通过、1 ignored 子进程夹具**；该 ignored 项实际由父测试运行，不是跳过待测行为。此证明仅涉及短本机派发子进程，不代表 Node Job 归属完成。

进一步定位更正 W0.41 对 Node 生命周期失败的初步归因：最先发生的 os error 1 来自 ordinary_path 查询孤立的 Windows VerbatimDisk 前缀，尚未到目录 fsync。现在路径遍历委托已有原生适配，并在原 symlink 拒绝位置覆盖全部重解析点。9 处明确目录同步委托 sync_directory；Windows 仍打开并核对真实目录后才跳过无等价的目录刷新，Mac File::open(dir)?.sync_all() 原样，文件刷新、Mac 已开 HANDLE 刷新及 NotFound 政策均不变。

新增真实 core 专项 **3/3 通过**：中文空格 canonical 路径与文件/目录/缺失区分，真实 JSON 与 start-intent 文件写入、不覆盖及缺失父目录不创建，真实目录联接自身/祖先拒绝且原目标完整身份/字节保留。只移除临时测试现场创建的 junction；没有运行真实项目删除。首轮并行审查时测试夹具曾误用旧类型名，编译失败后已改成真实 StartIntentOwnership，以上为修正后的实际测试结果。

在新的隔离用户目录重新运行完整桌面测试，结果 **77 通过、3 失败、1 ignored 子进程夹具**。仍失败的是 Node 生命周期 1 项（现暴露正确的 NODE_DELETION_PLATFORM_UNSUPPORTED）及窗口尺寸缓存发布 2 项（WINDOW_CACHE_UNSUPPORTED）。没有跳过或删除这 3 项取得绿色，也没有把替换平台夹具的前后总数当作相同覆盖。完整 core 尚未重跑，Mac CI 仍待执行。未启动平台服务或生成安装包，W0 未完成，未合入 main。

### W0.45 — 完整 core 重跑及 Windows 脚本组实测

在另一个新建的隔离用户目录运行当前完整 core 库，结果 **200 通过、173 失败、2 ignored**；因库失败，9 个集成测试仍未执行。相较 W0.41，新增的 3 项 Node 目录专项通过，原失败组有 13 项现在通过，未增加失败项。修复路径/目录观察后的结果包含真实输入格式及归属验证，不代表仍关闭的运行、Git、恢复、配置或删除后端完成。保持全部尚失败用例，不把过滤或门禁当作业务功能通过。

另按 CI 的同一安全单测白名单运行 12 个脚本测试文件（不包含 live/restart 测试），子进程 HOME/USERPROFILE/APPDATA/TEMP/npm cache 均隔离在新建临时目录：**154 项，97 通过、50 失败、7 skipped**。失败实际涉及 Mac app/DMG 输出路径、Unix 链接与权限夹具、Mac Node/Git 归档权限验证、Mac production preflight 及更新签名输入；也包含 Windows 盘符遍历的共用路径问题，须逐项分类修复，不能全部用平台跳过来取得绿色。Mac 原测试尚未在 Mac 运行，不能称这 50 项为原始 Mac 基线失败。W0.42 新 npm helper / process 共 11 项仍通过。

本次为补齐实际验证记录，没有改写测试或启用总运行门禁。两端原始基线 CI 仍排队，尚无可合并 main 的双绿色证据。Windows 普通源代码编译通过、Mac 目标类型检查、上述本机测试和最终安装包验收是不同证据，W0 至 W6 仍无正式完成项。

### W1 准备 — 在待合并的 W0 基础上继续，未宣称里程碑完成

按第 6 节 Mac 离线排队时继续工作的规则，再次 fetch，main 仍为 `ac3b5d72ab79e132fcedd149c73e1201a2896576` 且为当前分支祖先；从 W0 的 `42479c5` 开叠放短分支 `win/w1-startup`。W0 原分支保留，没有合入 main 或删分支。W0 仍有 Windows 功能后端及脚本测试失败，不只是缺 Mac 结果；后续逐项修复并按依赖顺序取得双 CI 绿色，才能依次快进合并。

当前并行开展真实 Windows Node 资源准备设计、窗口尺寸缓存原生事务及脚本单测平台范围修复。尚未开放总运行门禁，没有将 W1 计为正式完成，也未把未实现能力改成成功返回。两份官方 Node 24.22.0 校验文本与发行页已核对 Windows ZIP 摘要，后续必须完整验证 ZIP 后才解析、执行或记录成员摘要；现有 Mac release JSON/默认打包入口保持。

### W1.1 — 承接 W0 脚本平台夹具与混合分隔符修复

保留 22 项 Mac app/DMG 文件系统测试和 10 项 Mac 正式发行预检测试的原调用/断言，只在非 Mac 明确标注平台跳过。3 个纯打包计划/身份/参数测试与纯 JSON 验证继续跨平台，混合测试中的 CLI 参数与环境覆盖拒绝拆出继续运行。Mac 原断言未删改或减弱；Windows NSIS 及正式资源对应验收尚缺，平台跳过不提供这些证明。

更新验证测试拆开文件符号链接权限夹具、Unix FIFO 与共用的硬链接、大小预算、父路径、目录联接和真实文件替换断言。Windows 当前普通权限无法创建文件符号链接，只有 3 个对应子测试因实际 EPERM 明确 skip 并输出不构成该能力证明；其余断言继续执行。实际目录联接、硬链接、超过上限的稀疏文件、打开前/后真实同名文件替换均通过。Unix FIFO 测试在 Mac 原样执行。

拆分还暴露真实 Windows 路径问题：原 value.split(path.sep) 会漏掉 Windows 也接受的正斜杠父组件。两处生产验证脚本委托 `native-path.mjs`，Windows 同时识别两种分隔符；Mac 仍只用 `/`，反斜杠仍为普通文件名字符。保持原来只拒绝 `..`、允许 `.` 后 normalize 的规则，没有借此加严 Mac。没有开启 Windows 更新能力或修改密码学验证政策。

4 个相关测试文件在隔离 HOME/TEMP 中实际运行：**85 项，49 通过、0 失败、36 skipped**；32 项为上述 Mac 专属 I/O，1 项为 Unix FIFO，3 项为本机不允许的文件 symlink 夹具。原断言调用 AST 对比均保留，格式与生产脚本语法检查通过。尚未重跑整个脚本组，原 Node/Git 打包仍有待修复项；Mac 真实执行和两端 CI 仍待完成，W0/W1 均未正式完成。

### W1.2 — 工作区卷信息的真实只读观察

新增 Windows 原生 volume 适配，从实际已打开、拒绝重解析点并固定范围的目录 HANDLE 查询文件系统；当前用户可用空间和总空间取自系统查询，驱动器类型保留 GetDriveType 原始结果。未知类型的 local 为 null，不把 fixed 或 removable 的本机卷冒充内置设备。`workspace_file_io::filesystem` 已返回这些真实诊断信息，原 Mac statfs 分支不变。

原生专项 **3/3 通过**：本机临时中文空格目录实际报告 NTFS、fixed 驱动器及真实容量；核对完整卷/目录身份与原文件字节、目录内容未变；文件或缺失路径不当作目录、缺失不创建；未知类型不返回本机/内置证明。Windows native 全目标严格 Clippy 通过。真实 core 的工作区边界专项 **1/1 通过**：能只读观察，但迁移准入、同步目录检查、复制、阶段日志仍明确拒绝，并保留既有材料。

本步没有完成内置设备识别、云同步目录识别或任何迁移/删除，不开启相应权限。没有访问真实项目文件，只查询承载临时目录的卷。API 语义参考 [Microsoft GetVolumeInformationByHandleW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew) 与 Windows SDK 中的空间/驱动器类型 API；Mac 实际 CI、真实 USB/映射盘及安装包验收仍待完成。

### W1.3 — Windows 混合分隔符与实际祖先绑定

真实 Node 资源准备发现部分相对成员路径含 `/`，进入 Windows 扩展路径命名空间后会报无效名称。统一修在原生路径适配层：用 UTF-16 单元转换分隔符，先核对绝对路径/父组件，再构造扩展路径；不经过有损 UTF-8，不改大小写、权限或文件身份。更重要的是 no-reparse 打开和父目录固定也先使用同一规范化路径，不能只在最后 Win32 调用转换，否则原始 verbatim Path 的祖先分析可能遗漏中间目录。共用逐组件观察的 Windows 分支同样统一分隔符，Unix 分支不变。

新增真实私有文件创建/读取/改名专项验证中文空格与混合分隔符指向相同完整身份；输入父组件仍拒绝，UTF-16 未配对 surrogate 在编码中保留。目录联接原专项新增原始混合分隔符路径，叶读取和私有文件创建均拒绝，原目标未被写入。测试用原始 OsString 构造父组件路径，避免 Rust join 事先折叠父组件而产生错误的测试前提。

对受影响的整个 platform-native 重新实测：**36 通过、0 失败、1 ignored 专用子进程夹具**；包括真实 Job 子孙回收、平台宿主强制结束、管道大输出、私有 DACL、占用/长路径、日志轮转、文件发布及新增卷/路径观察。该原生夹具由相应父测试实际使用，不代表未实现业务流程被跳过。Windows 全目标严格 Clippy 通过。没有改 Mac 原生代码或声称 Mac 实际 CI 已通过，Node/网页/删除全流程仍待完成。

### W1.4 — Windows 窗口尺寸缓存发布、退役与中断恢复

原 Mac publish_cache / retire_file 搬入 `window_cache_io/macos.rs`，函数正文、错误政策和原完整 Mac 测试对比保持；read_snapshot / same_snapshot 未增加比较字段。业务入口仅委托平台适配并在加载/清理前恢复当前 owner 的事务；Mac recover 为空操作，不改变原处理路径。

Windows 实现固定真实父目录 HANDLE、完整 FileId 与私有不可变 journal，候选/记录真实写入和刷新后，使用两次不覆盖改名替代 SWAP；只在 journal 和身份/字节共同描述的确定状态续做。未知结果、损坏 journal、并发或外部同名替换保留现场，不覆盖/删除外部文件，不收养无 journal 的旧 stage。journal 的共享模式锁仅串行该事务，未增加系统父目录 ACL 准入。

审查后去掉没有 Mac 对应的 200 字节名字上限，并使退役阶段只沿用 owner 私有、实际普通 no-reparse 文件、完整身份与原可选字节规则；不额外要求 leaf DACL 或单链接。真实测试验证 >200 UTF-8 字节的合法中文文件名、bytes=None 时原本允许的大文件、缺失源不建立新 journal、只删除绑定名并保留另一硬链接、仅 leaf 权限变化仍可按身份退役。事务 journal 自身仍为私有独立文件，新增硬链接或取消保护均不能取得恢复许可。

真实完整 core 测试二进制中相关专项 **13 通过、1 ignored 专用写入子进程夹具**；包括两个改名边界真实 kill+wait 写入进程后重开恢复、每个确定边界中断、连续 12 次发布、并发排除、外部替换/字节/身份冲突、私有读取与 journal 政策。另用真实桌面二进制重跑原两项窗口缓存业务测试，均 **2/2 通过**。专项源码 harness 双目标严格 Clippy 通过；Mac 只类型检查，尚无真实 CI/运行结果。没有做断电实测。

完整桌面组在该后端初版完成时实测 **79 通过、1 失败、1 ignored 子进程夹具**，剩余失败为 Node 删除事务尚未实现；退役政策修正后的新增专项和原两项业务已再次通过，未把完整桌面组称为绿色。完整 core、正常资源打包、主界面启动与 W1 验收仍待后续，W0/W1 未正式完成。

### W1.5 — 普通用户权限的卷设备拓扑观察

在 W1.2 的真实卷事实基础上，新增只读内置性观察：从目录 HANDLE 取得卷 GUID，打开该实际卷并读取全部磁盘 extents；枚举当前 disk 接口，用实际 STORAGE_DEVICE_NUMBER 匹配 PnP devnode，读取 InLocalMachineContainer 的原生 BOOL。只有类型/长度/255 值正确且全部 backing disk 证据明确，才可能报告 true；false 与未知分开。保持卷/磁盘 HANDLE，前后复核设备号和完整 extent 映射，不缓存可能复用的盘号。

fixed、NVMe、没有 hotplug 或没有查询错误均不作为内置证明。缺失/冲突/变化保留 null 与诊断；虚拟、文件后端、Storage Spaces、fabric/iSCSI 和未知拓扑未溯源时不升格为 true。普通文件系统/空间事实仍可返回。core 的诊断 JSON 已接入 internal、internalDiskNumbers、internalDiagnostic；没有因此开放迁移准入或宣称所有虚拟存储支持完成。

原生专项 **9/9 通过**：extent 多盘/重复/截断/零值/超大值、BOOL 值与类型、聚合与未知 bus、GUID 路径，以及实际普通未提升用户的 C 盘临时目录 / D 盘仓库目录只读观察。两盘实际分别绑定 disk 0 / 1，NTFS、internal=true；测试只要求返回结构/身份保持，不要求所有测试机器都为内置。Windows 全目标严格 Clippy 与 Mac ARM64 交叉 Clippy 通过，后者仅类型检查。root 另执行真实 core 工作区边界专项 **1/1 通过**，迁移/复制/阶段日志仍拒绝且保留现场。

没有修改系统 ACL、注册表或安装工具，没有读写真实项目内容。USB、Thunderbolt、网络/映射盘、物理拔盘尚未实测，不能作为第 7 节迁移验收通过。原生语义依据 [Microsoft volume extents](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_volume_get_volume_disk_extents)、[storage device number](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_get_device_number) 和 [PnP property API](https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_get_devnode_propertyw)。同步根发现与实际工作区迁移仍待后续。

### W1.6 — 固定 Windows Node 发行资源与真实打包

新增独立 Windows runtime / development release JSON、打包入口与源码构建的 `native-packager`。原 Mac 两份 JSON 和两个打包入口保持；Cargo 仅增加成员及其锁定依赖条目，没有依赖版本漂移。新 JSON 受现有 LF 属性约束。源为 [Node 24.22.0 官方发行](https://nodejs.org/en/blog/release/v24.22.0) 的 win-x64 ZIP，完整 37,733,703 字节、SHA256 `f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3`，与官方两份 SHASUMS256 文本一致。先完整校验 ZIP，再解析/核对 PE AMD64、许可证、固定 JSON 及完整 npm 树，之后才执行 Node/npm/npx。实际 node.exe 为 93,882,184 字节，开发树 1,928 文件、106,576,314 字节；完整成员摘要写入源控 JSON，不由下载后的 metadata 提供替换信任。

Windows helper 使用实际私有文件、完整 FileId、父目录范围和 Job/pipes。审查修复了清理前必须绑定全部已创建目录、双 reader 收口、UTF-16/UNC 名字、接收进程 close 等待及尾部分隔符重叠检查。另发现路径改名存在源目录同名替换窗口，现新增只作用于 Windows 的 `rename_directory_noreplace_expected`：打开 DELETE HANDLE 并验证期望完整身份后，以 FileRenameInfo 改名；不把当前路径重新认领为原 stage，不覆盖已有目标。真实测试在打开后换走原目录并放入外部同名目录，确认只发布已绑定原对象、保留替代目录；打开前替换拒绝、空目标占位保留、改名后观察丢失仍返回 unknown 并保留现场。

最终专项实际结果：native 全组 **44 通过、1 ignored 专用子进程夹具**；packager **6/6**；重建 helper 后 Windows 两组 JS **8/8、无 skip**。JS 真实核验固定 ZIP、runtime/development 准备、中文空格路径迁移、Node/npm/npx 版本、离线 `npm ci --omit=dev --ignore-scripts`，并确认测试 install 脚本没有执行；损坏、占用、硬链接、目录联接及未知成员均拒绝并保留调用方材料。默认源码构建 helper 和独立新缓存的真实联网下载分支也通过。native/packager 的 Windows 与 Mac ARM64 严格 Clippy 通过；Mac 只是交叉类型检查。

所有资源与下载仅写到新建缓存/临时现场，没有重用或删除既有 resources、安装资源、runner 或用户项目；不提交下载、应用或本地证据。本步还未接入正常 desktop prepare、运行时路径和资源锚点，未启动主界面，不代表 W1 或安装包验收通过。两端 CI 仍排队，W0 未完成。

### W1.7 — core 的 Windows Node 锚点、布局与只读验证

新增 `node_tool_layout` 适配层：Windows 分别选择固定 runtime/development JSON、win32/x64 名称、node.exe 与 node_modules/npm/bin/npm-cli.js；Mac 保持原 JSON、darwin/arm64、bin/node 与 lib 路径。`managed_toolchain` 仍执行原完整树、字节一致 metadata、预算、无链接与前后身份/变化核对，只委托平台布局。`node_apps` 的运行摘要选择 Windows 固定 node.exe 摘要，资源模块提供执行引擎路径 API。运行环境报告不再显示不存在的 Windows bin/node；Windows 本地 npm 候选为实际 JS 入口，Mac 原四项候选逐值保持。

真实完整 core 库二进制重新构建后，新增 Windows verifier **3/3**、布局 **1/1** 通过。固定源控声明解析成功；临时合成字节从未执行，用于验证真实文件树、中文路径迁移、只读文件仍可读、篡改/缺失/未知成员/metadata 拒绝且不修复调用方材料。实际官方 Node/npm 可执行验证见 W1.6，不能把合成 verifier 测试称作再次运行发行二进制。

Windows 完整 core 编译通过（测试目标仍有 63 项既有/阶段警告）。Mac adapter 单模块实际编译为 ARM64 metadata 通过；尝试整个 core Mac 交叉 check，先补取 Cargo.lock 已锁定的目标源码，随后在 libsqlite3-sys 找不到 Mac C 编译器 `cc` 处停止，尚未检查完整 core。这是 Windows 主机的交叉工具限制，不能当作 Mac 基线测试失败或通过；没有安装 Mac SDK、伪造原生库或减少 Mac 测试。正常桌面资源准备、受管 Job 业务调用及主界面启动仍未完成，运行总门禁保持关闭。

### W1.8 — 启动范围描述符与真实实例协调状态

核对 Mac 构造正文后，Windows WriterOwner 仅按原平台/Node/Web ID 与绝对路径组件规则保存真实范围描述符；此构造不查文件、不创建 marker、不授予写入权限。Windows 自然接受的两种分隔符在 UTF-16 层先统一后分析，避免原始 verbatim 名字将 `/../` 藏在 Normal 组件中；保留未配对 surrogate，不额外拒根/UNC，不引入 ACL 或存在性准入。begin、owner/all clear 和测试 marker 仍明确拒绝，WriterGuard 仍不可构造，不以 descriptor 冒充进程闭合。

StorageManager / CleanupManager 与 Mac 相同，构造只保留实际 paths/store/tasks/maintenance/instance 上下文，创建本实例真实 stop、runner、reader、latest 与删除标记状态。latest_task 查询实例自己的实际 Option 和任务管理器，不再固定报错；None 仅表示该实例没有已提交任务，不证明磁盘没有旧 journal。shutdown/quiesce 真正 join 自己登记的 runner 并等待 reader；失败仍报告 SHUTDOWN_JOIN_FAILED、resource_closed=false。所有未完成扫描、清理、forget 和目录移除仍在接纳前拒绝；finish_deleted_project 只退役内存标记，不代替磁盘成功收据。没有对构造增加整个数据目录必须为空的条件。

真实 core 重新编译后 WriterOwner **4/4**、Storage coordinator **2/2**、Cleanup **3/3** 通过；后者包含原临时目录删除拒绝测试。真实 Tokio worker/reader 未释放时 shutdown 确实等待，故意失败 worker 的错误被实际 join 捕获。Writer 临时现场确认未创建缺失根、保留原文件。独立 coordinator harness 严格 Clippy 通过；首轮真实接线曾因 Rust path 子模块定位错误失败，已改显式路径并经真实 core check/test 验证通过。原 Unix 实现及测试正文未改，没有启动服务、认领旧现场或开放总门禁；主界面和完整 core 回归仍待后续。

### W1.9 — 管理器 WebView2 目录、运行时查询及 Windows origin

新增桌面 `desktop_webview_profile` 适配层。取得真实 RuntimePaths 和实例锁后，Windows 初始化固定 data-root/webview-manager 私有目录，持有实际目录/祖先 HANDLE 和完整身份至 app state 释放；创建管理器前复核身份，显式传入 `data_directory`，不回退到默认目录。已有偏好保留，不扫描或删除项目 store。Windows 原生启动准备只证明该隔离目录已初始化；没有把原 WebKit 默认清理改成成功。Mac 原 data_store_identifier、URL 判定、默认 clear-and-verify 行为及原测试正文经机械对比保留。

setup 在创建工作区/服务/WebView 前，通过锁定 Tauri/Wry 的真实 WebView2 loader 查询 Evergreen 版本；缺失/不可观察时显示原生中文安装提示，返回事件循环，关闭提示后退出。管理器 origin 适配为 Windows 默认 `http://tauri.localhost` 默认端口，保留原开发地址/无账号密码限制，其他 HTTP listener、项目地址和相似域名不能取得管理器权限。main 只委托适配；同时接上 W1.7 的执行引擎路径 API，不再写死 bin/node。

真实 desktop 测试二进制专项 **6/6 通过**：真实私有目录、偏好保留、持有期间目录/祖先替换拒绝、普通文件/目录联接拒绝且外部字节保留、origin 和缺失查询结果分类。实际 loader 只读返回 **156.0.4314.50**，没有创建 WebView。缺失分支是模拟查询结果，没有卸载本机共享 runtime，也未实测缺失机器上的原生对话框。独立专项/Windows 严格 Clippy 通过；Mac 交叉尝试因 Objective-C 依赖找不到 Mac `cc` 停止，未提供 Mac 执行证明。

真实 desktop 构建仍使用仅排除缺失 resources/externalBin 的诊断 TAURI_CONFIG，保留 3 项阶段 unused 警告；不代表正常资源打包、npm run dev 或 UI/安装包验收。未改项目 store 删除、native_stores 或 core webview_storage，未开启总门禁。管理器 HANDLE 的最终清理释放仍需 W5 整体流程验证。

### W1.10 — Node 删除目录的真实只读缺失/空清单

Windows NodeDeletionJournal::list 委托独立适配器：固定真实 data-root/祖先及 journal 根 HANDLE，从已绑定路径观察固定 node-deletion-tasks。真实缺失或无任何 entry 的根才返回空清单；二次观察及完整身份/修改时间发现变化时报告 unconfirmed。原始 verbatim 混合分隔符先由已打开目录取得真实路径，不能把标准 API 的错误路径 NotFound 冒充缺失。与 Mac 空根规则一致，不额外要求该空根受保护私有 DACL。

任何已有子项（包括未发布 .creating、空 UUID 容器、未知文件）、重解析点、类型/IO/身份变化均保留并明确报告未实现恢复或无法核实；不自动收养、续删、删除空壳或返回虚构任务。create/load、所有破坏性方法和卸载清理归属仍关闭。Mac 原完整实现正文机械比对保持。

真实 core 专项 **7/7**，覆盖缺失/继承权限空根、中文空格和原始混合 verbatim、检查期间新增材料、普通文件/缺失父目录、目录联接目标保留，以及实际 list 转发与其他能力仍拒绝。独立 Clippy 通过。随后重跑完整桌面：**84 通过、1 失败、1 ignored 专用子进程夹具**；同一个 Node 生命周期用例已越过 journal 构造，现实际失败在 LocalFileVault 尚未验证 Windows 密钥文件权限，注册已提交的状态仍如实报告。没有跳过或删除该失败，未称完整桌面绿色；完整 core 和主界面尚未重跑/启动。

### W1.11 — 官方 MinGit 固定发行、原生打包与真实 Git 操作

经 [Git for Windows 官方稳定发行](https://github.com/git-for-windows/git/releases/tag/v2.56.0.windows.4) 与 release API 独立核验，选择 2026-10-26 发布的 v2.56.0.windows.4（当时 latest，非 prerelease）。固定 MinGit-2.56.0.4-64-bit.zip 为 **39,846,926 字节**，SHA256 `e3bf78351fe777fba47ddee5950082f8a98bb0f02604fdc7ba3ee2cb68ffa877`。Windows 新 JSON/入口完整记录实际许可证、375 文件/96,237,338 字节文件树和执行文件清单，使用上游现有 UCRT64 布局、cmd/git.exe，不沿用旧 mingw64 假设。完整 ZIP 校验先于解析，完整树/编译声明匹配先于任何执行；保留整包，未把自带 getprocaddr32.exe 等辅助成员冒称全为 AMD64，主 Git 则核验 AMD64。

native-packager 新 Git 模块复用实际私有新建、全部目录先固定身份再清理、真实 Job 与双管道收口；探测 version/exec-path 后再次全树核验，再通过期望源 HANDLE 发布。下载 receiver 私有 partial 也改用绑定文件 HANDLE、不覆盖缓存名字；完整 size/SHA 不符仅清理自己绑定的新 partial，调用方缓存/未知结果保留。新原语没有改变 Mac 代码或已有目录原语政策。

最终实际组合 JS **14/14、0 skip**（原 Windows Node 8 + MinGit 6）；native 全组 **45 通过、1 ignored 专用子进程夹具**，packager **8/8**；Win/Mac ARM64 交叉严格 Clippy 通过，Mac 仍仅类型检查。真实 Git 2.56.0.windows.4 在中文空格迁移目录完成版本、exec-path、临时仓库 init/add/commit/switch/本地 clone；另无 --template 的默认 init，生成的 description、info/exclude、hooks/pre-commit.sample 与已验证模板逐字节相同。损坏包、占用、junction/hardlink、尾部分隔符重叠及错流 partial 清理均实际通过。

正式固定 URL 经 GitHub 官方 release-assets 重定向的联网下载/receiver/prepare 入口也通过；首次 TLS 建立前 ECONNRESET 没有生成缓存/输出，随后在全新目录重试成功。没有将本地 clone 冒充 GitHub/Gitee 远程导入，没有操作真实仓库或项目、旧 resources 或 runner。未改 Mac 三份原 release JSON/打包默认入口；尚未接正常 desktop prepare 或 core Git 锚点，不代表 W1/W4/W6 验收完成。

### W1.12 — Windows 安装资源与复制后 sidecar 的定位

`resource_info` 经 desktop_installation 适配层委托。Mac 原完整函数正文、错误与 debug fallback 逐字保留；Windows 仅使用 Tauri resource_dir/resources 和当前桌面 exe 旁的 local-projects-dev.exe，不选旧源码 resources、Mac 无扩展名或带 target triple 的打包源 sidecar。资源缺失时保留原布尔诊断/None，真正准入仍由 core 验证；不从路径存在性生成完整性证明，没有新增任意根覆盖或旧数据兼容入口。

真实桌面二进制专项 **3/3**：中文路径与部署后固定文件名、缺失时不选择同目录旧名字/源目录、临时资源布尔报告与不可读文本，所有合成 sidecar 字节从未执行，调用方材料保留。构建使用资源排除诊断配置，尚未证明真实 Tauri dev/安装包的部署行为；正常 Windows fresh 资源准备仍在接入验证。未改 Mac 行为，CI 仍待实际执行。

### W1.13 — Windows 明文配置与实际本地密钥槽

配置平台代码收拢到适配文件，Mac lock/read/commit/完整 LocalFileVault helpers 的生产正文逐字保留，共用 save 与回滚业务正文未改。Windows 使用当前 SID 私有 DACL、普通 no-reparse 文件、完整 FileId 和原单链接政策；普通配置按 Mac 原规则仅修已核验当前用户 leaf，凭据既有权限不符只拒绝，不读旧 provider 兜底，不改系统父 ACL。仍是平台私有明文槽，不引入 DPAPI、系统 vault 或不同的密钥契约。

配置覆盖使用私有新文件的实际 DELETE HANDLE，所有父范围、身份、类型/链接数/权限及路径检查在原生提交前完成。FileRenameInfo 成功就是与 Mac rename 相同的提交点，之后只 best-effort 目录同步，不再把提交后的观察失败变成 save Err，避免共用回滚删掉已被新配置引用的 secret。外部资源 packager 的后验 unknown 政策没有改。LocalFileVault 真实支持不可覆盖 insert/get/contains 和只回滚本次创建项；root/entry 替换、未知清理项均保留并拒绝，不把“初始空”冒充后端完整。

作者独立完整 core 配置组 **22/22**，root 随后在真实主 target 完整 core 测试二进制再次 **22/22**；包含原 18 项业务测试及 4 项 Windows 实际文件测试。覆盖真实 save/reload/启动快照/修订覆盖、private DACL、hardlink/junction、错误 credential DACL、空/超限/UTF-8、root/entry 替换、unknown 清理，以及真实 rename 被目录阻挡时只回滚新 secret、保留旧配置和旧 secret。native 全组 **45 通过、1 ignored 专用夹具**、packager **8/8**，两目标 native 严格 Clippy 通过（Mac 仅类型检查）。复审无阻塞。

没有运行应用后端或读写真实凭据；所有配置/删除测试只用临时新现场。原桌面生命周期用例与完整桌面组尚需在此后端上再次验证，不能提前记为绿色。W3 真实配置注入/日志脱敏、W5 项目全量删除和 Mac CI 仍待完成。

### W1.14 — core MinGit 原生 schema、固定路径及真实准备产物核验

managed_git/layout 选择固定 Windows JSON、win32/x64、cmd/git.exe 与 UCRT64 libexec/templates；Windows 使用独立 distribution/license/layout schema，不补造 Mac build/signature 字段。Mac 原 source/build/signature schema、JSON 和原 7 项测试正文保持。共用 verifier 继续按原文件/树预算、普通无链接文件、实际完整身份与变化、精确 metadata 字节、目录存在及全树摘要验证；Git 环境两处纯诊断/debug 路径改为同一适配 API，没有开启 Git 进程或仓库操作。

root 真实 core 专项 **9/9**（原 Windows 文件事实 4 + 新 payload/layout 5），作者独立严格 Clippy 和 Mac ARM64 交叉 Clippy 通过，后者仅类型检查。临时合成字节从未执行，覆盖原生 schema/外来伪签名拒绝、搬迁、只读、改/缺/多/metadata/空目录及必要辅助目录。另对正常 Windows prepare 实际新产物，以临时 inspector 调用生产 `managed_git::verify`，完整 **375 文件、96,237,338 字节** 与固定锚点匹配，路径/版本正确；生产 `managed_toolchain::verify` 也核验该准备产物的 **1,928 文件** 与固定 Node/npm 锚点匹配。此处只读核验没有再次执行工具、访问仓库或项目。

W1.13 后 root 另重跑原桌面生命周期失败用例：已实际越过配置/注册权限，现到第 73 行 submit_deletion 时明确报 NODE_DELETION_PLATFORM_UNSUPPORTED（W5 的 journal 创建尚未实现）。保留该失败，没有将单个前置后端通过记为完整桌面绿色。顺便移除 Unix 适配文件末尾的一行多余空白，不改任何 Mac 函数正文。正常 Tauri 部署、主界面与两端 CI 仍待验证。

### W1.15 — 正常 Windows prepare 与真实 Tauri 调试部署

正常 prepare/desktop 入口委托 Windows 适配，每次在仓库工具目录新建资源与独立 Cargo target，不重用旧 resources、旧 stage 或已有应用。准备固定 Node/runtime、MinGit、SDK、模板、技能与 README，最后编译本次 target 的实际 CLI。当前 Node 直接执行 Tauri JS；用户参数之后、应用参数分隔符之前依次合并 resources/externalBin reset 和本次精确配置，保留原参数数组。Mac 准备正文、Tauri 命令和标准输出工作流保持。Windows 配置已选择 currentUser NSIS / Evergreen downloadBootstrapper，尚未生成或安装 NSIS。

首轮 immutable SDK 检查发现早期 Windows 检出的干净 SDK 文本仍是 CRLF。核对每个文件归一化后逐字等于 HEAD，才恢复这五个干净文件的确切 HEAD 字节；刷新 Git 索引后 SDK 无差异。没有改版本、tgz、vendor、完整性声明，也没有绕过 immutable 检查。第二次联网 Git TLS 在生成输出前失败，保留准备现场；第三次正常 `node scripts/prepare-desktop.mjs` 完整通过，包括 SDK immutable、官方 Node/npm/Git、模板 npm ci/build，以及全新 target 的实际 CLI 编译。

使用该正常准备结果执行真实 Tauri `build --debug --no-bundle` 通过：生成桌面 exe **45,647,872 字节**，已复制 sidecar **11,423,232 字节**，SHA256 `2a760fde9e86146beb268c649587a11822c4520f286e8080c71793156cb91a4f` 与本次 CLI 精确一致，实际 --help 通过。部署后的七个资源根准确；11 个 SDK/模板/技能/README 必要文件与准备源逐字相同。另调用实际生产 verifier 核验已部署 Node **1,928 文件**和 Git **375 文件 / 96,237,338 字节**，完整树及固定锚点均通过。这里没有再次执行已部署工具或访问真实项目。

Tauri CLI help 入口也通过。复审补上受控 Windows 准备模式的转发 Cargo 输出覆盖参数拒绝，防止命令行 target-dir 覆盖本次独立 target；与 Mac 标准打包同样不接纳自定义输出，Mac 原参数处理保持。参数回归 **3/3** 验证 reset/map 顺序、两种覆盖形式拒绝及 Mac 原调用，普通 `--locked` 仍传入。首次真实完整部署不再依赖排除资源的诊断配置；仍有 9 项既有/阶段编译警告。没有启动桌面应用、服务或真实数据目录，不代表 `npm run dev` 主界面、设置、W1 或第 7 节验收完成。最近两次 CI 状态查询遇到连接传输错误，不能据此更新排队/执行状态；尚无 Mac 基线或双端绿色证据，分支不合并。配置合并和 Windows 安装选项依据 [Tauri configuration files](https://v2.tauri.app/develop/configuration-files/) 与 [Windows installer](https://v2.tauri.app/distribute/windows-installer/)。

### W1.16 — Git 添加在未实现操作前明确拒绝

新增 node_git_import/capability 适配。Windows 新请求保留原 URL/分支/UUID/名称校验、去重与已有状态语义，然后在创建接纳材料、归属、任务或临时文件前返回实际 sources 能力错误。recover 第一行及非终态 retry_cleanup 取得操作范围前明确拒绝未实现恢复；已终态快照仍照常返回。记录与未知材料保留，空内存记录也不冒充已完成磁盘恢复。Mac 三处 no-op；移除新增调用与模块声明后，原入口正文逐字保持。

真实 core 库二进制纯 guard 专项作者与 root 各 **2/2**，确认原错误全部字段和 retryable 保留，恢复明确 accepted=false/not_started 且无虚构 recovered；未启动 manager/service，未证明整个导入端到端无写入。真正导入、取消、恢复和清理仍待 W4，运行总门禁未改。此步另将 W1.7 合成 Node 临时文件的测试属性清理改为恢复捕获的原始 permissions；root 实际文件访问 **4/4**、payload verifier **3/3** 通过，没有改产品权限政策或执行合成字节。

### W1.17 — 初始化前核实迁移恢复范围

Windows 在 Paths 原 runtime/migrations 目录创建前，以及共享 recover_startup 入口，检查固定 data-root/migrations 的实际 Missing/Empty；不要求整个工作区或数据根为空。保持实际目录/祖先 HANDLE、完整身份与前后观察，任意子项、重解析点、IO/身份/观察变化保留并报告未实现或 unconfirmed。settings 仍引用迁移而 journal 缺失时报告 WORKSPACE_MIGRATION_FAILED / inconsistent / unknown，不清引用、不把物理空根冒充旧源清理完成。初始检查允许 settings 真实缺失，运行恢复要求真实设置存在。

复审发现设置读取最初误用了单链接 reader，已改为允许原规则接受的硬链接；保留 RuntimePaths 原普通/no-reparse、private DACL 与 1 MiB 大小政策，没有增加系统父 ACL 或链接数准入。真实 core 观察器最终 **8/8**、Paths 检查与共享接线 **5/5**，包含私有 hardlink settings、中文混合 verbatim 路径、并发新增、junction/文件、未知记录、有效但未提交/epochs/退休材料，以及只在真实缺失时创建原目录。独立严格 Clippy 通过；所有现场均临时创建，没有读写真实工作区或迁移记录。

Mac 恢复函数正文机械搬到适配层逐字保留，初始检查 no-op、原目录创建顺序保持；Mac 运行仍待 CI。没有调用完整 initialize、服务或 UI，没有改总运行门禁；Windows 真正迁移恢复与自动删除旧根待 W5。

### W1.18 — 自托管测试入口隔离复核

只读复核当前 CI 明确选择的 JS 单测和默认完整 Rust 测试入口：core for_test/集成项目使用真实 TempDir，测试构建的默认工作区也固定在该临时 data-root 内；CLI 默认测试在非法参数校验前不调用 discover，集成只执行离线 build-info/version/help；桌面单元测试不执行 main。没有发现此集合会初始化或发现真实固定开发根，未为此删除、跳过或运行测试。新增 desktop-host 参数回归纳入两端 CI 的明确单测白名单，语法检查通过。

重要边界：Windows dirs 的 Profile/LocalAppData 使用系统 KnownFolder，CI 的 HOME/USERPROFILE/LOCALAPPDATA 环境变量不能重定向 RuntimePaths。安全依据是已审测试集合与显式临时夹具，而非这些环境值本身。正常桌面/业务 CLI 以及 live/restart/compatibility 脚本会触及真实身份路径，不在普通 CI 白名单内，不应扩大为任意脚本通配。没有增加公开数据根覆盖或迁移命令；真实应用首启仍按固定身份单独验证并保留现有材料。

### W1.19 — 构建启动恢复不重放未实现事务

BuildManager 首条平台检查只观察工作区固定 `.platform/tasks` 与 `.platform/project-state/builds`，位于原 ensure 之前；任意其他目录/项目/状态 sibling 不作为准入条件。Windows 建目录后再次核实实际 Missing/Empty，才建立本实例真实空 registry，不进入 Mac 原任务中断写回、publication recovery 或 reconcile 循环。自动旧产物清理单独返回 build-artifact-startup-cleanup / not_started，沿用 best-effort 的“启动清理待处理”报告，保留材料而不伪造成功。

真实完整 core 库二进制专项 **7/7**：两固定范围缺失/空、已有最终/非最终/未知/发布记录保留、检查间新增、真实 junction、原始混合分隔符，以及首次检查后出现实际 running task 时第二次检查拒绝、原状态和 error=None 均不变。观察器严格 Clippy 通过。Mac 原恢复语句收拢到适配层，除新函数引用参数所需的两处借用和格式调整外保持；小型 Mac ARM64 签名 harness 直接引用该适配源码与真实 TaskRecord，目标 Clippy 通过，仅类型检查，不是完整 core 或 Mac 运行证明。总运行门禁仍未改，完整 manager/service/GUI 尚未启动，真正 Windows 构建恢复/清理待 W2。

2026-10-31（Asia/Shanghai）通过已登录浏览器只读核实原始 [CI run 41092968343](https://github.com/happypandaa/panda-drawer/actions/runs/41092968343)：页面明确 macos / windows 均 Queued，创建约 7 小时。没有执行结果，不能确认 Mac 绿色或列出基线失败；尚未超过 24 小时。Windows 运行器服务本机仍未发现，没有重新尝试之前被自动审批拒绝的用户态注册。

### W1.20 — 打包脚本共享回归与原生平台夹具

只运行 CI 明确列出的安全 JS 单测，未执行 live/restart/compatibility 脚本。该次基线 **185 项：126 通过、16 失败、43 跳过**，失败均来自旧打包测试将 Mac 绝对路径、rename/chmod 和链接夹具用于 Windows。保留共享 tar/hash/锚点/成员/超限/拒绝执行业务覆盖；Windows 文件树验证改用实际 Windows verifier，目录链接使用实际 junction，host metadata 夹具从对应固定声明派生再篡改，确保失败确实命中预期检查。

完整明确白名单最终实际 **189 项：137 通过、0 失败、52 跳过**；之后两处 host metadata 夹具细化，相关两文件再实测 **25 项：13 通过、0 失败、12 跳过**，没有据此追加完整组通过数。比基线新增的 9 个跳过为：Node/Git 两项 POSIX mode 位；runtime 成功 Mac rename/权限、成功缓存发布、目录 HANDLE chmod 三项；以及本机真实创建返回 EPERM 的四个文件 symlink 子项。其 parent 共享测试、hardlink/junction/并发不覆盖/材料保留断言继续运行。没有删除或跳过共享业务测试；不以 junction 冒充已测文件符号链接。

该完整组复用已建 helper 与官方缓存，实际执行 Windows Node/npm/npx、中文空格搬迁、离线 npm ci，以及 MinGit 临时仓库提交/分支/clone，均通过；新增 runtime 逐字节 metadata、LICENSE 篡改拒绝保留及同缓存第二个全新输出验证。未改产品源码或 Mac 算法，Mac 本轮只保留原测试执行规则，未实际运行。此处是本机脚本组绿色，不能当作双端 CI、完整 Rust 或 GUI 绿色。

### W1.21 — 首启工作区卷准入及独立同步根观察

Windows Paths 经适配层在新候选创建前检查最近实际既有普通父目录，创建后检查实际工作区，再发布原 workspace marker/settings；已有选中工作区和 discover 同样只查实际卷。沿用既有真实 HANDLE/完整身份/全部 backing disks/PnP 证据，要求 local NTFS 且 internal=Some(true)；不把 fixed/NVMe/未知当内置。不新增整个已选工作区必须为空的限制，不改父 ACL，不增加公开 root override。Mac 委托 no-op，反向内联原创建循环后 initialize_at/discover 与原正文机械一致。

独立同步原语以 [CfGetSyncRootInfoByHandle](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobyhandle) 观察真实 Cloud Files 成员，并枚举 [GetCurrentSyncRoots](https://learn.microsoft.com/en-us/uwp/api/windows.storage.provider.storageprovidersyncrootmanager.getcurrentsyncroots?view=winrt-26100) 的当前/legacy 注册根；两端实际目录 HANDLE 转同一 normalized Volume GUID 命名空间后逐组件比较，注册别名/junction/mount 不靠拼写排除。无法打开/查询/枚举保留 unknown，未猜 OneDrive/Dropbox 路径。Mac 原同步政策仅用于迁移目标，因此此原语没有接到默认首启或每次启动，尚未开放迁移。

真实并行测试发现每次新建/关闭最后 MTA 会遇到 CO_E_MTA_SHUTDOWN，未串行测试掩盖或吞错；改为进程寿命内单一专属 MTA 查询线程，不改 Tauri STA/Tokio 调用线程公寓。每次查询释放 COM 对象，启动/查询/通道失败明确返回；该只读线程等待请求，不持有项目文件或写入/注册内容。static sender 在进程结束时由 OS 回收线程，不声称正常退出前显式 join/RoUninitialize。依赖仅增加 Cargo.lock 已有 windows 0.62.2 的直接引用与 API features，无版本升级或新二进制来源。

实际 core Paths 检查 **8/8**（新增卷 3 + 既有迁移 5）；完整 native **51 通过、1 个既有专用子进程夹具 ignored**；Windows 和 Mac ARM64 交叉全目标严格 Clippy 通过，Mac 仅类型检查。新增 native 六项包括实际 STA、GUID 别名、unknown、并行查询、本机 NTFS 和临时 Cloud Files 正向注册：仅对新建 disposable 目录注册，根/子目录均被同步原语拒绝，而 volume-only 仍接受；按绑定完整身份实际注销并查询 NotRegistered，测试目录已不存在。所有成功注册的历次试验（含失败的 MTA 那轮）均确认注销，无测试注册遗留，没有改变既有 provider 或读取项目内容。

普通临时目录查询真实系统注册根仅记录数量，不记录用户根路径。外置 USB、网络/映射盘和真实移动工作区尚未验收；未运行真实 initialize/GUI，不能据本步称 W1/W5 或 Windows 10 验收完成。

### W1.22 — 延后迁移清理在任何接纳写入前拒绝

启动审计发现 native startup 完成或非桌面服务启动会延后调用 selected cleanup；仅有 earlier migration inventory 不足以授权该公开/延后路径。自动路径现在经独立适配，在实际 Some journal 时先拒绝，再可能进入原提交及 retain_cleanup_reopen_failure 闭包；实际 None 保持真实无工作。公开 retry 保留原 selection/ID 授权顺序，随后在 active task、新任务、journal 变更或旧源 worker 前拒绝。未把 submit_cleanup 错误传到会写入失败记录的旧分支，Windows accepted=false / not_started，材料均保留。

真实 core 适配专项 **3/3**：真实临时 pending journal 加 task/settings/source 字节检查，自动提交与失败记录回调均未进入，None 不调用提交，retry guard 字段准确。这里直接测试生产分派适配，不构造虚假的 RuntimeSwitch/store 或声称完整服务已启动。Windows 适配和 Mac ARM64 metadata 严格 Clippy 通过；Mac 原提交/错误保留正文机械一致，未实际运行。总运行门禁尚未改，实际迁移旧根收尾仍待 W5。

### W1.23 — 开放 x64 开发启动入口，真实基础服务通过

在上述未实现自动恢复/删除路径各自接纳前拒绝、私有路径/资源/管理器 WebView2 准备完成后，开放 Windows x64 开发 runtime policy；其他 Windows 架构保持 unsupported，Mac 原准入和正式版签名/资源证明门禁不变。此纯策略不授予旧 journal 权限，也不声明 UI/功能验收。更新旧独立 policy 检查脚本：它已依赖真实 build_profile/production anchors，不能继续单文件 rustc 或伪造常量；现在直接执行完整 core 编译后的专项测试。

本机实际 `node scripts/platform-support-test.mjs` **5/5**；完整 core Paths 组 **16/16**，覆盖临时根的真实初始化、私有凭据、单实例 LockFileEx 拒绝第二实例及释放复用、设置/marker 选择、目录保留，以及新卷/迁移检查。实际原基础服务测试 **1/1**：使用 RuntimePaths 临时夹具启动真实 loopback HTTP 服务，native readiness 前阻断、显式失败保持、完成后 health 成功，真实 shutdown 返回；该测试通过调用 readiness API，不创建真实 WebView，不能据此证明 WebView2 隔离或 GUI。

没有增加任意数据根覆盖，没有让普通 CI 启动真实应用。core 测试目标仍有 63 项既有/阶段警告；完整 Rust 回归尚未在新门禁后重跑。下一步正常 `npm run dev` 实际固定身份首次启动和设置检查；W1、双端 CI及第 7 节仍未完成。

### W1.24 — 第一次真实 Windows 管理器与当前 core 回归

Windows 11 25H2 / build 26200.9711 上，实际执行 `node scripts/desktop.mjs dev`（npm dev 的同一正常入口），未传诊断资源配置或任意数据根。新准备目录完整生成资源/CLI，真实 Tauri dev 编译通过并运行，固定身份 `dev.happypandaa.pandadrawer`；主界面显示 0 应用，工作区为 `C:\Users\g\PandaDrawer-Dev`，真实 loopback 服务启动。使用 computer-use 直接观察并点击通用、存储与清理、运行环境、给 AI 的平台说明、关于与更新、卸载前清理六个分区，均实际显示。

存储页准确显示 NTFS，并明确报告 Windows 存储扫描/清理尚未完成；运行环境页可显示固定 Windows 资源位置，但 Node/npm 进程探测仍未完成，不能据打包执行成功声称平台内构建工具已可用。初次打开设置时隐藏的 Node/Git pane 也会做完整诊断，tabs/完成曾被现有 pending 规则阻断，随后实际恢复，未量测等待时长。关于页版本、x86_64、开发身份和实际 exe 路径可读，当前开发版自动更新入口禁用；尚未验证安装包身份/体积统计。没有点击卸载清理按钮、创建项目或执行任何删除。

点击底部文件管理器入口，真实 Windows Explorer 打开 `此电脑 > 系统(C:) > 用户 > g > PandaDrawer-Dev`，可见本次初始化的 projects / .platform；验证后只关闭本次新标签。通过原生菜单“退出开发版 Ctrl+Q”正常退出，实际日志 ReadyForNativeExit、dev 命令 exit=0，管理器及本次 Explorer 窗口均确认消失。保留全部真实固定身份数据，没有重置或清理根目录。

仍有 W1 收尾：界面残留“访达”/Mac 关闭说明及仅接受 `/` 的工具路径控件，需要前端适配；WebView2 必须从真实 Environment7.UserDataFolder 读回并核对预期完整身份，不能只以 builder.data_directory 输入证明实际生效。系统 environment/policy 可能覆盖该值，读回验证前应保持管理器导航和 native readiness 关闭。第一轮只证明实际窗口/六分区和 Explorer，不提前认定隔离验收完成。

同时在独立 TempDir 夹具运行当前完整 core `--lib`，注册 **443 项：345 通过、94 失败、3 忽略、1 超时**，总计 196.105 秒；没有正常 libtest 汇总，不能记为全组通过。唯一超时 `service::shutdown_coordinator::tests::migration_registration_and_shutdown_share_the_same_submission_lock` 在 entered_rx.recv 无界等待，记录精确身份后只终止该次测试进程，夹具将改为有界失败，不删/跳测试。94 失败分布：service 32、build_artifacts 13、backups 11、metadata 9、object_layout 7、source_restore_guard 4、web_project_ownership 4、webview_storage 4、node_apps 3、project_cleanup_tests 3、projects 2、environment 1、resources 1。

有界精确复现确认当前 W2 多项目创建先报 PROJECT_DIRECTORY_MAPPING_INVALID，附带 owned-directory-cleanup 错误只是回退诊断；网页 SQL/附件端到端尚未到达。原生附件 CRUD、数据库队列/结果限制、基础 HTTP health 已实际通过，旧 W0 的门禁失败不能继续用作当前缺陷结论。未改/跳过原 Mac 测试，没有 Mac 运行结果；W0/W1 尚未正式完成，不合入 main。

### W1.25 — 迁移注册测试失约时有界结束

只修 W1.24 唯一超时夹具：登记信号使用 test 内单一 30 秒上限；超时先释放该夹具的 release sender，避免失败后的阻塞 worker 阻止测试退出，再报告已结束提交的真实错误。原 Mac 成功路径锁/关闭断言和产品迁移代码保持，没有删除、跳过或将未实现功能改判通过。

唯一真实 core `--exact` 最终在 **30.13 秒**正常结束，仍为 **0 通过、1 失败、0 忽略**：INVALID_MIGRATION_TARGET 包裹 workspace-native-migration / PLATFORM_FEATURE_UNAVAILABLE / not_started。最初 5 秒上限也实测能退出，复审后改为 30 秒，给自托管 Mac 的并发/磁盘等待保留余地。没有重跑完整 core，也不将上轮 1 timeout 私自换算成一次新的全组结果；迁移业务失败仍留待 W5。

### W1.26 — 界面平台文案和原生路径提示

将平台相关展示集中到 frontend-platform 适配，以 bootstrap.platform.build.target 的真实 Rust 编译目标选择文案，不依赖 UA、路径猜测或默认 Mac。Windows 显示资源管理器、本机内置 NTFS 迁移要求、关闭主窗口退出说明，以及 Node/Git/工作区的 Windows 路径示例。Node/Git 和迁移表单接受盘符、UNC、混合分隔符与扩展路径，拒绝相对/盘符相对路径；这是词法提示，提交原始修剪后的字符串，后端保留全部权限、身份和卷核验，未开放尚未实现的迁移。

Mac 原文案及 `/` 开头规则逐字保留；metadata 尚未返回或不认识时使用中性文案。统一 React provider 让异步 bootstrap 到达后设置与工作区组件同时切换，不改诊断加载、pending 或繁忙状态规则。真实 tsc + Vite 构建通过；相关测试 **58/58**、完整 UI **486/486**，均无失败或跳过，包含实际 DOM 的延迟 metadata、Windows 表单提交和原 Mac Finder/草稿/忙碌回归。这里是前端测试；更新后的真实 Windows 主界面仍待下一次正常 dev 启动观察，Mac 实际运行仍待自托管 CI。

### W1.27 — 引擎实际目录核验与首文档封闭

Windows 管理器现在从真实 [Environment7.UserDataFolder](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2environment7) 读回 UTF-16 路径，返回内存由 CoTaskMemPWSTR 按原生契约释放，包括失败 HRESULT 携带指针的路径。实际目录须为普通 no-reparse 目录，并与一直持有的私有目录完整卷序列号和 128 位 FileId 相同；builder 输入不能代替此证据。核验失败关闭本次 controller/window，保持 readiness 关闭并显示原生错误，不收养、修复或删除实际覆盖目录。

第一轮真实测试发现仅添加 on_navigation 并不能拦住创建期间的首文档：测试服务器收到 3 次提前请求，测试按实际失败保留现场。修复为创建不可见 about:blank，再在真实核验通过后开放原始目标导航和显示；按锁定 Tauri 桌面 URL 解析保留 dev/bundle 目标。原 native-storage/Node 删除续跑结果仍先处理；Mac 的默认存储清理、complete/fail 与 UI 派发顺序保持，独立只读复审通过。直接引用的两项 Windows COM crate 已存在于锁文件，无新版本或二进制来源。

最终默认管理器专项 **8 通过、2 个新增原生夹具 opt-in**；两个 opt-in 分别是需要交互桌面和空闲 1420 端口的真实引擎 parent，以及其 exact child。未删除或跳过任何原 Mac/业务测试，不将交互桌面结果当作服务 Session 0 证明。显式执行真实 parent，分别在两个新 disposable root 启动子进程：正常实际目录为 webview-manager、页面确实 loaded；WEBVIEW2_USER_DATA_FOLDER 指向另一个新 disposable 目录时返回 MANAGER_PROFILE_ACTUAL_MISMATCH、页面没有 loaded。两者核验前实际请求均为 0，均收到匹配浏览器 PID 的 [BrowserProcessExited](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2environment5) NORMAL 回执后退出。

所有这几轮临时引擎目录、覆盖目录及 sentinel 均保留为未提交本地证据。WebView2 在实际目录读回前可能已经写入引擎元数据，因此只证明管理页面未提前加载，不声称覆盖目录零写入。没有读取真实项目、使用 RuntimePaths 或新增产品根目录覆盖。桌面编译和 --no-deps Clippy（-Dclippy::all）通过，既有 dead-code 等 Rust 警告保留，不能称全工作区 -Dwarnings 绿色。下一步正常 npm run dev 检查仍在执行；Mac 真实 CI 和 W1 正式完成仍待核实。

### W1.28 — 正常 npm run dev 再次启动与关闭

在同一 Windows 11 25H2 真机字面执行 `npm run dev`，正常全新准备资源和 CLI，随后 Tauri dev 编译 **1 分 48 秒**并打开固定身份管理器。没有诊断 TAURI_CONFIG、任意数据根覆盖或复用旧生成 target。真实服务 ready，更新后的管理器确实加载；该生产路径只有通过实际 UDF 核验才可导航。使用 computer-use 再次逐个查看全部六个设置分区：通用已为 Windows 关闭退出说明；存储显示 NTFS 与资源管理器文案，展开迁移看到本机内置 NTFS 要求和 D: 路径示例；运行环境实际看到 Node 与 Git 的 C:\Program Files 示例；AI、关于、卸载前清除均显示，没有点击清除或提交任何配置。

Node/Git doctor 仍明确 PROCESS_PLATFORM_UNSUPPORTED，包内固定工具位置读取正常；存储扫描仍明确未实现。关于中的安装体积和构建标识尚未取得，开发版自动更新禁用。这些依赖分别留待 W2/W3、W5、W6，不能据页面可见称功能已完成。主界面仍为 0 应用，未创建/删除真实项目。

再次点击底部“在资源管理器中打开”，真实 Explorer 地址为 `此电脑 > 系统(C:) > 用户 > g > PandaDrawer-Dev`，包含本次原有 projects 和 .platform；只关闭这次新标签，用户原有 release 窗口保留。此次通过 Windows 标题栏关闭按钮退出，真实 ReadyForNativeExit、外层 npm 命令 exit=0，后续窗口清单确认管理器和本次 Explorer 均消失。真实固定开发数据全部保留。

退出日志仍有 WebView2 Chrome_WidgetWin_0 注销 Error 1412；Tauri 结束 beforeDevCommand 时 Vite 的 npm 子进程另报 4294967295。外层正常结束不等于这些原生/工具日志已修复，也不是浏览器进程族或安装包清理验收。W1 本机正常启动/页面检查现已有实际证据；W0/W1 的双端 CI 仍无绿色结果，分支没有合并。按无人值守规则继续在此基础上开 W2 栈分支，保留后续修复和最终验收。

### W2.1 — Windows 项目占名与真实多项目数据库

开始前再次 fetch --all --prune，origin/main 仍为 ac3b5d7；在尚待 CI 的 W1 基础上建立 win/w2-web，未合并 main。修复实际创建第二个项目时 PROJECT_DIRECTORY_MAPPING_INVALID 的已定位原因：占名函数此前无条件拒绝反斜杠。原 Mac 词法规则搬到项目目录适配层保持，Windows 解析原生前缀、盘符/UNC/扩展命名空间与两种分隔符；原始空/点/父组件仍在归一化前拒绝。只提取保留名字，不打开、重绑或修复既有绝对目标，真实项目定位的精确登记核验不放宽。

真实完整 core 编译后的专项：Windows 词法 **2/2**、实际临时登记/新预留 **2/2**。后者验证过期但安全的原生目标只占名，新目标取 -2 且未创建实体，旧登记及 sentinel 字节不变、旧绑定仍拒绝；带原始父组件的坏记录在新预留写入前拒绝。第一轮该坏记录夹具错误使用 verbatim PathBuf.join，Rust 已提前消掉父组件，实际失败；修正为原样构造序列化路径后通过，没有改产品准入来迎合测试。

原有多项目测试实际 **1/1**：创建两个网页项目，各建独立 SQLite 表，只向 A 写入后 B 仍为空；关闭 store、重开后 A 数据与 document epoch 保留。这里只执行完整 core 测试二进制和明确新临时根，不涉及真实开发工作区，也不是前端或 WebView 浏览器存储隔离证明。

首次较宽 directory_ 选集实际 **36 通过、5 失败**；除上述已纠正夹具外，剩余为 backups ZIP64、Node object-layout 和两项 WebView 删除/收尾未完成测试，均未删除或跳过。最终只重跑所列精确专项，不将其换算成新的全组通过数；core 仍有 63 项既有/阶段警告，Mac CI 仍待运行。

### W2.2 — 按持久凭据绑定项目浏览器目录

新增 Windows StoreLedger.bind_profile 与不透明 BoundProfile。只读取既有项目记录，实际 pin 数据根、container、所选目录，并用记录中原有完整 Windows 身份核对已开 HANDLE；不把另一次 capture 当作替换目录的授权。选中的 profile 要求原合同私有 DACL，祖先仅做普通/no-reparse 绑定，不检查系统父 ACL；record 读取保持 Mac 允许硬链接的规则。没有凭据、凭据损坏或移除待收尾时拒绝，不创建/清空材料。

实际完整 core 二进制专项 **6/6**，包括两个独立真实私有目录与持续 pin、完整 FileId 高字节篡改、替换根、未知根、实际 junction、record 硬链接，以及 pending/坏收据的原字节保留。实际目录接口核对接受本根、拒绝另一项目根与缺失根，但此步没有创建 WebView2，不能称 localStorage/IndexedDB 已隔离。只读独立复审通过；Mac 原 StoreLedger 正文和测试零改。首轮编译遇到并行 Git 模块尚未落齐而失败，未运行测试；跨模块稳定后重编才得到上述真实结果。

下一步桌面窗口会先用空白文档、读回实际引擎目录，再加载项目页，并持有浏览器退出 owner 及原产物租约到真实进程族退出。当前公开 API 仅完成绑定，尚未接到真实产品窗口；Windows 磁盘 purge/forget 收尾仍待后续实现，没有伪造删除成功或提前退役 ledger。

### W2.3 — 创建后失败仍保留原生进程归属

将 Windows 原语拆成 create_suspended、assign_job、resume：create_suspended 返回 Err 只表示尚未成功创建；成功后完整持有根进程、主线程、Job 与匿名管道，后续设置失败始终借用该 owner。兼容 spawn 的创建后错误携带可取回的 SpawnSetupFailure，明确 resource closure outcome is unknown，移除旧失败路径忽略 5 秒 Wait 结果后丢失归属的做法。未加入 Job 的根始终未恢复，只能按原创建 HANDLE 请求终止；Drop 仅为 best-effort 请求，不是退出或 I/O 完成证明。

复审同时修复 packager 的保留缺口：旧 Node 外层、MinGit probe scratch 与外层只识别旧 Probe unknown 文案，创建后新 unknown 可能误入 stage 清理。共享适配在任何原成功/失败清理分支前识别两种已有未确认结果，保留原始原因和每层现场；普通错误仍走原自有材料清理。对应真实 TempDir/Created/完整身份回归 **5/5**。错误注入使用受控文本验证清理分派，不冒充真实 OS AssignJob/Resume 失败。

实际完整 native all-targets **54 通过、0 失败、2 明确忽略**（精确 child 夹具和官方 Node opt-in）；另显式官方已核验 Windows Node **1/1**，在中文空格临时目录实际运行、200 KB stderr 完整排空。native 全目标严格 Clippy -Dwarnings 通过。新增默认测试实际验证挂起期间未执行、加入 Job 后恢复、真实已创建挂起根的 resume-before-assignment 本地验证失败仍可取回 owner 并取得退出/EOF，以及退出码 259 不冒充仍运行；后两类 OS 原生设置失败尚未故障注入。

既有真实父/子孙 Job、丢 Job 与异常终止专用 host 的测试在上述完整组继续通过。这里仍是原生 API 与打包边界；尚未接到产品 OwnedProcessRegistry、持久 writer 和 Node 应用，不是 W3 产品运行或任务管理器验收，也未取得 Mac CI 结果。
