# 2026-10-23 主线新功能 Windows 兼容审查

审查基线：`origin/main` 从 `770ec1d` 前进到 `f0b83ef`，共 43 个新提交；兼容分支本轮同步前为 `7427cfd`。本轮任务是同步和兼容性审查，不是将下列全部新功能实现到 Windows。源码行号以 `f0b83ef` 为准，合并后行号可能变化；本轮实际检查结果另行补录，不以主线 Mac 结果代替 Windows 验证。

Windows 已有 W2.1 新样本创建、只读打开和旧包正常重启，W2.2a/b1/b2 私有文件与 WAL 连接实验，以及独立业务 Node 的固定 x64 资源、配置 ACL、Job 进程树、代理与窗口接入。证据见 [W2 记录](windows-w2-verification.md)及 [Windows Node 记录](windows-node-verification.md)。公开 SQL、附件、用户构建和完整生命周期仍关闭；最新真实 GUI 在系统父目录 ACL 准入前被阻断，Win10、安装和真实媒体也未通过。不能放宽系统父 ACL、变更开发身份或换根来取得通过结果。

## 1 本次需要保留的确认与边界

- [技术实施计划](technical-implementation-plan.md)已确认 Git-only：退出新独立 ZIP 备份产品、维护基线耦合和专用 Git 恢复仓库建设。现有项目、ZIP、旧回收、独有 Git 历史及恢复现场继续保全；最小候选发布、操作日志、提交点与重启核对保护仍须实现。取消备份不等于可以开放未保护的写入。
- [直接删除契约](application-deletion.md)已确认新删除永久执行，不进平台或系统回收站。外部 Node 引用的整个外部目录保留；受管实例只删除预览明确列出的实例登记、程序、配置、数据与缓存，来源、独立安装现场、旧 `runs/` 及删除回执保留。旧 `/trash` 请求拒绝新删除，不能让旧客户端误以为可撤销。
- [本地目录流程](local-directory-onboarding.md)已确认检查、初始化和登记分别确认。真实空目录才生成零第三方依赖的起始 Node 项目；有效 Node 目录只预览并登记外部引用；其他目录仅提供 AI 检查说明。检查不安装依赖、不执行代码、不读秘密，不自动发送给 AI；部分初始化文件保留。
- [Node 安装更新](node-managed-installation.md)仅接独立来源副本、受限生产依赖和新的受管实例。普通更新要求应用 ID、配置声明及双方 `dataContract` 完全匹配，用户先关闭窗口与进程；保留实例 ID、配置和业务数据，不自动迁移数据或启动新版。仅原生管理命令提交，不增加绕过窗口锁的 HTTP／CLI 安装或 Node 删除入口。
- 当前 [Git 版本保存／源码取回](technical-implementation-plan.md)是源码子集，不是数据库版本系统。取回不替换 HEAD、index、运行产物或业务数据。SSH 工具版本可用不代表 SSH／HTTPS 私有仓库认证已验收；公开来源仍限 GitHub/Gitee HTTPS。分享和通用应用包下载不在本步范围。

## 2 新功能与可复用范围

