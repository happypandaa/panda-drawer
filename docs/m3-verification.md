# M3 验证记录

日期：2026-10-15。范围：隔离开发版整项目备份、持久维护与恢复。M3 已完成本机内部验收；本文记录实际检查、修复和开发包，后续发行门槛单列于限制。

## 环境与隔离

开发身份固定为 `dev.localprojects.platform.m0`，环境为 `development`，工作区固定在 `~/Library/Application Support/dev.localprojects.platform.m0/workspace/`。没有新增根目录覆盖，也不访问正式数据。实际实例测试只创建名称以 `M3 可丢弃` 开头的新项目，保留此前的开发项目；不通过清空开发工作区解决失败。

CLI 默认从 `target/release/bundle/macos/Local Projects Dev.app/Contents/MacOS/local-projects-dev` 取得；`M3_TEST_CLI` 仅选择测试客户端二进制，不改变服务的数据根。SDK 仍为固定 `@local-projects/sdk 0.1.0`，模板仍为 `react-ts-vite-0.1.1`，不以本轮平台改动替换旧项目文件。

本机为 macOS 27.0.1（26A434）、Apple Silicon，系统 Node 24.14.1。打包目标与原生 CLI 均为 arm64，应用声明最低 macOS 15.0；此次运行结果不代替 macOS 15 的真实设备验收。

## 自动与真实实例检查

| 检查 | 当前记录 |
| --- | --- |
| M3 Node 验收脚本语法 | `m3-test-utils`、`m3-live-test`、`m3-restart-test`、`m3-compatibility-test` 的 `node --check` 已通过 |
| 升级前已有开发项目 | 全部 12 项目升级后持久文件比对已通过；证据为 `.test-artifacts/m3-original-projects-verification.json` |
| Rust 单元、服务及中断测试 | 100 项通过：core 88、CLI 10、desktop 2；完整日志 `.test-artifacts/m3-rust-tests.log` |
| SDK 回归与固定发布包 | 12 项通过；固定 0.1.0 tarball 与源码发布、模板 vendor、包内副本摘要一致 |
| Rust 格式、Clippy、管理 UI 构建 | 最终格式、Clippy、管理 UI 构建及包含确认框修订的最终开发包均通过 |
| 页面状态与重载提示回归 | `npm run test:runtime-notice` 的 7 组行为检查通过；日志 `.test-artifacts/m3-runtime-notice-tests.log` |
| 已打包服务 M3 live probe | 第二轮及最终包独立重跑全部通过，最新报告 `.test-artifacts/m3-live.json`，完整日志 `.test-artifacts/m3-live-run.log` |
| 完整退出与重启后的维护 probe | 最终包重启后全部通过；报告 `.test-artifacts/m3-restart.json`，完整输出 `m3-restart-verify.log` |
| 真实桌面与普通浏览器 | 备份、维护、恢复、旧页拒写、两步确认取消保留草稿与明确重载均通过最终包实测 |
| `.app` / DMG、签名及摘要 | 最终包生成、源 bundle 与 DMG 内容比对及 ad-hoc 签名验证通过，详见下文 |

Rust 覆盖完整归档与 WAL 独立副本、隐藏资料及路径归属排除、源变化和暂存写失败、归档发布冲突、坏库与外键问题的区分、损坏/重复/大小写与 Unicode 别名/越界/符号链接 ZIP、ZIP64、两类可移植来源、已登记产物缺失与摘要不符、10 份自动备份加手动/保护引用的保留。维护测试覆盖合法 unbuilt、来源 unknown、损坏原库完整现场、操作冲突、基线失败重试和结束检查。

中断覆盖使用隔离临时目录构造持久阶段，再重建管理器执行启动核对：普通备份暂存但未发布、已发布但响应丢失、基线准备中断、基线已发布但未设置可修改、可修改已提交但任务终态丢失，以及恢复目录移动前后、提交前后和无法识别完整根的情况。缺失基线不会继续报告可修改；上一轮维护日志不会误关联新一轮。预期结果为明确的 interrupted/未发布/已提交、回退、恢复或待处理，不把响应丢失记为成功。目录切换与响应丢失使用持久阶段重建测试；另以不可写日志/控制文件等方式进行受控文件系统故障注入。没有执行硬件断电测试，不能把这些结果扩大为真实断电覆盖。

最终补充 3 项回归：恢复或回退已经解决但任务响应丢失，重启保留真实结果且不重放旧状态；恢复核对后的 journal 写入失败，即使控制状态也无法持久保存，当前进程仍保持阻断，不开放写入。修复完成后重新运行整个 workspace，合计 100 项全部通过。

