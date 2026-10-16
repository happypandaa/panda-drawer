# 历史副本显式清理验证记录

日期：2026-10-16。实现提交 `e5d65ec` 已提交并推送。29 项 React DOM 测试、183 项 Rust 测试、全目标 Clippy、格式检查、前端构建及最终 app/DMG 只读核验通过。当前“打僵尸”项目有活跃开发，已请求先确认未保存内容已处理，再切换新后端执行实际开发残留清理；目前没有删除这些开发资料。

本轮范围依据用户明确授权，为迁移旧工作区、已完成恢复保留的原现场、回收站提供范围预览及确认清理入口，并处理明确不再需要的平台测试残留。精确边界见[历史副本清理补充](history-cleanup.md)。本轮没有将历史接口讨论稿整体转为已确认契约；当前工作区的历史备份删除、备份另存与自动保留期限不在本次范围。

## 当前开发资料与审计基线

先前记录的 0 个活跃项目、29 项回收站不是本轮开始时的状态。本轮新审计发现 **1 个活跃项目、32 项回收站记录**。唯一活跃项目为 `prj_1bf930d58ac347979d55d62e7129de85`「打僵尸」，状态 `ready`、维护模式 `ready`、`recoveryRequired: false`、`busy: false`，源码状态为 `modified`。审计期间 `app/src/game/engine.ts` 又发生外部修改，增加 1295 字节；本轮没有写入或回滚该文件，不能宣称活跃源码从审计开始一直完全不变。

固定开发身份仍为 `dev.localprojects.platform.m0`，数据根为 `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0`，当前工作区为其下 `workspaces/current/`，旧迁移源为 `workspace/`。审计与实现验证期间保持原开发服务 PID `56852`，没有启动本轮新 app 或在该服务执行新清理接口。

私有审计目录：`.test-artifacts/cleanup-candidate-audit-2026-10-16T12-10-21-388Z/`。`protected-baseline.json` 记录 settings、当前项目树、全部当前备份和 project-state 的路径、类型、文件长度、SHA-256 与符号链接 target，不跟随链接；`current-settings.json` 保存设置原件的私有副本。审计当时的保护范围如下：

| 范围 | 基线普通文件 | 逻辑字节 |
| --- | ---: | ---: |
| settings | 1 | 296 |
| 当前项目树 | 3838 | 164149666 |
| 当前工作区备份树 | 44 | 6415971 |
| 当前 project-state | 99 | 40050 |

首次复核中 settings、备份和 project-state 全部不变；当前项目树只发现上述外部修改的 `engine.ts`，报告为 `protected-initial-verification.json`，状态如实记为 `changed`。这不是清理后的验证结果。后续核对须重新采样并区分用户开发变化与本轮操作，不能恢复旧源码来制造通过结果。

## 只读审计得到的候选与保留项

下表只是有来源依据的审计候选，不是已接受的服务计划，也不代表已删除。最终执行必须用新服务重新预览，重新检查任务、维护状态、保护引用、目录身份及内容指纹。仍被维护保护的原现场不能因列入审计候选就绕过门禁。

| 范围 | 审计数量 | 普通文件逻辑字节 | 归属依据 |
| --- | ---: | ---: | --- |
| 明确平台验证名称的回收项目 payload | 27 项 | 23602343 | 当前记录、实际 ID、状态及命名来源；不以泛称“测试”自动包含用户资料 |
| 已提交、已解决恢复的 `original/` payload | 12 个 | 265443173 | 2630 文件、23 符号链接；已完成恢复任务、原 journal 和迁移目标清单 |
| 已完成迁移保留旧根 | 1 个 | 1017882669 | 10328 文件、78 符号链接；源/目标身份、当前选择、退休标记及迁移内容证明 |

12 个恢复任务均已 `committed: true`、`resolved: true`，任务结果为 `succeeded/restored`，项目名为明确的 M3/M4 可丢弃验证。它们经迁移复制，当前 `original/` inode 与 journal 中原 inode 不同；审计逐项核对了当前 journal 的 SHA-256、长度和权限，以及 payload 全树的路径、类型、内容摘要、字节、链接 target 和权限，均与已提交迁移的 `target-manifest.json` 一致。证明见 `restore-migration-provenance.json`、`candidate-audit.json` 和各 `original-任务ID-inventory.json`。只允许清理经新预览确认的 `original/`，不删除该任务 journal、快照元数据、任务记录或整个 operations 目录。

旧根归属迁移任务 `task_1b8462ebb4e846eba48d4e1190423516`。迁移已 published、committed、resolved，阶段为 `completed`；实际源/目标身份与 journal 相符，目标仍是当前工作区。旧根持久文件与源 manifest 完全一致，旧根 42 个备份文件、4974449 字节均在当前备份树中找到相同长度及 SHA-256 的副本。旧根内 26 个历史项目目前都能对应到可恢复的当前回收项；历史 trash 的恢复或清除结果也有当前记录。审计详情见 `retained-current-ownership-with-trash.json`。

