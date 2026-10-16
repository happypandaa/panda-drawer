# M5 验证记录

日期：2026-10-16。状态：功能实现和内部开发包已交付，真实 CLI/API、进程重启、26 个既有项目兼容及最终包校验通过。管理 GUI、真实设备、Safari/Chrome 和原生保存框全流程未实测，不把这些验收条件标为完成。

实际测试环境为 macOS 27.0.1（Build 26A434）、Apple Silicon、Node v24.14.1、npm 11.11.0。macOS 15 仅核对了构建配置和 Mach-O 最低版本，未在 15 实机运行。

本页保留 2026-10-16 首轮的实际状态与旧包摘要。后续清理管理界面及浏览器能力的原生 GUI、真实采集/回放、文件和网络实测、修复与新交付见[清理历史 GUI 验证](history-cleanup-gui-verification.md)和 [M5 浏览器实测续记](m5-browser-real-verification.md)；不要将本页旧包摘要视为当前交付。

## 范围与数据保护

本轮实现统一项目概览、基本信息和 PNG 图标、编辑冲突、运行需求、空间分类与缓存、附件分页、安全预览与白名单清理、关于和环境诊断，以及文件、主动剪贴板、短录音和拍照助手。功能源码阶段已提交推送为 `fc12275`；最终资源同步包包含更新后的 M5 开发说明。固定 SDK 0.1.0 未修改，新增助手随模板 `react-ts-vite-0.1.2` 提供。

- 只读记录升级前 26 个开发项目，基线 `.test-artifacts/m5-original-projects.json` 权限为 `0600`。源码、清单、SDK、说明、网页和附件纳入摘要；SQLite/WAL 摘要另留，兼容检查不把 checkpoint 引起的原始库字节变化误判为资料被覆盖。未对既有项目注入新模板、skill 或文档。
- 当前工作区：`/Users/ethan/Library/Application Support/dev.localprojects.platform.m0/workspaces/current`。保留的 M4 迁移源：`/Users/ethan/Library/Application Support/dev.localprojects.platform.m0/workspace`。最终实例确认旧根 `available: true`、`identityMatches: true`、`canDelete: false`，没有直接改 settings 或清理旧根。
- M4 app/DMG 保留在 `.test-artifacts/m4-delivered/a28f95e8a02e329cd3b48d6de0f7fbdc0a9cd02a020d06a4f7308ccd326953d6/`。本轮两次包验证均核对其 DMG 摘要和 49 个应用条目不变。
- 写入和故障注入只发生在本轮新建的 `M5 可丢弃` 项目：主要样本 `prj_bfc0a54098304c33a5c617ce8919b74c`，坏库诊断样本 `prj_8578244ec579471bbf5f777289222d8e`。后者故意保留损坏库及缺失产物的诊断状态。

## 源码与定向检查

| 检查 | 结果与证据 |
| --- | --- |
| 完整 Rust 工作区 | 164 项通过：147 core、12 CLI、5 desktop；`.test-artifacts/m5-rust-tests.log` |
| 质量检查 | 全目标 Clippy `-D warnings`、fmt 通过；`m5-clippy.log`、`m5-fmt-check.log` |
| 管理界面与固定 SDK | TypeScript/Vite 构建通过，SDK 12 项通过；`m5-final-build-dmg.log`、`m5-sdk-tests.log` |
| 运行提示 | 9 组、22 次只读请求通过；`m5-runtime-notice-tests.log` |
| 运行声明 | 5 项 Rust 定向检查通过；`m5-runtime-policy-tests.log` |
| 标准媒体助手 | 15 项模拟行为通过；`m5-browser-tools-tests.log` |
| 原生下载底层 | 3 项 Rust 函数级检查通过；`m5-browser-download-tests.log` |
| 新模板 | 隔离 npm 安装、TypeScript 与 Vite 构建通过；`m5-template-build.log`、同名 JSON |

