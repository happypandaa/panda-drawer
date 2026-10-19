# Windows W1 环境与工程验证记录

日期：2026-10-19（Asia/Shanghai）。Windows 独立只读宿主已在本机 Win11 实际开窗，刷新、单实例与正常退出通过。W1 尚未整体通过，完整业务 core/CLI、Windows 10、Mac 回归和安装验收仍未完成。

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

## 5 W1.2 首片：先关闭尚未准备好的运行时

用户要求继续按小步骤推进。本片仅建立整体运行时门禁，不同时移植安全文件或开放业务操作。`platform_support.rs` 是不依赖第三方库的共享生产策略，按真实编译目标判断：Mac 维持原运行路径，Windows 返回 `PLATFORM_STAGE_UNAVAILABLE`，其他未支持系统返回 `PLATFORM_UNSUPPORTED`。没有参数、环境变量或测试开关能放行当前 Windows 运行时；显式平台描述函数只提供诊断数据，不能授权真实入口。

代码接入与副作用顺序经独立源码复核：

- 桌面 `main` 在 Tauri Builder、插件、setup 和 WebView 创建前拒绝，避免进入“连接磁盘或修路径”的既有工作区故障提示。本片的早期桌面错误只写标准错误并非零退出；Windows 图形提示尚未实现或验收。
- `RuntimePaths::initialize`、内部 `initialize_at`、`discover`、`acquire_lock` 在默认根解析、目录/marker/settings、发现文件及锁操作前检查策略；没有为 Windows 增加新数据根或公开自选根参数。
- `RuntimeService::start_with_resources` 和共用 `start_inner` 在资源读取、锁验证、迁移恢复、端口监听和管理器初始化前检查；`ProjectStore::with_resources` 在创建控制目录前检查。独立的 Node 配置写入口也先拒绝。
- CLI 发现入口保留共享阶段错误，转换为现有错误封装中的 `platform`、`capability: runtime` 与 `operationOutcome: not_started`，`retryable` 为 false；不包装成 `SERVICE_NOT_RUNNING` 或在 `env doctor` 中返回普通离线成功。

`npm run test:platform-support` 直接将同一个生产策略源文件用 Rust `--test --edition=2021 --target x86_64-pc-windows-msvc -D warnings` 编译成原生测试程序并运行，5 项全部通过，覆盖 Windows 明确拒绝、Mac 策略保留、其他平台拒绝、诊断与策略一致、实际编译目标选择。构建与结果回执只写新建的 `.test-artifacts/w1-platform-support-Dfl0mX/`；子进程临时目录也在本次证据目录，移除可能覆盖链接输出位置的继承参数，不更改全局环境。

新增脚本语法检查、未知参数非零拒绝及 `git diff --check` 通过；复跑既有 `npm run test:process` 5 项通过。新增策略模块的 `rustfmt --check` 通过。没有改动业务 SDK、模板或前端，本片未重复运行无关业务构建与设备测试。

这是纯策略模块的真实原生编译和运行，未编译 core 的其余部分、ApiError/CLI 适配或 Tauri；不是实际桌面/CLI 拒绝和不写入行为的端到端证明。Mac 仅做显式策略分支断言，未执行 Mac 实机回归。测试没有读取或初始化平台工作区；不能由此声称已经识别已有维护、迁移、恢复或清理现场。

完整 W1.2 的逐操作能力门禁、UI 能力展示、管理器隔离及只读现场识别继续待完成，Windows 运行时仍整体关闭；W1.3 的 ACL、文件身份、重解析点与安全发布也未实施。本片不声称已隔离所有接受任意路径的底层文件 API，更不通过关闭原有安全测试实现移植。

## 6 W1.2 第二片：维护和恢复记录的独立分类

新增内部生产库 `platform-preflight`，由 core 单向依赖并以 `startup_preflight` 导出；只接受记录种类、逻辑所有者 ID 和调用方提供的字节/缺失/不可读观察，不接受目录或文件路径，不执行扫描、恢复、补登记、对账或写入。测试直接编译该库及其锁定的 JSON 依赖，避开尚未移植的整个 core；没有复制生产解析器到测试桩。