旧根是完整迁移时点副本，包含「测试 1」等历史名称，不能将整根称为纯缓存。清理该范围会删除这份旧快照，也包括用户后来从当前工作区主动删除的历史内容；当前副本和保护资料仍须可访问。上述字节是互不重叠路径中的逻辑文件长度，不承诺会释放同样多的物理磁盘块，尤其不能忽略 APFS 共享块和元数据。

以下 5 项用户命名或含义不明确的回收记录不作为平台测试残留自动选择：

| trash ID | 名称 | 审计状态 |
| --- | --- | --- |
| `trash_7c94a58199a149f0af4006fb472d14f6` | 打僵尸 | trashed |
| `trash_39a74a0bf9fc400c9bcba262a7fedbd9` | 打僵尸 | trashed |
| `trash_e2fc989493c84119a634d2c053923f30` | Dc | trashed |
| `trash_7c872c96d787443bbd5d26b91ca357ae` | 打僵尸 | trashed |
| `trash_ada7eb58e2094cabb0053820104e457e` | 测试 1 | trashed |

`protected-user-trash-baseline.json` 保存这 5 个回收目录的完整摘要，第一次重扫全部通过，证据为 `protected-user-trash-initial-verification.json`。当前活跃项目、全部当前备份及这些保留项继续保护。17 条维护控制记录中有 3 条 maintenance、0 条 recoveryRequired，所有已知基线/保护备份引用均存在。未发现未解决的 restore/lifecycle journal，但其余 100 个 operations 目录不因此自动变成缓存，继续保留。

## 新机制的保护与失败语义

管理界面在“空间与工作区 → 清理历史”展示明确候选、完整路径、逻辑字节、数量、保护原因和保留范围，不默认勾选。用户选择具体对象、核对永久范围、确认外部工具停止使用选中历史目录并输入“永久清理所选内容”后才允许提交。刷新、选择变化、切走页签、工作区变化或过期会撤销确认。

CLI 的 `workspace cleanup-plan` 和 `workspace cleanup --input …`、管理 HTTP 和原生管理窗口复用同一服务模块。计划绑定当前实例、工作区根身份、对象 ID、父目录与内容清单，十分钟内有效；客户端不能改写任意删除路径。相同已消费计划及相同选择返回原任务，不重新删除；新实例使未消费计划失效。

提交取得与迁移互斥的管理操作门禁并检查活动任务，执行前重核预览证据。恢复现场要求已知恢复 journal 结构、匹配的最终 restore 任务及项目归属，当前维护或未解决操作继续阻断。迁移后的 inode 变化必须由匹配的迁移清单与 journal 内容证明，不能凭路径名称放行。清单严格拒绝重复、缺失、非法路径或不明字段类型；未知维护控制不按“无保护”处理。

旧根清理核对源/目标清单及平台允许的 trash 身份重绑定证明，并核对当前项目/回收归属和保护副本可访问。当前回收 payload 缺失或身份不符时，不能仅凭迁移曾提交就删除可能的唯一旧副本；明确永久清除须有对应退休代次的已提交、已解决公开 purge 证据。数据库检查只对固定 `cleanup/diagnostics/` 内唯一临时副本执行 SQLite 诊断，原 DB 和 WAL/SHM/journal 在复制前后按身份及摘要核对，不用只读 SQLite 连接打开保留原件。

删除按已确认清单使用目录句柄相对操作，拒绝链接穿越、嵌套挂载、特殊文件、替换与新增未计划内容，不以整根递归删除兜底。回收项由生命周期模块登记 purge intent，再执行绑定范围；部分失败标为不再完整可恢复，历史备份保留。

计划、逐项 unlink 意图、已删进度及回执位于固定 `cleanup/`，不在待删树。失败或中断不自动续删，计数不确定时明确报告下界。重新处理需重新扫描及确认，只有同一根、已接受计划中持久记录且实际已无的删除条目才可从原清单扣除；新文件、重新出现的路径和身份变化仍阻断。退休 marker 或 `projects/` 目录在前一次授权删除中已消失时，也不能把未知缺失当作通过依据。清理完成的旧根保持历史回执，空间页不继续把它算作未知占用；同路径后来出现的新目录不继承已清理状态或删除资格。

## 已完成的测试与静态检查

| 检查 | 结果与证据 |
| --- | --- |
| React DOM | 29 项全部通过；`.test-artifacts/history-cleanup-dom-final.log` |
| Rust 工作区 | 183 项全部通过：platform-core 166、CLI 12、desktop 5；`.test-artifacts/history-cleanup-rust-full.log` |
| 全目标 Clippy | 通过；`.test-artifacts/history-cleanup-clippy-full.log` |
| Rust 格式 | 通过；`.test-artifacts/history-cleanup-fmt-final.log` |
| TypeScript/Vite | 通过；`.test-artifacts/history-cleanup-frontend-final.log` |

