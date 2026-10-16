# 删除后空间占用与 AI 接手说明验证记录

日期：2026-10-16。实现提交：`82e56c7`，已提交并推送。本轮完成空间占用调查、保留原因说明和可见的“复制 AI 接手说明”入口；24 项 React DOM 交互测试、167 项 Rust 测试、格式检查、core Clippy、前端构建、最终 app/DMG 只读核验与打包后资料保留核对均通过。本轮未启动新应用或执行真实 GUI 交互。

## 真实空间调查

通过固定开发身份的公开 CLI 只读查询 `project list`、`workspace info`、`workspace storage` 和 `trash list`，确认当前活跃项目为 **0**，回收站为 **29** 项，29 项均为 `trashed` 且 `restoreAvailable: true`。用户删除操作已经将项目从活跃列表移入回收站，未发现本次删除失败或项目仍活跃的证据。本轮没有恢复用户已删除的项目。

开发身份保持 `dev.localprojects.platform.m0`，数据根保持 `/Users/ethan/Library/Application Support/dev.localprojects.platform.m0`。当前工作区是其下 `workspaces/current/`，保留旧迁移源是 `workspace/`。调查时服务 PID 为 `56852`，instanceId 为 `01b67e41-6980-480f-874f-51215deff9cd`。工作区没有活跃迁移任务，迁移状态为 `ready`；保留旧根的身份核对通过，API 返回 `canDelete: false`，原因为旧根清理接口尚未定稿。

调查时普通文件的逻辑字节如下。MiB 按 `1,048,576` 字节换算；界面显示使用十进制 KB/MB/GB，因此两者显示数字不同。

| 范围 | 精确字节 | MiB | 普通文件数 |
| --- | ---: | ---: | ---: |
| 当前工作区合计 | 317022802 | 302.34 | 4957 |
| └ 操作现场 `.platform/operations` | 267565352 | 255.17 | 2887 |
| └ 回收站 `.platform/trash` | 24263984 | 23.14 | 1227 |
| └ 共享缓存 `.platform/cache` | 18158793 | 17.32 | 91 |
| └ 历史备份 `backups` | 4974449 | 4.74 | 42 |
| └ 其他平台控制记录 | 2060224 | 1.96 | 710 |
| 迁移保留旧工作区 | 1017882669 | 970.73 | 10328 |
| 调查时运行的程序安装目录 | 18579791 | 17.72 | 36 |
| 数据根中不属于上述工作区的设置与运行记录 | 3245897 | 3.10 | 13 |
| 已知互不重叠范围合计 | 1356731159 | 1293.88 | — |

当前工作区的分类是其合计的组成部分，不能再次相加。调查时的程序体积仅代表当时运行安装目录，不是本轮新包的最终体积。已知范围合计不包含仓库构建产物、外部导出 ZIP、下载的 DMG 或其他未登记的个人文件。

操作现场共有 107 个目录。其中 12 个恢复 journal 均已 `committed: true`、`resolved: true`；最大的 4 个目录合计 `260999222` 字节，各约 65.25 MB，主要是历史恢复操作保留的完整 `original/`，其中包含依赖。这些是既有恢复证据，没有因本次删除而重新产生，也不能据体积判定为未完成或失败操作。其余目录包含生命周期记录、快照等。共享缓存主要是约 `18157053` 字节的 npm 缓存。

`du -k` 核对当前工作区为 `322580` KiB，即 `330321920` 分配字节、315.02 MiB；保留旧根为 `1020280` KiB，即 `1044766720` 分配字节、996.37 MiB。逻辑字节与分配块大小口径不同；目录元数据、文件分配粒度、硬链接及 APFS 共享块等会影响两者关系。即便使用 `du`，也不能把该数字承诺为删除后可释放的物理空间。

扫描不跟随符号链接，只统计普通文件。当前工作区及旧根的扫描问题均为 `symlink_not_counted`，未出现读取失败；同一链接会在两遍一致性扫描的问题列表中出现两次，因此问题条数不是独立链接数。扫描保守地标记为部分统计，不能称为全盘物理占用已精确统计。独立元数据遍历得到旧根 `1017882669` 字节，与公开 API 一致。

公开 `GET workspace storage` 是查询接口，但会按现有实现更新自身扫描缓存。调用后独立遍历当前根比 API 采样结果多 `12363` 字节，来自本次生成的工作区统计缓存；这不是遗留活跃项目或重复相加。该更新边界不包含项目、回收站、备份、操作现场和旧迁移根。