本片先覆盖两种没有 `schemaVersion` 的现有记录。独立的严格解码不会复用 `ProjectControl::load` 的缺文件默认值或 `ControlState` 的字段默认值；无法完整识别的旧形状保留为未知，不改写原字节，也不改变 Mac 现有加载器及存储格式。其他记录仍明确未支持分类，不能因局部没有报错就视为完整现场正常。

| 记录与逻辑位置 | 本片范围 |
| --- | --- |
| 工作区 `.platform/project-state/maintenance/{projectId}.json` | 分类维护中、要求恢复和非活跃维护控制；后者不代表项目或运行时可以开放 |
| 工作区 `.platform/operations/{taskId}/journal.json` | 分类未解决操作或已识别的历史已解决记录；同时识别嵌套前后控制状态，核对逻辑 task ID |
| 固定根 `migrations/{taskId}/journal.json` | 后续实现迁移记录解码；目前显式未支持 |
| 工作区 operations 的 `lifecycle.json`、回收站 `record.json` | 后续实现生命周期及回收记录解码 |
| 工作区 operations 的 `snapshot.json`、附件清理/发布日志及持久任务 | 后续纳入完整检查清单与汇总，不能忽略残缺或未解决记录 |
| 固定根 `cleanup/plans/`、`cleanup/receipts/` | 后续实现计划、回执和进度配对检查；任务终态不等于删除完整 |

缺失仅表示本条没有观察到证据，不是 ready；不可读、过大、损坏、未知形状和身份不匹配各有明确分类。已解决恢复记录可以是已提交恢复，也可以是回滚；回滚日志合法地没有 `finalControl`。历史控制快照不覆盖当前控制，记录中的数值身份元组只做格式识别，不冒充 Windows 文件身份；基线 ID 也不证明归档实际存在。该解析器本地的记录字节上限是防止解码无限输入的保护，不是新确认的项目资源限额或数据删除规则。

实际检查：

| 检查 | 结果与范围 |
| --- | --- |
| 独立库原生测试 | 首次定向构建及公开 `npm run test:preflight` 的 `--locked` 检查均 32 项通过；实际运行 Windows x64 MSVC 测试程序，覆盖正常/维护/要求恢复、缺失/不可读、空对象/缺字段/未知字段、重复及转义重复键、嵌套控制、大小边界、错误身份、提交/回滚矛盾和历史快照 |
| 独立库 Clippy | `node scripts/rust.mjs clippy --locked -p platform-preflight --all-targets -- -D warnings` 通过；只检查新库及其测试，不是整个 workspace 的 lint 结果 |
| 格式和差异 | 新库 `cargo fmt -p platform-preflight -- --check`、`git diff --check` 通过；未重排既有 core/宿主源文件 |
| 锁文件 | 只增加本地 package 及 core 的依赖边，外部 crate 的版本、来源和 checksum 没有变化；本轮下载并编译 serde/json 链，没有编译 core、SQLite、CLI 或 Tauri |
| 启动策略回归 | `npm run test:platform-support` 5 项通过；证据在新的 `.test-artifacts/w1-platform-support-SD6Ntk/`，当前 Windows 仍明确拒绝运行时 |

首次子代理环境的 Cargo 因依赖源 DNS 解析失败退出，未进入编译，不登记为通过；随后主环境正常取得既有锁定依赖并完成以上原生测试。没有为此变更全局网络配置、降低 TLS 或漂移依赖版本。测试全部使用合成内存字节，构建产物在忽略的工程 `target/`，没有扫描真实平台数据根。

Windows 整体启动门禁继续关闭；安全文件读取、全部现场汇总、管理器隔离、真实拒绝链路和 Mac 实机回归仍待完成，不以合成字节测试替代这些验收。

