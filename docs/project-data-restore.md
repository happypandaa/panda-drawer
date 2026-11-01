# 项目数据版本取回：预览与应用边界

状态：2026-11-01 已退役。平台源码／数据版本、取回及相应恢复入口已删除；以下仅保留历史设计和验证记录，不应据此执行命令。当前代码版本由外部 AI 使用标准 Git 管理，现行范围见[项目配置与网页接入契约](unified-project-manifest-proposal.md)。

“预览数据版本”核对本机指定 Git 提交中的完整数据快照，并实际读取当前 SQLite 和附件，形成本次摘要及文件、目录变化清单。macOS 支持范围内的成功预览返回 `applyImplemented: true`、`canApply: true`，但不会自动应用。明确确认后的完整取回、继续/撤回和终态释放已独立实现，见 [应用与同操作恢复](project-data-restore-apply.md)。其他平台的应用能力为 false 并说明原因；此能力字段不扩大平台运行支持范围。

## 支持范围

- 首期为 macOS、健康的 project-v1 项目、项目根普通 SHA-1/files 引用标准 Git，当前 HEAD/索引匹配且没有待处理操作。旧 `app/.git` 仍为仅源码仓库；不会移动 Git 根或创建另一套仓库。
- 目标必须是调用方明确提供的完整 40 位小写本机提交 ID。没有 `version-data`、清单错项目、数据库/附件路径不符、对象缺失、文件多余或缺失、长度/摘要不符、链接/子模块/可执行文件、损坏 SQLite 均拒绝。不会获取远程对象。
- 目标只读取 `version-data/snapshot.json`、`database.sqlite`、清单列出的附件。清单保存空附件目录；Git 实际树也必须恰好对应列出文件的父目录。
- 数据库和每个附件上限为 256,000,000 字节，总数据 1 GiB，附件文件和目录合计 10,000 项，清单及 Git 元数据各自受 8 MiB 上限保护。超限整体拒绝，不截断继续。

## 读取和一致性

原始对象读取器调用已选择的 Git `cat-file --batch`，按 64 KiB 分块直接写入自己新建的临时候选；对象内容不通过通用 8 MiB stdout 收集器。每个对象严格检查 ID、类型、声明长度和分隔符，再以 `hash-object --no-filters`（没有 `-w`）核对完整原始对象 ID，并完整校验清单 SHA-256、附件目录和 SQLite 健康。不会经过 shell、checkout、Git 内容过滤器或业务 SDK。

预览持有项目操作租约，阻止同一项目开发操作、删除和工作区迁移。目标校验完成后，复用数据保存的一致性读取：冻结新业务请求、等待既有 SQL/上传排空、在项目独占锁内使用 SQLite Backup 和完整附件前后核对。阻塞读取任务持有锁、冻结和租约直到完成；异步请求被取消也不会提前放开正在读取的项目。阻塞任务在与停机相同的提交门禁内登记到 Git manager 的退出排空列表，丢失请求接收端后仍等待真实读取结束，停止后不能登记新任务。Git 对象读取和校验不占业务停写窗口。

`liveDataDigest` 是本次锁内生成的实际当前数据清单摘要，包含数据库和附件内容；它不是普通 Git 状态预览。`targetSnapshotDigest` 绑定目标清单的原始字节。`snapshotDigest` 继续绑定当前源码、Git、项目身份和已保存数据树；目标读取前后重新核对，发现外部源码/Git 变化即拒绝。预览之后业务可以继续写入，应用步骤仍重新读取并匹配新鲜当前数据摘要，不能将预览当成永久授权。

预览临时候选是本次独立临时目录，读取完成后清理；预览不会生成整项目 ZIP、第二套可浏览数据历史或持久恢复现场。预览本身不关闭 SQLite worker、不替换数据库/WAL/SHM/附件，不更改源码、HEAD、索引、程序、配置、页面代次或当前 `version-data`。明确应用入口才会建立保留原件的恢复现场并交换五个数据槽。

## 接口

输入 `GitDataRestorePreviewInput { targetCommit }`，未知字段拒绝。

