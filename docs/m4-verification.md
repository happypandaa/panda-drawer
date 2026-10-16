# M4 验证记录

日期：2026-10-16。整项目导入导出、复制、回收站和工作区迁移已实现并生成开发包；真实服务 CLI/API、公开迁移及进程重启验证通过。M4 生命周期 GUI 全流程受工具访问限制未完成，不能将本记录理解为所有交互验收通过。

## 结果与证据

| 检查 | 结果 | 证据（仓库 `.test-artifacts/`） |
| --- | --- | --- |
| Rust workspace | 132 项通过：118 core、12 CLI、2 desktop | `m4-rust-tests.log` |
| 格式与 Clippy | workspace fmt、Clippy `-D warnings` 通过 | `m4-fmt-check.log`、`m4-clippy.log` |
| 固定 SDK | 12 项通过；0.1.0 所有副本内容不变 | `m4-sdk-tests.log`、`m4-final-sdk-hashes.json` |
| 页面状态提示回归 | 8 组行为、21 个请求通过 | `m4-runtime-notice-tests.log` |
| 新项目生命周期 live | 首个 M4 包 10 组检查通过，29 个最终任务：25 成功、4 个预期失败 | `m4-live-run.log`、`m4-live.json` |
| 公开工作区迁移 | 4,453 文件、313,973,506 字节；任务成功并提交新根 | `m4-actual-migration.json` |
| 迁移后的真实进程重启 | 新 PID、新实例，仍选择已提交的新根 | `m4-process-restart.json` |
| 迁移后内容及功能核对 | 26 活跃项目、14 棵保护树；维护保留、旧身份拒绝、回收恢复/清除通过 | `m4-migration-static.json`、`m4-migration.json`、`m4-migration-verify.log` |
| 原有项目兼容 | 升级及迁移后，17 个项目纳入基线的持久文件摘要一致 | `m4-compatibility-run.log`、`m4-compatibility-after-migration.log`、`m4-original-projects-verification.json` |
| 最终开发包 | app/DMG 签名、身份、架构、内容一致性通过 | `m4-final-*` |
| 真实桌面/浏览器交互 | 仅完成原生管理窗口启动、列表及“在浏览器打开”按钮；后续交互未完成 | 下文说明 |

## 数据保护与当前位置

M3 原开发包完整保留在 `.test-artifacts/m3-delivered/bfdc16db30ce7f9e7a102b14a049ad42c54d7036d2c9c6ddbef08db0dd348049/`，包括可恢复的 `Local Projects Dev.app`、原 DMG 和 `preservation.json`。保存时拒绝覆盖已有不同内容；49 个条目与原 app 一致，副本严格签名校验通过。M3 DMG 为 6,933,266 字节，SHA-256 仍为上述目录名。

M4 开工时服务离线，只读固定开发 settings 并记录全部 17 个既有项目。权限 `0600` 的 `m4-original-projects.json` 覆盖源码、SDK、说明与 skill、清单、图标、网页和附件；已识别依赖根、项目控制区及 VCS 内部排除。SQLite/WAL/SHM 的离线原始摘要另行保留，但 checkpoint 可以改变字节，因此兼容核对不将数据库字节相同作为业务完整性的唯一证明。升级及迁移后，17 个项目全部仍在活跃列表，纳入比较的持久文件一致。

当前固定身份仍是 `dev.localprojects.platform.m0`、`development`，数据根为 `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0`。实际位置为：

| 用途 | 绝对路径 |
| --- | --- |
| 当前工作区 | `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0/workspaces/current` |
| 保留的迁移源副本 | `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0/workspace` |
| 本次迁移日志 | `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0/migrations/task_1b8462ebb4e846eba48d4e1190423516/` |

通过公开 CLI 迁移，未直接改 settings 或改名原树。固定设置、服务发现和凭据继续位于开发数据根；迁移日志 `committed`、`resolved` 均为 true，旧根是仍存在的实体目录，工作区状态为 ready。Node 环境恢复为 `ready: true`、`configuredNodePath: null`，见 `m4-delivery-runtime.json`。此后编辑和使用应以新 `project handoff` 路径为准；不继续写入旧副本。

## 生命周期实际实例验证

`node scripts/m4-live-test.mjs` 在首个 M4 包运行，只写本轮名称以 `M4 可丢弃` 开头的新样本，主项目为 `prj_400c6305f6484228ba234266de5f4af2`。最终迁移修复包的后续验证单列于迁移章节。已验证：

- 完整导出保留源码、固定 SDK、skill、清单/图标、隐藏资料、网页及附件；附件中名为 `node_modules`、`.git`、`.platform` 的路径不被误删。
- 默认不覆盖目标，拒绝自包含和不可用路径；显式覆盖成功。原 ID 冲突、坏 ZIP，以及校验值正确但 SQLite 损坏的归档均被拒绝。
- 临时不可用 Node 时导入和复制成功；新平台 ID 与独立文件保留业务主键、UUID、附件名和用户文字。`try/finally` 恢复 Node 后，真实构建再导出/导入，`projectBuild` 来源与可运行网页保持，导入不再次构建；初始网页的 `bundledTemplate` 也通过。
- 维护副本和维护归档导入获得独立维护 ID、新 ID 的可恢复基线；恢复能还原业务数据，但不能抹掉已知待构建要求。源项目维护状态保持。
- 回收站继续占用原 ID；恢复、永久清除后原 ID 重新导入均推进页面身份，新 Cookie 不能让旧文档继续写入。清除保留历史备份和外部 ZIP。
- 损坏数据库原字节进入回收站并恢复，恢复后为未准备好的维护状态、拒绝 SQL；携当前维护 ID 再次删除和清除成功，健康历史备份仍完整。