SDK 发布包 SHA-256 保持 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9`。证据为 `.test-artifacts/m3-final-sdk.log`、`m3-final-sdk-sha256.txt`；格式和 Clippy 证据为 `m3-final-fmt.log`、`m3-final-clippy.log`。

`node scripts/m3-live-test.mjs` 新建一个独立测试项目，使用公开 CLI、业务 HTTP、项目内测试文件与该项目自己的备份目录验证：

- starter 与真实用户构建的归档来源；完整源码、固定 SDK、README、skill、清单、PNG 图标、隐藏资料、SQLite 与附件内容。
- 附件中的 `node_modules`、`.git`、`.platform` 及隐藏目录保留；仅已识别工程依赖根、根控制区和版本控制内部目录排除。
- 普通备份失败后解冻；基线失败持久维护并禁止修改，同一维护 ID 重试；仅展示资料变化可结束，无需构建。
- 在系统 Node 选择暂时不可用时恢复 starter；恢复后仍在维护，明确结束后才能继续使用；源码和数据回到同一时点。
- 旧文档拿到新会话 Cookie 仍被 `PROJECT_RELOAD_REQUIRED` 拒绝；普通备份不推进文档代次。
- `ready` 快照源码 modified 可保留原网页；维护期间修改构建输入则必须成功构建，恢复维护快照也不消除该要求。
- 损坏 ZIP、文件清单摘要正确但 SQLite 内容损坏、仅该项目备份发布路径不可写入不损伤当前根，不登记虚假成功；测试保留并复原自己的备份目录及 ZIP 原字节。
- 备份前存在已提交 WAL 数据，恢复后读取对应记录；恢复结果指向的完整原现场保留恢复前源码与附件。

上述 live 检查在第二轮完整通过，并在全部后端修复后的最终 M3 `.app` 及包内 CLI 上再次独立重跑全部通过。报告写入忽略的 `.test-artifacts/m3-live.json`，不保存管理令牌或 launch ticket。最终项目为 `prj_9ae2e79513ad4430894fdb973ed4393c`；备份前确认 `main.sqlite-wal` 为 12,392 字节，恢复后对应业务记录仍在。

| 快照或关键结果 | 实际标识 |
| --- | --- |
| 初始模板完整快照 | `backup_cc45e660457a45ba8da9e72436ab7387` |
| 用户成功构建快照 | `backup_3905a8806ef44fcc886545de434c37b0` |
| ready + modified 源码快照 | `backup_645f988984b845979f26a4499aef2bb4` |
| 维护中待构建快照 | `backup_4f53edef31524412b13487e3c68acf00` |
| 无 Node 恢复初始模板 | `task_a69cae8857024968814a339550cecb4d`，成功 |
| 恢复后的待构建要求 | `task_e7f1b61adc5b4a2bab05fd4268e30274`，明确失败 `BUILD_REQUIRED`；后续匹配构建成功后可结束 |
| 损坏 ZIP 候选 | `task_d6a4a511b91b4dd182e101ecd8327be4`，`BACKUP_INVALID` |
| 摘要匹配但 SQLite 损坏 | `task_13247da380c74f32a475e6b79fc668b7`，`DATABASE_INVALID` |
| 备份发布目标不可创建 | `task_b1f36cae77be48cc83ae3a6cdb5c759f`，`FILESYSTEM_ERROR`，原项目可继续使用 |

首轮实际运行使用新建项目 `prj_955530ec983c41a0969c4cffa712425f`，完整日志保留在 `.test-artifacts/m3-live-run-01.log`。从完整 starter 归档到损坏 SQLite 候选拒绝均通过；最后用 `chmod` 注入备份目录写失败时，平台管理目录的权限修复使备份实际成功，因此该断言未通过。这是故障注入失效，未作为后端写失败处理已通过的证据。脚本改为保留该新项目的备份目录，在原路径放普通文件阻止创建目标目录，并在 `finally` 完整复原；第二轮用全新项目 `prj_ad6287d134254d6ebe3b7388e6ca8eec` 完整重跑通过；最终包再新建第三个 live 项目完整通过。前次成功报告与日志保留为 `.test-artifacts/m3-live-previous.json`、`m3-live-run-previous.log`，三轮项目全部保留。

重启验证使用 `node scripts/m3-restart-test.mjs prepare`，准备一个新的维护中项目；明确退出桌面服务并重新启动后执行 `node scripts/m3-restart-test.mjs verify`。检查同一维护 ID、可用基线、业务记录与附件保留，旧实例 Cookie 失效，显式结束才推进文档代次。临时旧会话证据保存于权限 `0600` 的 `.test-artifacts/m3-restart-state.json`；公开结果为 `.test-artifacts/m3-restart.json`。

本轮准备项目 `prj_fd580b505ac442eeb3b09fac8db51631`，随后用桌面 `Cmd+Q` 完整退出，CLI 确认 `SERVICE_NOT_RUNNING`，证据 `.test-artifacts/m3-full-quit.json`。最终包启动后的 verify 全部通过：实例从 `e34e79ff-cdf9-4d6a-ae82-5739a222ad44` 变为 `8f1b247b-5f1e-4153-ae10-f9edbc280c45`，同一维护 ID 和基线保持、维护前/中两条记录及附件保留，旧 Cookie 失效；显式结束 `task_a27642b3a4fb4546b818127b53d207ab` 才推进文档代次，新 Cookie 配旧代次仍被拒绝。启动没有自动解锁。最终包运行后对原有 12 项目的文件摘要比对再次全部通过。

`node scripts/m3-compatibility-test.mjs prepare|verify` 比对原有项目的持久文件；数据库、WAL/SHM 文件字节会因连接打开或 checkpoint 变化，单独排除，不能把该摘要比对扩大为数据库逐字节未变化的证据。其余排除仅根控制区、版本控制内部目录及已识别工程依赖根。

## 真实桌面与浏览器

使用新建项目 `prj_b86b12c0589f47cab9e8fdec0a08bd5e`（“M3 可丢弃 桌面浏览器草稿验证”）。普通 Chrome 与独立桌面窗口均已验证保留未提交草稿；管理 UI 成功创建手动备份 `backup_eefa6af462774cd5883fc9914418542c`，并完成进入维护、结束维护。旧页面的业务请求分别收到 `PROJECT_MAINTENANCE` 和 `PROJECT_RELOAD_REQUIRED`，没有在结束后自动恢复写入。

首次 Chrome 原生确认框取消后仍保留草稿，再次确认显式重载有效；同轮真实桌面发现 Tauri/WebKit 中 `window.confirm` 没有可用效果。现已替换为 shadow DOM 内的页面两步确认，并在最终包的桌面 WKWebView 与普通 Chrome 重新验证通过。这一问题来自实际交互，修复后分别核对两端，未以 HTTP 门禁或浏览器结果代替桌面结果。

管理 UI 整项目恢复 `task_92528ecb915043f9aada469d168c3df3` 已成功；任务详情能展开原现场位置 `.platform/operations/task_92528ecb915043f9aada469d168c3df3/original`。再由 UI 完成结束维护 `task_f600c8b2496e467eaf4441c9238bd92a`，Chrome 明确重载后记录从恢复前 3 条回到快照的 2 条，显示“快照基线记录”和“浏览器已保存便签”，“恢复后应移除记录”已不在恢复根中。

对已经关闭连接的原现场数据库使用只读 immutable 连接核对，全部原 3 条仍完整保留。证据 `.test-artifacts/m3-ui.json` 含任务结果、交互观察及 `preservedOriginalRows`；不是把被移除数据永久删除后声称可恢复。修订后的页面内两步确认已通过自动回归，包括取消/Escape 保留草稿、只有最终确认才重载、新一轮维护撤销待确认重载、固定文档代次和页面缓存恢复连接；最终包的桌面/Chrome 真实交互复验全部通过。两端点击“处理草稿后重新加载”均显示确认区，点击取消后通过实际页面状态与桌面截图确认草稿仍在；再次展开并点击“已处理草稿，确认重新加载”，均实际重载，草稿清空、提示消失，恢复后的两条数据仍在。最终 UI 项目为 `ready`。对应维护任务为 `task_ff53da3e5a2340e383ac71bd92c47185` / `task_317c0b81ce9f4a028eed30d6b55a7c3c`，证据 `.test-artifacts/m3-ui-final-maintenance.json`；`m3-ui.json` 已记录问题解决及两端最终结果。

## 最终开发包

- 应用：`target/release/bundle/macos/Local Projects Dev.app`。
- DMG：`target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg`，6,933,266 字节。
- DMG SHA-256：`bfdc16db30ce7f9e7a102b14a049ad42c54d7036d2c9c6ddbef08db0dd348049`。
- 应用与包内 CLI 为 arm64，应用标识为 `dev.localprojects.platform.m0`，版本为 `0.0.0`，最低系统声明为 macOS 15.0。
- 源应用及 DMG 内应用均通过 `codesign --verify --deep --strict`，签名为 ad-hoc，并启用 runtime；没有 Developer ID 签名或公证。
- DMG 只读挂载后核对 49 个条目与源 bundle 一致，检查后已卸载。包内固定 SDK 与模板 vendor 摘要未变化，不包含 Node。

最终构建日志为 `.test-artifacts/m3-build-dmg.log`；身份、签名、只读挂载、内容比对、大小及摘要证据分别保存在 `.test-artifacts/m3-final-*`。最终应用已实际启动并完成上述重启、兼容性和完整 live 验收。

## 本轮范围与限制

本轮只交付已确认的 M3 基础能力。导入、导出、复制、回收站、工作区迁移、完整空间统计和元数据编辑界面属于后续阶段；历史备份另存、删除与迁移旧根的讨论稿接口不视为已实现。手动备份不自动清理，普通自动备份按已确认规则保留最近 10 份可清理归档；受保护基线和未解决现场单独保留。

SQL、上传、构建、归档测试预算和日志限制继续是内部测试参数。全项目一致性要求外部 AI/编辑器/数据库工具暂停直接写入；摘要和平台互斥不是任意外部文件写入的系统级锁。平台数据库检查不证明业务数据或附件引用全部正确。

内部包仍使用开发身份与 ad-hoc 签名，不包含 Node，不是已签名公证的公开发行包。M6 的 Developer ID、公证、最低系统实机与外部用户验收继续单独完成。
