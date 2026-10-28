# 停止并删除：第一阶段

2026-10-28 用户确认第一阶段。第二阶段的协调器、锁范围、持久删除任务和自动续删尚未实施，开始前须另交设计说明并获得确认；第三阶段的最终删除范围按实际目录归属确认。本记录不授权对真实项目做删除验收。

## 已确认的第一阶段边界

- 新运行标记记录进程 PID、PGID、启动时间、可执行文件及每次运行独立的身份随机值；身份随机值与应用控制凭据分开。
- 标记采用启动意图与完整运行身份两阶段落盘。只有启动意图、没有完整身份且属于本次开机的记录，按无法确认处理。
- 标记早于系统本次开机时间时，其对应进程已无法继续存在，可按精确标记身份清除。此规则适用于旧格式。其他情况不能仅凭主进程或旧宿主 PID 消失便当作整个组已停止。
- 核对和信号发送绑定完整进程身份，不使用标记里的裸 PID 猜测。当前应用进程组内没有存活进程才表示进程已停止；强制停止、非零退出和协议错误只保留诊断。平台自身的代理、文件及输出工作仍须收尾，不新增系统级端口或文件句柄扫描。
- 无法确认的旧记录提示重启电脑后重新核对。不会把不能确认的记录静默当作可运行，也不替用户杀掉无关进程。
- 删除失败反馈沿用弹窗固定底部区域与焦点，不恢复成滚动区顶部的不可见错误。

## Git 导入实际产生的目录与归属

下文 `D` 是固定开发数据根 `~/Library/Application Support/dev.happypandaa.pandadrawer/`，`W` 是当前工作区。初始 `W = D/workspace/`；公开工作区迁移后，`W` 可在另一个位置或磁盘。`node-apps` 始终位于 `D`，不能假设所有下列路径处于同一卷。

一次成功的 Git 导入包含彼此独立的下载来源、安装实例和可编辑源码，不能把它们看成同一个项目目录：

```text
W/
├── .platform/
│   ├── sources/src_<来源ID>/
│   │   ├── source.json
│   │   ├── repository/                 # 获取并核对的来源候选
│   │   ├── .acquiring/                 # 获取中：repository/home/empty-template
│   │   └── install-tasks/              # 存在时：来源与安装任务的绑定
│   └── source-publications/<实例UUID>/
│       ├── publication.json            # 来源、实例、目标目录及复制状态绑定
│       └── payload_<ID>/               # 发布前暂存；成功后移走，失败现场可能保留
└── project-sources/<仓库名派生的唯一目录名>/
    ├── .git/
    ├── local-app.json
    └── ...                             # 已发布的独立可编辑源码

D/node-apps/
├── .installations/
│   ├── commit.lock                     # 安装提交共享锁
│   └── install_<候选ID>/
│       ├── install.json                # 指向来源、目标实例、程序代次的安装记录
│       └── program/                    # 准备中程序；提交后移入 generations
└── <实例UUID>/
    ├── registration.json               # 当前程序代次、来源摘要等登记
    ├── development-source.json         # 独立可编辑源码的绑定
    ├── generations/gen_<代次ID>/       # 本实例实际运行的受管程序与依赖
    ├── configuration/                  # 本实例配置；credentials/ 是本机秘密文件
    ├── data/                           # 本实例业务数据
    ├── cache/                          # 本实例缓存
    ├── runs/                           # 各次运行目录与临时目录
    └── active.json                     # 存在时：启动／运行身份标记
```

目录是否仍存在取决于阶段与显式收尾结果。`source_import_publish` 先核对安装实例与来源摘要，将来源复制为独立可编辑源码，再绑定到实例；安装程序不会直接使用该源码目录运行。`source_import_cleanup` 在核对引用和已结束任务后，可清理本次下载及安装临时候选，因此成功导入后不保证 `.platform/sources/src_*` 或 `.installations/install_*` 仍有完整载荷。发布记录可能保留此前中断复制的 `retainedPayloads`。