本轮留下的维护样本为 `prj_9ac26458fe234c84881d73cbced3bfaf` 和 `prj_36617dbc17f04a4fb7448b4294910d3f`，其待构建要求属于测试事实，未强行结束维护。

## 真实迁移、重启与回收操作

`m4-migration-test.mjs prepare` 记录 26 个活跃项目和 14 棵历史备份/回收站/恢复原现场保护树。正常样本为 `prj_6245fea32afe499591219e62485061d3`，维护样本为 `prj_5908b801201e49d4be3bff9ce521636d`，回收项为 `trash_f822312207304c98a5fc79b0e28e9e4b`。受保护现场内依赖目录与符号链接完整保留，不追随链接读取树外内容，也不按缓存名称递归排除。

最终包通过 `workspace migrate` 提交 `task_1b8462ebb4e846eba48d4e1190423516` 并成功切换。迁移即时核对旧管理凭据及旧会话均返回 401。之后对已通过固定 development discovery 和可执行路径确认的开发进程发送 SIGTERM，再直接启动 app 内可执行文件；旧 PID 5667 变为 5770，实例变为 `220cc517-76cc-49b0-9d44-09c60653878f`，新根仍生效。这是实际进程终止/重新启动，**不是本轮通过 GUI 菜单正常退出的验收，也不是迁移中硬件断电测试**。

重启后 `m4-migration-test.mjs verify` 通过。所有 26 个项目保持 ID 并指向新根，文档代次推进；维护 ID、基线和业务数据保持。14 棵源保护树与迁移前摘要一致，目标全部历史文件和符号链接一致；1 个有效回收站记录只修改已验证的 `rootIdentity`，两端各自匹配实际目录的 device/inode，其他 JSON 字段保持。重绑授权和复制证据留在固定迁移任务下的 `trash-rebindings.json`、`target-manifest.json`。

旧管理凭据、旧 Cookie 及新 Cookie 携旧文档代次均被拒绝。新根中的数据和附件可读，新增写入成功且源数据库摘要不变，交接说明指向新位置。静态核对先保存到 `m4-migration-static.json`，随后单独对准备的回收项执行恢复、网页会话、SQL 写入、再次删除和永久清除；三个状态任务均成功，历史备份摘要不变，最终清除项为 `trash_e9c8f3ad6c7a40cab518e24c7c878ccb`。这组后续变更只影响本轮可丢弃样本，区别于之前的静态树比较。

迁移后再次执行原 17 项目兼容核对通过。跨重启需要的旧会话/凭据只保存在 `0600` 本地状态文件中，不输出至运行日志、项目文件或验收摘要。

## 修复与故障覆盖

只读审查及回归发现并修复了设置提交后日志未记 committed、原源失联阻止新根启动、异常目录名阻止全区复制、快速二次迁移、暂停后数据库 worker 重生等窗口。精确阶段测试覆盖提交前后、目标缺失、设置丢失与旧源防回退，并验证未解决操作日志阻止迁移、既有身份异常不会因复制被当成健康项目。

首包后的审查还发现回收站记录保留旧 inode 会使迁移后恢复/清除失败，实际迁移前已修复并重包。最终实现只对源身份有效的回收项重绑目标身份；源记录不变，坏记录或源身份错配保持阻断。跨模块回归覆盖迁移后真实恢复、含符号链接的回收树、部分清除迁移后的显式重试；最终包上述实际回收恢复/清除也通过。阶段重建、受控文件系统故障和真实进程重启分别记录，不宣称覆盖任意断电或硬件损坏。

## GUI 证据与限制

CUA 实际启动首个 M4 原生管理窗口，列表显示原 17 项目及新样本；UI 样本 `prj_a4555636d23747309da98f665e659077` 的真实构建任务 `task_5834e70a274b4a759aad7baa11a6a4c5` 成功，点击“在浏览器打开”后管理端显示成功提示。随后获取 Chrome 时 CUA 因工具 URL 访问限制终止会话，没有换用其他 UI 工具绕过。

因此导出、导入、复制、回收站与迁移的完整 GUI 操作，以及本轮真实浏览器业务交互未完成。CLI/API 的 HTTP 会话、页面 HTML、SQLite 和文件响应，以及运行提示脚本回归，不替代真实交互验收。最终修复包通过包内进程启动及后端验证，没有补写为 CUA 最终包全流程通过。

## 最终开发包

- App：`target/release/bundle/macos/Local Projects Dev.app`
- DMG：`target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg`
- DMG 字节数：`7602556`
- DMG SHA-256：`a28f95e8a02e329cd3b48d6de0f7fbdc0a9cd02a020d06a4f7308ccd326953d6`

应用与 DMG 内应用均通过 `codesign --verify --deep --strict`，ad-hoc 签名、runtime 标志、无 TeamIdentifier，identifier 为 `dev.localprojects.platform.m0`，最低 macOS 15.0，桌面程序和 CLI 均为 arm64。DMG 只读挂载的 49 个条目与构建 app 内容完全一致，已卸载。固定 SDK 的七份核验摘要均为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9`。本轮没有修改已分发 SDK、既有项目 skill、源码或用户说明；资源更新只用于新项目和包内后备资源。

包内 `resources/README.md` 是开发说明副本，其中相对链接所指验证文档并未整体打包；完整记录以仓库 `docs/` 为准。本轮未为此改变冻结后的最终包摘要。

历史备份另存/删除和旧迁移根清理接口仍未定稿，当前只显示保留旧根，不提供删除命令。完整空间管理、正式品牌、Developer ID 签名、公证、最低系统实机和发行验收仍属后续阶段。SQL、上传、构建和归档数值继续作为内部测试参数。