调查时磁盘可用 `221740871680` 字节，未发现需要紧急清理的空间压力。本轮没有进行清理、永久删除、恢复、迁移或手动更改 settings。删除项目默认进入回收站；现有永久清除入口仅处理明确选中的回收项，历史备份和受保护操作现场继续保留，旧迁移根清理仍未定稿。

界面在空间总览增加“为什么删除项目后仍有占用”，按当前 API 的回收站、历史备份、操作现场、共享缓存及迁移保留目录展示大小与保留原因。说明行引用上方已统计范围，不重复计入合计，也不新增未确认的旧根或历史备份删除能力。

## AI 接手入口与内容

项目详情内提供可见的“让其他 AI 继续开发”区块和主按钮“复制 AI 接手说明”，说明它会复制软件介绍、当前项目目录和操作规则。新建成功后打开新项目详情，创建本身不会隐式写入剪贴板；用户明确点击后才获取和复制说明。

交接文本介绍 Local Projects Dev 是 macOS 本地业务应用平台，管理项目源码、独立 SQLite、附件、运行、构建、备份与维护；明确接手对象是业务项目。文本提供实际项目 ID、当前绝对根目录、当前安装 CLI、接管 skill 和清单路径，并要求以清单声明读取实际源码、产物、数据库、附件和文档位置。默认模板路径明确标为示例，不能覆盖自定义清单。复制、导入、迁移后存在的平台交接记录会按当前路径引用。

文本包含通过现有 CLI 查询项目和 schema、管理 SQL/事务、构建、等待任务及读取日志和入口的示例；命令路径和项目 ID 经过 shell 引号处理。固定 SDK 仍是 `0.1.0`，要求保留用户数据和草稿，遵守维护及基线规则，不直接替换运行中的数据库或手工覆盖发布目录，不持久保存凭据。接收文本的 AI 需要具备本机目录访问能力才能直接改文件；纯聊天 AI 可以理解背景，但不因此获得文件访问能力。

读取交接说明不改写旧项目 README、skill、源码、SDK 或平台交接文件；旧项目缺少项目自带 skill 时引用当前安装的后备说明，不向旧项目补写文件，也不跟随项目 skill 或交接记录的符号链接。

前端在项目切换、移出列表或服务实例变化时使迟到的交接响应失效；请求期间阻止重复点击。系统剪贴板写入通过队列排序，避免较早项目的已开始写入最后完成并覆盖较新项目的内容。已经交给系统的写入不能撤销，因此此处保证顺序和后续响应边界，不声称能够取消原生剪贴板操作。完整文本会在同一详情内展开；剪贴板拒绝访问时仍可选中全文手动复制。

## 已完成检查

| 检查 | 结果与证据 |
| --- | --- |
| React DOM 交互 | `npm run test:ui`，24 项通过；`.test-artifacts/ai-handoff-dom-ordered-clipboard.log` |
| Rust | 167 项通过：platform-core 150、CLI 12、desktop 5；`.test-artifacts/ai-handoff-rust-tests.log` |
| Rust 格式 | 通过；`.test-artifacts/ai-handoff-fmt.log` |
| core Clippy | 通过；`.test-artifacts/ai-handoff-clippy.log` |
| TypeScript/Vite | 通过；`.test-artifacts/ai-handoff-frontend-final.log` |

DOM 覆盖包括保留空间解释与统计引用、原有弹窗和维护草稿边界、新建成功打开详情但不自动复制、复制失败后的文本回退、跨项目迟到响应与剪贴板写入排序、列表移除时保留详情草稿且使交接请求失效。后端覆盖软件说明、当前路径、带引号的 CLI 示例、旧项目后备 skill 及文件不变、符号链接拒绝。

DOM 测试使用真实 React/React DOM 与 jsdom，模拟原生命令和剪贴板，不调用真实项目写操作；Rust 测试在隔离的测试夹具中验证行为，不修改当前开发项目。测试结果不代表新后端已经替换正在运行的实例。

## 数据保留证据

私有调查目录是 `.test-artifacts/storage-audit-2026-10-16T06-11-38-792Z/`，包含 `projects.json`、`workspace.json`、`storage.json`、`trash.json`、`du.json`、`breakdown.json` 和精简 `findings.json`。这些本机证据不提交到仓库。

本轮按用户当前已删除项目的状态建立新基线，没有沿用此前 28 个活跃项目的断言。`preserved-baseline.json` 记录下列范围的递归路径、类型、文件长度和 SHA-256；使用 `lstat`，符号链接仅记录 target、不跟随。