| 新增组 | 可复用的共同规则／实现 | Windows 需要补齐的能力 |
| --- | --- | --- |
| 开发 Node/npm 与 Git | 显式选择优先、受管默认、失效不自动换来源；固定发布声明、完整树摘要、许可证与诊断模型 | 两套 Windows 载荷、树布局和固定摘要；安全资源准备／校验、工具发现与执行环境。现有业务 `node.exe` 不包含开发 npm／Git |
| 对象边界、目录名与管理 UI | 源码／程序／数据／配置边界、稳定 ID、转写规则、结果类型与共同组件 | 真实受限宿主数据接入、Windows 名称碰撞／保留名检查。只为新对象应用目录规则，不改名既有目录；未开放后端不能挂载完整管理器 |
| AI 交接与打开 Codex | 当前项目路径／服务身份绑定的交接正文、明确复制与失败后保留内容 | 原生发现与可信程序验证、Windows 系统打开及租约复核；Mac bundle／签名和 `/usr/bin/open` 不能复用。不自动安装、发送需求或运行项目代码 |
| 本机目录检查／初始化 | 三态检查模型、清单规则、零依赖模板字节、预览修订与部分结果语义 | 安全枚举外部普通目录，以及绑定 HANDLE 的不覆盖创建；不能把外部目录变为平台私有 ACL 目录 |
| 公开仓库获取／本机源码副本 | 来源任务、精确 commit、大小／文件预算、内容摘要、固定排除规则、取消及失败保留 | Windows Git 子进程环境／Job、路径碰撞和安全复制／发布、来源租约与迁移／退出协调；不调用普通 checkout 执行过滤器 |
| 候选依赖构建与发布 | 独立候选、`npm ci`、输入重核、产物来源、失败保留旧成品 | 安全复制与依赖树校验、开发 Node/npm 执行器、Job 收尾、完整目录代次发布与中断核对；W2.4／W2.5 仍独立验收 |
| Git 状态、初始化、保存、分支 | 源码范围、预览摘要、HEAD／index 约束、受控作者身份、禁 hooks／filters／全局配置的策略 | 安全仓库读取、受控环境、ID128 绑定的锁与 index 发布、ref 提交后状态核对；不把 `fs::rename` 当作全部原子性保证 |
| Git 源码取回／中断处理 | 文件计划、目标 commit 与当前摘要绑定、显式继续／撤回、保留被替换原件、构建阻断规则 | Windows 按身份移位／替换原语、逐文件持久日志与实际提交点分类、重启恢复、占用和部分失败处理 |
| Node 独立安装／更新 | 依赖政策、生产材料摘要、来源及配置修订、数据契约匹配、安装结果状态 | 私有安装现场和代次目录准入、依赖复制与校验、原生锁内发布及不确定结果处理；Windows 受限 `NodeApps` 不复用完整 core 编排 |
| 永久删除 | 预览／修订、所有权、运行／恢复阻断、明确保留范围与删除回执 | 按保留 HANDLE／ID128 的受限递归删除和持久阶段；文件占用、换链／换对象、中断及残留范围报告；不能调用系统回收站或整槽清空 |

共同 UI、清单、数据契约、任务／错误类型和纯政策逻辑可以共享。Mac 完整管理编排继续在 `platform-core`；Windows 继续使用受限 `platform-host`／`platform-node`。共享 manifest、配置、运行协议和代理不反向依赖完整 core。功能关闭须覆盖 UI、IPC、HTTP／CLI 及后台初始化；单纯隐藏按钮不足以阻止管理器对账或写入。

新增网站、文案与静态页面可独立同步，不将其宣传或下载入口当作 Windows 安装／分享能力已经验收。目录名生成参考数据参与逐字节校验，Windows 检出时的 CRLF 转换需由受控 Git 属性固定，不可放宽摘要来接受变更。

## 3 具体源码证据与原语缺口

### 开发资源和进程环境

[managed_toolchain.rs](../crates/platform-core/src/managed_toolchain.rs) 使用 Unix `MetadataExt`／`OpenOptionsExt`（13 行）、`dev/ino`／mode／nlink 身份与 `O_NOFOLLOW`；105–124 行拒绝不匹配平台，344–345 行固定 `bin/node` 和 `lib/node_modules/npm/bin/npm-cli.js`。[Node 发行声明](../scripts/development-node-release.json)只固定 `darwin/arm64` Node 24.22.0／npm 11.19.1 的完整树。[准备脚本](../scripts/package-development-node.mjs) 25–26 行生成 `/bin/sh` 启动脚本，279 行只接受 Mac arm64。Windows 需要独立的官方包／npm 布局、树摘要、许可与 PE 检查；不能仅复制原有 Windows 业务运行二进制或跳过树校验。

