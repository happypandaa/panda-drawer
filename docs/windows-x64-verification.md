# Windows x64 移植验证记录

执行标准：[windows-x64-port.md](windows-x64-port.md)。开始日期：2026-10-31（Asia/Shanghai）。未完成、排队和未执行均不算通过。

当前执行补充（用户 2026-10-31）：**CI 和运行器注册延后处理，先完成其余 Windows 功能、安装包与真机验收。** 不等待/重试注册，不把未跑检查记为绿色；短分支继续叠放，CI 补齐后再按序合入 main。用户执行的服务安装脚本已实际校验官方 runner，但 GitHub 认证返回 NotFound、status.success=false，本机未发现注册文件或 actions.runner 服务；不记为安装成功。令牌及原始注册诊断不提交。

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

### W2.4 — 网页归属目录实际私有创建

发现 WebProjectOwnership 原目录创建仍只在 Unix chmod，Windows 普通 create_dir 不能作为原私有 DACL 的证明。将原 check_no_symlinks、缺失时单层创建、已选目录权限修复收拢到适配层；Mac 语句和顺序保持，Windows 原生私有创建或仅修已选 leaf，不递归补数据根、不改系统或工作区父 ACL，不改变 ownership 索引提交顺序。

实际完整 core 二进制专项 **2/2**：新的两级 scope 及既有普通 leaf 修复后保持完整身份和 sentinel；原临时父目录身份和非私有状态保持；父目录缺失不创建额外根；实际 junction 拒绝且外部目标身份、内容和原访问状态保持。全部仅新 disposable TempDir，没有访问真实开发目录。此步补齐持久 writer 所需的原私有 owner 基础，不证明构建、删除或迁移已完整接通；Mac 实际运行仍待 CI。

### W2.5 — 模板树摘要的原生分隔符适配

将 hash_tree 的路径标签转换集中到适配层。Mac 原 to_str 字节不变，包括文件名中合法的反斜杠；Windows 逐组件连接 `/`，与既有 package-template 的跨平台摘要规则相同。遍历、文件内容、长度编码、摘要 v1 格式和源码准入保持，不做旧 Windows 数据兼容。

实际完整 core 二进制默认专项 **2 通过、1 个新增资源 opt-in**：原生相对路径与中文空格 TempDir 的真实文件树通过独立 Node crypto 黄金向量。随后显式 opt-in **1/1**，只读本轮正常 Windows 桌面准备的 template/app 和 template/starter，两棵真实树分别与 starter.json 的 sourceDigest、artifactDigest 相同。没有写入准备资源或真实应用数据。Unix 反斜杠文件名专项已保留，尚未在 Mac 实际执行；此处不证明完整 npm 构建、产物发布或三次构建清理已完成，CI 仍待运行。

### W2.6 — 单项实际 HANDLE 移除原语

新增 Windows 原生已归属名字移除边界，固定普通父目录及祖先，打开 leaf 本身并用完整卷序列号、128 位 FileId 与目录类型核对，然后以该 DELETE HANDLE 请求移除。观察允许重解析 leaf 而不进入其目标，删除不新增私有 DACL 或单链接准入。原有 Mac 边界零改。按已确认只读处理原则，实际 HANDLE 清除只读位；移除失败或有其他硬链接时，在同一 HANDLE 恢复原属性，避免改变外部原件。SetDisposition 成功仅是请求，core 尚须实际缺失证明。

实际新 native 专项 **3/3**，真实临时只读文件、空目录移除；已换名原件及同名替换对象保留；真实不共享删除的 open HANDLE 拒绝，释放后可移除；只读硬链接仅所选名字消失、外部另名内容与只读位保持。native 全目标严格 Clippy -Dwarnings 通过；Windows 专属清除只读位语句局部注明该 Clippy Unix mode 提示不适用，没有放宽 Unix 权限或整组屏蔽检查。

本轮完整 native 另实测 **57 通过、2 失败、2 明确忽略**：原已通过的 suspended_owner_is_registered_before_any_child_code_and_then_resumes 与 observed_exit_259_is_not_reported_as_still_running 均在真实等待后未取得退出状态。large stderr/真实子孙 Job/异常 host 退出仍通过，不能把全组记为绿色。进程专项也在独立 native harness 复现入口前等待，正在定位，没有删/跳原失败测试。此提交仅移除原语，不宣称完整目录清理、删除任务、迁移或 W2 验收通过。

### W2.7 — 无 Git 网页构建的实际准入和恢复只读观察

Windows BuildGitGuard 只在实际 marker 扫描证明无仓库时通行，不解析 Git settings、不选择/探测工具、不创建进程或临时 Git 目录；存在根/祖先/嵌套仓库、gitfile 或异常 marker 时仍明确拒绝未实现的索引核验。原源码排除规则按原生组件转换，普通源码 reparse leaf 如 Mac symlink leaf 不跟入目标；没有添加源码目录时间戳或深度准入。capture/recheck 保持真实 keep_alive/read gate 到同步扫描结束，独立复审发现的 wildcard 提前 Drop 已修正并用实际许可验证。

恢复 inventory 只对固定 .platform/project-state/git 真实缺失/空目录通行，保留整个扫描中的原生 pins，并重查早先空目录及列表变化。任何 pending、终态、损坏或未知记录均保留，不伪造恢复结果。项目目录 1000 上限与 Mac project_git_data.rs::check_data_workspace_available 一致；复审曾怀疑新增限制，核对实际 Mac 入口后保留原预算，没有对外改变契约。

完整 core 实际 project_git 组 **20/20**（18 个 Windows 边界与 2 个共享 wire 测试），无失败、忽略。覆盖实际临时 ProjectStore、坏工具配置/缺失工具仍无进程、构建中新增 .git 被拒、同用户普通源码新增/改名允许、真实 junction、大小写、混合路径、早先项目迟到记录，以及 capture/recheck 扫描期间真实 lease 和 read gate 保持、返回后释放。第一次根代理误用 project_git::windows:: 过滤得到 0 项，不算验证；随后用实际注册名完整执行得到上述 20 项。Mac 相关原文件零改、实际 CI 仍待运行；仓库索引、Git 写入及崩溃恢复仍未完成。

### W2.8 — 实际 live writer 发布与闭合退役

Windows writer 在既有私有 Node/Web owner（或显式平台 owner）内独占创建 preparing 文件，记录 kind/id/token/component 和实际完整身份，刷新后用原 HANDLE 不覆盖发布；并发目的名字或源名字替换不能让平台覆盖/改收养外来对象。finish 只供实际 no-child 或真实 Job/I/O 全闭合路径调用，匹配原身份及原字节后请求删除，仍有 reader 导致 delete-pending 时保持 guard，实际固定名字缺失才闭合。Drop 不删除 marker，不以任务状态代替进程资源回执。

实际完整 core writer 专项 **18/18**，无失败、忽略。包括私有创建/刷新/发布及显式 finish、原字节/高位 FileId/硬链接改变保留、并发准备与目的名字保留、原 HANDLE 发布、真实共享 reader 延迟删除、Node/Web owner 缺失不重建、junction/混合分隔符、早先空 scope 迟到 marker 的完整重查。独立只读审查通过；Mac 原 writer 文件与业务规则保持。

此步只实现本进程掌握的活 writer，不伪造崩溃恢复。启动 inventory 对真实 Missing/Empty 通行，任何 preparing/live/终态样式/未知字节均保留 unknown，错误明确不能确认实际进程及 I/O，未许诺重启电脑即可修复。真实 Windows boot/崩溃后的安全退役仍待 W3/W5；全目录删除及卸载不能据本步称已完成。Registry 新接线另有实际专项，待独立提交记录，不重复累加到这 18 项。

### W2.9 — Win32 参数路径与实际系统目录

