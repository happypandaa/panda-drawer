# 开发环境与数据隔离

2026-10-20 最新用户选择：本轮 Node 应用秘密不用 macOS Keychain，放在各自本机受管槽 `configuration/credentials/`，目录 `0700`、文件 `0600`，以当前用户文件权限保护明文，不能称为系统加密库；不读删旧 Keychain、不写作者源码／分享包／Git。当前范围见[本机配置与运行](node-native-configuration.md)，仍不包含完整 project-v1 Node 生命周期。本文较早的 OS 凭据库建议保留为历史或其他未来能力讨论，不作为本轮 Node 槽的执行要求。

状态：M5 功能与内部包已有交付，基础自动化、真实服务、进程重启及兼容结果见 [M5 原记录](m5-verification.md)。后续清理历史已通过[原生 GUI 验证](history-cleanup-gui-verification.md)；WKWebView 文件选择、原生保存框、剪贴板、真实录音拍照与持久回放、运行门槛和 HTTPS/CSP 通过的实际范围见 [M5 浏览器实测续记](m5-browser-real-verification.md)。Safari/Chrome、拒绝权限、独立系统设备释放测量和物理断网仍未完成，不将 M5 三端验收标为全部通过。历史验证见 [M0 记录](m0-verification.md)、[M1 记录](m1-verification.md)、[M2 记录](m2-verification.md)、[M3 记录](m3-verification.md)及 [M4 记录](m4-verification.md)。产品名已确认为「熊猫应用小抽屉」，简称「熊猫抽屉」；[名称与图标](branding.md)已接入内部代码，固定开发身份和数据根保持。

2026-10-19 新增方向：平台内置 Node，运行用户应用自己的 Node 后端，统一启动、停止及展示运行状态；总体方案见[受管 Node 运行时](node-app-runtime.md)，说明文件与协议见[启动规范草案](node-app-startup-spec.md)，小步安排见[实施计划](node-app-implementation-plan.md)。共享运行时资源，各应用独立受管进程；AI、TTS、音乐和业务 SDK 由应用实现，Claude Code 不是平台必需依赖。该方向取代固定 Claude Code／网易云连接器设想，本轮仅整理方案，未写运行代码；当前程序仍是静态网页与 Rust 核心，未内置 Node 或实现应用后端。现有静态 v1 格式、SDK/CLI/HTTP 和系统 Node 构建流程保持原语义，新类型及协议仍为草案。受管进程不是沙箱，平台密钥管理没有因此实现。

2026-10-20 当前进度：用户已授权开始尝试并允许修改 radio 源码。已新增私有 Node 资源准备、实验声明读取与隔离进程管理，实际记录及复现入口见[首次受管试验](node-runtime-trial-verification.md)。既有项目打开与管理 API 未接 Node，新类型的界面、秘密存储和归档保护仍未完成；下文较早“未实现”记录按原时点解释。

2026-10-20 新增需求：作者声明 Node 应用所需变量的用途、获取方式与必填项，由平台提供 API 地址、AI Key 等可视化配置；普通使用者无需编辑 `.env` 或配置终端。目标后端运行凭据管理已纳入路线，见[应用配置草案](node-app-configuration.md)，不再一概后置。配置表单、系统凭据库存储、启动环境注入及相应 SDK/API 均未实现，具体保护和重启策略仍为草案建议，尚未逐项确认。

草案建议秘密只在受信管理页输入时短暂持有，保存后仅返回已设置状态；业务网页、普通 GET 接口和 AI 交接不获得 Key，目标 Node 后端会读取自己的注入值。秘密使用操作系统凭据库、普通配置使用受管私有数据，Mac／Windows 分别验收；凭据库不可用明确失败，不回落明文。用户填写值不写清单、源码、`.env`、日志或分享 ZIP；只注入目标应用进程创建环境，不修改全局环境，并拒绝未声明及 Node／系统控制变量。保存后提示待重启，不自动重启后端。

默认备份、导出和复制不携带秘密值或可用秘密绑定；原实例同机恢复保留当前绑定，不随旧备份回滚 Key，新副本及导入重新填写，重装失去绑定也重新填写。这些生命周期规则仍须随配置草案定稿；真实带 Key 运行前，先验证秘密存储、目标进程注入、日志与生命周期保护。现有静态 v1、系统 Node 构建和通用云连接器／任意代理范围不因该需求改变。

2026-10-19：W1 独立只读宿主已有本机 Win11 x64 的 release 开窗、状态刷新、单实例及正常退出记录。当前代码进入 W2.1：已实现固定默认根下创建可丢弃新样本、桌面/普通浏览器打开预编译欢迎应用，以及真实空库结构只读查询和会话事件。受限 HTTP 服务不链接完整业务 core；完整 core/CLI 仍受门禁保护，SQL 执行、附件、构建、维护、备份、恢复、导入、回收、迁移和清理未开放。仅识别本 W2 阶段已完成登记且完整匹配的样本，旧历史、创建残留、未知条目或外部修改均保留并阻断，不自动修复、补登记或接管。

本轮原生 host 后端 62 项、共享格式 14 项、desktop 3 项及相关 Clippy/fmt 通过，Windows 界面、构建入口及资源测试 28 项通过；既有 preflight 96 项结果保留。另有自写费用分摊计算器的 10 项功能 DOM 测试通过。本机 Win11 的 release 已从资源管理器实际创建并打开欢迎应用和计算器，读取真实项目名与空库；100 元分给 3 人、刷新清空、正常退出并重启后两个项目再次打开及计算均通过。原项目 38 文件、创建后全部 73 文件的摘要一致，未产生 WAL/SHM，应用进程树没有启动 Node。Chrome 页面验收因自动化无法可靠识别 URL 而停止；旧浏览器会话跨实机重启失效尚未补验。W1/W2.1 退出条件仍未全部满足。Win10、Mac 回归、无 Node 干净机器、物理断网、跨登录会话与断电均未实测。当前机器使用已安装的 WebView2，安装器及 WebView2 分发仍未确定。证据分别见 [W1 记录](windows-w1-verification.md)和 [W2 记录](windows-w2-verification.md)。

Windows 开发机运行 `npm run dev` 或 `npm run build` 会先执行资源准备：Node/npm 在打包时构建欢迎网页，核对源码/产物摘要及 SDK 0.1.0 原字节，再把源码、SDK、网页、skill 和文档内嵌到 exe。`npm run build` 生成 `target/release/local-projects-desktop-dev.exe`；`node scripts/desktop.mjs build --debug -- --locked` 生成 debug exe，均不生成安装器或捆绑尚未开放的 CLI。成品 exe 创建/打开样本不调用 Node/npm，不依赖 Vite 或资源旁文件；这一实现边界不代替无 Node、物理断网实测。