| 对象 | 归属与用途 | 后续删除范围判断 |
| --- | --- | --- |
| `W/.platform/sources/src_*` | 一次获取／捕获的来源及任务绑定；同一来源可用于安装候选 | 不能因为删一个运行实例便删整个来源树，须核对当前引用 |
| `D/node-apps/.installations` | 多实例共享的安装操作根 | 不能整根删除；单条候选／引用要按其实例与来源绑定判断 |
| `D/node-apps/<实例UUID>` | 单个运行实例的程序代次、配置、数据与运行材料 | 用户已确认第三阶段拟删除完整实例私有槽；第一阶段不扩大实际删除目标 |
| `W/project-sources/<唯一目录名>` | Git 导入后独立发布的可编辑源码，带自己的 `.git` | 需同时证明该项目归属和无其他实例引用；证据不足则保留，并在结果列明目录与原因 |
| 用户自己选择的外部源码目录 | 用户原有目录；外部引用实例运行依赖它 | 始终保留整个目录及其中文件，不改名、不递归删除 |
| 来源发布记录／中断现场 | 绑定复制代次、目录身份和来源摘要 | 不能凭目录相邻、仓库同名或 Git URL 相同推导归属；后续按明确引用与恢复状态处理 |

归属依据是登记中的实例、来源 ID、摘要、程序代次和源码绑定，路径相邻或“Git 导入”标签本身不构成删除授权。托管实例对外展示的 `sourcePath` 由登记中的当前程序代次派生，指向已安装程序位置；独立可编辑源码由 `development-source.json`／受管登记中的 `developmentSource.sourceRoot` 关联，二者必须分别核对。

关键实现入口：`source_acquisition.rs::root/view`、`source_publication.rs::publish_author_source`、`service_source_publication.rs::source_import_publish`、`node_app_install.rs::prepare_inner/commit_inner/managed_program_path`、`node_app_development.rs` 和 `source_import_cleanup.rs`。第一阶段不会删除上述真实目录，也不以本报告提前实施第三阶段范围。

## 第一阶段验证记录

### 实现结果

`active.json` 升为 v2：先以 `launching` 写入启动意图，保留原文件句柄，再在同一 inode 上写入 `running` 和完整进程身份。完整身份发布失败不能报告启动成功，须停止本次运行或保留清理句柄。运行身份随机值通过 `PANDA_DRAWER_RUN_IDENTITY` 注入，独立于控制 bearer；声明配置不能覆盖平台保留的 `PANDA_DRAWER_` 前缀。

macOS 从内核取得开机时间、进程启动时间、实际可执行路径、PGID 及运行身份随机值，连续核对组成员。信号前再次核对完整身份和已确认的组成员锚点。SIGTERM 默认等 5 秒；仍未停止则重新核对后发 SIGKILL，再等 2 秒。主进程消失但组内仍有活进程，不能作为已停止回执；身份或枚举不足时不发送信号。Unix 身份读取与 `kill` 不属于同一个原子系统调用，本实现不声称消除了两者之间的极窄竞态。

平台发布管理命令前核对所有实例标记。开机前普通标记或完整身份所指进程组确已清空时，仅收尾精确捕获、复核 inode 和字节的标记；不会因旧宿主 PID 不存在而清除本次开机的旧格式记录。早于开机的 `.start-intent-cleanup-*` 证据保留在磁盘，但不再阻止当前运行。本次开机的未知记录显示“运行待核对”及重启说明；完整身份匹配的遗留运行提供“停止遗留运行”，也允许继续到原有删除流程。

当前宿主持有的运行在进程组、输出 reader 和平台代理／文件工作均已关闭后可收尾标记。协议错误、非零退出和强制停止仅作为诊断，不再冒充资源仍存活。标记收尾失败会显示具体错误并保持启动门禁，即使公开标记已被外部移走也不会显示为可启动。失败启动的清理先登记 owner 再等待，等待者取消后仍保留重试归属；创建 runtime／代理时尚未返回 owner 的既有取消窗口仍按磁盘未知标记保守处理，本阶段未用协调器重写整条启动链。