本轮启动依赖审查还确认：项目 list/info 可补写登记；项目详情的空间信息可提交扫描任务；工作区空间查询可写缓存；清理预览会创建持久任务和计划。因此后续只读现场检查不能复用这些业务查询，也不能为了读取记录而初始化现有管理器。

## 7 W1.2 第三片：分支隔离与迁移日志分类

用户要求明确 Windows 分支与主线关系，并继续每一小步 commit、push。建立分支前 `main` 与 `origin/main` 均为 `81bd817`，工作区干净；fetch 后确认两者没有差异，随后从该点建立并推送 `codex/windows-compatibility`。分支规则先单独提交为 `eafa2ba`，见 Windows 规划第 5.1 节及 `AGENTS.md`。既有主线历史保持原样；当前没有合回 `main`，也没有把尚未通过的检查改称已通过。

迁移分类继续限制在 `platform-preflight` 的纯字节接口内：期望的应用标识和环境由可信宿主调用方提供，不能从待检查日志中取值当作期望值；任务 ID 与逻辑所有者核对。缺少可信期望身份时明确拒绝，不猜测环境。维护、恢复两类原有输入继续使用其既有分类规则。

本片识别 `MigrationJournal` 当前完整 schema 1 的 21 个字段。`blockedProjects`、`startedAt`、`retainedSourceWarning` 在旧 Mac reader 有默认值，但本片缺少它们时保守归为未知，保留原字节，不自动补成“没有受阻项目”。这不修改旧 Mac 加载器或实际记录格式；若要支持特定旧形状，另行核对并单独验收。

日志阶段与任务阶段分开识别。当前日志写入的阶段仅为 `queued`、`copying`、`publishing`、`committing`、`completed`；`inventory`、`verifying`、`reopening` 等任务展示阶段不冒充已识别的迁移日志阶段。发布前暂存目录身份已记录但 `published=false` 是合法窗口，不能误报成损坏；同样，`committed=false` 可能处在设置已提交但日志未更新的窗口，分类器只能报告还需核对，不能断言未提交或允许回到旧根。

日志中的路径只做文本形状识别，不依照当前 Windows 的路径语义解析 Mac 历史路径，也不访问源、暂存或目标目录。目录身份对象只识别其原有 `{device,inode}` 形状，不充当 Windows 文件身份或真实对象匹配证据。已解决记录及其中的受阻项目、旧根警告都仅是历史资料，不清除当前维护或恢复保护；工作区选择、原子提交、路径与文件身份、任务及设置配对仍需后续实际安全检查。

实际检查：

| 检查 | 结果与边界 |
| --- | --- |
| 原生字节分类测试 | `npm run test:preflight` 的锁定依赖检查 56 项通过：原有 32 项加新增迁移 24 项；覆盖已知中间态、日志提交标志滞后、环境/所有者身份、阶段/标志矛盾、严格字段、epochs/受阻项目、递归重复键、大小边界及不回显记录内容 |
| Clippy 与格式 | 新库 `clippy --locked -p platform-preflight --all-targets -- -D warnings`、定向 `fmt --check` 和 `git diff --check` 通过；独立源码审查未发现必须修复的问题 |
| 启动门禁回归 | `npm run test:platform-support` 原生 5 项通过；证据位于 `.test-artifacts/w1-platform-support-Y9RjG2/`，Windows 继续拒绝运行时 |
| 依赖与执行范围 | Cargo 锁文件、第三方依赖及现有 Mac 记录写入器未改；只运行独立字节分类与策略测试，没有编译整个 core、CLI、Tauri 或运行服务 |

Windows 运行时仍整体关闭；本片不执行迁移、恢复、目录清理或真实工作区扫描，也不宣称 Windows 文件安全、真实 settings 配对或 Mac 实机回归已经通过。

## 8 W1.2 第四片：生命周期与回收记录一起分类

用户允许步骤稍大，本片同时加入生命周期日志与回收记录的识别，依旧只检查调用方传入的合成或历史字节。没有连接管理器、添加扫描器或更改记录写入器；没有调用复制、导入、导出、删除、恢复、永久清除及其重启对账。`Cargo.lock` 和第三方依赖未改，Windows 运行时整体门禁继续关闭。