- Core：`ProjectGitManager::data_restore_preview(id, input, stop)`。
- RuntimeHandle：`project_git_data_restore_preview(id, input)`。
- HTTP：`POST /api/v1/projects/{id}/git/data-restore-preview`，需要管理认证，成功 HTTP 200；它不创建任务。
- 原生管理命令：`project_git_data_restore_preview`；CLI：`project git data-restore-preview --project <id> --input <明确输入文件>`。
- 管理界面使用明确选择的完整提交号；预览不自动发起，展示数据库大小、附件及目录的变化。刷新、目标变化或其他版本操作撤销旧比较，实例变化后的迟到响应不回显。取回须再次明确确认并提交本次预览全部八项绑定；恢复入口独立于普通 Git/活库状态。

成功 DTO `GitDataRestorePreview`：

| 字段 | 含义 |
| --- | --- |
| `repositoryRoot`, `head`, `targetCommit`, `documentEpoch` | 当前项目和明确目标 |
| `snapshotDigest`, `liveDataDigest`, `targetSnapshotDigest`, `planDigest` | 源码/Git 状态、实际当前数据、目标数据和本次计划绑定 |
| `current`, `target` | `databaseBytes`, `attachmentCount`, `attachmentBytes`, `directoryCount` |
| `attachmentChanges` | 目标相对当前的 `{ path, kind: add/modify/delete }` |
| `directoryChanges` | 目标相对当前附件目录的 `{ path, kind: add/delete }`，包括空目录 |
| `databaseChanged` | 当前和目标数据库内容是否不同 |
| `canApply`, `applyImplemented`, `reason`, `issues` | 当前平台的明确应用能力及原因；预览成功本身没有故障 |

## 明确应用与恢复

应用独立执行新鲜摘要核对、排空业务并关闭 SQLite worker、数据库及三个伴随文件和附件的可恢复交换、固定新页面代次发布、持久门禁以及同操作继续/撤回。原现场与未知内容保留，迁移/删除不能绕过未完成现场。完成回执和 0400 terminal 封存解除门禁，已提交 epoch 只允许同向收尾。源码取回仍不改数据；本流程不复用已退役 ZIP。完整 API、未知状态处理和真实故障矩阵见 [应用与同操作恢复](project-data-restore-apply.md)。

## 验证记录

2026-10-24 在当前 macOS 主机上完成：

- `node scripts/rust.mjs test -p platform-core data_preview -- --nocapture --include-ignored`：9 项通过（2 项无外部工具校验，7 项指定官方 Git 2.56.0 与真实 SQLite 的临时项目验收）。包含超过 8 MiB 的真实数据库和 12 MiB 加 19 字节的非均匀附件完整读取；缺失/错误类型/错项目/多余附件/清单损坏目标拒绝；外部 SQLite 和同长度附件变更刷新摘要；同数量空目录改名产生明确变化清单；源码外部变化拒绝；取消后冻结和租约释放；丢请求后的退出排空和临时候选收尾；raw Git 不触发目标过滤器哨兵；HTTP 200、管理认证和 RuntimeHandle 接点。
- `node scripts/rust.mjs clippy -p platform-core --all-targets -- -D warnings` 通过，`git diff --check` 通过。
- 默认线程栈中的早期失败暴露了大型嵌套 future 和栈缓冲占用，已改为固定堆缓冲及关键 future 箱化；没有提高测试线程栈。早期工具失败回执摘录和后续原始成功日志保存在忽略的 `.test-artifacts/data-git-preview-2026-10-24/`，未作为项目数据或发布内容提交。

上述 2026-10-24 记录是最初只读预览步骤的验收，只使用明确命名的一次性项目及指定工具包，不操作开发项目。当时尚未提供数据替换和故障恢复；后续应用矩阵独立记录，不能倒填为当时已经完成。未执行 256 MB 实体对象压力、实际系统重启、最低 macOS 版本或干净机器验收。

提交前，主任务在暂存区独立源码中完成全工作区严格 Clippy、格式、TypeScript/Vite、378项全部DOM和本步9项真实/轻量专项复验。工作区测试先完成CLI23项、core602项通过（39项按原要求忽略），随后遇到一项旧Node登记断言而停止；该断言已按“普通登记不建立开发绑定”的现有契约单独修正，登记集成10项重新通过，剩余desktop14项、preflight37项分别完成通过。此为分包完成的实际检查，不将首次中断的整条命令改写成一次全绿。