DOM 回归包括未选择/未确认时不提交、修改选择/刷新/切换页签撤销确认、过期与不同工作区计划拒绝执行、逐项结果和不确定计数下界、持久回执展示及已清理旧根的历史记录，并保留既有弹窗、草稿、AI 接手与回收站交互回归。测试使用真实 React/React DOM 与 jsdom，模拟原生命令及返回，不调用当前开发服务的删除接口。

Rust 测试在隔离临时数据根中实际创建目录、项目、备份、恢复和迁移夹具，并执行清理及重启核对，覆盖：

- 不跟随符号链接、替换祖先/文件或特殊文件拒绝清理；新增未计划文件在部分删除后仍保留，并报告实际已删和残留。
- 回收清理保留备份和记录、同计划不重复执行；计划过期、实例变化、错误选择、活动任务和未解决引用不删除。
- 部分 purge 的真实生命周期状态及活跃同 ID 冲突，已完成 original 只删 payload、保留 journal、备份和当前项目；未知 journal 或错配任务类型继续保护。
- macOS 本机临时目录的完整迁移、旧根清理及管理器重启，之后空间不出现虚假旧根占用；旧路径重用不会继承清理状态。
- 旧源新增内容、当前应保留 trash payload 缺失、目标选择身份不符均阻止旧根清理；迁移后的 original 需要 journal 与全 payload 清单匹配，新增未知文件后拒绝。
- 已接受计划在退休 marker 删除后模拟失败，通过新的显式预览、确认只清理已授权剩余范围；不自动重放、不扩大到新增路径。
- 损坏/重复清单、错误控制列表和超出 JavaScript 安全整数范围的字节数不被静默忽略、当零或浮点舍入。

这些是真实文件系统夹具行为，不是当前开发资料已被清理的证据，也不代表原生 GUI 或普通浏览器已实测。

## 历史交付保留与本轮交付核验

覆盖打包输出前，上一轮 AI 接手交付保存在 `.test-artifacts/ai-handoff-delivered/8aa51f53cbc608b99495bf2248bacf26cd12af057a8ff7291d2b581fc86a5b66/`。旧 DMG SHA-256 仍为目录名，54 个 app 条目核对一致，证据 `.test-artifacts/history-cleanup-retained-ai-package.json`。这只证明原交付保留，不作为本轮新包验收。

`npm run build` 已完成，日志为 `.test-artifacts/history-cleanup-build-dmg.log`。既有 `scripts/m5-package-verify.mjs` 对本轮包的只读核验结果为 `passed`，报告为 `.test-artifacts/history-cleanup-package-final/verification.json`，输出为 `.test-artifacts/history-cleanup-package-verify.log`。核验没有启动应用、重签或修改交付文件。

| 项目 | 本轮结果 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 字节数 | `8281705` |
| DMG SHA-256 | `8b5363997456af601d6babac4fa68dbade2c78124fafe85da23d30797cc81dbf` |
| 身份和架构 | `dev.localprojects.platform.m0`，版本 `0.0.0`；桌面与 CLI 为 `arm64`，最低系统 `15.0` |
| 签名 | 源 app 和 DMG 内 app 均通过严格深度验证，ad-hoc、runtime、无 TeamIdentifier |
| DMG 一致性 | 校验成功，只读挂载 app 的 54 个条目与源 app 一致，核验后已卸载 |
| 固定 SDK | 8 处 `0.1.0` 副本 SHA-256 为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9` |
| 媒体配置 | 相机、麦克风用途及 camera/audio-input 权利保持，未增加调试放宽权利 |

实际开发目录的清理仍待安全切换到新实例后执行，不把上述包核验或隔离夹具当作本机残留已删除的证据。后续应重新取得服务计划，分别记录清理旧根、明确测试回收项及解除保护后的测试原现场的实际任务与回执，并复核当前项目、44 个当前备份文件、5 个保留回收项和恢复/迁移元数据。私有 `.test-artifacts/history-cleanup-live/` 已准备限定审计 ID 的预览、执行与保护库存脚本，尚未执行删除。

## 验证限制

- 当前开发服务仍为原 PID `56852`，未启动本轮新 app、未在原服务运行新清理模块、未清理开发残留。新 CLI 对既有服务的只读查询不能证明新后端已上线。
- 活跃源码有外部持续编辑；未改写或回滚该变化，不能用旧 0/29 状态或审计起点的整树完全不变断言掩盖它。
- 未执行真实管理 GUI 点击或浏览器自动化，也没有绕过此前工具 URL 访问限制。jsdom 不证明 WebKit 布局、滚动、窗口尺寸、原生确认、系统权限或 Safari/Chrome 兼容。
- 隔离夹具中的删除、迁移与重启测试不替代当前开发数据的实际操作证据。实际执行必须重新预览，并记录最终状态；部分失败不能声称完整可恢复或保证释放同等物理磁盘空间。
- 文件系统核对要求外部工具停止写入所选历史目录，不承诺隔离同一 macOS 用户的恶意并发篡改。开发包仍为固定开发身份和 ad-hoc 签名，不据此声称 Developer ID、公证或互联网发行通过。