| 记录 | 当前识别范围 |
| --- | --- |
| operations 的 `lifecycle.json` | 完整 schema 1 的 16 字段；`export`、`copy`、`import`、`delete`、`trashRestore`、`trashPurge` 各自已知的日志阶段、身份、预备控制/目录身份及结果关系 |
| 回收站 `record.json` | 完整 schema 1 的 11 字段；匹配逻辑回收 ID，识别保留、清除中、部分清除及历史恢复/取消/清除记录 |

分类按真实保存窗口处理，不能照搬迁移日志的提交标志组合：

- 六类操作均可能保存已提交但完成记录尚未写好的日志；未解决状态继续保留。复制/导入的基线建立失败也可能已经登记项目，结果为 `registered_maintenance_not_ready` 并携带错误，不能据错误把已提交记录改称未提交。
- 重启对账可在回收恢复的早期阶段确认目录已搬回，随后保存 `committed=true`，但结果仍完全为 `{}`。仅识别这个有来源依据的窄形状，不补造成功结果；它与其他已解决日志一样只作为历史分类。
- `purge_incomplete` 即使 `resolved=true` 仍分类为 `IncompleteOperation`，包括历史清理产生的子任务；`purgeIncomplete` 回收记录保留同样区别，错误列表为空也不能推断清除完整。
- 删除先保存 `state=trashed` 的回收记录，再搬目录，所以 `RetainedTrash` 不是载荷存在或可恢复的证明。`lastTaskId` 可以是后续恢复/清除任务，不要求等于原删除任务。`restored`、`cancelled` 中的控制是旧快照；`purged` 的已知写入器会重置为完整默认控制并清空问题列表，残留元数据形状保守归为未知。

`IncompleteOperation`、`RetainedTrash` 是预检查库的内部证据分类，不增加持久任务终态或项目运行模式。嵌套控制的维护/恢复要求只验证旧快照的形状，不据此设置当前健康状态。目录身份仍只识别旧元组格式；路径不依照当前 Windows 规则解析、不访问实际对象。复制/删除输入中未使用的 `destinationPath` 由旧入口原样保存，因此只核对字符串/null，不把它当作将执行的路径；导出的实际目的地仍识别其非空、无 NUL 文本形状。UI 读失败时显示的 `needsAttention` 未发现持久回收记录写入点，本片保守保留为未知。

缺失/不可读、大小上限、严格字段、嵌套及转义重复键和不回显原始内容继续使用统一入口。快照、附件清理/发布日志、持久任务、清理计划与回执的配对，以及真实文件安全读取与现场汇总仍待完成。这里的“识别到历史已解决”始终不授予启动、恢复、清除或改写权限。

实际检查：

| 检查 | 结果与边界 |
| --- | --- |
| 原生字节分类测试 | `npm run test:preflight` 89 项通过：原有 56 项加生命周期 20 项、回收记录 13 项。真实运行 Windows x64 MSVC 测试程序；覆盖六种操作的阶段/提交窗口、部分清除、历史控制、严格字段与身份、重复键、输入边界及不回显私有信息 |
| Clippy 与格式 | `node scripts/rust.mjs clippy --locked -p platform-preflight --all-targets -- -D warnings`、定向 `fmt --check` 及 `git diff --check` 通过；源码交叉审查后补齐早期恢复空结果的窄例外，并收紧清除完成控制及提交阶段规则 |
| 启动门禁回归 | `npm run test:platform-support` 原生 5 项通过；证据在新的 `.test-artifacts/w1-platform-support-EcgI67/`，Windows 继续拒绝运行时 |
| 分支与范围 | fetch 后 `main`、`origin/main` 仍为 `81bd817`，本片继续在 `codex/windows-compatibility` 提交、推送；没有合入主线。未改 Cargo 锁文件、既有记录写入器或宿主；未编译整个 core、CLI、Tauri，也没有真实文件读取、GUI、Windows 10 或 Mac 实机回归通过结果 |