运行声明检查覆盖默认空声明、connect/images/media 分离、用途、精确 HTTPS origin、IP/本地名称/路径/凭据拒绝、重复项及 JSON 形状。测试发现 serde 可接受数组形态结构体后，补上显式对象/数组验证。

媒体模拟以真实 `context.state.mode = normal` 为基线，覆盖安全上下文、声明、用户手势、MIME/扩展名、时长和最终分片限额，以及维护、隐藏、离开、断线、迁移和重载释放设备。权限请求期间取消、权限晚到、结束回调晚到均不恢复旧采集或覆盖新选文件。上传与 SQL 关联分别报告结果，不自动重试不确定写入。这些使用模拟设备和函数，未接触真实麦克风、摄像头；示例只有用户明确保存才建立 `media_samples` 表。

下载底层验证本项目/精确 blob origin、符号链接、目录身份替换、完成文件大小、安全发布和保护路径。管理页附件保存与项目下载复用独立临时名、目标身份复核及父目录同步；最终宿主保护固定数据根、当前和已登记保留工作区及应用包。真实保存对话框没有操作记录。

## 真实服务与故障诊断

`node scripts/m5-live-test.mjs` 在开发包真实服务执行 7 组验证，报告 `.test-artifacts/m5-live.json`，完整输出 `m5-live-run.log`：

1. 元数据修改携带 revision；模拟 AI 外部修改后旧 revision 被拒绝，未涉及字段保留。名称/图标修改不触发构建、不作废当前业务会话。
2. 已有无效 PNG 回退显示，项目继续可用；新的无效图标引用被拒绝。
3. 同次空间扫描七类之和等于项目总量，附件中的保留名称仍按用户资料处理。
4. 109 个附件按 40/40/29 三页完整返回，无遗漏或重复，图片筛选正确。
5. 依赖清理只删除已识别的源码依赖根，附件、当前网页和固定 SDK 保留，任意清理类别被拒绝。
6. 声明的公开 HTTPS 来源和相机/麦克风用途反映在真实 HTML 的 CSP、Permissions-Policy 和策略指纹中；没有向示例公网服务发请求，也未在真实浏览器执行外站 CORS 或请求阻断检查。
7. 运行包身份、安装位置、程序占用、工作区及其他本机范围分别可读。旧根中的链接等未计入部分携带 `complete: false` 和问题明细，没有声称扫描为完整原子快照。

`node scripts/m5-diagnostics-test.mjs` 的续跑 `--resume` 复用同一诊断样本，记录 `.test-artifacts/m5-diagnostics.json`、`m5-diagnostics-run.log`。坏库为 43 字节，读取/编辑元数据、空间扫描及失败 SQL 后 SHA-256 仍为 `ce459d520a2b0503804dec731e79e811f0e3ab38d512b9ca7abb1178c2eb3795`；没有创建替代空库。删除该测试项目已登记的 `dist` 后状态为明确的 `error`，有效卡片资料与元数据仍可读取；这不是合法的 `unbuilt` 场景。

这些是本地 CLI/HTTP 真实服务证据。未实际操作业务表单，因此会话保持和产物不变不能代称完整 GUI 草稿验收；本轮也未重新执行 M4 全部生命周期 GUI 流程。

## 实际重启与兼容

主流程先核验开发进程身份，以 SIGTERM 终止，再从签名 app 的包内可执行文件启动。最终文档同步包启动为 PID 29089、实例 `80ed236a-5260-43bb-b7e3-7f49a1a0c1b1`；启动就绪前一次只读探测返回 `SERVICE_NOT_RUNNING`，随后等待就绪再读取，没有重发写操作。此方法验证真实进程重启，不声称菜单/Cmd+Q 正常 GUI 退出通过。

最终包重跑重启和兼容验证通过：旧 Cookie 与旧 metadata revision 拒绝；元数据、图标、数据库和附件保留，新会话读取原内容，固定 SDK 与网页字节不变；升级前 26 个项目仍在且持久资料摘要一致。证据为 `.test-artifacts/m5-restart.json`、`m5-final-restart-run.log`、`m5-original-projects-verification.json`、`m5-final-compatibility-run.log`。