开发机可用 `node scripts/desktop.mjs build --windows-template "D:\完整模板目录" -- --locked` 预编译独立验证样本。目录须包含 `app/`、`template.json` 和 `README.md`，固定 SDK 原字节不变；先复制完整普通文件快照，再编译该快照。费用分摊示例使用独立版本 `windows-split-bill-0.1.0`，旧欢迎样本继续可读。未提供参数时仍使用原欢迎模板，Mac 流程不变。这是构建平台 exe 时选择初始资源的开发选项，不是成品软件的源码导入、`local-app.json` 安装或用户构建/发布入口；不能直接覆盖已有受管项目源码或登记。

两端管理界面现共同使用 `ManagerView.tsx`、`ProjectCard`、`Modal` 和 `styles.css`；平台控制器保留真实请求及阶段门禁。Windows 只使用已有五个管理 IPC，不挂载完整维护、元数据、回收站和构建面板。Windows 管理页 18 项、原管理页 34 项 DOM 回归与双入口生产前端构建通过，实机状态及 Mac 原生回归边界见[共用界面记录](shared-manager-ui-verification.md)。

直接运行 Rust 宿主检查前先生成嵌入资源。以下命令检查新样本实现，不调用平台 CLI 或接管已有用户项目：

```powershell
node scripts/prepare-windows-resources.mjs
npm run check:host
npm run test:host
node scripts/rust.mjs test --locked -p platform-project-format --lib
npm run test:preflight
npm run test:windows:ui
npm run test:desktop
node --test scripts/windows-resources-test.mjs
npm run build
```

`prepare-windows-resources.mjs` 只生成忽略目录内的平台资源，不创建业务数据根；首次准备可能需要下载构建依赖。合成 DOM 与资源测试不替代真实 WebView2、普通浏览器或系统权限验收。全量业务核心和 Mac 宿主回归继续在相应 Mac 环境执行，下面全量管理流程仍指 Mac 已有路径。

Windows 开发者使用 `npm run env:windows` 查看系统、卷、WebView2、Node/npm、Rust 和 MSVC/SDK；`node scripts/windows-environment.mjs --json` 输出可保存的 JSON。脚本不安装工具、不扫描或初始化平台工作区，不执行平台 CLI，也不证明默认路径的 ACL、链接或磁盘边界已通过。仓库本地 Rust 放在 `.tools/cargo` 与 `.tools/rustup`，Windows 的 `cargo.exe` 仅加入子进程环境，保留原 PATH，不改全局设置；`npm run test:process` 验证这项环境选择逻辑。

MSVC/SDK 安装完成后，可用 `npm run check:windows:toolchain` 验证独立 C 静态库与 Rust 的 x64 编译、链接和运行。该命令在唯一的 `.test-artifacts/windows-w1-toolchain-smoke-*` 中保存源码、产物和报告，不清理旧证据、不构建或启动平台；它与只读环境盘点是不同检查。需要纯 JSON 时执行 `node scripts/windows-toolchain-smoke.mjs --json`。成功只证明本机开发工具链小样，不代表 Windows 产品兼容或 bundled SQLite 通过。

平台使用者与平台开发者的要求不同：运行现有静态网页产物不需要用户安装系统 Node/npm、Rust、Visual Studio 或独立 SQLite 服务；修改业务源码后构建才需要系统 Node/npm，Rust/MSVC/Windows SDK 只用于开发平台本身。Windows W1 原环境准备不包含 Node 内置分发；本次另确认的私有运行时方向及尚未实现的状态见第 7 节，已有完整项目 ZIP 和拟议网页应用包的规则不因此改变。

`npm run test:platform-support` 直接使用所选 Rust 编译、运行 `platform_support.rs` 的无依赖生产策略测试，输出位于新的 `.test-artifacts/w1-platform-support-*`，不启动平台或读取用户工作区。Windows 使用真实 `x86_64-pc-windows-msvc` 目标；Mac 分支的策略断言不是 Mac 实机回归。这个独立检查不编译整个 core、CLI 或 Tauri，不能替代后续完整构建、入口拒绝和现场保护验收。

`npm run test:preflight` 通过根 Cargo 锁文件单独编译、测试 `platform-preflight` 生产库和合成的维护/恢复/迁移/生命周期/回收记录字节，不编译整个 core 或启动任何服务。该库只有 JSON 解码依赖，不接受用于读取文件的路径，也不执行恢复、修复或写入；它不会访问平台数据根。记录中的路径只作为文本识别，迁移的可信环境身份由调用方提供，不能从待检查记录中自行取作期望值。生命周期日志结束不等于清除完整，回收状态也不证明载荷实际存在；内部分类不会授权恢复或删除。首次测试需要取得锁定的 Rust 依赖，不能据此承诺离线平台构建。解码分类与安全文件读取分别验收，后者及完整现场汇总尚未完成。

## 管理界面入口

“我的项目”采用用户确认的 Radio Bob 风格启动台，图标下显示名称，点击进入独立原生窗口；右键、键盘菜单键或图标旁“更多操作”进入详情及删除确认。搜索仍匹配名称、简介或 ID，状态筛选、刷新、添加本地应用及导入集中在“添加”菜单。网页项目删除前仍须核对，移入回收站后可在“空间与工作区 → 回收站”恢复。Node 只显示绿色六边形类型角标，配置、运行和异常状态分别显示；详情与读屏保留完整中文类型。

顶部保留“我的项目 / 空间与工作区”两个入口及 DARK / LIGHT 外观开关。主题只保存在管理界面的本机偏好中，不修改项目、工作区或业务网页；设置齿轮内集中运行环境、关于及使用指南，高级 Node 检测配置、程序信息按需展开。管理窗默认为 960×640，最小为 640×480，窄窗采用四列并滚动内容。实际验证见[启动台界面记录](radio-launcher-verification.md)。