[managed_git.rs](../crates/platform-core/src/managed_git.rs)同样依赖 Unix 身份和权限，106–122 行有平台门禁，344–356 行固定 `libexec/git-core`／模板布局。[Git 发行声明](../scripts/development-git-release.json)是 Mac arm64 源码构建及系统库方案。Windows Git 的分发来源、exe／helper／DLL、证书和模板布局需单独确定并固定可信摘要；本文不擅自冻结新的发行来源或版本参数。

[git_environment.rs](../crates/platform-core/src/git_environment.rs) 18 行和 243–271 行固定 Unix PATH、清空环境并设 POSIX 进程组；285 行用负 PID 发信号；134 行检测 `/usr/bin/ssh`。[git_remote.rs](../crates/platform-core/src/git_remote.rs) 108–114 行因冒号分隔规则拒绝含 `:` 的临时路径，Windows `D:\...` 也会被拒；243–296 行包含 `/dev/null`、`/usr/bin/false` 与 `process_group`。需要 Windows 绝对路径与列表分隔规则、受控必要系统环境、非交互认证隔离和 Job 执行器。现有业务 Node 的 Job 所有权可作为基础，不能直接把常驻应用协议当成 Git／构建进程执行器。

公开 HEAD 地址政策本身可复用：`git_remote.rs` 151–185 行在解析前限制 GitHub/Gitee 标准 HTTPS 地址，拒绝凭据、端口与额外路径；禁用重定向、凭据助手、提示和 SSH 的边界在移植时仍须保持。支持真实 Git for Windows 版本输出格式、证书和 HTTPS 成败，需另有样本验证。

[codex_handoff.rs](../apps/desktop/src-tauri/src/codex_handoff.rs) 11–12 行固定 Mac bundle／Apple 团队签名身份，39、62、82 行分别调用 `/usr/bin/plutil`、`codesign` 和 `open`。Windows 可复用编码后的项目 URL 与租约边界，程序可信身份、安装发现和系统调用需独立实现；未实现时保留复制交接的可用出口。

### 来源、构建与 Git 写入

[source_acquisition.rs](../crates/platform-core/src/source_acquisition.rs) 18 行依赖 Unix 文件原语，609–656 行本机捕获安全扫描／复制后以路径重命名发布；684–730 行是独立浅克隆、`fsck`、逐对象物化及 index 填充，未运行 checkout／项目代码。1659–1735 行仍是 Unix 隔离环境与进程组。Windows 需同时移植名称碰撞、文件 ID／单链接、目录链保留、内容重核、受限新目录发布和 task／来源 lease；不能把本机捕获简化为递归 `copy`，已知排除规则也不是通用秘密扫描。

[builds.rs](../crates/platform-core/src/builds.rs) 842–854 行在独立候选执行 `npm ci` 并重核锁文件／链接，881 行起核对源码与既有成品，949–1005 行使用 npm CLI、隔离环境和 POSIX 组收尾，1752 行起执行代次发布。[build_candidate.rs](../crates/platform-core/src/build_candidate.rs)及 [build_result.rs](../crates/platform-core/src/build_result.rs)可共享候选和来源模型，复制、普通文件身份、目录交换与持久发布需 Windows 实现。移植时继续保留源目录的 `node_modules`、现有网页、数据库和附件；构建不是安全沙箱，也不证明任意原生扩展可用。

[project_git.rs](../crates/platform-core/src/project_git.rs) 1–3 行明确源码子集；765–908 行以 index 锁／独立 index、内容重核和 HEAD 事务保存版本。2035–2085 行的 `O_NOFOLLOW`／index 重命名与 fsync、2175–2203 行的 Unix身份、2227–2257 行的 Mac `renameatx_np(RENAME_EXCL)` 均不能原样用于 Windows；非 Mac 初始化明确返回不支持。2649 行起的 Git 子进程仍使用 Unix环境／信号。应先迁移安全只读状态，再开放 init／save／branch，各自验证提交前与提交后失败。