`.test-artifacts/m5-final-runtime.json` 确认当前新根与保留旧根状态、Node 自动探测 `ready: true`、`configuredNodePath: null`，以及包内开发 README 与打包时仓库 `docs/development.md` 逐字一致。未修改已签名包来补文档；该 README 留有打包时的“最终核验中”状态，最终结果以本记录为准。

## 最终 app 与 DMG

`node scripts/m5-package-verify.mjs` 使用只读方式完成两轮核验，不启动或重签应用。首轮证据 `m5-package-2026-10-16T03-14-39-211Z/` 对应文档同步前包，保留但不作为最终产物摘要。最终证据目录为 `.test-artifacts/m5-package-2026-10-16T03-18-39-961Z/`，汇总为其中的 `verification.json`。

| 项目 | 最终结果 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 字节数 | `8065405` |
| DMG SHA-256 | `8e76040db59d07c69c76381ce57758aae0d6114d80b8eab75ca7c13ce89d21a7` |
| 身份 | `dev.localprojects.platform.m0`，版本 `0.0.0`，development |
| 桌面及 CLI 架构 | 均为原生 `arm64`，Mach-O 最低系统 `15.0` |
| 签名 | 源 app 及 DMG 内 app 均通过 `codesign --verify --deep --strict`；ad-hoc、runtime、无 TeamIdentifier |
| macOS 媒体配置 | Info.plist 相机和麦克风用途说明存在；实际签名权利 camera/audio-input 为 true，无 App Sandbox |
| DMG 一致性 | `hdiutil verify` 通过；只读挂载 54 个应用条目与构建 app 的类型、内容 SHA-256 和链接一致；已安全卸载 |
| SDK | 源码、模板、生成资源、源 app 和 DMG app 共 8 处副本 SHA-256 均为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9` |
| 模板 | `react-ts-vite-0.1.2`，源码和预编译初始页版本匹配，助手资源存在 |

包检查和实例重启完成后没有修改包文件。本轮最终构建日志为 `.test-artifacts/m5-final-build-dmg.log`；应用启动日志为 `m5-final-app-runtime.log`。

## 明确保留的验收边界

- M4 计算机使用会话曾被工具的浏览器 URL 访问限制终止；本轮没有通过其他 UI 工具绕过。M5 管理 GUI、文件拖入、原生保存框、真实录音/拍照、拒绝权限、Safari/Chrome 和 WKWebView 媒体兼容性仍未实测。没有真实硬件或最低 macOS 15 设备支持矩阵。
- 外链目标先经过原生确认，明确同意后才交默认浏览器，源项目窗口不外部导航。程序请求也能进入确认；没有可信链接点击检测的原生桥接，不能声称已验证请求来自鼠标点击。
- 权限回调处理新请求；浏览器已有许可可能跳过回调。Permissions-Policy、标准助手释放与系统授权是不同环节，不保证瞬间撤销旧页面或任意业务代码的权限。用途及签名配置依据 [Apple 媒体授权说明](https://developer.apple.com/documentation/bundleresources/requesting-authorization-for-media-capture-on-macos) 和 [Tauri macOS bundle 文档](https://v2.tauri.app/distribute/macos-application-bundle/)，配置通过不等于设备交互通过。
- 未关闭本机 Wi-Fi 或物理断网测试。7 组真实服务在 loopback 上无需外网请求，不等同于完成实际离线设备验收。
- 当前助手 60 秒、10 MiB 与服务上传上限取小值，以及完成下载文件 256 MiB 均为内部测试保护。后者不是下载网络中途的流量上限；正式资源预算仍未冻结。没有公开密钥代理、后台长录音、转写或视频处理。
- 没有 Developer ID 签名、公证、互联网分发首启或外部用户验收；正式品牌、发布身份和 M6 仍待落实。历史备份另存/删除和旧迁移根清理仍是未定稿讨论，不新增这些命令。