业务应用窗口另有可选的清单 `window` 初始尺寸，默认 1080×780；之后默认记住每个项目的上次普通窗口逻辑尺寸。该本机偏好独立保存在固定数据根 `app-window-sizes.json`，不修改业务数据库或工作区设置；声明字段、关闭保存、迁移隔离及已登记 Node 声明摘要门禁见[应用窗口尺寸](app-window-runtime-contract.md#应用窗口尺寸)。管理窗尺寸与此分开，原生拖动缩放等实际验收另记。

“详情”弹窗集中展示概览、空间与文件、备份维护、更多操作；原生窗口进入、构建和 AI 交接也在详情中。未提交的基本信息、备份备注和导出/复制表单在页签间保留，关闭时提示处理；提交或原生文件选择尚未完成时阻止关闭。切换页签会撤销删除、恢复与结束维护的确认，重新操作需要重新核对。

新建成功后直接打开项目详情，点击“复制 AI 接手说明”获取可直接发送给其他 AI 的中文说明，包含熊猫应用小抽屉的用途、当前业务项目目录与 ID、实际 skill/CLI 路径、文档与构建/数据库操作方式。复制仅由点击触发；同时展开全文，剪贴板不可用时可选取手动复制。接收说明的 AI 需要能访问本机目录才能直接修改文件。说明按当前平台生成，不覆盖旧项目文档；项目移出列表或工作区切换后会撤销旧的待复制结果。

“空间与工作区”使用“空间总览 / 工作区位置 / 回收站 / 清理历史”页签。默认只显示三类空间摘要与项目排行，明细按需展开；工作区迁移需先展开表单并明确确认。所有维护、删除、恢复及迁移继续使用已有原生任务与门禁。

空间总览常显“为什么删除项目后仍有占用”，解释当前回收站、历史备份、恢复操作现场、共享缓存、迁移保留目录和软件文件；这些数字属于上方范围，不另行重复相加。项目列表清空不等于抹除这些资料；永久删除回收项仍保留历史备份。历史备份仍不提供手动删除入口；迁移旧目录、已完成恢复的原现场和回收项可在“清理历史”中扫描范围、显式选择并确认后处理，未解决现场继续保护。统计是文件逻辑大小，不能直接视为可回收磁盘空间。

## 1 开发身份

所有当前源码运行与内部打包产物均固定为：

| 项目 | 值 |
| --- | --- |
| 显示名称 | 熊猫应用小抽屉 / 熊猫应用小抽屉 · 开发版 |
| 应用标识 | `dev.localprojects.platform.m0` |
| 环境 | `development` |
| CLI | `local-projects-dev` |
| 数据根 | `~/Library/Application Support/dev.localprojects.platform.m0/` |
| 工作区 | 初始为数据根下 `workspace/`；公开迁移后为固定设置中已提交的新位置 |
| 服务发现 | 数据根下 `runtime/connection.json` |
| 凭据 | 数据根下 `runtime/control-token`，仅当前用户可读写 |

产品名称已经确认；上述应用标识、CLI 与数据根继续属于隔离的开发身份，不据此建立正式生产实例。当前暂不提供生产构建或根目录环境变量覆盖。用户通过公开工作区迁移选择新位置，不能直接编辑设置跳过迁移。CLI 编译时绑定相同身份，只查本开发环境，不回退连接其他平台实例；固定数据根、设置、凭据、发现文件及锁成套隔离。

初始化验证开发环境标记和所选工作区；非默认位置必须有固定迁移日志、已发布的目标标记及匹配的目录身份。拒绝不明的已有非空目录、错误身份、符号链接和无依据的设置重定向；已提交目标缺失时拒绝启动业务，不重建为空目录或自动打开旧副本。回收站的显式永久清除只删除选中项目副本；没有通用清除全部数据的命令。旧迁移根的清理只接受已登记、可核对并明确确认的计划，见[历史清理补充](history-cleanup.md)。自动测试使用临时目录，真实实例脚本只修改明确新建的可丢弃项目。

2026-10-19 修复跨重启设备号变化误判：macOS 新持久身份使用卷 UUID 与 inode，运行期 fd/设备号/父链检查继续保留。旧版已完成迁移目标可通过离线 `workspace identity-repair-plan --json` 预检，再以精确计划、工作区、卷 UUID 及显式确认调用 `workspace identity-repair --input ... --json`；原件保留，范围及不支持的情况见[身份兼容规则](workspace-identity.md)。该入口不启动服务、不改变工作区位置，不替代公开迁移。

## 2 运行与构建

在根目录执行 `npm ci` 后使用 `npm run dev`。准备步骤构建 CLI，在临时目录按锁文件构建标准模板，再生成包内 SDK、模板源码、预编译初始网页、skill 和说明资源副本；Tauri 启动管理界面和 Rust 服务。模板构建发生在平台开发／打包时，不发生在用户创建项目时。Vite 开发地址固定为 `127.0.0.1:1420`，端口被占用时不静默选择另一个端口。

Rust 服务仅绑定 `127.0.0.1`，优先复用开发环境上次端口，冲突时分配可用端口。当前提供项目登记、SQL 与托管事务、附件、SSE 通知，以及系统 Node 构建任务。每个项目有独立 `data/main.sqlite`，活跃时由专用线程持有 SQLite 连接，同项目串行、不同项目并行；现有静态网页不启动 Node 服务，新增应用后端尚未实现。

按用户同日最新要求，macOS 平台管理窗口 X 只隐藏管理窗口并保留 Dock，已开的业务窗口、受管 Node 和平台服务继续运行；点击 Dock、Reopen 或重复启动恢复同一个管理窗口及其页面状态，不创建第二套服务。菜单“退出开发版”／Cmd+Q 才走整个平台的正常退出流程；非 macOS 管理窗口 X 维持正常退出。关闭业务窗口只收尾本应用及其受管 Node／普通子进程。全部应用统一“我的项目”，详情／配置采用页内 tabs 弹窗，缺必填时进入先开配置，不禁用整个入口，保存不自动启动。此前“管理窗口 X 退出”的实测保留为旧版本结果，不证明新要求已验收。明确退出先停止接收新工作、取消未发布的上传、结束构建子进程及其普通进程组、记录未完成任务状态，等待已接受的数据库工作与项目创建完成，再关闭服务并仅移除本实例发现与凭据。慢速下载在退出收尾时可能断开，已保存文件不受影响；锁文件本身不递归删除。异常中断留下的发现信息不能作为在线依据；CLI 会进行身份与实例握手。

平台界面变更由 Vite 更新；Rust 变更由 Tauri 重新编译启动。开发构建和包构建属于相同开发环境，同一时刻只运行其中一个；先明确退出再切换，不能将单实例复用误当作运行了新二进制。

`npm run build:app` 生成内部 `.app`，`npm run build` 生成 `.app` 与 DMG，输出位于根 `target/release/bundle/`。页面资源全部本地打包，无需 Node 服务运行。CLI 作为原生外部二进制打入包内；包内包含固定版本 `@local-projects/sdk 0.1.0` tarball、匹配说明、`react-ts-vite-0.1.4` 标准模板及其初始网页，以及 `local-projects-app` skill。这些仍属于内部开发包，尚未作为正式产品对外发行。

2026-10-20 起，标准 Mac 打包先检查运行状态；生成目录中的新旧任一名称的 `.app` 正在运行时明确拒绝打包，须先从菜单退出。新品牌「熊猫应用小抽屉」及历史「Local Projects Dev」两个确切名字的旧生成应用与 arm64 DMG，分别在核对固定身份、各自包名、签名及镜像后暂存；新构建和验包全部成功才删除旧生成副本，`build:app` 成功后也删除失效的旧 DMG。构建或验包失败恢复上次可用生成包。范围仅 `target/<release|debug>/bundle/`，不扫描或清理安装目录、项目、数据、备份及 `.test-artifacts/`。自定义输出／身份参数不进入自动清理，`dev`、`--no-bundle` 和非 Mac 构建不清理包。中断后若留下 `target/.desktop-package-build.lock` 或 `.desktop-package-previous-*`，保留恢复现场，核查后处理，不自动删除。保护性回归：`node --test scripts/desktop-package-output-test.mjs`。

## 3 工具链与校验

Node 支持 `^22.12.0 || ^24.0.0`，使用系统 npm。可使用系统 Rust，也可安装到工程 `.tools/`；脚本优先使用项目私有 Cargo/Rustup 并只向子进程设置对应环境，不修改 shell 配置。Cargo 与 npm 依赖锁文件随工程保存。

本次验证使用 Rust 1.99.0。没有系统 Rust 时，可在仓库根目录执行以下步骤安装同版本的工程私有工具链；安装脚本来自 Rust 官方，`.tools/` 不进入发行包：

```sh
mkdir -p .tools
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o .tools/rustup-init.sh
CARGO_HOME="$PWD/.tools/cargo" RUSTUP_HOME="$PWD/.tools/rustup" \
  sh .tools/rustup-init.sh -y --no-modify-path --profile minimal \
  --default-toolchain 1.99.0 --default-host aarch64-apple-darwin
```

`npm run check` 和 `npm test` 自动准备 CLI 与资源后再校验，不依赖先启动一次桌面；`npm run test:sdk` 单独执行浏览器 SDK 测试。`node scripts/rust.mjs …` 将参数传给 Cargo；需要格式和 lint 检查时另装 rustfmt/clippy 组件。所有 macOS Rust 子进程统一使用 `MACOSX_DEPLOYMENT_TARGET=15.0`，桌面和包内 CLI 均按相同最低系统构建。

工程开发依赖、Rust 工具链和中间产物分别位于仓库的 `node_modules/`、`.tools/` 和 `target/`，它们不是用户工作区。macOS/WebKit 可能在当前开发应用标识对应的 Library 缓存目录生成浏览器缓存；这些缓存不存储项目业务数据，也不与正式身份共享。自动测试用系统临时目录并在结束时清理。

```sh
npm run check
npm test
npm run cli -- service status --json
npm run cli -- env doctor --json
```

CLI 的状态命令离线时报告结构化错误，不启动应用、不初始化数据、不仅凭端口可连接就认为服务正确。`env doctor` 在线时返回桌面服务实际选定的 Node/npm 路径、版本与可用性；离线时只报告平台连通性和构建环境尚未读取，不把 CLI 自己的 PATH 当作服务环境。项目 schema 使用 `db schema`。

单元和服务测试重点覆盖目录身份、符号链接、互斥锁、文件权限、退出清理、HTTP 授权与 CLI 目标校验。开发测试和打包测试都要核对 UI 实际显示的资源状态；在只有浏览器页面时显示“需要桌面宿主”，不生成虚假的运行信息。

## 4 内部包的限制

最低打包系统设为 macOS 15，目标为 Apple Silicon。内部包采用 ad-hoc 签名；没有 Developer ID 公证、互联网下载首启或最低系统实机通过记录时，不称作已完成正式分发。正式名称、应用标识、签名资质与设备验收在相应发行阶段完成。

当前阶段不为用户添加登录启动项、后台守护程序或全局 CLI 快捷方式；新内部构建已接入私有 Node 资源准备，实际应用包验证见首次试验记录；不携带 Rust 开发工具链。Node 后端只在隔离实验路径使用，不自动接管现有系统 Node 构建或已安装的旧应用。更新或卸载正式软件的规则继续按发行契约执行，本开发标识不读取正式目录。

## 5 当前项目和工具入口

项目实际目录：`workspace/projects/{id}/`，包括 `project.json`、`README.md`、`AGENTS.md`、`CLAUDE.md`、`.agents/skills/local-projects-app/`、`docs/database.md`、`docs/platform.md`、`meta/`、`app/`、`dist/`、`data/main.sqlite`、`data/attachments/`、`.platform/`。新项目同时复制可构建的标准源码和平台预编译的欢迎应用，校验两者与打包摘要一致，再完成项目登记；不创建业务表或示例数据。新项目立即可在桌面和浏览器打开，创建及打开不调用 Node/npm、不下载依赖。项目登记与文档代次保存在工作区 `.platform/project-state/{id}.json`；登记还验证项目目录和数据库文件身份。数据库缺失、符号链接、硬链接或未经协调替换时拒绝操作，不创建空库蒙混通过。

给 AI 的窗口约束现由 skill 主入口链接到随目录携带的 `references/app-window-runtime.md`；它说明原生单窗口、打开自动启动 Node、关窗停止、禁止额外窗口／业务弹窗，并先区分 project-v1 与独立 Node 源码槽。主规则见[应用窗口契约](app-window-runtime-contract.md)，全部文档分类与阅读顺序见[文档地图](documentation-map.md)。准备资源／新建时沿用整个 skill 目录复制，不为补说明覆盖既有项目文件。

仓库 `skills/local-projects-app/` 是 skill 的维护源，准备步骤复制到包内资源，新建时复制到项目 `.agents/skills/local-projects-app/`。README、AGENTS.md 和 CLAUDE.md 引用它；`project handoff` 和“复制 AI 接手说明”提供实际项目、README、skill 和 CLI 绝对路径。skill 说明业务项目的读取、修改、数据库操作、构建、验证、备份维护、生命周期与迁移后的交接，不要求 AI 修改平台工程。资源更新只用于新建项目和安装包后备说明；旧项目的 skill、SDK、源码和文档不随升级覆盖，调用新能力前核对当前 CLI 帮助。它不是全局安装项，不承诺所有 AI 客户端自动识别；无法自动发现时按交接文本显式读取。

项目 ID 由平台生成。下列命令均支持 `--json`，复杂输入使用文件或 `--input -` 从标准输入读取；不要把 SQL、令牌或多行 JSON 拼入 shell 命令。

| 命令 | 当前行为 |
| --- | --- |
| `project list` | 项目列表及真实状态 |
| `project create --input request.json` | 输入 `name`、可选 `description`；创建可直接打开的初始应用、匹配源码、skill、文档和独立空数据库 |
| `project info --project ID` | 清单、目录、状态、代次和来源状态 |
| `project metadata get --project ID` | 基本信息、图标状态及 `metadataRevision` |
| `project metadata set --project ID --input metadata.json` | 带 `expectedRevision` 修改名称、简介或图标；维护时携带 `--maintenance ID` |
| `storage info --project ID` | 已扫描空间摘要、完整性、过期状态与问题；未知值不当作零 |
| `storage scan --project ID` | 提交只读空间扫描任务 |
| `storage files --project ID --input filters.json` | 附件筛选及游标分页 |
| `storage cleanup --project ID --input cleanup.json` | 显式白名单清理任务；维护时携带 `--maintenance ID` |
| `workspace storage` | 工作区与关联历史、保留旧根、平台程序空间信息 |
| `workspace cleanup-plan` | 检查已登记历史范围并返回异步预检任务，不删除文件 |
| `workspace cleanup --input cleanup.json` | 以计划 ID、已选择对象 ID 和明确确认词提交历史清理任务 |
| `workspace identity-repair-plan` | 服务离线时只读预检旧版已完成迁移目标的身份兼容范围，不初始化环境 |
| `workspace identity-repair --input repair.json` | 服务离线时以精确计划、工作区、卷 UUID 和 `confirm: true` 绑定已核对旧记录，保留原件 |
| `project url --project ID` | 60 秒单次使用的浏览器入口 |
| `project refresh --project ID` | 通知页面重新查询 |
| `project handoff --project ID` | 返回项目路径、README、skill、CLI 路径与可交给 AI 的说明；不改写项目文件 |
| `project build --project ID` | 提交构建，立即返回任务 ID |
| `task get --task ID` | 读取持久任务状态与结果 |
| `task wait --task ID --timeout 30` | 有界等待；超时返回当前任务，不取消任务 |
| `task logs --task ID --cursor N` | 读取日志片段；用返回的 `nextCursor` 继续 |
| `env node set --path /绝对路径/node` | 检查并保存开发版的显式 Node 路径 |
| `env node clear` | 恢复自动探测 |
| `db schema --project ID` | 实际表、列、索引、外键、视图 |
| `db execute --project ID --input request.json` | 一条 SQL 与位置参数 |
| `db transaction --project ID --input request.json` | 有序语句整体提交或回滚 |

例如 SQL 输入：`{"sql":"SELECT ? AS value","params":[{"$int64":"9223372036854775807"}]}`。大整数与字节按主契约无损传输；超预算结果报错，受事务保护的写入回滚。连接配置、附加数据库、扩展加载和调用方自行控制事务被拒绝。当前暂不提供 `VACUUM` 这类不能放入托管事务的维护 SQL；普通 DDL/DML 可用。

管理 CLI 通过固定开发发现文件、实例握手和 Bearer 凭据访问。项目网页只拿到项目会话 Cookie；入口 HTML 注入 `platform-document-epoch` meta，SDK 在当前文档中固定读取一次并随请求携带。ticket 在 URL fragment 中传递，连接页兑换前移除 fragment。不同项目 Cookie 不能串用，服务重启会使旧会话失效。首版仍只运行用户信任的自建项目；共享 origin 不是恶意网页的隔离沙箱。

`examples/m1-runtime` 仍只用于 M1 底层回归。当前新项目直接使用随包初始应用，后续源码修改通过持久构建任务发布；旧项目不自动补写或覆盖源码、SDK、锁文件、文档、skill 或初始网页。旧网页缺少构建来源时显示 `unknown`，仍允许使用；合法的旧项目没有产物时保留 `unbuilt`。旧项目未自带 skill 时，交接只读引用包内后备 skill；采纳新模板或写入项目 skill 需要 AI 按用户任务明确修改。

## 6 M1 验证与参数状态

`npm run test:m1:live` 需要已启动的开发实例和 `target/debug/local-projects-dev`。它新建两个明确命名的测试项目，只在这些可丢弃项目安装 M1 受控产物，不删除旧项目、不覆盖既有项目产物。当前带初始应用的运行时通过平台构建发布测试页面，因此该回归脚本需要可用的 Node/npm；历史 M1 运行时仍直接复制受控页面。这不改变普通新项目创建和打开无需 Node 的规则。可通过 `M1_TEST_CLI` 指向当前开发包内 CLI；该覆盖只选择测试客户端二进制，平台自身没有可绕过开发隔离的根目录覆盖参数。

真实实例验证结果写入仓库忽略的 `artifacts/m1-live-test.json`。用于下一次重启验证的旧会话仅存入 `artifacts/m1-old-sessions.json`，权限为 0600，不进入项目源码或版本库。完整退出重启后运行 `npm run test:m1:restart`，结果保存到 `artifacts/m1-restart-test.json`。

2026-10-15 用户选择继续把限额作为**内部测试参数**。目前 JSON 2 MB、SQL 默认 1,000 行／8 MB／30 秒、事务 100 条、上传 256 MB，以及队列和并发上限均有实现保护，但不是正式 SDK 参数承诺。接口上下文明确标注 `limitsStatus: internalTest`；正式限额验收仍待后续确认。


## 7 M2 SDK、模板与构建

`packages/sdk/` 保存 ES module、TypeScript 声明及匹配说明，无外部运行时依赖。`npm run package:sdk` 校验或生成 `packages/sdk/releases/local-projects-sdk-0.1.0.tgz`，模板通过 `file:vendor/local-projects-sdk-0.1.0.tgz` 引用，锁文件一同复制。已分发同版本包内容不得覆盖；修改 SDK 后明确升版本。`npm run prepare:desktop` 将这些固定资源打入开发包；升级平台不会重写已有项目。

模板位于 `templates/react/app/`，版本元数据在 `templates/react/template.json`。当前固定 React 19.3.1、TypeScript 5.9.3、Vite 8.3.4；脚本为 `tsc --noEmit && vite build`，资源基址为 `./`。默认页面显示项目连接、使用引导与可展开的媒体工具示例；打开和构建不创建业务表，只有用户明确选择示例保存才建立其业务表。黑白灰 Button/Card/Badge 在 `src/components/ui.tsx`，主题在 `src/theme.css`；它们是小型本地 React 组件，扩展时建议 shadcn/ui，没有安装或强制依赖 antd。

`scripts/package-template.mjs` 在临时副本执行 `npm ci` 和模板构建，核对构建未改写源码后，将 `starter/` 网页及 `starter.json` 来源写入包内模板资源。记录模板版本、源码／产物摘要及实际打包工具版本；用户新建时验证并复制这些资源，安装到项目 `dist/` 和不可变初始代，同时记录 `.platform/starter.json`。该来源表示平台提供的模板，不是用户在本机完成过构建；不会创建成功任务、任务日志或冒充本机构建的 Node/npm 环境。

项目构建 Node 先使用显式选择的绝对路径，否则探测 nvm、Volta、常见本机安装路径及可用 PATH。实际 Node/npm 版本经运行检查；npm 由所选 Node 执行，避免 shebang 选择另一个版本。本机选择保存到开发数据根 `toolchain.json`，不写入项目清单。系统构建工具探测不安装或升级 Node；新增私有运行时准备与其独立。缺少系统工具只阻止构建，已发布静态网页和数据库仍可运行。首次依赖安装可能需要联网。已新增的[隔离 Node 实验](node-runtime-trial-verification.md)不由当前构建探测接管，也不使私有运行时自动替代构建工具链。

构建按锁文件执行 `npm ci`，依赖指纹未变化且安装记录有效时可复用项目 `app/node_modules/`。共享 npm 缓存位于 `workspace/.platform/cache/npm/`。任务按项目互斥，并对输入取 SHA-256 基准，在依赖准备后和发布前复核；输入变化、依赖失败、脚本失败、无效产物均不发布新网页。构建只接受 `--outDir` 指定的平台暂存目录，不自动建表或重置数据库。

构建脚本及 npm 生命周期脚本是具有当前用户权限的普通本机代码，**不是沙盒**。只运行用户信任的项目；脚本不得启动后台服务、脱离进程组的子进程，或写入用户数据。平台在正常结束、超时及退出时清理普通构建进程组；这不构成隔离任意本机代码的保证。

主要持久位置如下：

| 位置 | 用途 |
| --- | --- |
| 项目 `.platform/builds/{taskId}/` | 暂存输出、发布现场及恢复所需材料 |
| 项目 `.platform/artifacts/{buildId}/dist/` | 不可变的各代网页资源 |
| 项目 `.platform/starter.json` | 初始应用来源和本项目输入基线；初始代 ID 为 `starter_…` |
| 项目 `dist/` | 当前发布产物的完整副本 |
| 工作区 `.platform/project-state/builds/{projectId}.json` | 当前成功构建来源；作为发布提交点 |
| 工作区 `.platform/project-state/builds/{projectId}.publication.json` | 中断发布核对日志 |
| 工作区 `.platform/tasks/{taskId}/` | 持久任务记录和分段构建日志 |

入口注入对应构建代的资源基址 `/p/{id}/_builds/{buildId}/`；旧页面继续引用同一代资源，新页面读取当前代。正常打开复用原桌面窗口；用户选择“重新加载项目”才替换页面。构建成功发送 `project.updated`，不自动丢弃表单。当前暂存和已发布旧代资源没有自动删除策略；正式保留期限及空间清理规则仍待确定，不把这些资源视为用户可选择的版本历史。

`project info` 和管理窗口显示 `buildInfo.sourceState`、`checkedAt`、`stale`、`reason`。初始应用附 `origin: bundledTemplate`、`templateVersion` 和 `starter`，用户成功构建后使用 `origin: projectBuild` 和 `provenance`。状态为 `unbuilt`、`matched`、`modified`、`unknown`；新项目正常为 `matched`，不是 `unbuilt`，也没有伪造的成功任务。`matched` 只表示最近扫描匹配。首次用户构建失败仍保留初始应用，成功发布后旧初始页继续使用原 `starter_…` 代资源。产物不符合已登记摘要时报告异常。重启按发布日志和来源核对实际提交结果；无法确认时报告恢复错误，不凭脚本退出码推断成功。

当前构建并发、队列、超时和日志预算都是**内部测试参数**：全局一个执行、等待队列 8、构建总预算 600 秒；单任务日志保留开头 2 MB 与末尾 8 MB，分页最多 64 KB，遗漏范围通过 `gaps`／`truncated` 明示。任务和旧代资源暂不按天数或数量自动删除。这些数值未冻结为正式产品默认值。

管理窗口可创建项目、复制给 AI 的交接说明、发起构建、查看任务与日志、检测或指定 Node，并在桌面或浏览器打开产物。CLI 命令可在任意工作目录执行，但每次显式给出项目 ID；不提供任意源码编辑或 shell 代理接口。完整外部 AI、桌面与浏览器流程的实际结果记入 [M2 验证记录](m2-verification.md)，不能仅以工程构建通过替代。

本轮初始应用与 skill 的补充验收单独记入[验证记录](starter-skill-verification.md)：应覆盖无可用 Node 时创建及打开、初始来源核对、首次失败保留、成功发布及旧初始页资源、独立 AI 读取交接说明，以及旧项目文件保持不变。历史 M2 结果不作为这些新增场景已通过的证据。

## 8 M3 备份、维护与恢复

基础入口为 `backup create/list/info/restore` 与 `maintenance status/begin/end`，均显式指定 `--project ID`。创建备份、进入/结束维护和恢复复用任务提交、`task wait` 和 `task logs`；超时后继续查同一任务，不重发写入请求。维护请求及恢复请求通过 `--input` 传 JSON；`begin` 需要原因，准备失败重试携带原 `maintenanceId`，`end` 需要该 ID，`restore` 必须指定 `backupId`。

维护中的 SQL、事务、构建和备份使用 `--maintenance ID` 或输入 JSON 的 `maintenanceId`，两者同时提供必须一致。进入任务成功且 `readyForChanges` 为 true 后才允许修改；基线失败和平台重启都保留维护。结束检查数据库、网页和构建输入；已知待构建要求不能通过重复进入或恢复抹掉。恢复后仍须明确结束维护，旧页面必须由用户决定重载，失败的请求不会自动重放。

```sh
npm run cli -- backup create --project prj_实际ID --json
npm run cli -- backup list --project prj_实际ID --json
npm run cli -- backup info --project prj_实际ID --backup backup_实际ID --json
npm run cli -- maintenance begin --project prj_实际ID --input maintenance.json --json
npm run cli -- maintenance status --project prj_实际ID --json
npm run cli -- backup restore --project prj_实际ID --input restore.json --json
npm run cli -- maintenance end --project prj_实际ID --input end-maintenance.json --json
```

归档在 `workspace/backups/{projectId}/{backupId}.zip`，包含 `backup.json` 与 `project/`。数据库通过 SQLite Backup API 生成独立一致性副本，关闭副本后校验；不复制源 WAL/SHM 拼库。文件清单包含路径、字节数和 SHA-256。快照保留源码、固定 SDK、skill、隐藏资料、清单、图标、网页、附件；排除按已识别根路径进行，附件中的同名 `node_modules`、`.git`、`.platform` 不被递归误删。

可移植 `buildProvenance` 保留 `bundledTemplate` 或 `projectBuild`、输入与产物摘要、规则及工具版本，去除本机执行路径、任务 ID 和凭据。恢复将网页安装为新的不可变资源代并重建来源，不依赖原来被排除的 `.platform/starter.json` 或原任务日志。已有可复用网页的恢复不运行 Node/npm；合法无产物项目仍可为 `unbuilt`，已登记产物缺失则报告异常。

持久维护记录在工作区 `.platform/project-state/maintenance/{projectId}.json`，与旧项目登记、构建来源分开保存；升级读取默认 ready 状态，不重写已有项目文件。操作暂存、恢复候选、日志和原现场在 `.platform/operations/{taskId}/`。恢复先验证候选，再停止业务、保留完整原目录并切换；原库损坏时也保留 WAL/SHM。未解决现场不作为缓存清除，重启根据日志和目录身份核对完成、回退或待处理结果。

手动备份不自动清理；普通自动备份在新备份成功后保留最近 10 份可清理项，基线与恢复来源受保护。基础 UI 显示备份列表、详情、任务和维护/恢复状态；历史备份另存、手动删除、迁移旧根清理仍未按讨论稿提供。M5 已实现统一空间扫描和白名单清理；其当前用法见第 10 节。

M3 验收脚本在 `.test-artifacts/` 保存报告。`m3-live-test.mjs` 新建可丢弃项目；`m3-restart-test.mjs prepare|verify` 在真正退出重启前后检查维护保留。`m3-compatibility-test.mjs prepare|verify` 只读核对升级前已有项目持久文件，不因 SQLite checkpoint 改变数据库字节而误判为平台覆盖。执行结果、故障模拟与交互证据见 [M3 验证记录](m3-verification.md)。

## 9 M4 生命周期与工作区迁移

以下为当前开发 CLI 的命令；真实包的验证进度单列于 [M4 记录](m4-verification.md)。导入、导出、复制、删除、回收站恢复/清除和迁移均返回持久任务 ID，沿用 `task get/wait/logs`。任务可能在 CLI 等待结束后继续；实例切换只重新发现服务并恢复查询，不自动重发写入。

| 命令 | 输入与结果 |
| --- | --- |
| `project export --project ID --input export.json` | `destinationPath` 为绝对 ZIP 路径；覆盖须显式 `overwrite: true`；维护时携带 `maintenanceId` |
| `project import --input import.json` | `archivePath` 为绝对路径，`mode` 必须明确为 `original` 或 `copy`，可选 `name` |
| `project copy --project ID --input copy.json` | 可选 `name`、`maintenanceId`；创建独立新平台 ID，保留业务主键 |
| `project delete --project ID` | 移入回收站；维护中通过 `--maintenance ID` 或 `--input` 传本轮 ID |
| `trash list` / `trash info --trash ID` | 回收项状态、原项目、路径和恢复可用性 |
| `trash restore --trash ID` | 原 ID 恢复并推进文档身份；坏库保留原字节，转维护待处理 |
| `trash purge --trash ID` | 永久删除选中回收项的项目文件，保留历史备份、外部导出和受保护现场 |
| `workspace info` | 当前根目录、文件系统、迁移任务及保留旧根；未知统计为 `null` |
| `workspace migrate --input migration.json` | `destinationPath` 为已存在、空、可写且独立的本机 APFS 目录 |

这些命令均支持 `--json`。请求例子分别为 `{"destinationPath":"/绝对路径/project.zip"}`、`{"archivePath":"/绝对路径/project.zip","mode":"copy","name":"副本名称"}`、`{"destinationPath":"/绝对路径/新的空工作区"}`。导入原 ID 时，活跃项目、回收站和正在登记的操作均占用身份，不能覆盖冲突对象。复制与导入为副本不全局替换用户文字、业务 UUID 或附件名；不升级固定 SDK 或覆盖业务说明。已验证网页直接复用，不要求 Node 或重新构建。

维护快照与来源不明的快照登记为新的本机维护会话，建立对应目标 ID 的完整基线；已知待构建要求继续保留。若登记成功而基线失败，任务会返回 `registered_maintenance_not_ready`，项目已经存在但 `readyForChanges: false`；先查询项目/维护状态，以该会话重试基线，不自动再次导入或复制。回收站恢复遇到坏库也不创建空库；恢复字节保留并阻断业务，修复或再次删除均按当前维护 ID 操作。清除部分失败时为 `purgeIncomplete`，不能承诺仍完整可恢复；按原回收项明确重试。

迁移覆盖全部项目、历史备份、回收站、维护状态、身份注销记录、日志和受保护恢复现场。活跃项目的可重建依赖及指定空间缓存可以排除；受保护现场和回收站内部不按目录名递归过滤，符号链接保留为链接、不追随到树外。迁移前停止外部编辑器与 SQLite 工具写入，平台等待已接受任务并关闭业务连接后复制和校验，不运行项目脚本。

固定 `dataRoot/migrations/{taskId}/` 保存迁移日志与任务，不在被复制树中写入进度。目标安置并验证后原子更新固定 settings，作为提交点；提交前失败可继续使用源，提交后只初始化目标。新位置重新生成服务实例和管理凭据，保持维护状态并推进活跃项目代次；旧 Cookie、旧管理凭据和旧页面不能恢复写入。目标缺失、身份不符或提交无法核实时保留两边并报告错误；不自动切回旧副本。旧源离线不阻止已验证的新根启动。

完成后重新读取 `project handoff` 并在新绝对路径继续编辑；旧 AI 工作目录不会自动跟随。旧项目内业务说明及 skill 不批量重写，平台仅更新生成的本机交接信息。迁移仍默认保留旧根。用户于 2026-10-16 确认增加显式历史清理：在“空间与工作区 → 清理历史”扫描已登记范围，选择旧根、已完成恢复原现场或回收项，核对目录并确认后提交持久任务。历史备份另存、删除及自动期限仍不在本次范围；清理计划、保护检查和中断边界见[历史清理补充](history-cleanup.md)。

M4 实际验收已保存已有项目摘要和 M3 可恢复开发包。`m4-live-test.mjs` 只创建新的 `M4 可丢弃` 项目；`m4-migration-test.mjs prepare` 记录正常/维护/回收站样本及全部保护树，经公开迁移与完整退出重启后运行 `verify`。脚本不改 settings，不自动启动迁移，不通过复制单个数据库代替工作区协议。兼容结果与完整日志保存在 `.test-artifacts/`。本轮最终通过 132 项 Rust、12 项 SDK、8 组运行提示回归、10 组 live 行为与迁移后 26 项目/14 保护树检查；原 17 个项目在升级及迁移后摘要一致。实际迁移目标是固定开发数据根下 `workspaces/current/`，源 `workspace/` 实体树保留。重启采用已核验开发进程 SIGTERM 后从包内可执行文件重新启动，不能称为 GUI 正常退出验收；GUI 全流程未完成及最终包证据见 [M4 记录](m4-verification.md)。


## 10 M5 概览、空间与浏览器能力

M5 管理界面提供项目概览、基本信息与 PNG 图标、运行需求、空间分类与附件浏览，以及关于和环境诊断。名称、简介与图标属于持久项目资料；修改不会改变项目 ID、触发构建、覆盖其他清单字段或重载已有业务页面。图标由清单引用 `meta/` 下 PNG，原生选择器导入为独立文件；CLI 的 `icon` 是项目内相对路径，不能把任意本机源文件路径当作上传接口。坏图标回退到默认图标并保留问题提示。

先执行 `project metadata get`，再把刚读取的 `metadataRevision` 放入修改输入：

```json
{"expectedRevision":"刚读取的 metadataRevision","name":"新的名称","description":"新的简介"}
```

`METADATA_CHANGED` 表示读取后清单已改变，应重新读取、核对再决定修改，不自动覆盖。`icon: null` 移除图标引用；维护中还需本轮 `maintenanceId`。元数据入口不修改 `runtimeRequirements` 或任意业务源码。

空间扫描按逻辑文件字节分类，附扫描时间、完整性、`stale` 和问题列表；扫描不是文件系统原子快照。`storage info` 可返回尚未扫描或过期摘要并安排重新扫描，不能把未知容量显示为零。`storage scan` 返回任务，使用原任务 ID 等待完成。附件查询例子为 `{"type":"image","limit":50}`；支持 `type`、`prefix`、`query`、`sort`、`order`、`limit`、`cursor`，下一页使用返回的 `nextCursor` 并保持筛选条件不变。文件变化导致 `STORAGE_CURSOR_INVALID` 时从第一页重新读取，不能假定旧游标仍指向同一组文件。

清理输入只接受明确分类，例如 `{"categories":["dependencies","buildCache","logs"]}`。`dependencies` 是可重建依赖，`buildCache` 只处理已识别且无保护引用的构建缓存，`logs` 只处理可清理的已结束任务日志；数据库、附件、备份、当前产物、未知目录和未解决现场不归入缓存。清理返回持久任务，结果中的跳过项与部分失败须保留，不把任务结束等同于全部删除。没有任意路径或历史备份删除命令。旧迁移根及已完成恢复原现场使用独立的[显式历史清理计划](history-cleanup.md)，不混入项目缓存类别。

`runtimeRequirements` 是 `project.json` 中独立的可选运行需求，不改变固定 SDK 的 `capabilities`。公开联网需求分别放在 `network.connect`、`network.images`、`network.media`，每项包含精确 HTTPS `origin`、`purpose` 与 `dataSent`；不接受路径、凭据、通配符、IP 或本地名称。CSP 只放行对应资源类型，公开服务器仍须允许浏览器 CORS。摄像头与麦克风分别通过 `devices.camera`、`devices.microphone` 的 `purpose` 声明；默认新项目不声明设备。更改需求后已有页面提示明确重载，不自动重载，也不声称瞬间撤销已有系统权限。现有静态 v1 不提供保密密钥代理；新增 Node 配置另按前述规范设计。

模板自 `react-ts-vite-0.1.2` 提供独立浏览器助手 `app/src/lib/browser-tools.js`，用法见项目 `app/BROWSER_TOOLS.md`；当前模板为 `react-ts-vite-0.1.4`。它探测安全上下文、浏览器 API 和实际 MIME，支持用户主动选择或拖入文件、下载、写入剪贴板及降级提示、短录音和拍照。运行上下文的正常模式是 `normal`。维护、忙碌、迁移、回收、离线、实例或文档失效，以及隐藏和离开页面都会停止采集并释放设备；权限请求的晚到结果也会被释放。当前录音/预览上限为 60 秒，素材上限为 10 MiB 与服务上传上限的较小值，均为内部测试保护，未定稿为正式限额。

0.1.3 修复真实 WKWebView 中“已提示维护，但重新选文件后仍可点击保存”的问题。示例采集与保存按钮同步最新运行提示，迟到的 context 不解除门槛；保存开始及附件完成后写 SQL 前各自重新核对。暂停时仍可选择文件、预览、下载本地副本和手动复制，不自动重放保存。16 项新增组件 DOM 检查与模板 TypeScript/Vite 构建通过；实际窗口复测、新包和三端设备验收的准确状态见 [M5 浏览器实测续记](m5-browser-real-verification.md)。旧项目不会因平台升级收到这些源码修改。

0.1.4 为真实返回的 `OverconstrainedError` 增加中文设备不可用或不满足采集要求提示，不把空约束字段推断为某个参数，也不将其当作权限拒绝。相机约束、权限请求与重试策略不变，固定 SDK 不变。18 项助手模拟回归、16 项组件检查及模板 TypeScript/Vite 构建通过；这些检查不代表摄像头已恢复可用。

真实 WKWebView 的麦克风许可拒绝、主动停止、隐藏和维护中断、迟到许可处理已有实测，并配合独立只读系统输入状态观察。物理外网断开时本地读取、附件和 SQL 保存成功，恢复后没有自动重放；准确时序、在线预检与离线保存的区分，以及本轮摄像头不可用的限制见 [M5 韧性实测](m5-resilience-real-verification.md)。这些结果不代表 Safari/Chrome 或所有设备释放分支均已通过。

示例只有用户明确保存时才先写附件，再用事务建立或更新 `media_samples` 记录；保存路径、MIME、扩展名、大小、时长、原文件名与创建时间。附件已写入而 SQL 失败时显示已保存路径和未关联状态，不自动重传、不把失败称为整体回滚。打开、构建和升级不创建该表，也不覆盖旧项目的源码、SDK、skill 或说明；新模板与说明只进入新项目和安装包后备资源。固定 SDK 仍为 0.1.0。

桌面项目下载先进入平台私有暂存，再由非阻塞原生保存对话框让用户选择目标；完成文件上限为 256 MiB，这不是网络传输中途的流量上限。保存前后核对目录及文件身份，拒绝当前工作区、固定数据根和应用包内部目标。外链请求先进入原生确认，确认后交给默认浏览器，源项目窗口不导航到外部；程序发起的请求也可能进入确认，因此不宣称已识别可信鼠标点击。相机/麦克风已配置 macOS 用途说明和 hardened runtime 权限，宿主按声明与项目状态决定是否交给系统提示；浏览器既有授权可能跳过新的权限回调。

M5 首轮源码通过 164 项 Rust（147 core、12 CLI、5 desktop）、全目标 Clippy、fmt、12 项 SDK、9 组运行提示及 15 项媒体助手模拟测试。首轮真实服务验证 7 组通过，包括 109 附件的 40/40/29 分页；SIGTERM 进程重启后元数据、图标、数据库、附件、SDK 和网页保留，旧 revision/cookie 拒绝，26 个原有项目兼容核对通过。该首轮记录没有声称 GUI 正常退出、真实设备、Safari/Chrome 或原生保存框通过；历史包与边界保留在 [M5 原验证](m5-verification.md)。后续真实窗口补验、修复及当前交付分别记录于[清理历史 GUI 验证](history-cleanup-gui-verification.md)和 [M5 浏览器实测续记](m5-browser-real-verification.md)，不重复套用首轮测试数量。