[project_git_restore.rs](../crates/platform-core/src/project_git_restore.rs) 1107–1150 行使用 Mac `RENAME_SWAP`／`RENAME_EXCL` 保全原件，其他平台明确拒绝；1473 行结果和后续日志明确不替换 HEAD、index、运行产物和业务数据。[source_restore_guard.rs](../crates/platform-core/src/source_restore_guard.rs) 302–334 行阻断存在待处理取回的开发／工作区操作。这要求 Windows 的逐对象移位与替换方案、日志和原件保全共同成立；不能改为 `git reset --hard`、普通覆盖复制或失败后自动重试。

### 外部目录、安装与删除

[node_app_directory.rs](../crates/platform-core/src/node_app_directory.rs) 81／152／454／646 行依赖 `openat`，350 行核对预览修订，369 行核对根和真实空目录，482–507 行保留部分创建结果。现有 [WindowsSourceDirectory](../crates/platform-host/src/windows_node_files.rs)可安全打开、枚举、读取和保留外部源码对象，但 `create_private_directory`（181 行起）要求私有父目录。初始化用户选择的普通空目录需要新的、按原 HANDLE 不覆盖创建的原语，并逐项核对已创建 ID；不能给整个外部目录强加私有 ACL、接受 junction 规范化后的替代路径或失败后递归清理。

[node_app_install.rs](../crates/platform-core/src/node_app_install.rs) 22 行依赖 Unix 文件扩展；487 行及 1297–1317 行以 `npm ci --omit=dev --ignore-scripts` 准备生产依赖，1387–1405 行为 POSIX 进程组回收，1504–1702 行为复制、内容与执行权限摘要和安全文件创建。配置修订、代次、来源摘要和数据契约可复用；Windows 仍缺受管安装现场／代次的类型准入、私有 ACL、Windows 执行权限模型及原生提交。安装准备的来源租约与迁移／退出等待也需一起接入；保留旧外部登记，不自动转换为受管实例。

[node_app_delete.rs](../crates/platform-core/src/node_app_delete.rs) 77–87 行只将配置、数据、缓存及受管代次列为目标，151–169 行要求原生操作锁和已停止状态，198–258 行持久记录删除意图／部分失败；当前递归删除仍调用 Unix [history_cleanup.rs](../crates/platform-core/src/history_cleanup.rs) 原语。网页永久删除同样见 [lifecycle.rs](../crates/platform-core/src/lifecycle.rs) 1072–1190 行的身份、document epoch、删除 marker 和结果核对。现有 Windows `remove_file_checked` 只覆盖指定文件，不是安全递归树删除。Windows 新实现需逐节点绑定卷／ID128、拒绝 reparse／硬链接／占用、保留回执并准确报告残留；不得递归删除 `node-apps/<id>` 或 `.installations`／`.deletions`／旧 `runs`。

## 4 建议适配顺序与相对难度

下列顺序是实施建议，不重编号 W／NR，不是新的已确认功能参数或工期承诺。目录流程与工具载荷两条小链可独立推进；安装和删除不要求先补已取消的 ZIP 体系。