| 范围 | 基线内容 |
| --- | --- |
| 当前 `projects` | 空目录，0 文件 |
| 当前 `.platform/trash` | 1227 文件，24263984 字节 |
| 当前 `backups` | 42 文件，4974449 字节 |
| 当前 `.platform/operations` | 2887 文件、23 符号链接，267565352 字节 |
| 当前 `.platform/project-state` | 88 文件，35529 字节 |
| 保留旧 `workspace` | 10328 文件、78 符号链接，1017882669 字节 |

当前 runtime、其他 control 和扫描 cache 不纳入不变断言。基线建立后立即重扫核对已通过，报告为同目录 `preserved-initial-verification.json`；打包后再次逐文件核对上述六个范围，全部 `unchanged: true`、`changedCount: 0`，报告为 `.test-artifacts/ai-handoff-data-preservation.json`。当前项目仍为空，所有回收项及保留资料未恢复、清理或改写。复用方法如下，默认生成带时间戳的私有结果，不覆盖既有证据：

```sh
node .test-artifacts/storage-audit-2026-10-16T06-11-38-792Z/verify-preserved.mjs verify
```

## 新 app/DMG 最终核验

覆盖打包输出前，上一轮 UI 交付包已保存在 `.test-artifacts/ui-refresh-delivered/444d2f80448da52b0c553e3d9dd1916dc581335ac9d1a15465b71dee1ccf096a/`；54 个 app 条目与原清单核对一致，旧 DMG 的 SHA-256 仍为目录名，证据是 `.test-artifacts/ai-handoff-retained-ui-package.json`。这属于历史包保留核对，不计作本轮新包验收。

`npm run build` 成功生成新包，日志为 `.test-artifacts/ai-handoff-build-dmg.log`。使用既有 `scripts/m5-package-verify.mjs` 对本轮产物做只读核验，结果为 `passed`；报告为 `.test-artifacts/ai-handoff-package-final/verification.json`，完整输出为 `.test-artifacts/ai-handoff-package-verify.log`。核验不启动应用或修改签名。

| 项目 | 本轮结果 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 字节数 | `8080089` |
| DMG SHA-256 | `8aa51f53cbc608b99495bf2248bacf26cd12af057a8ff7291d2b581fc86a5b66` |
| 身份和架构 | `dev.localprojects.platform.m0`，版本 `0.0.0`；桌面与 CLI 均为 `arm64`，最低系统 `15.0` |
| 签名 | 源 app 与 DMG 内 app 的严格深度验证通过，均为 ad-hoc、runtime、无 TeamIdentifier |
| DMG 一致性 | DMG 校验通过，只读挂载 app 的 54 个条目逐项匹配源 app，核验后已卸载 |
| 固定 SDK | 8 处副本均为 `0.1.0`，SHA-256 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9` |
| 媒体配置 | 相机、麦克风用途说明与 camera/audio-input 权利保持，不新增调试放宽权利 |
| 包内说明 | 包内资源 README 与本轮 `docs/development.md` 逐字一致 |

新包内 CLI 只读连接原有服务，确认 PID `56852`、实例和已选择工作区未变，当前仍为 0 个项目、29 个可恢复回收项；没有启动本轮后端、重启服务或刷新业务窗口。证据为 `.test-artifacts/ai-handoff-packaged-cli-readonly.json`。因此增强交接文本的后端验证来自隔离的 Rust 测试，不能把现有服务查询当作新后端已经运行。

## 验证边界

- 保持现有开发服务进程及业务窗口，未启动本轮新 app，未重启服务或重新加载业务页面。新包内 CLI 连接既有服务的只读查询只说明兼容该实例，不能证明新后端代码已经上线。
- 本轮未执行真实 GUI 点击、原生剪贴板、项目浏览器、Safari/Chrome 或 WKWebView 的实测；没有使用 UI 自动化绕过此前工具的 URL 限制。DOM 回归不提供真实布局、窗口尺寸、滚动或系统权限验收。
- 未调用真实项目写操作验证复制入口、删除、恢复、永久清除或迁移；真实生命周期能力的已有证据仍以 M3/M4/M5 记录为准。空间调查读取真实当前状态，不等于重复执行上述写流程。
- 固定开发身份、数据根和 SDK 保持不变。开发包仍使用 ad-hoc 签名，不能据此声称具有 Developer ID、公证、互联网发行或实机浏览器兼容验收。