新增两个独立原生适配。api_paths 参考 [Rust Windows make_dirp](https://raw.githubusercontent.com/rust-lang/rust/main/library/std/src/sys/process/windows.rs)，只在 GetFullPathNameW 对候选 UTF-16 完全等值时，把标准扩展盘符/UNC 表示转换成普通绝对路径；不检查或授权目录、不改文件操作的扩展路径规则，特殊文件名和其它命名空间保留。SystemPaths 只从 Win32 系统目录 API 取得真实路径，处理实际返回容量，不读 SystemRoot/WINDIR/PATH 猜位置；不引入新依赖。

对应 native 策略/系统目录专项 **6/6**，覆盖盘符/UNC、超 260 字符的纯词法转换、未配对 surrogate、不能安全改写的名字、实际现有系统目录、动态容量及错误返回。它们包含在本轮完整 native **71 通过、0 失败、2 明确忽略**，全目标严格 Clippy -Dwarnings 通过。调用方的 cwd/项目 profile 接线将分别提交记录；本提交本身不宣称产品路径已全部接通。

实际短 cwd 的 std/native 对照已能保持 GetCurrentDirectoryW 和根相对 GetFullPathNameW 一致；本机超 260 中文空格 cwd 两种输入的 std/native 均在 CreateProcess 前返回 OS 267。没有跳过测试、加产品路径长度门槛或改注册表；纯词法不加长度限制不等于 OS 支持该 cwd。长文件树操作另有实际通过证据，Windows 10 及安装包仍未验收。

### W2.10 — 网页窗口实际 WebView2 隔离、持久化及退出占用

网页窗口通过平台 facade 绑定原 StoreLedger 凭据。Windows 先创建 about:blank，读回真实 Env7.UserDataFolder 并核对一直 pin 的完整身份，再导航原项目页；不以 builder 输入代替实际证据。owner 保持原 profile 与产物租约，只有封住本 generation、收到真实 Destroyed 及匹配最后浏览器 PID 的 Env5 BrowserProcessExited 后，下一主队列才退役 COM handler/资源。过期 PID、未封 generation 或未知关闭不提前释放；全局 shutdown 同样排空实际 owners。Mac 原 identifier、导航、许可、产物 receipt 与 destroy 顺序经 facade 保持并独立复审，实际 Mac CI 未运行。

初次真实两窗口已证 localStorage/IndexedDB 隔离，但 Cookie 重开丢失，未按成功计。实际引擎日志及新临时目录 ACL 观察定位到当前用户权限未继承；仅在新临时 profile 为同一 user FA 添加 OI/CI 后，真实 Cookie 数据库出现且持久化通过。生产 helper 仅修已由原记录/full ID 授权的 selected 私有 profile，自身 protected/唯一用户保持，不新增 SID、不改系统祖先/owner/SACL、不放宽 sandbox，重复调用无写入。后续产品 helper 实测还发现 verbatim UDF 的 Cookie 失败，改用 W2.9 共享精确路径适配，并再次以原完整身份验证转换结果、持有其原生 scope，未盲目去前缀。

最终 native helper **2/2**；桌面默认 owner 专项 **4 通过、2 个新增交互 opt-in**。显式真实 parent **1/1**（Windows 11 25H2，4.28 秒），分别在两个新 disposable root 使用普通 DOS 和 verbatim 输入：每轮同一 origin 的 A/B localStorage、IndexedDB、Cookie 不互见，A 关闭/真实退出后重开仍保留 A1，仍开着的 B 可继续写 B2，A2 更新不影响 B；各三代真实 Env5 NORMAL、lease 全部退役，核验前请求数均 [0,0,0]。每轮 A/B 各有实际 20480-byte Cookies DB，只读确认后保留全部现场。没有通过 sleep、清 Cookie 或减弱断言放行。

native 全目标严格 Clippy -Dwarnings 及桌面 --tests --no-deps -Dclippy::all 通过；桌面既有 dead_code Rust 警告保留。这里是复用生产适配的真实 Tauri/WebView2 夹具，不是完整管理器新建/三次构建 GUI 或安装包验收；没有磁盘 purge/物理媒体/长 UDF 路径证明，不宣称 W2 或完整删除完成。失败与成功临时证据均未提交，真实开发数据未读取/删除。

### W2.11 — 已归属目录的实际 inventory、有限重试与缺失回执

普通 remove_owned_directory 接入 Windows 原生删除，原记录完整身份先绑定 root/祖先，沿用 Mac 普通删除的身份、类型、大小、权限和重解析 leaf 规则及 256 深度/500000 条目预算。逐层保持已登记父目录身份，每次移除前重查；未计划内容与同名替换保留。父 scope 从原生请求前一直持有到实际名字缺失回执，包括最后 root 移除，不能在另一同名父目录里证明缺失。占用错误 5/32/33 总约 2.3 秒有限重试，失败报告剩余路径和部分计数，可释放真实占用后重试；请求成功后只观察，不再删除同名替代对象。

独立复审发现 W2.6 临时清只读位存在硬链接竞态及提交后恢复失败被当作占用重试的风险。本步取代该策略，使用 [Windows SDK 的 IGNORE_READONLY_ATTRIBUTE](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/WinBase.h) 直接对原 DELETE HANDLE 请求移除，不设置 POSIX/ON_CLOSE，不读改共享对象只读位，也不申请 FILE_WRITE_ATTRIBUTES。避免 [属性变化传播到其它硬链接](https://learn.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions)；不支持该系统请求时明确失败保留，不回退到修改外部原件属性。SDK 版本范围不能代替目标 Windows 10 实机验收。

实际 native 移除专项现 **6/6**，含真实只读/非空目录失败、持续共享 HANDLE 的原生拒绝、原/替代对象、晚于初始 links=1 观察新建的硬链接、真实 junction 本体移除。删除句柄/所选名字/外部句柄观察证实原只读位未变化、完整身份及外部内容保持。本轮完整 native **71/0/2**（通过/失败/明确忽略）与严格 Clippy 通过，数量已含这些专项。

实际完整 core history_cleanup::windows 组 **9/9**（其中新增目录移除 6 项），包括中文空格超 260 文件树、真实只读硬链接外部原件、junction 目标、替换 root/leaf、新增未计划文件、占用后部分失败及释放后重试清完；2 个既有 coordinator 及 1 个未实现内容快照门禁仍通过。后者在任何 verify 回调/删除前拒绝，未伪造迁移快照。原 Mac 文件零改并独立只读复审；完整项目删除任务、重启续删、历史计划/迁移仍待 W5，不据此称完整删除完成。

### W2.12 — 构建目录交换的实际 journal 与确定状态恢复

将 replace_directory 搬到平台适配，原 Mac/Unix 全函数去新增可见性后逐字机械相同。Windows 在 staged.parent 自有任务范围内先私有发布不可变 journal，记录源/旧目标、两父完整身份、原生 UTF-16 leaf 名和预定 held 名；固定两父，依次旧 current→held、staged→current、held→staged，不覆盖任何名字。每步重验原 journal 身份/字节，按四种互斥完整身份组合继续，不以路径存在或摘要认领替代对象；fresh 目标同样持 journal，源与目标为同一对象时沿用原 rename 语义。

实际完整 core 新专项 **14/14**，0 失败/忽略，0.98 秒。真实 TempDir/HANDLE/rename 覆盖每个实际移动后中断、fresh 两断点、反复 resume 无额外交换、高位 FileId 改变、双 parent 替换、空目录占位、未配对 UTF-16 名字、实际发布后外部替换、journal 并发锁与最终退役时替换。中断由实际 move 后注入错误，不是实际断电或 kill 进程；没有第二物理卷、Windows 10 或并发 reader 可见性矩阵证据。原语目录发布后 unknown 另沿用 native 已有专项，未将它算进本组。

独立只读复审未发现新阻塞。所有 active-journal 后失败明确 artifactExchangePending=true / outcome=unknown；外层必须先停止回滚再按原 publication/provenance 对账。resume_pending=false 仅为当次无 active journal，不能当作“从未交换”，也不能再次调用 replace 交换回旧版本。外层发布、提交与启动接线尚在实现；不据原语 14 项称三次网页构建或旧产物回收已完成。CI 按用户要求后置，本步未合入 main。

### W2.13 — 复制产物以原写入 HANDLE 完成真实刷新

定位 Windows artifact 失败之一为 copy_tree 已通过每个新输出 HANDLE sync_all 后，又调用 sync_tree 从只读 File::open HANDLE 重复刷新；Windows FlushFileBuffers 需要写权限，实际返回 OS 5。将复制完成处理集中到适配：Windows 保留所有原写入 HANDLE 的实际刷新，只对目录经既有 facade 观察（无目录 fsync 等价）；Mac 仍调用原 sync_tree，原逐文件二次刷新和目录顺序保持。没有清源只读位、提高源硬链接准入或省掉输出文件刷新。

实际完整 core 新复制专项 **2/2**：中文空格及 Unicode 文件、真实只读外部硬链接源复制为独立私有可写副本，内容一致，原件完整身份及只读位不变；空树产生真实私有目标且父身份保持。未执行原始 staging 的最终发布/恢复验收；旧 artifact 选集此前 **2 通过、13 失败**，不能将复制两项转换成全组选集通过，下一步继续完整发布、启动恢复和真实三次构建。

### W2.14 — 新目录继承和既有目录的单 leaf 权限修复

新私有目录在 protected、唯一当前用户 FA 的原准入下增加 OI/CI，让 Node/WebView 等普通工具生成的子项仍能由同一用户读写；显式文件、credential slot 保持原 protected/不继承策略。真实启动恢复曾出现 OS 5，定位到 SetSecurityInfo 对既有目录权限修复会传播并移除子项 inherited ACE；改为在原 WRITE_DAC HANDLE 使用 [Microsoft 文档指定的用户态 NtSetSecurityObject](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-zwsetsecurityobject)，只更新选中 leaf。已私有目录直接保持原 ACL，不重新设置或递归修复。没有申请 MAXIMUM_ALLOWED（实际会与既有 pin 冲突）、变更 owner/SACL/父目录、增加准入或引入新二进制；仅启用既有 windows-sys 的 SDK feature。

实际新增专项 **5 通过、0 失败、1 个仅供 parent 启动的明确忽略子进程入口**。覆盖普通真实子进程多层目录/文件继承、占用目的不改写、显式凭据策略、持有原 pin 时修复、重复修复，以及无 WRITE_DAC 的实际 OS 5。修复前后重新打开子项、孙项、兄弟、父目录与凭据，比对完整 FileId、内容及 NtQuerySecurityObject 原始 SD 字节保持；不用 GetSecurityInfo 对 inherited ACE 的重算显示代替原始字节。本轮完整 native **79 通过、0 失败、3 明确忽略**，严格全目标 Clippy -Dwarnings 通过，已包含本组。Mac 文件与策略未改，实际 Mac CI 按用户要求后置；没有 Windows 10 或安装版证据。

### W2.15 — 真实 Registry Job、管道闭合和 Node/npm 探测

Windows Registry 接通挂起创建、实际 Job/根/主线程 owner、创建前准备的三条原生管道 thread、恢复和原共享 driver。实际 root 退出、Job 空、所有 session/drain/pump join 及 driver join 才退役 keep_alive/live writer；消费者提前退出仍实际排空输出。std Command 不公开 env_clear，新增显式 inherited/cleared sidecar；Windows 缺 sidecar 在任何 marker/child 前拒绝，Mac 仍原 Command 原策略。创建后设置失败保留实际 owner，终止/管道闭合后才解决 setup issue，不把“有错误”当作永久无法观察。Windows 环境 block 按 CompareStringOrdinal 的 UTF-16 大小写规则排序/应用覆盖，保留隐藏 =C: 与未配对 surrogate，不用 lossy/Unicode 大写合并不同 key。

实际完整 core owned_process 组 **28 通过、0 失败、2 明确忽略**；其中新增 Windows **8/8**，另显式官方 Node **1/1**。覆盖 4 MiB 双输出及提前丢消费者、真实 stdin EOF、丢 waiter 后 TempDir/reader 继续保持、超时实际 Job、退出码 259、环境继承/清空、无 executor/no-policy/seal 的无 child 拒绝，以及真实挂起/已入 Job 后控制注入 setup 错误（不是 OS AssignJob/Resume 故障注入），工厂未调用且最终 owner/writer/driver 均实际退役。native 环境纯测试 **4/4** 已计入 W2.14 的 79；原生 std/native 子进程逐 UTF-16 观察及 cwd 对照也在该全组中。官方 Node 原生 staged 专项另显式 **1/1**，0.54 秒。

Windows 环境适配只找 node.exe、由 Win32 查询系统路径、用本机分隔符拼 PATH；managed 探测清空继承后显式注入 Windows 必需目录并禁用 compile cache/npm log 写入。实际 global npm 发现 Node 不接受 canonical verbatim npm-cli.js 参数（EISDIR/lstat C:），改经共用精确 conventional_path 适配 npm 脚本、prefix、cwd、outDir，文件身份/文件操作仍用原路径。Mac candidates/tool_path/managed Command 原正文移入 unix 适配；参数 facade 在 Mac 返回原 PathBuf，不加过滤。

实际环境选集 **10 通过、0 失败、1 明确忽略**（6 项 environment 与 4 项同名过滤匹配的 Git 文件边界），node layout **2/2**。显式官方 managed/explicit 全 npm 模块树测试 **1/1**，30.56 秒：新 disposable 中文空格路径中 Node 24.22.0、npm 11.19.1，清空环境 managed 与显式 configure_node 都就绪，运行前后完整受管树核验一致。较早旧样本 metadata 超声明预算曾如实拒绝，保留原样，未放宽生产预算；后来从正常 dev 资源新建样本通过。非交互 libtest 子进程仅 fixture 移除 TERM，生产环境继承不变。真实项目运行/协议/日志/崩溃恢复仍归 W3；没有把这些专项称 W3 或 CI 完成。

### W2.16 — 真实发布、启动恢复和 npm 构建流程

Windows publisher 由原写 HANDLE 刷新过的私有副本形成 generation/publication，再接入 W2.12 的不可变交换 journal；provenance 保持原提交点。inner 交换 unknown 或 current provenance rename 已可能提交的 unknown 均阻止外层回滚，保留实际材料，避免重读旧/缺失名字后误认未提交。恢复只 resume_pending；无 inner journal 不重新 swap，原 task parent 真实缺失时重查项目原绑定，再让原 outer digest 对账。成功提交后 journal 退役仍 best effort。Mac publisher 全函数经去可见性机械比对保持，共享两处目录刷新改经 Unix 等值 facade。

Windows 启动恢复采用实际固定 task scope/no-reparse HANDLE；沿用 Mac 的非目录/无 task.json/非 publication skip、既有硬链接读取及原 modified 时间标签，不增加 ACL/大小/ctime 限制。原非终态任务写 Interrupted，不自动重试命令；再恢复发布、对账任务及执行原 best-effort 回收。早期 8/11 的权限失败经 W2.14 真实 leaf 修复解决，夹具写回使用生产 atomic_json；保留单独 raw/hardlink final record，证明没有以 fixture 私有化掩盖新增准入。

完整 core 实际 artifact 组 **15/15**（1.49 秒）、Windows startup **11/11**（0.63 秒）、共享 portable 构建组 **16/16**（10.56 秒）。包括三代发布仅 current、晚窗口租约资源、重启先恢复再回收、真实 npm 成功/失败/源码变化/越界 outDir、内部 workspaces 与嵌套本地依赖、原目录/依赖保持、关机取消与重复构建、失败初始项目数据库保持。原 Mac 测试未删除；Windows 注册原 portable 测试，真正 Unix symlink/进程测试保留原 cfg。首次误用 artifact 过滤得到 0 项未计入，后以实际 build_artifacts 注册名执行 15 项；更早失败 publish 的夹具曾双 panic，改为先让真实 guard drop 再 unwrap，不减弱断言。

新增 publisher 回滚边界 **3/3**（0.17 秒）。其中 provenance 场景真实 flush 私有 stage→原生 rename 成功→新记录移名并置旧/缺失，按实际身份确认失败，经很窄 writer seam 返回 unknown；断言 outer journal、新产物/新记录/旧代均保留且禁止回滚。它验证 publisher 消费提交不确定回执的规则，不是 native 原 callback 的端到端故障或实际断电。独立复审发现并修复该缺口，其余恢复/skip/commit 顺序无新阻断。上述并非完整 GUI 新建/三次 Vite 构建或安装包验收，继续实际桌面验证；CI 按用户要求后置。

完整 core --lib --tests 的额外 Clippy -Dclippy::all 本轮失败两处：已有 node_git_import/capability.rs 测试对字面 Err 调 unwrap_err、build_support/signed_resources.rs 测试模块后还有项；另有 65 项 Rust dead_code/unused 警告。没有报告静态检查全绿，也没有删除/跳过测试；后续做不改变断言/行为的代码整理再重跑。

### W2.17 — 完整开发版的实际网页项目验收

2026-10-31，本机 Windows 11 25H2 x64（26200.9711）、WebView2 156.0.4314.50，正常 **npm run dev**，未用安装包或测试数据根覆盖。准备本次独立资源/target 后正常管理器启动，固定开发 identity/root 保持。主界面原 0 项；通过真实新建表单创建“临时移植验收 W2 A 中文 space”，经实际 CLI 创建同样明确 disposable 的 B，A 初始 bundled 页面在独立 Tauri 窗口显示“本地已连接”。实际 env doctor 返回 managed ready、Node 24.22.0/npm 11.19.1，无问题、未改用户工具设置。UI 的“操作状态暂未核实”与存储 unavailable 仍属 W5 未完门禁，不当作全部功能可用。

使用两个新项目实际 session/document epoch API，各做 SQLite CREATE/INSERT/UPDATE/DELETE/SELECT，分别保留 A/B 中文行；附件 PUT 上传和 GET 返回各自不同的中文内容。没有操作既有用户项目。仅在这两个 disposable 源码加入相同 origin/key/database/cookie 名的交互验收页，经生产 build 接口执行真实 npm/Vite：A 三次、B 一次均成功，使用内置工具，并保留每次本地结果。第二次后 A 旧 starter 窗口仍显示并收到“新的网页已准备好，当前页面会继续保留”，没有破坏其晚资源。

真实 UI 写 A1 后打开 B：B 的 localStorage/IndexedDB/Cookie 均为空；写 B1 再回 A 点击重新读取，A 的三项仍为 A1。A 第三次构建后旧 A 窗口仍运行，磁盘保留其实际租用的第二代与当前第三代；正常关闭 A/B 后，只读核实各 .platform/artifacts **恰好一个 current generation**，.platform/builds 均空。重新从管理器打开 A，实际 URL 已是第三代，localStorage/IndexedDB 仍 A1；测试 Cookie 为未设 Max-Age 的 session Cookie，浏览器退出后为空，未把它记成持久 Cookie 失败或持久化通过。W2.10 另有明确持久 Cookie 的实际引擎验证。

构建后及重开后再次通过真实 API 核实两项目的数据库行和附件原内容，全部一致。正常关闭项目及管理器，日志给出 ReadyForNativeExit、外层 dev 退出码 0；仍有 WebView2 注销类 OS 1412 和 Vite 结束码 4294967295 的退出日志，未宣称日志无错误。两个 disposable 项目、所有资源 stage/失败和成功本地证据均保留，未提交证据、应用、安装包或凭据。

W2 的本机开发模式功能标准已完成；CI/合入 main 按用户要求后置，Windows 10、中文/空格的实际用户名或工作区、安装包和第 7 节完整验收尚未完成。本轮产品项目显示名/内容含中文空格，项目目录为自动转写名，不能据此宣称实际工作区路径含中文空格（TempDir 专项另有覆盖）。继续堆叠 W3，不将本记录算作安装版或两套 CI 绿色。

W2 收尾静态检查：保留 source-failure 两次原断言，仅用 let-else 取出原 Err；把 signed_resources 的原两个路径测试原样移到文件末尾。原对应测试各 **2/2**，完整 core --lib --tests --no-deps **-Dclippy::all 通过**，既有 65 项 Rust 警告仍保留。没有跳过 Mac 测试或把本地 Clippy 当 macOS CI。

### W3.1 — Node 声明与实际静态网页 HTTP 文件边界

开始 W3 前实际 fetch --prune 成功，origin/main 仍 ac3b5d7；CI 后置，在已推送 W2 fbbb826 上堆叠 win/w3-node，未合入 main。Node manifest/read_bytes/checked_child 和代理 open_at/open_root/open_file 抽到平台文件适配；六个 Mac 原函数正文在仅忽略格式与可见性后机械比对保持，Mac 测试保留。Windows 声明按完整 FileId、原 len/mtime/links=1 前后比较，不拿 Unix metadata tuple 或 DACL/owner/ctime 加严。路径在与原 Mac 相同位置拒绝 junction/reparse，原 root canonicalize 政策保持。

Windows 代理以原根 HANDLE 解析实际名字并核对原身份，固定各层目录至实际叶文件读取，沿用 Mac 静态文件单链接政策，不增加私有 ACL/属主要求。启用原 portable HTTP/停止测试，两个真正 Unix symlink/rename 专项保留原 Mac 注册，Windows 新增实际 junction/硬链接与原生 pin 专项。

为与并行 runtime 源码隔离，使用新 disposable detached checkout 复制这六份实际源文件、其余为 W2 HEAD，实际编译完整 core（不是替身模块）。Node proxy **25/25**，2.19 秒，覆盖真实本机 HTTP/CSP、独立 cookie/host/origin、只转发 backend bearer、请求预算、重定向/流、慢文件/真实上游 socket/driver join、丢 stop/start waiter、中文空格超 260 文件树、junction/硬链接不泄露和外部原件保持。manifest **14/14**，0.03 秒，其中两个新增实际文件/链接专项。Windows pin 对原根 rename 返回真实 OS 占用错误并保持内容，未把这当成 Mac 也会拒绝改名。不是 Node 应用进程启动或完整产品开窗验收，runtime/StartIntent/崩溃恢复继续实现。

该独立完整 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（64 项原有 Rust 警告保留），git diff --check 通过；实际 macOS 和 Windows 自托管 CI 仍按用户要求后置。

### W3.2 — 长驻 Node 的实际 Job、协议、日志与停止

NodeRuntimeTrial 的平台资源收拢到 facade，Mac 原 ProcessOwnership、path/storage、Drop 正文经去新增可见性/格式、Drop 的 self→runtime 参数名映射后机械比对保持；Mac 原测试注册不删。Windows 与 Registry 共用原匿名管道桥，两个实际 pump 正文机械相同。实际挂起根、主线程、Job 和三条原生 worker 在 resume 前登记，原两个协议 reader 与 Trial Self 也先就位；assign/resume 创建后错误返回保留的 owner 进入原停止/pending，不冒充未创建。只依据实际 HANDLE/Job/线程 join 闭合；Windows 不向数值 PID/进程组发信号，协议宽限后用原 Job 强停。已退出 root 的子孙仍保留在 Job，Drop/失联消费者不以关闭 facade 伪造实际 EOF。

Node 校验探测显式保留 cleared policy。Windows preflight 只核对原 Mac 要求的当前用户 owner，用只读 helper，不要求/修复普通 data/cache 私有 DACL；新 run/tmp 私有创建，传给引擎的 entry/cwd/APP 路径经现有精确 SDK 参数 facade。最初官方 Node **0/2**，真实 current.log/独立 Node -e 对照定位为 cleared 环境缺 SystemRoot：--version 仍能过，正常 JS 在 CSPRNG 初始化断言 exit 134。只在 Windows adapter 从 Win32 查询实际系统目录注入 SystemRoot，并显式本次 TEMP/TMP，不继承平台环境或修改 Mac 环境；SYSTEMROOT 已在原共享业务变量保留列表内，未另加限制。

最终真实完整 workspace 默认 Runtime 专项 **5 通过、0 失败、3 明确忽略**（1 子进程入口、2 需官方 Node 的 opt-in），含实际 suspended 未执行、实际终止主线程后的 resume 失败保留、root 先退/子孙仍在 Job、普通当前用户目录无 DACL 加严、清空环境准确项。显式官方 Node 24.22.0 **2/2**，8.82 秒，证明 initialize/ready/ping/health/真实 HTTP、stderr 超 64 KiB 不被终止、配置值注入且宿主测试值缺失、当前日志私有且密钥已替换、重开同 data 计数 1→2/run 新鲜/日志轮转，以及协议超时实际 Job 强停、所有 pump/reader 已 join。原 Registry 抽桥后 **28/28**（另 2 ignored），含真实 4 MiB 排空与丢 waiter/driver 回执，未将其重复相加为 Node 项目数。

完整 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（63 项既有 Rust 警告保留），native 全目标 **-Dwarnings 通过**。修复两个 drain/collect 为 mem::take；子孙 fixture 显式进程 exit 保持“根先退出、子孙待实际 Job 清理”条件，不用 wait 消除被测条件或加 lint allow。并行验证使用 source snapshot 时发现 Windows 复制保留旧写入时间可使 Cargo 误判 fresh，0-test 结果未计入，显式使快照入口重编后确认实际注册/执行；随后真实工作区全编译及上述同组复验通过。

本组是真实 Node 引擎与原生产 Runtime facade 的 TempDir 回归，尚不是 NodeApps 公有配置/管理器、任务管理器终止平台、安装版或 Windows 10 验收。StartIntent 持久退役、Node 专属桌面 UDF 和公有管理器整链继续完成；不据此称 W3 全部完成或两套 CI 绿色。

### W3.3 — 启动标记持久退役、公有 NodeApps 和真实 fixture host 崩溃

将 NodeApps 的原记录/目录/StartIntent 文件操作集中到平台适配，11 个 Mac 原函数正文机械比对保持；原测试仍保留。Windows 新记录私有独占创建、普通记录按原单链接/256 KiB/属主政策读取，不要求或修复已有普通记录的私有 DACL。StartIntent 绑定实际原 HANDLE/完整身份/原字节，运行后原地更新，保留原创建时间；停止后独占捕获到私有 gate，原读 HANDLE 关闭后才观察实际缺失，短暂占用有限重试。

初版只保留 Rust owner 中的退役状态，直接 owner 重试 6 项通过；独立复审指出公有 Inspection::stop_and_retire(self) 在失败时消耗 owner，会丢失该重试状态。补为 gate 外的私有不可变 v1 receipt：仅在原调用方已确认 no-child 或全部运行资源闭合后发布该事实，包含实际 parent/gate/source/journal 完整身份、原 bytes 和 UTF-16 active leaf，实际刷盘后才 capture。恢复按真实 prepared/captured/absent 状态继续，永远先 marker 缺失、再 gate 缺失、最后 receipt 缺失；不借 PID/boot 推导闭合、不重新收养外来对象。原 unknown/preboot 门禁保持：未知或 preboot 损坏材料原样交 Mac skip 规则，boot 不能授权执行删除。读取收据按 compact 编码的 256 KiB×4+64 KiB 上界有界，原记录预算未收紧。

最终实际 Windows receipt 专项 **13 通过、0 失败、1 专用子进程入口忽略**，2.35 秒。覆盖原地更新完整身份/创建时间、普通非私有 DACL 记录不修复、共享 reader 实际延迟与释放重试、原 owner 已丢失后的 prepared/captured 收敛、真实 gate HANDLE 占用时保留外部 receipt、receipt/gate/source/bytes/closed=false/parent 高位身份冲突全部保留、未知/preboot 材料原身份与字节保持、大于收据预算的真实文件不读不改、接近原 256 KiB 的中文记录仍可完成。Inspection 公有续退测试先取得真实子进程完整 100ns 身份并实际退出其 Job，再让真实共享 reader 使已消费 owner 的首次退役失败，释放后重新 inspect/stop 完成。受控 panic 中断只证明已刷盘阶段续退，不算物理断电。

另外明确执行官方 Node 24.22.0 的 **2 个 NodeApps 公有整链 opt-in，2/2**：14.66 秒验证 register→必填配置门禁→save 密钥→start/open_url 的真实代理页面/API→大 stderr/私有脱敏日志→save 保持旧快照→restart 同 UUID/data 计数 2→stop→全新 manager 读持久密钥/数据再 start 计数 3→stop_all；16.97 秒用专用 Rust fixture host 持实际 Node 根/子/孙，父亲保留原 host Child HANDLE 并实际 kill/wait，逐个原完整身份均证明 Stopped，新 manager 还实测占用后 stop 失败、释放 reader 后持久续退。Node SHA 按独立固定 release JSON 核验，未 override digest 或借 PATH/模拟解释器证明 Node。

真实工作区完整 core 编译通过，--lib --tests --no-deps Clippy **-Dclippy::all 通过**（63 Rust 警告保留）。上述使用明确 disposable 源码/数据；旧 register API 仅为核心集成夹具，未替代 W4 的本机目录确认流程。fixture host 退出证明实际 Job/恢复链，仍不等于在任务管理器结束正常产品 GUI 或安装包验收；继续 Node 桌面 profile 接线与后续整机验收。

### W3.4 — Node 桌面窗口复用实际独立 profile 和浏览器退出回执

审查发现原 Node builder 只传 data_store_identifier，Windows 上不会据此生成独立 UDF。将 Node 创建/导航/关闭接到 W2 的同一 Profile facade：Node kind、原 UUID、node-app label 精确核对，Windows 先 blank/隐藏创建、真实目录核验与 browser observer 注册，再导航显示。普通关窗、启动失败及 monitor 关闭都在主线程先封 generation 再 destroy；同步 Destroyed 之后仍等匹配的实际 BrowserProcessExited/COM 退役才完成 session 释放。共享总 shutdown 同样识别精确 Web/Node labels，未知 label 不放开。Mac 继续原 data_store_identifier、导航判定、即时 Destroyed 回调和直接 destroy 路径，实际 Mac CI 尚未运行。

独立冻结源码快照完整桌面编译通过；默认 profile 专项 **6 通过、0 失败、3 明确交互 opt-in 忽略**，包含新增精确 kind/label 拒绝和真实 TempDir Node profile 绑定/高位身份/pin 保持测试。后者的退出回执部分是合成单元输入，不据它宣称真实引擎关闭。

明确交互桌面执行新增 Node-kind parent **1/1**，5.19 秒，实际 Windows 11/WebView2：普通 DOS 与 verbatim 两次新 disposable roots、同 origin 的两个 Node-kind profile，A/B localStorage/IndexedDB/持久 Cookie 初始互不可见，A 关后 B 保持并可写 B2；A 重开读回 A1，再写 A2 不影响 B。每次 Env7 原目录与完整 FileId 实际核对，导航前请求均 [0,0,0]，三代匹配 Env5 exit/lease 退役，释放后可原生 rename 证明 pin 已释放，未 sleep 绕过核验。原 Web-kind parent 回归另 **1/1**，3.82 秒，原断言保留。

桌面 --tests --no-deps Clippy **-Dclippy::all 通过**，3 项既有 Rust 警告保留。仅提交源码与验证文字，实际引擎 JSON/数据目录/生成应用均保留在本机未提交。该 parent 是生产 Profile 适配的 Node-kind 真引擎测试，不是正常产品 NodeApps backend 的端到端 GUI、物理设备或安装包验收。

W3 本机引擎/公有管理器/持久标记和独立浏览器专项已通过，正常管理器添加起始 Node 应用的入口还受 W4 本机目录四入口门禁影响，随 W4 补完后做正常 GUI 与任务管理器验收。CI/合入 main 仍按用户要求后置，继续从最新 main 信息上堆叠 W4，不把上述夹具结果算作第 7 节两种系统的安装版验收。

### W4.1 — 本机目录四入口与真实生成后启动

W4 开始前实际 fetch --prune 成功，origin/main 仍 ac3b5d7；在已推送 W3 9c0e412 上继续 win/w4-import。用户再次确认 CI 后置；不重试运行器注册，不把栈分支当已合并。原 Unix 本机目录文件零改动，Windows facade 替换明确拒绝的门禁，提供只读检查、初始化、初始化并登记及已有目录登记四入口。

Windows 使用绑定的实际目录 HANDLE/完整 128 位 FileId、UTF-16 名称及原类型/只读属性/mtime 观察戳，沿用顶层 4096 项和声明 64 KiB 预算，不新增父目录 ACL 门禁或修复。已有外部目录只登记原路径、不复制、不运行 npm；生成流程先记录 UUID 与目录归属，再独占创建原起始文件。失败仅回滚实际捕获身份与原字节一致的创建项；创建已发生而无法确认时保留 UUID/归属及未知项，不声称清干净。

完整 core 实际默认专项 **16 通过、0 失败、1 官方 Node opt-in 忽略**，1.39 秒。覆盖只读声明、外部目录与重复登记、两种生成入口同 UUID、登记失败后重查复用 UUID、过期预览、真实 pin、部分只读回滚、并发修改/外来文件/替换目录保持，以及 root/声明/unrelated junction 和硬链接/声明预算。另显式执行固定官方 Node 24.22.0 的生成后公有登记、启动及停止整链 **1/1**，4.58 秒；样本为新 disposable 中文空格目录，没有修改用户项目。

完整 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（既有 Rust 警告保留）。首次 PowerShell 未给 lint 参数加引号导致 rustc 收到拆分的 :all，未计为检查结果；准确引号重跑通过。该组是真实公有 backend 和 Node 引擎的临时目录验证，正常产品 GUI/任务管理器、Git 导入、安装版与两系统验收继续进行。

### W4.2 — 官方 MinGit 离线选择与隔离探测

Git 工具路径、离线 Command、系统 SSH 位置和 Apple 开发工具选择抽到平台适配；Unix 原函数正文机械移动保持。Windows 用实际 Win32 系统目录和本机 PATH 分隔符，显式 SystemRoot、NUL 配置位置与 cleared Job，不继承宿主 Git 配置/凭据，不通过 PATH 发现替代 Git。原显式选择的跟随路径政策保持，版本识别保留 2.56.0.windows.4 后缀，识别成功不等于允许远程操作。

独立完整 core 快照默认 Git 环境专项 **3 通过、0 失败、1 官方 MinGit opt-in 忽略**；显式官方测试 **1/1**，16.10 秒。在新 disposable 中文空格目录复制正常 dev 准备的固定 MinGit，先完整树核验，再实际 managed doctor/resolve 和 explicit configure_git，均 ready，实际 Git 2.56.0.windows.4，exec/template 路径一致。前后完整树和 manifest 字节一致、未产生隐式 settings，全部真实进程关闭。测试最初将接口 selection 文字误写 configured，按原公有接口修为 explicit 后重编执行通过，没有改产品接口。

实际工作区完整 core Clippy **-Dclippy::all 通过**（与 W4.1 同一次已包含四份 Git 源码）；GitHub/Gitee HTTPS、clone、依赖安装与导入提交/中断仍在接线，不以离线 doctor 当作 W4 导入验收。

### W4.3 — 实际匿名 GitHub HTTPS HEAD 和 Job 关闭

将 remote Command/version 与临时 cwd 词法适配分离：Unix 原隔离 Command、支持版本规则原样机械移动；共享两个 Mac 测试调用只展开永远成功的 Result。Windows 接受本机盘符 cwd 和原基线对应 .windows.N 数字 vendor 后缀，显式原生系统/临时目录、NUL 配置与禁提示，core.longpaths=true；仍只允许原 GitHub/Gitee 公开 HTTPS 和原 20 秒/64 KiB 预算。进程交给 cleared sidecar，Mac Command 的原 env_clear 政策不变。

独立完整 core 默认 remote 专项 **6 通过、0 失败、1 网络 opt-in 忽略**，包括原四个 URL/HEAD/版本测试与两个 Windows 实际请求构造检查。显式官方 MinGit 2.56.0.windows.4 通过生产 GitRemoteChecker 对公开 demo 仓库真实 **ls-remote HTTPS，1/1**，8.80 秒；HEAD 指向 refs/heads/codex/manual-demo、返回有效对象名，全部实际 Job/reader/driver 关闭、完整 Git 树再次核验一致、无工具设置文件产生。没有继承代理、降低 TLS 校验或修改用户 Git 设置。

完整冻结快照 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（65 Rust 警告保留）。未执行 Gitee 网络、clone/取消/注册或实际依赖安装，不把 HEAD 检查当作完整导入验收。

### W4.4 — 真实 npm、长路径安装和原脚本禁用

npm Command 抽到平台适配，Unix 原构造正文机械比对保持；共享原输入/下载/空间预算、250ms 扫描、有界诊断和实际 reader/driver 收尾保持，声明原 env_clear sidecar。Windows 直接 Node 加原 npm-cli.js/完整原参数，精确 API 路径用于 prefix/cwd/私有配置缓存；SystemRoot/WINDIR/ComSpec 来自 Win32，Windows home/temp 均在已经过原空间扫描的本次私有 operation 内。

首次真实超 260 字符 cwd 测试实际 PROCESS_SPAWN_FAILED；对照已有 native/std cwd 测试及 Microsoft SetCurrentDirectory 文档确认 Win32 限制。尝试固定 Node eval 先 chdir 再载入 npm 也实际 ENAMETOOLONG，未保留该失败方案。最终 Windows 保留直接官方 npm 和原 --prefix 的安装目标，只有 cwd 的 UTF-16 长度已超过 Win32 可用范围时，从实际系统目录启动；不增加路径准入上限、不用 shell、短项目仍原 cwd，Mac 无变化。文档依据：https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory 。

完整 core 默认依赖准备专项 **6 通过、0 失败、1 官方 opt-in 忽略**：4 个参数/环境/UTF-16/无 shell 边界，另 2 个实际临时目录验证无包跳过、预取消、缺锁/双根/.npmrc 均在执行工具前失败且不产生 operation 材料。显式固定官方 Node 24.22.0/npm 11.19.1 整链 **1/1**，13.31 秒：在新 disposable 中文空格、根和 backend 两种超过 260 字符目录分别实际 npm ci，从官方 npm registry 按固定版本与 SHA-512 integrity 装入 is-number 7.0.0；实际 node_modules 文件存在且超过 260 字符，项目 package 原字节保持，preinstall/install/postinstall/prepare 均未执行。全部实际 Job/reader/driver 关闭。没有更改机器长路径注册表或宿主 npm 设置。

完整冻结快照 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（63 Rust 警告保留）。这是实际 npm/依赖文件测试，尚非 GitHub 导入整链、正常 GUI 或安装版；长 cwd 的一般应用子进程能力未据此宣称完成。

### W4.5 — 非执行源码获取、持久发布和未知结果保留

Windows SourceAcquisitionManager 保留 Mac 同名字段/API、原 10 分钟/文件数/字节/磁盘预算及非执行序列；Mac acquisition 与 process 两个原文件零改动。Windows 记录使用完整 FileId 与原 len/mtime/单链接规则，普通既有源码不要求私有 DACL；独占新写文件私有创建且实际 flush，原 blocking writer 和 Registry/Job 排空流程保持。目录摘要按 '/' 相对名和 Windows 无执行位的实际语义固定执行字节 0，materialize/recheck 两端一致；原 Git mode 仍留在 index，不伪造 Windows 宿主执行权限。

发布用实际父 HANDLE/祖先 pin、原 payload 完整身份和原来源/task/app 三 ID 计划；复制预算保持 Mac 50000/512 MiB，原写 HANDLE flush，目标只不覆盖改名。native 仅新增 8 行错误分类 helper，真实 rename 后核验不确定保持 unknown，已确认移动后的 journal/归属收尾错误标 committed；两者都保留原 author reservation。重开接受已发布原对象的后续编辑，不重复制、回滚或误删它。只读 reconcile 在目标/副本均缺失、替换、坏记录时保留 unknown 和显示路径；显示路径不取得删除权限。仓库名 CON/NUL.txt/foo. 只映射新本地可读目录名，原 URL 保留；合法长中文组件按 Windows 255 UTF-16 处理，不沿用附件 API 的 255 UTF-8 字节限制。

最终完整 core 源码/发布默认专项 **26 通过、0 失败、1 官方 Git opt-in 忽略**，2.55 秒（含原 2 URL、Windows 获取 5、发布 19）。涵盖真实长路径、86 中文字符目录/文件、readonly、完整高位身份、hardlink/junction/祖先替换与原件保持、部分复制不二次创建、租约、目标碰撞、实际移动后 journal 失败及新 manager 重试、unknown 保留、坏/超预算记录，以及本机合法保留名映射。首次 22 pass/1 fail 的 Σ/ς 断言经真实 NTFS 核实为错误假设：两者为不同完整 FileId，Σ/σ 才实际碰撞；改 fixture 保留合法名字，最终 native create_new 的真实 AlreadyExists 也返回需要修改并保持原文件。

显式官方 MinGit 非网络引擎专项 **1/1**，5.54 秒：真实新 disposable Git 仓库、commit/fsck/ls-tree/cat-file/read-tree、required filter 配置故意不能执行，实际原字节正确展开、index 就绪、全部进程关闭，原 writer clear 并实际清除 owned 临时目录。首轮完整 public Git 导入另实际走通 HTTPS 下载、npm、发布和登记；仍在完成公有启动和正常 GUI 的收尾验证，不把上述离线/文件专项称为全部 W4 完成。

### W4.6 — 公有导入协调器、真实取消和公开原码运行

完整 core coordinator 默认 **9/9**，0.96 秒，含原来源错误保持、真实 TempDir 接纳/同 importId 去重/取消信号/中断接纳恢复/坏 journal 原物保持/登记三态；publishing 无路径时的缺失或坏 receipt+实际 author reservation 公有 recover/retry 均 Unknown/cleanup_pending，原 UUID/完整身份/字节保持且不创建进程。目录 sync 委托原 Unix 等值 facade；新增 Windows 恢复 hook 在 Mac no-op。完整冻结快照 core Clippy **-Dclippy::all 通过**（62 Rust 警告保留）。

已显式执行固定官方 Git/Node 的真实公开导入与真实 materializer barrier 取消。取消已通过：实际 writer 阻塞时取消保持 source slot/任务租约及 CleanupPending，释放后实际结束 root/Job/I/O，原临时 source/owner 不存在；显式 Registry shutdown join final driver 后完整收据成立，同 importId 不重放。最初错误地把仅 Closed 当成已 JoinHandle join，已改为明确执行原 Registry 收尾并保留完整断言，不改产品规则。公开 demo 零依赖 npm 成功可以不创建 node_modules，移除此错误样本假设，保留真实 npm 成功/进程收据；W4.4 另有实际 registry 依赖长路径证明。

公开 demo 已实际 import/npm/登记成功，启动先按正常必填配置拒绝，再保存人工配置和人工密钥。其 src/server/main.mjs 自身直接 directory.sync；独立固定 Node 24.22.0 新 TempDir 对照实际返回 EPERM/syscall=fsync。未修改平台来模拟该 API、未修改远端，也未执行临时源码适配草稿；改选已有 Windows 自然文件适配的公开 RadioBob 进行原码验收。第三轮早退另暴露 stdin-pump 的 OS 232 永久未闭合问题，见 W4.7 实际修复。

最终显式官方 Git 2.56.0.windows.4/Node 24.22.0 公有成功整链 **1/1**，80.91 秒：HTTPS 从 https://github.com/happypandaa/radio-bob.git main 获取，实际 commit 必须为 163923d99f4b8cce188e3372dc135d4ae41bee78；下载、原码发布、真实 npm、登记和三处临时材料清理成功。缺配置实际拒绝启动且无 child，再以公有 save 保存人工 ANTHROPIC_API_KEY，FISH 保持未配置；实际 NodeApps start、本机静态页及 GET /api/status、/api/state 成功，stop 后新 manager 重开，持久配置和同一 source/app UUID 正确，import 恢复及重复 ID 均不重放。所有原 author 文件完整 FileId/SHA 与原 commit 在两次运行后保持，没有修改下载源码；npm 生成树和 .git 内部单独按其原流程处理。只证明本机生命周期，不调用或宣称外部 AI、音乐业务可用。

完整冻结快照 core Clippy **-Dclippy::all 通过**（62 Rust 警告保留），最终默认 coordinator 再 **9/9**，0.95 秒。正常 npm run dev 已可启动；为让正在并行的测试源码编辑不自动重开 GUI，正常关闭旧会话（ReadyForNativeExit、外层 0，仍有原 WebView2 1412/Vite 退出日志），改用原脚本转发的 npm run dev -- --no-watch 进行稳定 GUI 验收。当前 GUI 空目录确认继续进行，尚不据后端专项声称安装版或两种系统验收。

### W4.7 — 应用真实早退时 stdin 断管的闭合

仅 Windows 共享 pipe_bridge 的实际 File.write_all 分支把 BrokenPipe/109/232/233 当作对端断开而结束该 worker；原始 5/6/13/29/87/995 等其它错误仍报告。微软 WriteFile/WinError 定义为依据；这一分支不授予 root、Job、其它 reader 或 driver 的闭合回执，仍逐项实际观察/join。Mac 代码不改。

完整 Registry 默认 **32 通过、0 失败、2 opt-in 忽略**，0.22 秒；Runtime 默认最终 **7 通过、0 失败、4 专用/官方入口忽略**，包括真实 Rust child 早退、仍存活的 native child 在父 4 MiB 写已阻塞时关闭实际继承 stdin HANDLE，父实际 BrokenPipe、root/Job 仍活，最后原 Job 强停和全部 worker/reader join。另显式官方 Node 三项 **3/3**，9.17 秒：真实 initialize 后 exit 23 的启动回滚给出 ResourcesStopped、无 pending owner，原大 stderr/协议/日志/持久数据和协议超时回归均保持。

新夹具曾仅 fs.closeSync(0) 导致 writer 实际未断开，3/4 官方测试中该项超时；独立对照 process.stdin.destroy 也仍保持别的 endpoint，未增加超时或假称断开。替换为直接接管并关闭实际继承 HANDLE 的新 disposable 原生 child，官方 Node 早退真实专项仍保留。完整 core Clippy **-Dclippy::all 通过**（62 原 Rust 警告保留）；本轮不改变或跳过任何 Mac 测试。

### W4.8 — 正常 GUI 目录选择、生成登记和起始应用

正常开发版主界面保持原两个 W2 disposable 网页项目，未触碰用户旧项目。新建明确 disposable 的中文空格空文件夹，从真实本机文件夹入口与 Windows native folder picker 选择；实际返回“目录检查结果不完整或路径不符”，尚未生成或登记 Node 项目。正在核对 Windows canonical verbatim 与 picker DOS 路径的响应表示，不能把 W4.1 后端临时目录专项当作 GUI 已通过。稳定会话使用原 npm run dev -- --no-watch，避免并行 Rust 测试编辑触发自动重开；所有本机应用/资源/证据未提交。

已修实际阻断：LocalDirectoryPanel 按契约严格核对响应 sourcePath===请求路径，Windows canonicalize 的 verbatim 与 picker DOS 表示不同。仅 Windows 四入口成功 JSON/preview prompt 原样回显已经验证的请求表示；内部 canonical Preview.source、完整 FileId、revision、生成归属及登记持久路径均不变，不改前端确认条件或 Mac。DOS 正反斜杠、明确 verbatim 与真实尾点目录不混用其 dotless sibling。完整 core 目录默认 **19/19**（另 1 opt-in），3.21 秒；官方 starter 公有登记/启动专项 **1/1**，4.58 秒；core Clippy **-Dclippy::all 通过**（59 Rust 警告保留）。

按原 desktopInvocation 复用本次已核验的 rZS2kg 开发资源/target 重编，23.96 秒，未更改固定 identity/data/workspace。真实 native picker 再选同一空目录，GUI 检查成功，生成并登记“临时 W3 W4 GUI Node 2029ec8a”，实例 b1ead98e-4512-4426-8761-3bc4040d4128；实际 ownership.generatedSource 记录原根完整身份。真实成功页打开独立 Node 窗口，起始页点击“检查应用连接”显示“你的本地应用已经连通。”；管理器显示 3 项/1 运行。源码仅在明确 .test-artifacts disposable 目录，平台数据在固定开发数据根，保留供 W5 真正删除验收。

同时修正 Windows 空目录/成功/复查三处说明，按真实 ownership 说明平台生成目录随删除、已有外部原始目录保留；仅 frontend-platform 适配提供文案，Mac 原文及删除业务保持。TypeScript build **通过**，完整界面测试 **486/486**、0 跳过，15.70 秒。当前继续在这一 disposable 起始项目验证配置立即重启与任务管理器终止；尚非安装版/Windows 10 或 CI 验收，不据此宣称 W0–W6 全完。

### W5.1 — 删除持久任务、历史清理与真实空间核算

从已推送 W4 f1cd1d2 开 win/w5-storage；fetch 后 origin/main 仍 ac3b5d7。CI/运行器继续按用户要求后置，未合入 main，不把新增测试写成 macOS 或安装版验收。下面均是 Windows 11 25H2 本机新 TempDir/明确 disposable 项目专项；Mac 原平台实现与测试不删除、不跳过。

Node 删除日志用完整 128 位 FileId、原同父卷 stage 和持久 previous/next CAS intent，文件原写 HANDLE 先刷新，不覆盖替代物；保留未确认和 delete-pending 现场，改名前放弃/重开续做不重放别的对象。默认专项 **51/51**，10.85 秒，含真实改名/移除每个中断点、父/根/暂存替换、长中文空格、只读、junction 外部原件保持及实际 pending HANDLE。

历史清理完整计划/TTL/确认/任务保护/forget 与内容摘要回调已接；真实 runner join 后才清结果，回调后重扫及逐项核对真实身份/字节/只读位，不新增时间戳准入。专项 **23/23**，2.31 秒。Storage 完整信息/逻辑字节/附件分页/缓存失效、白名单清理及删除 forget 专项最终 **39/39**，15.04 秒（含 6 个既有 WebView storage 回归）。首轮 36 pass/3 fail 的 OS5 实际来自 maintenance 目录 File::open+sync_all 与 Windows mtime 夹具：前者委托已有 native 目录同步 facade，Unix 原 File::open/sync_all 保持；后者改真实 BACKUP_SEMANTICS/WRITE_ATTRIBUTES HANDLE 后重跑通过。开发版程序占用尚无安装根证明，报告 unknown，不把整个 Cargo target 计成程序安装目录。

网页普通文件删除继续原 owner 计划和所有原状态，用同卷私有目录捕获原名字，再经实际 DELETE HANDLE 移除；硬链接不额外拒绝，readonly 不修改外部其它名字属性。默认 **21/21**，13.59 秒；独立 native allowing-hardlinks 改名 **5/5**，0.03 秒。首轮 19 pass/2 fail 是两项新断言写错 outcome 字段，按原 API 改为 operationOutcome 并保留 unknown/file_in_use/full-ID/释放后实际缺失断言；产品未加另一字段或放宽失败语义。

持久 writer v2 使用 Microsoft 文档的当前进程 ProcessTelemetryIdInformation class64 BootId 与显式来源，0 为合法；不与 Prefetch 注册表计数混比，不用 PID 或当前时间差授权删除。原无 boot 标记不自动升级，same boot unknown，different boot 全组核验后再回收。native 实际父子进程同 boot **2 通过、1 子夹具入口忽略**；core recovery/live/inventory **30/30**，0.87 秒。首次一项失败来自新 fixture 以 fs::write 创建非 protected DACL 却要求可回收，已扩成真实 private 与 nonprivate 两种：私有按原规则回收，非私有精确原物保持且 unknown。未修改生产准入来满足测试；冷启动、休眠和 Fast Startup 尚未物理验证。官方依据：https://learn.microsoft.com/en-us/windows/win32/devnotes/process_telemetry_id_information_type 。

Windows 配置锁 guard 关闭前显式 unlock，Mac/common save 业务不改。配置默认 **24 通过、1 子夹具入口忽略**，2.73 秒，含真实 DuplicateHandle 与三工作线程保存/读取实际 join；新的官方 Node、Runtime-bound+IoLease+并发 list/config、运行后新增配置声明再保存/重启整链旧裸 File **1/1**（14.31 秒）、新 guard **1/1**（14.85 秒）均过。因此这只是独立锁释放健壮性修正，不能据此声称 GUI 保存卡住根因已修。正常 GUI 曾实际写入 revision1 后持续显示保存中；只读 WCT 无可解析多节点等待，Ctrl+R 后管理器正常显示已保存 2 项/1 运行/待重启，正在再做真实界面复现和立即重启。

冻结完整 core --lib --tests --no-deps Clippy **-Dclippy::all 通过**（52 Rust 警告保留）；最终网页测试仅改正确断言后重编专项通过。迁移、公有停止并删除整链、磁盘 WebView2 purge、卸载前清除、任务管理器中断与安装版验收继续进行，W5 尚未全部完成。

### W5.2 — 迁移复制、真实重开和原目录自动清理

Windows 工作区 IO 已接本机内置 NTFS / 原生同步根核验，目录范围用实际 pins，文件原写 HANDLE 刷新后核验原摘要；Entry/删除清单使用真实 readOnly，不伪造 Unix mode。链接通过 Microsoft FSCTL_GET/SET_REPARSE_POINT 保留原 symlink/junction 数据，不进入外部目标，不替换同名物；特殊非链接 reparse 报错保留。目录 sync 委托已有 facade，Mac 仍原 File::open/sync_all；Node 删除预览摘要包含完整高半 FileId。原 Unix CopyEntry/entries 校验和启动 recover 函数正文机械比对保持。

真实新 TempDir 默认专项：复制 **5/5**，0.32 秒（中文空格、超过260字符、readonly、普通hardlink独立副本、排除项、junction外部原件、变更/越界/目标已存在拒绝、private实际追加日志）；启动只读 inventory **10/10**，0.06 秒；RuntimePaths 真实选中恢复 **6/6**，0.29 秒；复制证明/重开清源 **16/16**，0.33 秒；Node migration 原根/Prefix路径规则 **1/1**；删除 revision 高半位 **1/1**。首次 root copy 编译4错是误用私有separator函数和不存在PinnedDirectory.identity字段，修为公开纯词法函数/读取真实HANDLE身份后重编通过，没有削弱断言。

完成原目录清理适配后开放该能力派发，不把派发当删除授权；保留无选中无工作与原失败传播。真正 RuntimeService 公有迁移/IO lease/丢失waiter、附件reader实际排空、提交后native重开失败/retry、新业务保持、旧版本不增删除权等原专项在本机 **9/9**，2.97 秒，另派发 **3/3**，0.02 秒。完成冻结 core --lib --tests --no-deps Clippy **-Dclippy::all通过**（53既有Rust警告）。这些是真实临时运行环境与文件行为，尚未声称GUI迁移到第二块内置盘或安装版验收。

真实GUI第二轮保存普通配置成功返回“已保存，重启后生效”，点“立即重启”后新起始页面实际后端显示“GUI 第二轮配置 · 第1次启动”；原W4 binary未换，因此初次长时间pending不能归因于新RAII修复，尚未重现。该次disposable后端实际写出root/child/grand PID并输出96行各超1KiB stderr，未因大输出终止。TaskMgr已实际打开但其高权限进程不接受本工具输入，精确TaskMgr交互验收未通过；改用核对原image/启动时间的Stop-Process强制结束平台，实际原root25256/child3012/grand13648全部不存在，证明该次Job kill-on-close，并不等同TaskMgrGUI或安装版通过。项目和数据继续保留给W5删除验收。

### W5.3 — 实际 WebView2 退出、崩溃恢复与精确磁盘清理

Windows项目profile先持久记录私有active-browser会话（完整root/file FileId、原生boot和随机token），只有实际Env5 browser family退出、Destroyed、owner/reader退休与pin释放后才退休会话。重开无RAM owner时用同一原UDF的不可见about:blank真实引擎核验Env7并观察Env5退出；不导航业务、不运行Node，不靠PID消失或空内存表冒充闭合。随后沿原StoreLedger持久clear receipt删除精确原目录，实际缺失再退休receipt/owner；delete-pending、替换/外来物和未释放pin均保留可重试。Mac原native store函数和模块逐字比对不变。

core StoreLedger/binding/session实际临时文件专项 **23/23**，2.41秒；native removal组 **7/7**，0.02秒，含只读DELETE访问probe，没有Disposition或新增ACL/links门禁。首次session编译4错来自Uuid未启用serde，改已有协议风格的canonical字符串后通过，没有新增依赖feature。完整core Clippy **-Dclippy::all通过**。

冻结桌面完整编译成功，默认 **99通过、10专用/引擎入口忽略**，0.40秒。显式实际WebView2精确profile purge **1/1**，17.00秒，覆盖Web/Node两kind的普通与held-root-pin四轮：真实Cookie/localStorage/IndexedDB互隔离、关闭A不阻断B，A重开数据保留；实际三代browser退出/退休后只删对应profile，peer保持。真实same-boot宿主Child HANDLE强制结束→新宿主blank核验/实际Env5退出恢复删除 **1/1**，3.73秒，Web和Node各一轮，旧浏览器100ns创建身份逐一确认不再活，原UDF实际不存在。这是实际引擎与进程测试，不是synthetic回调。

管理器卸载前清除改Windows自然流程：封门/Destroyed/Env5实际退出→释放pin→清原UDF子项但保留原空root完整身份给原LocalCleanupPlan最终删除；结果用无UDF的原生对话框，真实worker已join且原退出条件齐备才退出。修复terminal报告已发布但menu Quit绕过worker join的竞态，新增真实worker持root pin等待测试；另在cleanup seal后禁止tray/普通builder重开管理器，真实fixture在原pin仍有效时断言拒绝重开。最终实际manager清理专项 **1/1**，1.20秒：原root直到最终删除前保持原FileId，browser真实退出、root实际缺失，核验前导航请求0。上述不等于整个卸载前清除GUI或安装版完成；后者继续验收。

### W5.4 — 公有 Node 删除整链与外部导出发布

固定官方Node24.22.0的公有后端删除父专项 **4/4**，10.98秒，另真实宿主捕获后Child HANDLE强制结束、新manager同任务续删 **1/1**，12.87秒。覆盖真实生成/再登记/人工配置保存/HTTP运行/日志、WriterGuard原文件同步关句柄后才捕获、readonly Git内容随完整删除、外部原目录全树完整FileId/原字节保持、实际文件占用失败列remaining、释放后同task恢复和改名前放弃不自动重启。首轮两项取得旧failed结果过早，修新夹具为resume后先有界等待真实workers join，再读最终结果，全部断言保持；产品/Mac目录同步与返回语义未改。此组没有创建浏览器，ledger确实Absent，不伪造native purge；本机生成没有源码获取publication，Node也无外部partial登记API，未伪造这两类材料来称覆盖。

Windows lifecycle外部导出新增同父目录、完整源/父HANDLE身份与不覆盖改名；普通硬链接按Mac允许，替代文件不发布/不归属，提交后不确定明确unknown并保留。Mac原Unix publisher不改。真实TempDir专项 **5/5**，0.01秒，含其它硬链接原件保持、目标碰撞、高半身份错、改名前源替换、改名后目标替换与实际父pin。冻结core --lib --tests --no-deps Clippy **-Dclippy::all通过**（51 Rust警告）。浏览器下载包含用户明确覆盖的交付事务是另一接口，仍在独立实现，不以此不覆盖publisher替代。

本组最终完整桌面 --bin local-projects-desktop-dev --tests --no-deps Clippy **-Dclippy::all通过**（4 Rust警告保留），包含最后的manager重开封门；该封门的真实manager专项重编后再次1/1通过。

### W5.5 — 正常界面删除、配置重启与路径适配

Windows 11 25H2 开发版正常界面实际发现网页删除预览仍用 `/` 开头判断，合法的本机 Windows 路径因而被拒绝。删除、清理、导入、求助五处统一委托已有 frontend-platform 绝对路径策略；Node 删除预览的父目录、文件名及共同目录也移入同一适配层，支持盘符、UNC 与扩展路径，不同盘符显示多个磁盘位置。Mac 原词法和分组函数正文保持。TypeScript/Vite build 通过，完整界面测试 **490/490**、0 跳过，15.67 秒，包含实际组件接纳 Windows 绝对路径、拒绝盘符相对路径和跨盘分组。

正常 GUI 对明确 disposable 的 W2 网页 A 展示完整删除预览，勾选确认后公有任务成功；管理器由 3 项变 2 项。实际核对其项目根、该 UUID 独立 WebView2 数据目录和 web-project-ownership 目录全部不存在；网页 B 和 Node 测试项目仍保留。没有据此声称其余所有历史路径逐个完成核对，也没有删除用户外部原始目录。

已有 disposable Node 在正常 GUI 第二次保存配置和立即重启成功，页面显示新值及第 1 次启动。真实日志 100280 字节，人工密钥原串不存在且有 `***`，ACL 为当前用户私有。此操作发生于原 W4 二进制，不能归因于后来配置锁 guard 修正；之前一次保存写入 revision1 后界面持续等待的原因仍未确证。精确核对平台进程后以 Stop-Process 强制结束，原 Node 根、child、grandchild 全部结束，重开为 0 运行。Task Manager 窗口提升完整性使本轮自动化不能实际操作，故这项只记真实强制终止验证，不记为第 7 节 Task Manager/安装版验收。

### W5.6 — 浏览器下载与用户所选外部交付

Windows 外部文件交付采用实际完整身份、私有不可变 intent/decision、固定父 HANDLE 和不覆盖改名。新建与明确覆盖均有提交前恢复原文件、提交后仅向前完成及释放最终交付归属；碰撞、占用、替代物或未知状态保留证据。普通硬链接按原 Mac 允许，私有原文件观察不新增单链接门禁。实际 native publisher **6/6**，4.64 秒；每个已发布中断点恢复等事务 **4/4**，4.84 秒；native 私有硬链接原语 **2/2**，0.01 秒。包含真实公有项目删除后完成的外部交付、其它硬链接及外部旧原件保持，不保留最终交付的项目归属。

WebView2 Requested/Finished 已接原 writer、stage 与取消流程，原生取消回收实际临时材料，未受理的 Finished 不取得交付权限。桌面默认 **103 通过、12 专用入口忽略**，0.47 秒；真实 WebView2 Requested/Finished/native cancel 专项 **1/1**，1.41 秒。尚未完成正常项目窗口到原生保存对话框的完整 GUI 交付，不能以该专项代替安装版验收。冻结 core/native/desktop Clippy **-Dclippy::all 通过**，保留原 Rust 警告；Mac 原正文和测试未改。

### W5.7 — 桌面程序范围、空间报告与外部保存边界

Windows 桌面启动从实际进程 exe、已选 CLI 与 resources 树构造进程内 typed InstallationScope；headless 不从 HTTP、环境变量或任意磁盘声明认领安装目录。父目录仅展示，计量三个精确成员，不统计整个 Cargo target 或相邻文件。实际完整身份、exe stamp、资源签名、双遍原生元数据与缓存绑定；替代物或扫描中变化保持部分/未知。外部下载、导出按同一成员范围及身份别名拒绝写入，普通相邻目录仍允许。Mac scope 为 None，原 `.app` 范围和处理正文保持。

独立 scope **4/4**、0.03 秒；实际 Runtime 下载边界和 lifecycle 导出 **3/3**、0.59 秒；程序计量 **8/8**、0.14 秒。首次程序测试模块路径误指向既有 forget 测试，已显式指定 program/tests.rs 并核对实际注册名称后重编，未用错误过滤得到的通过数充数。完整 storage::windows 回归 **60/60**，17.87 秒，其中 19 项是既有网页存储回归，实际 Storage 41 项。core/native/desktop Clippy **-Dclippy::all 通过**。这些是开发资源范围及真实临时目录证明，尚非 NSIS 安装副本或正式版签名验收。

### W5.8 — 全量回归暴露的快照、路径与 SQLite 句柄问题

第一次完整 core 回归 **823 通过、28 失败、30 专用入口忽略**，90.89 秒，保留全部原断言继续修复。11 个归档失败源于 Windows 只读 HANDLE 不能 FlushFileBuffers：SQLite 快照改为保留最初实际新建读写 HANDLE，固定名字并在两连接关闭后刷新原对象，Unix 原顺序不改；原归档 15 项、新 junction 1 项、新原生刷新/WAL/名字绑定/UTF-16 4 项在随后完整回归全部通过。最初新增名字锁测试 19/20，属性查询 HANDLE 不参与所需共享约束，改为原已获 read 权限的真实读取 HANDLE 后通过，没有增加 DACL 或硬链接要求。

目录报告 7 个原测试和服务整链的失败源于查询孤立 Windows Prefix，改委托已有 native 路径组件与 reparse 适配，**9/9** 通过；Node 交接日志用原生逐层 join，原 Mac 输出保持，**1/1**。原 identity 测试在 Windows 活 SQLite 句柄时不能先改名，保留完整 Mac 测试，新增对应 Windows 用例实测活句柄阻止改名、仍未删除时明确 incomplete、真实关闭后才移动，且保留原 writer/gate/cache/epoch/retained-handle 断言。M4 归档交付删除/迁移 **5/5**、退出协调 **21/21** 通过；其余退出问题由下一节真实 Git 管理器实现解决。

最终组合快照完整 core **877 通过、0 失败、32 专用入口忽略**，97.33 秒；core Clippy **-Dclippy::all 通过**（61 Rust 警告保留）。该组合还包括以下独立小步记录的 Git 读取和登记恢复。测试全部使用新 TempDir/明确 disposable 项目，未据此称 macOS CI、Windows 10 或安装版已验收。

### W5.9 — 实际 MinGit 版本读取与构建准入

ProjectGitManager 在 Windows 接真实 manager、只读 status/versions 和私有索引 BuildGitGuard；数据版本原 schema 的只读清单/详情/校验也已接原生文件观察。原 49 个 free-function 中 47 个正文原样，其余仅委托 inspect tuple/hash 路径；13 个 manager 方法、Output.success 和原 BuildGuard 正文保持，原 Mac 测试文件未改。Windows 处理本机盘符、NUL、实际 SystemRoot、Windows vendor suffix、UTF-16/API 路径、完整 FileId/readOnly/ChangeTime，Git registry 调用显式声明已清空环境，不继承宿主配置和凭据。

默认 Git 分组 **29 通过、2 官方入口忽略**，1.27 秒；首次真实官方进程 0/2 暴露缺失 OwnedCommand::cleared 声明，补原 Command 的环境来源标记后，官方 MinGit 2.56.0.windows.4 **2/2**，32.29 秒。实际验证 status/versions、100755 不产生虚假 dirty、dirty 源码允许构建、config 改动 recheck 拒绝、busy/unmerged 拒绝，原 index/config/HEAD 完整身份及字节不变，真实 registry driver 已 join。新只读数据专项 7 项也在默认分组和完整回归通过。

完整 core 回归和 Clippy 结果见 W5.8。此小步仍关闭 source/data restore 写 API，不复活 Mac 已退休的 init/save/create-branch/data-save 功能；源码恢复和数据恢复整链必须继续实现后才能声称完整功能一致。

### W5.10 — 数据恢复证明与真实登记交换

FileProof 捕获、匹配与稳定观察移入平台适配，Mac 原 tuple/mode/schema/strict-identity/错误政策保持；Windows 用完整 FileId、readOnly、原生 LastWrite/ChangeTime 和原文件摘要、单链接规则，不增加 DACL 限制。独立真实证明专项 **9/9**，0.04 秒。

Windows 登记交换在原 recovery permit、freeze 与独占 gate 中使用实际三次不覆盖改名、精确不可变 journal 与 FullId；可见前恢复原物，可见后仅向前，重启后不重复交换。只读状态/容量扫描不移动文件，额外元数据仅按实际精确名称和字节纳入原预算。Mac 原 swap_preserving/move_directory_exclusive 函数体机械保持，Windows 目录同步与移动进入专用 I/O 适配层，Unix 原 staging HANDLE 生命周期保持。

首轮 **10/11**，真实 OldHeld 暂缺 live 登记使普通 control 检查返回 PROJECT_REMOVED；没有绕开普通项目门禁。修为仅在原 permit、根、epoch、任务身份均核验且精确 pending=OldHeld 时提供恢复 control。新增 fresh manager 用例确认普通 control/project 仍拒绝，只有原许可+lock 能复位，最终专项 **12/12**，4.40 秒；包含每个移动/退休中断、占用释放重试、外物/父替换、完整高半身份、readonly/hardlink、实际 header→permit→gate 和预算。完整 core **877/877** 及 Clippy 通过；Clippy 唯一新测试多余借用已最小修正，不改变断言。整体数据 apply 仍关闭，不能把这一登记子事务称为完整项目数据恢复。

### W5.11 — Windows 已安装 Codex 识别与定向派发适配

用 current-user PackageManager 核对官方 Store 包 name/family/publisher、SignatureKind、Status 和实际 VerifyContentIntegrityAsync；在独立 MTA 线程运行原生操作。派发重新核对当前注册及完整安装目录身份，显式 ActivateApplication(family!App, --open-project 参数)，不猜 exe/PATH 或用环境协议。路径按原生 UTF-16 和 Windows 命令行规则转义。依据本机安装 26.1027.3384.0 的 manifest/Explorer verb 和原模块实际参数；[官方 Windows 部署说明](https://learn.chatgpt.com/docs/enterprise/windows-deployment)、[Microsoft ActivateApplication 契约](https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-iapplicationactivationmanager-activateapplication)作为平台依据。Mac 文件、include 和原测试零改动；只扩已有 windows crate 功能，无新依赖二进制。

默认新增 3 项通过，真实 CommandLineToArgvW 验证中文、空格、引号、尾反斜线、shell 字符和长路径仍是单个项目参数；伪安装目录拒绝且不创建接手材料。实际已装官方 Store 包只读完整性专项 **1/1**，0.02 秒，未启动/修改 Codex。完整桌面默认 **104 通过、13 专用入口忽略**，0.93 秒；Clippy **-Dclippy::all 通过**（测试目标 5 Rust 警告保留）。首轮 HSTRING.as_wide 不存在，修为直接 &[u16] 后真实编译通过；尚未执行真实激活，不将其记为 GUI 打开编辑器成功。

正常 GUI 点击“在资源管理器中打开”，实际打开 C 盘当前用户 PandaDrawer-Dev，窗口树与地址面包屑确认路径及 .platform/projects 内容。仅是当前开发版操作，不替代第 7 节安装版检查。

### W5.12 — 正常 GUI 运行中 Node 项目的停止并完整删除

本轮重新实际读取系统信息：Win32_OperatingSystem 为 Windows 11 Pro 10.0.26300 x64，CurrentVersion 为 DisplayVersion 26H2、CurrentBuild 26300、UBR 9711（内核文件自身版本 26100.9711）。后续证据以本次实际观察为准，不沿用早期 25H2/26200 的记录；这仍不是 Windows 10 22H2 证明。

正常退出旧管理器，在原已核验 rZS2kg 资源/CLI 配置下重编最新冻结开发版，32.12 秒，真实独立启动；本轮复用原准备资源，没有称为 fresh npm run dev 或 NSIS 安装。实际 native folder picker 选新建的中文空格空目录“临时 W5 完整删除 Node 74e184d8”，生成并登记 UUID 015f4961-df19-44b4-a778-46baef078cf7，ownership.generatedSource 绑定真实 FullId。正常打开独立窗口，点击连接检查显示“你的本地应用已经连通。”，管理器 3 项/1 运行。

保持应用窗口和后端运行，从管理器右键菜单进入完整删除。预览明确展示 C 盘实例、源码 owner、D 盘生成源码、ownership，跨盘标签正确；核对后按原复选框和最终按钮提交。任务成功返回主界面 2 项/0 运行，独立 Node 窗口实际消失，原服务监听端口不再存在；逐一物理核对上述四个项目路径及 node-UUID WebView2 目录均不存在。原网页 B、旧 Node 测试项目仍保留。该成功仅适用于这一个明确 disposable 项目，不推论其它项目残留、安装版、Task Manager 中断或全局卸载前清除已通过。

### W5.13 — 平台 AI 说明按实际编译目标显示

主界面和设置共用的“给 AI 的平台说明”不再在 Windows 固定写 macOS；显示名称集中到 frontend-platform，以 bootstrap 的实际 target 选择 Windows x64，Mac 原整句和纯说明构建默认值保持，缺失目标使用中性说明。复制和全文预览共用同一来源，不增加项目路径、诊断或密钥。TypeScript/Vite build 通过，完整界面 **491/491**、0 跳过，15.88 秒；新增真实组件 DOM/人工 clipboard 测试确认 Windows 说明及原版本号、完整接入契约正确，原 Mac 说明断言保留。该测试没有向任何外部 AI 发送内容，不作为物理系统剪贴板证明。

### W5.14 — 普通 D 卷的同步目录查询适配

正常 GUI 将 C 盘当前 disposable 工作区迁移到 D 盘新建中文空格空目录时，实际在启动迁移前报 Cloud Filter HRESULT 0x80070001；原根和数据未改，目标仍空。实际新 TempDir 对照：C 卷 DOS/verbatim 和 ByPath/ByHandle 四种均返回不在同步根内，D 卷四种均 ERROR_INVALID_FUNCTION，已排除路径表示；完整 WinRT 注册根枚举实际有 1 项且逐项取得 Path/实际 HANDLE。没有在 D 卷注册 provider、加载驱动或修环境以掩盖问题。

仅该 capability 错误归类为独立 QueryUnsupported；仍必须完成系统注册根全清单及原 HANDLE/VolumeGUID 范围比较，清单、任一根解析失败或其它 Cloud Files 错误仍拒绝。当前卷真实新 TempDir、原哨兵/完整身份保持、匹配根拒绝和清单未知保留等 native workspace **8/8**，0.07 秒，包含既有 C 卷真实临时 sync registration/注销及 STA 调用回归。注入范围政策测试不冒充 D 卷真实 provider 登记证据。native Clippy **-Dclippy::all 通过**、无警告，0.64 秒；Mac 文件/政策未改，正常 GUI 重编后的跨盘整链继续验证。

### W5.15 — 显式 Git 源码取回与正常可写数据取回事务

源码取回沿用原预览、确认、任务许可和独占 gate；Windows 原生 stamp 使用完整 FileId、readOnly 和原摘要。三次不覆盖改名的 pending 状态留在本任务 journal 内，控制块 kind 和五位 entry 固定宽度，任务根身份不变；各 phase 的最大 journal 字节仍按原规则精确计量。纯 status/classify 不改目录名，实际继续/撤回才在原许可内稳定：可见前恢复本次移动的原侧，可见后先完成该项，再保留用户对整个源码取回事务的继续或撤回选择。固定 capture、foreign live、无关源码变化和替代任务根均保持，不自动收养或删除。

源码默认 **6/6**，8.52 秒，涵盖真实三次移动各中断点、新 owner 双方向、原完整身份和只读位、实际 reader 占用释放、未知 capture/替代根，以及固定宽度预算。固定官方 MinGit 2.56.0.windows.4 的公有管理器专项 **2/2**，111.36 秒，实际完成 preview/apply 和真实移动中断后新 manager 的两个恢复方向，并核对 Git、业务数据和输出保持。日志为本机 `.test-artifacts/w5-source-restore-{default,official}-tests.log`；临时项目和日志没有提交到仓库。

数据取回接入原 SQLite/附件一致快照、独立 stage、五 slot、不可变 plan、registration epoch 提交、completion/terminal 和实际 join 的 task runner。Windows DirectoryProof/FileProof 保留完整身份、真实 readOnly 及原生稳定观察，不伪造 Unix mode；登记交换中间材料只按原计划验证后的精确名字和实际字节计入预算。Mac slot 文件实现、metadata publish/seal 原函数体机械保持，原 schema、错误政策和已可见 sealed terminal 的释放点不改；已退休的保存/初始化/建分支接口没有重新开放。旧数据保存库存只有真实 Missing/Empty 才报告 clear，未知记录保持 blocked，不把它记为已恢复。

首轮暴露两个实际接线错误（模块 import 歧义、测试所需的内部可见性）及 prepared journal 的只读 HANDLE 刷新错误；均修复后重新编译。prepared journal 已存在时改由原 read HANDLE 和原完整身份约束真正同对象的可写 HANDLE，再实际 flush，不跳过刷新。测试夹具最初把快照直接放入 version-data，违反原独立准备目录规则，已改为新 TempDir 准备后真实不覆盖改名，再由夹具外部 Git 正常提交；没有通过旧保存 API 绕过约束。

最终组合快照完整 core **893 通过、0 失败、37 专用入口忽略**，370.69 秒（`.test-artifacts/w5-final-source-data-core-default-tests.log`），其中数据默认 **9/9**：真实五 slot/epoch/terminal、A/B 隔离、新 manager 撤回、原 reader 占用与重试、foreign slot 保留、完整身份/readOnly/硬链接准入、128 候选和原大小预算、逐实际移动/metadata/封存中断及 reserve 前缀续写。普通完整故障矩阵曾单独实际通过；旧 reserve 笛卡尔重复运行因成本过高按授权核对唯一测试进程后中断，保留 TempDir，**不计通过**。随后 reserve 夹具按原 Mac 范围先进入 epoch 已提交的 metadata 阶段，再补满 128 个候选并逐 reserve partial-write/publication/seal 边界恢复；全部独有断言保留，最终完整回归通过。

官方 MinGit 公有管理器 preview/apply、epoch 重开及失败后 status→新 manager Undo 两项实际通过；这轮三项组为 **2 通过、1 夹具失败，33.32 秒**。失败的 raw writer 夹具误将 9 MiB stdin 送入原 8 MiB Context 收集预算，改为真实文件经 hash-object --no-filters 准备对象后，9 MiB cat-file 流式专项单独 **1/1**，7.90 秒（`.test-artifacts/w5-data-restore-raw-writer-final.log`）。实际取消时仍持有原 lease/gate/candidate，报告 cleanup pending；释放阻塞 writer 后，所有原生泵/worker 真正 join、resourceClosed，才释放材料。reader 重试夹具曾在 pending 中用 RW SQLite SELECT 自行产生 WAL/SHM，已改为原完整身份和全部字节不变核验，关闭占用后再实际读取恢复结果，精确复验 **1/1**，5.22 秒。上述官方三项均有实际通过记录，没有把仅 Unix Python 夹具当作 Windows 管道证明。

**只读刷新实际限制：** 实际新 TempDir 对照中，普通、只读位、当前用户只读 DACL 及两者组合的只读 HANDLE，FlushFileBuffers 返回 OS 5，NtFlushBuffersFile/NtFlushBuffersFileEx 各测试 flag 均拒绝访问；原先仍持有的 WRITE HANDLE 是成功正控。来自 live 的旧 DB/附件不能仅凭完整身份和摘要推断曾被本事务刷新。实现没有新增 owner、DACL 或只读位预检查，不修 DACL、不临时清只读位，也不跳过原件 flush；真实原生刷新因权限失败时，沿用原 I/O 错误处理保持 pending/原材料，绝不报完成。这是第 2 节的同一保证与真实操作系统错误语义，不是另加 Windows 安全准入；此前把它列为第 9 节第 2 项待用户批准的问题已撤回，无需另批。正常可写测试通过不代表这些只读刷新操作成功，也不承诺 Windows 目录 fsync 或断电耐久等价。

这里的继续/撤回是用户确认的 Git 版本取回及中断事务续作，**不是项目永久删除后的找回**；完整删除的原合同不变，不保留删除成功的恢复副本。结果只覆盖本次冻结快照及明确 disposable 项目，不代表正常 GUI Git 取回、安装版、Windows 10 或 Mac CI 已验收。上述 893 项快照早于后续 Node root 两文件与 Windows 迁移入口移除十一文件；这些后续变化不计入该次结果，其验证分别记录。

### W5.16 — 按用户范围调整关闭 Windows 新工作区迁移

用户最新明确授权迁移继续困难时可以移除，本轮据此关闭 Windows 新工作区迁移，包括 C→D。Mac 原行为不改；Windows 保留当前工作区位置和在资源管理器中打开，隐藏“更换位置”表单，公共 preview/migrate 在任务创建、settings 写入和复制前明确拒绝。已有记录的内部启动恢复、真实重开和精确旧源清理继续按原绑定工作，未知材料不删除，不通过内部入口发起新的公开迁移。W5.14 的卷/同步根原语证明不构成 C→D 完整 GUI 迁移成功，本轮也不再以迁移成功作为 Windows 交付要求。

已实际通过的范围：迁移相关 UI 分组 **86/86**；平台门禁 **6/6**，新公共 API 无任务/目录/journal 写入专项 **1/1**、0.37 秒；既有内部迁移/重开/清理 **9/9**、5.38 秒，source restore guard 回归 **12/12**、1.93 秒。均使用实际编译目标和新 disposable 临时目录；内部事务回归通过不表示公开迁移已重新开放。日志为本机 `w5-windows-migration-removal-ui-tests.log` 及 `w5-removed-migration-*-tests.log`，没有提交测试证据。

同轮完整 UI 首次有一项 radio 测试仍点击已撤回的 Windows 迁移按钮；改为 Windows 不提供该按钮的负断言，Mac 原测试行为保持，radio 分组 **34/34** 后，完整 UI **494/494、0 跳过**，13.93 秒。不是通过删除断言或把专项结果冒充全量解决。此范围变更与项目永久删除、数据取回的 I/O 错误保留规则无关，不改变完整删除合同。

### W5.17 — Node 归属根私有创建、原生桌面派发与收尾回归

Node 归属更新原先使用普通 create_dir，私有权限设置只在 Unix 执行；Windows ownership 根/leaf 可能不具备原 writer inventory 所要求的私有 DACL，导致 PROJECT_WRITER_UNCONFIRMED。现在两个已选目录委托已有 web_project_ownership_io：新目录原生私有创建，既有目录按 Mac 原 chmod 范围只修所选 leaf；不修改系统父 ACL、不传播至原子项、不重认领未知目录。实际 NodeOwnership **2/2**，1.98 秒，验证完整身份、parent/child/sibling 原 SD 和字节保持，以及真实 writer 未闭合时更新 ownership 不能将它清掉。

后续 project cleanup **3/3**，3.43 秒，M4 回归 **5/5**，2.02 秒。此前七个独立测试进程同时运行时有一项 PROJECT_BUSY；原失败项精确复验 **1/1** 后，正常 M4 整组 **5/5**，没有据此删测试、改原断言或记作已修复的产品故障。core Clippy **-Dclippy::all 通过**，9.77 秒（38 个 Rust 警告保留）；desktop Clippy 同样通过，9.96 秒（bin 7、test 5 个 Rust 警告保留）。这些是后续明确分组和 lint 结果，不改写 W5.15 那次 893 项快照的范围。

桌面项目文件夹、README、普通外链和 Node 求助的系统派发统一委托 desktop_shell 适配。Windows 文件管理器通过实际 SystemPaths 获取 explorer.exe，原位置作为单个 literal 路径；浏览器通过独立 STA 的 ShellExecuteExW 派发单个 URL，无额外参数和环境扩展，实际 join 派发线程，不等待或终止用户进程。Mac 三处原正文机械保持。该实现的实际桌面默认测试 **105 通过、0 失败、13 专用入口忽略**，1.18 秒；覆盖原生参数和派发边界，**尚未执行真实浏览器派发**，不将它记作浏览器窗口已经打开。正在准备的正常 GUI 清理验证尚无成功回执，不计入本节。

### W5.18 — 正常 GUI 全局清理预览的实际进度与阻断

重编正常开发 GUI 实际用时 **47.30 秒**。新建明确 disposable 的外部原目录（名称后缀 `255ca4c8`），经 native folder picker 按已有非空项目登记，项目 UUID 前缀为 `84765e5d`；登记前后 9 个文件的 SHA256 和长度均保持，D 卷原目录完整 FileId `0x0000000000000000000300000007ed42` 未变。这是外部原目录登记及保持证明，不是删除或清理成功证明。

此前唯一临时迁移试验（任务 ID 前缀 `task_d613`）实际为 queued、0 files、0 bytes，未 published/committed，没有 stage，外部目标为空，settings 也未选中它。正常退出 GUI 后，将该试验的 3 个文件完整复制至仓库忽略的 `.test-artifacts/w5-withdrawn-migration`，逐一核对 SHA256，再精确移出实际 migrations 目录，结束用户已撤回范围的这次临时试验；没有修改外部目标。这是人工处理明确临时试验的验证操作，**不是产品迁移恢复、自动清理或全局清除成功**，不赋予普通产品清理保留副本的权限。

新的清理预览已不再被该迁移试验阻塞，但当时仍报 **FILESYSTEM_ERROR，Windows 33**，未删除文件或清除 WebView 数据。随后用独立真实 TempDir/fs2 复现：原 HANDLE 持锁时，新 HANDLE 的 try-lock 返回 raw 33、ErrorKind::Uncategorized，原 WouldBlock 判断为 false；实际释放原锁后，新 HANDLE 的 try-lock 成功，明确了 `local_cleanup::verify_controls` 的错误分类问题。原 local_cleanup 的 Unix 测试不在 W5.15 的 893 项 Windows 回归范围；后续 Windows 修复与真实测试见 W5.19。**全局清除和卸载前清除仍未通过真实 GUI 验收。**

### W5.19 — 全局清理实例锁竞争的最小平台适配

清理控制核验仅委托新增 `local_cleanup_io`：Mac 原 `error.kind() == WouldBlock` 表达式保持；Windows 仅接受 fs2 提供的原生锁竞争错误码（当前为 ERROR_LOCK_VIOLATION、raw 33），其它原生错误及人工构造的 WouldBlock 不证明持锁。原 root/lock/marker 完整身份、实际 try-lock、TTL/scope、writer、runtime shutdown 及原生窗口/Node 关闭前置条件均不放宽，不改变删除范围或归属政策。

新增 Windows `local_cleanup::` 分组 **5/5**，3.63 秒。其中 4 组真实 RuntimePaths::for_test 用例验证：实际 Runtime 持锁时 prepare 成功，真实 quiesce 后 execute 仅保留原完整身份的环境 marker/instance lock；原锁实际释放或被替换时拒绝旧计划；真实未 closed writer 阻止预览及后续清理；公有 Node 登记/初始化入口产生的 managed 源码实际缺失，外部原目录及各文件完整 FileId/字节保持。另 1 项核对错误分类不误收其它错误。没有启动 Node 子进程或创建 WebView，不用伪造 native purge 回执证明闭合。

初轮为 **4/5**：夹具在 writer finish 后仍持有自身 parent pin，明确 drop 这些夹具 guard 后全组通过，未修改产品权限、身份或实际缺失要求。NodeOwnership 回归 **2/2**，3.75 秒；core Clippy **-Dclippy::all 通过**，8.48 秒，保留 lib 30/test 38 个 Rust 警告。该检查点尚待包含此修复的新二进制验证；后续真实 GUI 执行失败见 W5.20。首个 NSIS 包含旧清理 guard，不以本节测试替代其真实 GUI 收尾。

### W5.20 — 真实 GUI 执行全局清除后原生阶段拒绝访问

在原 debug 资源根重编正常 GUI，实际用时 **17.65 秒**；直接 StartProcess 并核对 debug PID **17116**。界面实际有 **4 个临时项目**：网页项目 `prj_f43`（第一安装版检查项目）和 W2 B，以及生成 Node `b1ead` 和外部原目录 Node `84765`（均为 ID 前缀/测试名称）。新 Windows 清除预览文案实际显示正确；随后实际勾选、输入确认文字，完成两次确认并执行全局清除。

普通 manager 窗口实际 Destroyed，随后不使用 UDF 的原生结果窗“熊猫小抽屉 · 本机数据清理结果”显示 **FILESYSTEM_ERROR，拒绝访问，OS 5**，**14 个路径全部列为 remaining**。实际核对原 workspace 根、生成源码根及外部原目录仍存在，DataRoot 原顶层条目也仍全在；这不证明每个子文件都未被删除，原生清理可能已局部执行，尚未完整核对其影响。不能据根目录仍在推断没有任何删除或 WebView 数据变更。

随后只读 native probe 明确定位访问失败：根 metadata 和原根 DELETE-access open 成功，观察到 32 个条目后，对 `Cache_Data` 的属性访问 open 返回 OS 5。manager root 的私有当前用户 FA ACE 为 flags=0、无 OI/CI，Cache 父目录同样没有当前用户继承；`Cache_Data` 仅有继承的 AppContainer capability ACE、没有当前用户 ACE，owner 仍为当前用户。DOS/canonical 指向同一实际对象，未见 readonly 或 delete-pending 状态。`ManagerProfile::initialize` 遗漏了 project profile 已使用的 `enable_private_directory_inheritance`；当时开始最小接线及新 TempDir 真实回归，没有现场手工 ACL 写入，随后专项结果见 W5.21。

原生结果窗已正常点击“确定”并退出，确认 PID 17116 不再存在；外部原目录 9 个文件的 SHA256/长度仍逐项匹配。**本次真实全局清除失败，未通过验收。** 原因定位和源码修复进展不是清除成功回执，也不将后续处理称为删除项目的恢复。

### W5.21 — 管理器原绑定私有目录的继承初始化修复

仅 Windows `ManagerProfile::initialize` 在原完整 FileId/private 校验及目录 pin 绑定后，调用现有 `enable_private_directory_inheritance`；project profile 早已调用同一 helper。本步只有该接线及新测试两个源码文件，native helper 和 Mac 没有改动；不在现场手工修 ACL，也不增加父目录 ACL 准入或认领其它目录。

真实新 TempDir 人工构造旧 flags=0 根及 capability-only `Cache_Data`，先实际得到 OS 5，再经正常 initialize，使原 root/child/payload 完整 FileId 和字节保持；显式 protected 子项、外部 parent/sibling 的原二进制 SD 保持，重复初始化幂等，释放实际 pins 后真实 clear_contents 成功。该旧根组、新 fresh 根组及原 3 项合计 **5/5**，8.46 秒。desktop no-run **23.03 秒**；默认桌面回归 **107 通过、0 失败、13 专用入口忽略**，5.48 秒；Clippy **-Dclippy::all 通过**，18.37 秒，保留 core lib 30、desktop bin 7/test 5 个 Rust 警告。

既有实际 manager 专用入口另 **1/1**，1.24 秒：独立 TempDir 的 Env7 actual UDF 匹配，pageLoaded=true、requestsBeforeVerification=0，browserExited=true；原根保留到最终移除，之后实际 absent。为运行该夹具，先核对自有 Vite PID 23320 的 exe、完整命令和启动时间，只停止该进程以释放 1420；没有因此清理默认数据根。该真实引擎专项不替代默认根全局 GUI 验收。

接下来仍需在同一 fresh stage 重建包含此修复的新包，通过正常 manager 初始化而非手工 ACL 写入，再重试全局 GUI 清除。**截至本检查点，该默认根 GUI 重试尚未通过，W5.20 的失败不改写为成功。**

## W6 — 安装包（进行中）

### W6.1 — 独立 CLI 静态 CRT 与 fresh 准备阶段

本次 fetch 后 `origin/main` 仍为 `ac3b5d7`；W5 已提交并推送 `dce7c83`，安装包工作叠放于 `win/w6-installer`，尚未合入 main，CI/运行器仍按用户授权后置。本小步仅修改 Windows 独立 CLI 构建环境 helper 及其调用、测试共 3 个文件：保留 Cargo encoded flags 对 RUSTFLAGS 的优先级、显式空值和已有参数，再追加静态 CRT 标志；明确 target 保持 host 构建隔离，helper 环境只交给独立 CLI Cargo 调用，Mac 及 manager 构建参数不改。相关纯脚本测试 **11/11、0 跳过，69.57 毫秒**。

正常 fresh build 本次明确打印准备 stage `.tools/desktop-windows-preparation-x02kxw`，独立 CLI release 实际构建 **3 分 05 秒**。Cargo target 产物与复制至该 stage 的 CLI SHA256 均为 `0B826A442E2E08F2C10197A16F16F1D314BF0782FD22543603D56E5EA2B496CA`；实际 build-info 核对固定开发身份 `dev.happypandaa.pandadrawer`、版本 `0.0.0`、Windows x64、锚点 `development-source-v1` 和真实 runtime 标记为 true。没有将旧 CLI 或旧准备目录当作此次 fresh 产物。

该新 CLI 的实际 PE 解析为 **13 个 normal import、0 个 delay import，无 VC 运行库名称匹配**；本地解析器夹具 **10/10**，覆盖两类目录、RVA/旧 VA 表示、终止和映射边界。旧 CLI 负控实际为 23 个 normal、0 个 delay，检出 VCRUNTIME140.dll 并拒绝；既有 manager 正控为 30 个 normal、0 个 delay，通过直接导入检查。该检查只覆盖 PE 直接 normal/delay DLL 描述符，不证明传递依赖、动态加载、干净机器运行或安装成功。

截至本准备检查点，manager 与 NSIS 仍在构建，尚无结果；随后首包与本机安装事实见 W6.2。本节不是完整包、安装/卸载、WebView2 引导、干净机器或 Windows 10 验收证明，也不改写 W5.18 尚未通过的全局清除状态。所有 stage、二进制及本地解析证据保持忽略，不提交到仓库。

### W6.2 — 首个未签名 NSIS 包与本机当前用户安装

manager release 实际构建 **6 分 59 秒**，首个 NSIS 包成功生成，**63,220,105 字节**，SHA256 为 `6317AA6F8CDE322F980D26F12D1D99FAC4B48686F09E64498A44ABE2F128ACD2`，**未签名**。实际 PowerShell 会话不是管理员；直接执行 NSIS `/S` 返回 **Exit 0，8.20 秒**，HKCU 卸载项及 InstallLocation 均实际存在，安装位置为 `C:\Users\g\AppData\Local\熊猫小抽屉`。这证明本机此次当前用户安装，不代表其它系统或最终版本安装验收完成。

实际启动安装目录内的 GUI，主界面显示 **3 个临时项目、0 运行**。最初 20 秒等待曾超时，之后实际窗口出现，未把该超时当作崩溃，也未把安装程序退出码当作 GUI 已启动的证明。安装后的 CLI SHA256 与 W6.1 stage 相同。安装后 manager 与 Cargo 原产物长度相同、差异仅 3 字节 `UNK`→`NSS`；本机 `tauri-utils 2.10.2` 的 `platform.rs:368` 定义该 NSIS 标记。**不声称 manager 与 Cargo 产物散列相同。**

运行时 `runtime.platform.exe` 指向实际安装程序；typed scope 的主 exe、CLI、resources 三个成员均实际 bound，wholeDirectory 为 false，resources 指向安装资源，canonical 路径呈系统 PackageLocalCache 表示，未回退源码资源或把整个安装父目录当作程序范围。native-packager 对 runtime Node、development Node 和 Git 的实际验证 **3/3**；环境 doctor 实测 Node **24.22.0**、npm **11.19.1** 且 managed ready=true，Git **2.56.0.windows.4** 且 managed ready=true。独立 system SSH 诊断失败如实保留；这些结果不验证认证、远端访问或 SSH 可用性。

**此第一包仍包含旧清理 guard，不能作为最终卸载前清除版本。** 该检查点尚待包含 W5.19 修复的正常重建及 GUI 清除；后续 debug GUI 的失败见 W5.20，最新包构建见 W6.5。本机已经安装 WebView2，尚未测试缺失时引导；SmartScreen、干净 Windows 10/11、卸载重装也未测试。未由首包生成、静默安装和本机启动推出 W6 完成，安装包及证据均不提交。

### W6.3 — 第一安装版 CLI/公开 HTTP 的临时网页项目验证

第一安装版实际 CLI/公开 HTTP 验证临时网页项目（UUID 前缀 `prj_f43`）：SQLite CRUD、中文空格人工附件上传/下载及同名 409 不覆盖均通过；实际提交的 **3 个 build 任务均 succeeded**，provenance 指向当前产物，磁盘只保留 **1 个 generation**。重新取得 ticket/session 后，实际查询仍有 1 行，附件 SHA256 保持。来源为仓库忽略的 `.test-artifacts/w6-first-installed-web-check.log`；这是实际安装版接口和文件证明，**不是网页原生 GUI、管理器重启或断电恢复证明**。

安装 GUI 的 footer 实际打开 Explorer，窗口确认 `C:\Users\g\PandaDrawer-Dev` 及 `.platform/projects`，随后正常关闭本次 Explorer。这项真实 GUI 系统派发与上述 CLI/HTTP 检查分别记录，不由接口成功推断原生项目窗口已验证。

期间 UI 一度显示 **TASK_NOT_FOUND**。对照原 main `ac3b5d7`，`submit_with_maintenance` 正文与当前规范化 LF 后完全相同：先 reserve ID，再 await environment.resolve，之后才插入 registry；App 又将 task_get 失败统称 maintenance failed。未捕获 banner 当时的任务 ID，因此只能记为**与已有竞态一致的非阻塞已知 UI 问题，未确认该次提示的具体因果，也未修复**。事后三个任务均可正常 scoped 查询，maintenance 为 ready、activeTask 为 null，没有丢失记录的证据；Mac 共用流程未改。

### W6.4 — Windows 全局清除预览按实际归属说明

实际预览原先把 Node 生成源码标成迁移工作区，并将两个 Node 均称为外部目录，而实际计划只保留其中 **1 个外部原目录**。Windows 文案集中委托 frontend-platform：准确说明原始目录保留、生成源码随删，受管范围使用精确标签“平台管理的项目内容”，Windows 术语为 WebView2/回收站。仅改变预览文字；paths、后端权限、删除计划和确认状态不变，Mac 原计数整句、labels 和 warnings 保持。

scope 界面分组 **21/21**，2.26 秒；完整 UI **496/496、0 跳过**，14.70 秒；TypeScript/Vite build 通过，保留原 chunk 大小警告。测试核对 Windows 文案与 Mac 原文，不把文字修正作为实际清除范围或原生闭合证明。

本次文案检查点正在构建的第二个正常 fresh 包（stage 后缀 `1HCHrQ`，CLI 已实际构建 **3 分 12 秒**）早于该 UI 文案修改，不能作为最终 UI 版本；随后复用本轮资源的增量重建见 W6.5。当时全局 GUI 清除尚未执行，之后真实执行失败见 W5.20；**没有全局清除成功回执，W6 仍未完成。**

### W6.5 — 第二轮 fresh 资源上的文案版本增量重建

Windows 清除预览文案提交 `2655210` 已推送。第二轮正常 fresh 准备（stage 后缀 `1HCHrQ`）的 CLI **3 分 12 秒**、manager **6 分 56 秒**已完成，但开始于该文案修改之前；文案更新后使用这个本轮新鲜准备目录进行 **incremental Tauri rebuild，没有重新运行 prepare，不记为第三次 fresh 构建**。

重建前对该 stage 的 runtime Node、development Node 和 Git 再次实际原生验证 **3/3**；CLI target 与 sidecar SHA256 同为 `3BBCE4FF47EB879B5BD963C6145D054AD094992AC6BC48A4CF471802D7EAC46C`，实际 build-info 为固定开发身份、版本 **0.0.0**、Windows x64。desktopInvocation 保持原 reset、精确 resource config 及 target；before-build TypeScript/Vite 通过。实际 release 重建 **4 分 03 秒**后，NSIS 成功生成，实际 **63,182,688 字节（60.26 MiB）**，SHA256 为 `4D54CB07F5BB1E2EB1440187BD9083584DF1CB6E368BB2E1170E86B9C28D7132`，日志为本机 `w6-cleanup-copy-prepared-nsis-rebuild.log`。该包的实际 PE 检查为 CLI 13、manager 30 个 normal import，各 0 个 delay import，均无 VC 运行库直接导入匹配；仍不证明传递依赖或动态加载。

该包随后已再次实际当前用户 `/S` 安装，**Exit 0，8.43 秒**；原 workspace 与外部原目录仍存在，安装后 CLI build-info 核对正确。**此包早于 ManagerProfile 继承接线，不能作为最终可通过全局清除的版本。** 修复专项见 W5.21，包含接线的新包与默认根全局 GUI 重试仍待完成；不由旧包安装成功推出清除成功。按执行标准第 7 节及第 9 节第 6 项，已异步询问用户 Windows 10 验收选择，尚待回答；本机 Windows 11 的其余工作继续，不把未运行的 Windows 10 检查记为通过。