| 顺序 | 建议独立交付范围 | 相对难度与退出证据 |
| --- | --- | --- |
| 0 | 合并主线并保住静态宿主、业务 Node 和后置能力门禁 | 中；共享 UI／编译与既有 Node 回归，新入口不创建任务／目录，不初始化完整 core；Mac 原编排保留 |
| 1a | 外部目录安全检查、对象／目录边界及 AI 检查说明 | 中；普通目录、隐藏条目、路径别名、链接／碰撞与变化冲突；只读操作不写目录、不启动程序 |
| 1b | 独立 Windows 开发 Node/npm、Git 资源与诊断／非交互 Job 执行器 | 高；固定资源全树及 PE、中文空格／长路径、显式选择保留、损坏不回退、无系统工具与只读程序位置；取消／超时／后代收尾 |
| 2a | 空目录初始化 → 重新预览 → 外部 Node 登记 | 高；先增加普通外部目录 create-new 原语，证明竞争创建不覆盖、部分失败保留；零依赖 starter 不必等待 npm 或 Git |
| 2b | 本机来源副本和公开 HTTPS 获取 | 高；独立来源准入、名称／ID／内容重核、来源 lease、取消／断网／超限／失败保留；获取不自动安装或运行 |
| 3 | 候选构建执行 → 产物发布 | 高；先 W2.4 执行与停止，再 W2.5 安全发布／中断；失败仍可打开旧产物，实际 AI 新样本和重启链路另验 |
| 4 | Git 只读状态 → init／save／branch | 高；只读和每个写提交点分别开放，锁冲突、文件并发变化和 ref 提交后错误不误报未提交；不新增数据库版本承诺 |
| 5 | Git 源码取回、继续／撤回与构建阻断 | 很高；先确定保全原件的 Windows 移位／替换原语；逐文件中断、重启、占用、未知状态及 continue／undo 均核对 |
| 6 | Node 独立安装准备 → 新实例提交 → 更新 | 很高；来源／配置／代次及数据契约门禁、窗口／进程锁、部分发布和重启证据；保存数据不变、不自动启动 |
| 7 | 新永久删除独立小步 | 很高；先 typed ownership 与删除日志，再受限递归原语，最后 UI 确认；逐文件失败、外部目录／其他实例／旧证据保留与重启核对分别验证 |

永久删除可以在其自身保护链路齐备后独立实施，不与完整 Git 源码取回绑定，也不以“备份已退出”为理由省略操作保护。各完整小步通过相关检查后按分支规则提交推送，不能积攒成一次不可审查的大改。

## 5 验收建议和检查命令

全部写入与运行验证使用明确命名的可丢弃样本，固定开发身份和根保持原样；不对现有业务项目、SDK、数据库、附件、凭据或旧恢复现场做试验。文件用例至少覆盖保留名／尾点空格／ADS／大小写与 Unicode 碰撞／短名别名／长路径、junction／reparse／硬链接、外部写入占用、预检后换对象、ACL 不符、磁盘不足及提交后验证失败。不确定结果必须保留材料、标注 `operationOutcome: unknown` 并要求重读，不自动重放写入。

工具、Git 与 npm 要用真实 Windows 载荷验证。公开 HTTPS 测试和私有认证分别记录，网络失败不能由工具版本成功掩盖；禁止依赖用户全局 Git/npm 配置、凭据助手、自动安装脚本或失控子进程。完整 GUI 应另验选择／取消、预览后变化、启动中关窗、重复聚焦、配置保存与显式重启、平台 X、WebView2 会话及正常重开。Win10／Win11、普通用户／干净机器／只读安装位置分别取证；物理媒体、断网和硬崩溃不以合成 DOM 或人工音频字节代替。

以下是合并后的基础回归建议，不是本报告声称已执行的结果；脚本以最终合并的 `package.json` 为准。Windows 不运行仍被门禁关闭的完整 core／CLI 工作流。

```powershell
npm run test:project-directory-names
npm run test:local-directory-starter
npm run test:ui
npm run test:windows:ui
npm run test:windows:node-runtime
node scripts/rust.mjs test --locked -p platform-host -p platform-node -p platform-project-format -p platform-desktop
node scripts/rust.mjs clippy --locked -p platform-host -p platform-node -p platform-project-format -p platform-desktop --all-targets -- -D warnings
npm run build -- --debug -- --locked
git diff --check
```

真实业务 Node 回归沿用 [Windows Node 记录](windows-node-verification.md)的 `NODE_RUNTIME_TRIAL_TEST_NODE` 固定摘要路径和 opt-in 命令。新增 development Node/npm／Git 的完整 Windows 测试须随各自实现补入，不能把现有 Mac 资源测试或业务 Node 15 项算作通过。Mac 的新开发工具、来源、构建、Git、安装／删除在 Mac 设备分别执行现有专项及完整受影响回归，Windows 编译不代替它们。

## 6 本轮实际合并与验证（2026-10-23）