## 9 W1 只读宿主：隔离业务运行时后实际开窗

用户明确授权继续做到 Windows 应用能够启动。本片开放的是原生桌面窗口与只读状态刷新；项目服务、创建/打开/构建、数据库、附件、维护、生命周期和历史清理仍未开放。

桌面按真实编译目标分发。Mac 原宿主整体移至 `macos_host.rs`，仅调整入口与本地模块路径，正文保留；Windows 使用 `windows_host.rs`，目标依赖不包含 `platform-core`，没有假实现 core 或取消其原有 Unix 安全测试。固定身份、环境、接口版本和纯运行门禁移到轻量 `platform-host`，core 通过原公共路径重新导出，取值和业务运行策略不变。

Windows 仅注册 `host_status`，绑定 manager 窗口、固定管理来源与 `/windows.html`。独立页面不会挂载原业务 App、创建任务或发起 HTTP 请求；未取得检查结果与刷新失败均明确展示，不能用空项目列表冒充正常工作区。拒绝导航至其他页面、新窗口、下载和设备许可；使用单实例插件，关闭管理窗口就退出。

现场观察调用系统 KnownFolder 定位同一 `LocalAppData/dev.localprojects.platform.m0`，不接受任意根参数、不创建缺失根、不解析 settings 选择、不启动已有运行时。句柄逐级检查 NTFS 固定本地卷、重解析点、文件身份、控制文件硬链接、当前用户所有权和已识别私有 ACL，并有界浅枚举已知顶层目录及其他条目。所有已有根均阻断业务，不把未分类的快照、清理、任务或孤立载荷忽略为安全；计数仅是浅层条目，不代表记录已经解码或操作完成。

该观察器没有读取控制记录内容、令牌、数据库、附件或源码；此前五类纯字节分类仍保留独立测试，尚未与完整文件扫描配对。本片不宣称 DRIVE_FIXED 等于物理内置盘，也没有验收 SUBST/DOS 别名、完整并发路径策略、业务私有文件写入或安全发布。因此 W1.2/W1.3 的完整退出条件仍未满足，开放项目写入前继续落实。

WebView2 即使处于 InPrivate 模式也会使用用户数据目录。为避免 Tauri 默认创建业务根，本片通过 KnownFolder 在其外新建 `local-projects-dev-w1-webview-<GUID>` 可丢弃缓存，只采用 create-new，显式设置当前 SID/SYSTEM/Administrators 的受保护 DACL，验证目录/权限/卷后返回，不复用旧目录，也不自动清理缓存。它不是替代业务根或业务安全写入后端，不承诺隔离有权直接修改数据的同用户进程。

真实原生测试否定了早期缓存长锁方案：只申请元数据访问时，共享标志没有提供预想的互斥；改成实际读取/列目录权限后，长期锁住祖先又阻碍了浏览器所需的子文件原子重命名。因此缓存仅在创建时持检查句柄，交给 WebView2 前全部释放。探针自身的短时保护测试继续保留；Windows 文件系统夹具因共用祖先而在该模块内串行，其他单测照常执行。没有把失败的长锁设计或静态推断记为通过。