删除弹窗仍使用已有固定底部错误区及错误焦点；未知运行的预览直接返回阻挡原因。第二阶段的全局锁等待、持久删除任务、阶段状态和崩溃后续删，以及第三阶段最终文案／扩大删除目录尚未实施。没有新增任意数据根或身份迁移入口。

### Radio Bob 的实际分支

只读核对固定开发数据根下实例 `fa8c857d-e3c1-484a-b558-9bd196558f69/active.json`：

| 证据 | 本机实际值（北京时间） |
| --- | --- |
| 本次系统开机时间 | 2026-10-26 10:22:36.561126 |
| 标记创建时间 | 2026-10-27 17:45:29.384853 |
| 格式 | 旧 v1，记录旧平台宿主 PID 37431 和运行目录，没有应用完整身份／PGID |
| 结论 | 创建晚于开机，属于“本次开机、旧格式、无法确认的遗留运行” |

因此这次 **不能** 用开机时间规则立即清除 Radio Bob 的标记，也不能据旧宿主 PID 消失发送信号或认定整个应用已停止。按已确认规则，用户重启电脑后，新版本启动核对时才可将它作为开机前标记收尾。本轮没有改写、移动或删除该真实标记，也没有对 Radio Bob 提交删除。

### 验证范围

- 完整管理界面自动测试 515/515 通过；TypeScript／Vite 构建通过，既有 JavaScript chunk 超过 500 kB 的构建提示仍存在。
- Rust 工作区回归 931 项通过（含 2 项文档编译检查），54 项专用环境条件测试按默认规则忽略；其中下列 5 项真实 Node 条件测试另行显式执行通过。工作区全目标检查、严格 Clippy（`-D warnings`）、格式和差异空白检查通过。
- 真实固定版本 Node 的两项显式运行时测试通过：完整身份与实际可执行文件绑定，以及受管子孙清理且独立 Node 存活。
- 三项显式 Node 遗留集成测试通过：新管理器可识别并停止 v2 遗留运行；组已清空才收尾 v2 标记；标记指向身份不符的活 PID 时拒绝停止且该进程存活。测试保留独立救援 Child，模拟管理器失去内存句柄，不冒称真实平台崩溃或内核实际复用 PID。
- 标记单元测试覆盖模拟跨开机旧标记、同次开机旧格式／不完整记录、第二阶段落盘保持 inode／创建时间、替换对象保护、清理证据保留和不安全文件类型。跨开机使用注入开机时间验证分支，未真实重启电脑。
- 真实临时进程／协议夹具覆盖忽略 SIGTERM 的强制停止、主进程先退出但子进程仍存活、异常退出后资源已清理、停止期间标记移走／替换、停止并删除时另一个实例仍运行。删除测试只操作临时实例，并核对外部源码哨兵和其他实例登记／数据未变。

主要命令：

```sh
node scripts/rust.mjs test --locked --workspace -- --test-threads=1
node scripts/rust.mjs check --locked --workspace --all-targets
node scripts/rust.mjs clippy --locked --workspace --all-targets -- -D warnings
node scripts/rust.mjs fmt --all -- --check
npm run test:ui
npm run build --workspace @local-projects/desktop
```

真实 Node 条件测试使用经固定版本摘要核对的包内 `apps/desktop/src-tauri/resources/node-runtime/bin/node`，通过既有 `NODE_RUNTIME_TRIAL_TEST_NODE` 测试变量显式指定，分别运行 `node_runtime_trial::tests::pinned_node` 和 `orphan_tests`，均加 `--ignored --test-threads=1`。输出证据保留在 Git 忽略的 `.test-artifacts/stop-delete-phase1-*.log`，不提交本地运行证据。

未进行真实用户项目删除、真实原生窗口的最终删除确认、物理重启、删除中杀平台再续删、第二阶段操作准入或跨系统验收。本阶段没有重新生成安装包。其余专用环境条件测试未执行。