本轮把主线新功能保留在 Mac 完整宿主中，共享管理界面、清单／`dataContract`、配置、运行协议与代理；Windows 的受限管理器保留原有源码槽登记、配置与运行。Windows 明确关闭新目录初始化、受管安装／更新、永久删除、来源获取、项目 Git 和 Codex 入口，未初始化完整 core。旧 `node_app_register` 在 Windows 仅服务原有外部引用登记，补充预览声明与目录身份的 `expectedRevision` 绑定；不是新目录三态流程的实现。Mac 的旧同名 IPC 也改为必须携带修订、转交现有 `node_app_directory_register`，不能绕过新目录确认。

合并修复包括：把新增完整 Node 编排和生命周期测试留在 core；共享窗口模块中的安装／删除命令限定到 Mac；合并运行提示时同时保留 Windows 只读说明和主线永久删除／部分删除状态。新增目录命名数据在 Windows `autocrlf` 检出后曾因 CRLF 与生成器的字节校验不一致失败，现以 `.gitattributes` 固定该生成参考目录为 LF，并保留主线原始内容；重新校验通过。

| 实际执行 | 结果及范围 |
| --- | --- |
| `npm ci --ignore-scripts` | 成功；安装锁定依赖，未运行依赖生命周期脚本 |
| 共享 UI／Windows UI | 301／36 项通过；含未开放命令不被调用、登记修订与未知结果处理的回归，使用合成 IPC，不是原生 GUI 验收 |
| 受影响 Rust 单元测试 | `platform-host` 105 通过、1 辅助测试忽略；`platform-project-format` 14 通过；`platform-desktop` 9 通过；`platform-node` 56 通过、15 opt-in 另测；新增预览后声明变化、相同声明的目录替换、保留 HANDLE 阻止并发替换三项 |
| 官方 Windows Node 实进程回归 | 15 项全部通过，90.26 秒；固定 SHA-256 的官方 Node 24.22.0 x64，使用可丢弃样本；覆盖登记、配置／代理、正常重开、Job 后代收尾、宿主被终止和失败启动。补齐登记修订后另重跑受影响的登记／配置／代理／正常重开用例，1 项通过，23.32 秒（是重复验证，不另算一个新用例） |
| 进程／桌面入口／平台支持／Windows Node 资源脚本 | 31 项 TAP 通过，其中平台支持脚本另执行 5 项 Rust 政策测试；Windows 资源相关 19 项包含在 31 项内，不重复计数 |
| 运行提示、零依赖目录 starter、目录命名参考数据 | 运行提示 30 次请求通过；starter 1 项通过；40,258 条映射与 4,120 个固定样本精确校验通过 |
| 四个受影响 Windows crates 的 `clippy --all-targets -- -D warnings` | 通过 |
| `npm run build -- --debug -- --locked` | TypeScript、Vite、Windows Rust／Tauri 构建通过，生成 `target/debug/local-projects-desktop-dev.exe`；未打安装包、未发布 |

另尝试了 `node --test scripts/desktop-package-output-test.mjs`：4 项通过、16 项失败。该脚本验证 Mac `.app`／DMG 输出，Windows 上的模拟路径被旧的 Unix 目录拆分处理为 `C:\\C:`，符号链接样本还遇到 `EPERM`；同一目录拆分逻辑在本轮合并前已存在。实际桌面入口只在 Mac 调用此打包流程，Windows debug 构建不经过它。保留这些失败记录，不将其计为通过，也不据此推断 Mac 打包结果；该专项应在 Mac 执行。

本轮没有重新运行真实 GUI，既有系统父目录 ACL 准入阻断仍未解决；未验证 Win10、安装器、干净机器、Mac 编译／原生生命周期或真实媒体。当前证据仅支持主线同步和既有 Windows 能力回归，不代表新增开发 Node/npm／Git、构建、安装更新和删除已在 Windows 可用。

差异检查：相对 `origin/main` 的兼容分支改动，以及相对合并前 HEAD 排除原样引入 `website/` 后的改动均通过 `git diff --check`。完整同步差异仍报告主线网站第三方资料／样式的既有尾随空格，未为兼容同步重写这些上游文件。