宿主还在开窗前只读拒绝 WebView2 的适用路径/参数覆盖环境变量与注册表策略，不读取或回显策略内容、不修改全局设置；创建失败用原生消息框说明并退出，不回落到默认 profile 或别的工作区。依据为[微软环境创建说明](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/webview2-idl#createcorewebview2environmentwithoptions)。

Windows 专用 Tauri 配置关闭安装器与 Mac sidecar/resources；开发脚本使用 Node 调用 Tauri 的 JS 入口，Windows 构建仅输出内嵌页面的原生 exe。没有顺便确认 NSIS/MSI、签名、离线 WebView2 或正式发行方案，也没有把 CLI、SDK、项目模板称为已随此 Windows 宿主交付。

本机 LocalAppData 另有工具环境授予的权限；首次 release 启动在父目录私有 ACL 检查处拒绝并以 101 退出，未创建业务根，不记为开窗通过。随后明确区分要求：既有缓存父目录核对当前用户所有权、路径和卷，新缓存才采用并完整验证私有 DACL，父目录 ACL 不修改；业务根探针仍保留严格私有 ACL 拒绝规则。同时修复 Tauri setup 错误会转成 panic 的处理方式，在 setup 内先显示原生错误再退出。

实际检查：

| 检查 | 结果与范围 |
| --- | --- |
| 原生宿主测试 | `npm run test:host`：`platform-host` 26 项、桌面来源限制 1 项通过；含 13 项真实 Windows 文件夹具，全部位于新建的 `.test-artifacts/windows-host-disposable-*`，未用真实项目作夹具 |
| 字节与界面回归 | `npm run test:preflight` 89 项；新 Windows DOM 4 项、原共用 UI DOM 33 项；构建路由 4 项与 process 环境 5 项通过。DOM 结果不替代真实 WebView2 验收 |
| 检查与构建 | 两个宿主 package 的 `clippy --locked --all-targets -- -D warnings`、新 host 定向格式及差异检查通过；TypeScript/Vite 双入口、Windows 原生 debug 与内嵌页面 release 构建通过；外部 Cargo 版本/checksum 未变 |
| 实际开窗与刷新 | 从 release exe 启动真实 WebView2/InPrivate 管理窗口；页面为 `http://tauri.localhost/windows.html`，显示真实固定根缺失；点击刷新后检查时间由 `09:44:25` 更新至 `09:44:50`（Asia/Shanghai），没有运行 Vite 或项目服务 |
| 单实例与退出 | 再次启动同一 exe 后仍只有同一 manager 窗口与一个桌面进程；通过标准 Alt+F4 关闭后桌面进程数量为 0，没有借终止进程代替正常退出 |
| 数据边界 | 首次启动前、开窗/刷新/再次启动后、正常退出后，固定业务根均不存在；没有为测试创建或切换业务根。浏览器只使用新建的私有可丢弃缓存；现有项目/SDK/备份/维护/回收/恢复证据未被应用访问或改写 |

最终 release exe 为 `target/release/local-projects-desktop-dev.exe`，8,974,848 字节，SHA-256 `c356db298242fa2105b82f26b4e251710e7cb7584a56745e377bd587dc504250`。最终版本于 `09:48:09` 重新开窗，仍显示固定根缺失，桌面进程为 1；保留窗口供用户查看。实测摘要保存在忽略的 `.test-artifacts/w1-desktop-startup-joZHty/result.json`，exe、浏览器缓存及本机证据不进入 Git。

上述真实宿主验收仅覆盖当前 Win11 x64、普通用户及已装 WebView2 `155.0.4283.62`；不推广为 Win10、物理设备、安装/更新或完整业务功能通过。Mac 仅核对入口正文和常量迁移不变并通过共用 UI 回归，没有 Mac 原生编译或实机结果。

## 10 待完成与下一步

1. 本机工具安装、独立编译链接与环境干扰回归已完成；不再把系统许可列为当前阻塞。后续修改工具链或脚本时按影响重新验证，不能沿用旧版本证据。
2. Windows 10 22H2 x64 测试环境尚未落实，本轮 Mac 回归未执行。W1.1 的双系统环境安排未满足，故保留进行中；不能把当前机器检查推广为两个系统或 Mac 通过。
3. W1 已推进至独立只读宿主，完整业务运行时仍拒绝。W1.2 继续补齐现场格式/文件配对与能力隔离，W1.3 继续完善业务操作所需文件安全；W1.4 的 Win10 与其余宿主矩阵继续待完成。安装形式、WebView2 分发/最低版本和正式发行条件继续待定。

成品用户运行带有效网页产物的项目无需 Node、Rust 或 C++ 工具；修改业务源码后才需要系统 Node/npm 来构建。内置 Node、外部网页应用文件夹安装和资源限额讨论稿没有在本轮获得新确认。
