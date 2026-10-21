# 文档地图与阅读顺序

更新：2026-10-21。本仓库原有总入口是根 [README](../README.md)，`docs/` 没有 `README.md`。本页是新增的分类索引，避免把历史验证记录和现行约束混在同一级阅读。

[产品最终目标](product-goal.md)已由用户确认，涵盖制作自用、获取他人项目、分享与定制四条路径。后续先明确范围与验收，再推导整体计划和批次；此前具体界面、Git 和分批建议仍按候选方案处理。

本轮用户确认的 Radio Bob 风格、亮暗主题、图标启动台及设置窗口，实施与验证见[启动台界面记录](radio-launcher-verification.md)。Node 角标简化覆盖此前卡片常驻中文类型方案，读屏、详情及运行门禁仍保留。

## 权威关系

1. 用户在当前会话中的明确要求，以及仓库 [AGENTS.md](../AGENTS.md) 的项目保护、固定身份、公开迁移和分步提交要求。
2. [平台主约定](platform-development-contract.md)与用户已经确认的配套契约。[应用开发环境与交付要求](app-development-environment.md)规定优先前端与 SQL、Node 单服务、提前说明外部依赖及完整运行文件；最新的[应用窗口与后端约束](app-window-runtime-contract.md)包括统一“我的项目”、类型标识、详情／配置 tabs 弹窗、缺配置进入引导、原生单窗口及业务关窗收尾。[尺寸节](app-window-runtime-contract.md#应用窗口尺寸)集中定义清单可选 `window`、本机普通尺寸记忆与 Node 声明摘要门禁。macOS 管理窗口 X 只隐藏并保留 Dock、业务窗口及受管服务，Dock／Reopen／重复启动恢复同一个管理窗口；菜单退出／Cmd+Q 才安全收尾全部，非 macOS 管理窗口 X 维持退出。该要求覆盖此前“管理窗口 X 退出”的约定，历史实测不能作为新行为已通过的证据。此前浏览器试跑不是标准打开入口。
3. 具体类型的规范与当前已实现子集。JSON Schema 是机器声明检查的一部分，不代表安装器、运行时、系统权限或完整生命周期已交付。
4. 实施计划、待确认问题和讨论稿。计划不是完成状态，候选数值／字段不因写入文档就变成用户已确认要求。
5. 验证记录只证明其注明的版本、样本、动作和环境。历史测试数量、截图或临时浏览器成功不能替代当前原生窗口、真实服务或其他平台验收。

发生冲突时先核对最新用户确认和具体文档的状态；保全项目与恢复现场，不能以一份旧记录推导出删除、迁移或执行权限。已被取代的固定连接器方案继续保留，不作为当前指令。

“我的项目”统一的是展示与进入流程，不把独立 Node 本机源码槽迁移、合并或冒充成 project-v1。详情沿用已有页内 tabs 弹窗，缺必填时入口可点击并先开配置，保存不启动；业务关窗须核对本应用受管 Node 及普通子进程。macOS 管理窗口 X 只隐藏并保留已开的业务窗口和受管服务，Dock／Reopen／重复启动恢复同一个管理窗口；菜单退出／Cmd+Q 才安全收尾全部，非 macOS 管理窗口 X 维持退出，当前以[窗口契约](app-window-runtime-contract.md#2-窗口驱动-node-生命周期)为准。[统一入口历史复验](node-native-configuration.md#2026-10-20-统一项目入口配置弹窗与完整关闭复验)按当时“管理窗口 X 退出”版本保留，不能把旧独立 Node 页、托盘行为或只检查主 PID 的记录当作本轮验收。

同日用户进一步明确本轮 Node Key 使用每槽 `configuration/credentials/` 私有文件（目录0700、文件0600），不是 macOS Keychain／系统加密库。该选择覆盖较早 OS 凭据库建议；后者保留历史，不得用于继续弹系统授权或读取旧条目。本机槽与作者源码、分享包及 Git 不同，具体边界见[当前接入](node-native-configuration.md)。

## 顶层文档分类

本次整理后 `docs/` 顶层实际为 **49 份 Markdown**，其中 **18 份是验证记录**；下表按用途互斥归类，不把所有文件称为并列规范。

| 类别 | 数量 | 文档 |
| --- | ---: | --- |
| 分类索引 | 1 | 本页 |
| 总体入口与状态 | 5 | [已确认产品目标](product-goal.md)、[主约定](platform-development-contract.md)、[里程碑](development-milestones.md)、[技术决定与待定项](remaining-technical-decisions.md)、[开发说明](development.md) |
| 已确认的配套契约及分工 | 12 | [本地服务与 AI](local-service-and-ai-tools.md)、[备份维护](backup-and-maintenance.md)、[生命周期与工作区](project-lifecycle-and-workspace.md)、[能力与秘密](project-capabilities-and-secrets.md)、[元数据与概览](project-metadata-and-overview.md)、[存储与素材](project-storage-and-assets.md)、[发行更新](release-and-updates.md)、[工作区身份](workspace-identity.md)、[历史清理](history-cleanup.md)、[用户／AI 文档分工](application-documentation.md)、[应用开发环境与交付](app-development-environment.md)、[名称与图标](branding.md) |
| 格式、运行与配置规范 | 5 | [项目与应用包 v1](project-and-app-format-v1.md)、[Node 总体方案](node-app-runtime.md)、[Node 启动规范](node-app-startup-spec.md)、[Node 配置设计](node-app-configuration.md)、[原生应用窗口约束](app-window-runtime-contract.md)；其中 Node 公开格式与部分细则仍按各页草案状态处理 |
| 实施计划与当前独立子集 | 4 | [AP 外部包计划](app-package-implementation-plan.md)、[Windows 计划](windows-support-plan.md)、[NR Node 计划](node-app-implementation-plan.md)、[本机 Node 配置与运行](node-native-configuration.md) |
| 待确认讨论稿 | 3 | [资源限额](resource-limits-proposal.md)、[历史管理接口](history-management-interfaces-proposal.md)、[外部包安装分享设计](web-app-package-design.md) |
| 已被取代的路线 | 1 | [早期内置 Node 与固定连接器](bundled-node-and-connectors.md) |
| 实际验证记录 | 18 | 见下一节；只作为证据与限制，不作为新增产品要求 |

另有 **5 份嵌套 Markdown**：配置声明实验规范的 [README](node-configuration-v1/README.md)，静态最小应用的 [README](project-format-v1/examples/minimal-app/README.md)、[用户说明](project-format-v1/examples/minimal-app/docs/USER_GUIDE.md)、[开发说明](project-format-v1/examples/minimal-app/docs/DEVELOPMENT.md)，及[最小项目说明](project-format-v1/examples/minimal-project/README.md)。所以 `docs/` 共 **54 份 Markdown**；再加 **3 份 JSON Schema、3 份 JSON 示例和1份示例 HTML，共61个文件**。该计数不包含平台 SDK、模板、skill、生成资源或 `.test-artifacts/`，新增文档后应重新盘点。

## 验证记录目录

| 范围 | 记录 |
| --- | --- |
| M0–M5 基础交付，6 份 | [M0](m0-verification.md)、[M1](m1-verification.md)、[M2](m2-verification.md)、[M3](m3-verification.md)、[M4](m4-verification.md)、[M5](m5-verification.md) |
| 管理、交接及 starter，4 份 | [管理界面](ui-refresh-verification.md)、[空间与 AI 交接](storage-ai-handoff-verification.md)、[初始应用与 skill](starter-skill-verification.md)、[Radio 启动台与窗口补验](radio-launcher-verification.md) |
| M5 浏览器与设备补验，2 份 | [真实 WKWebView](m5-browser-real-verification.md)、[拒绝／释放／断网](m5-resilience-real-verification.md) |
| 清理与身份，3 份 | [历史清理](history-cleanup-verification.md)、[清理 GUI](history-cleanup-gui-verification.md)、[工作区身份修复](workspace-identity-repair-verification.md) |
| Node 早期试验，2 份 | [隔离运行](node-runtime-trial-verification.md)、[radio 外部服务与浏览器试跑](node-radio-live-verification.md) |
| Windows，1 份 | [W1 环境与门禁](windows-w1-verification.md) |

当前本机 Node 接入与后续原生窗口的实际结果归入[本机配置与运行](node-native-configuration.md#验证记录)，不把早期浏览器记录改写成原生验收。

## 按任务阅读

- **制定产品路线与分批计划**：已确认产品目标 → 当前支持范围及配套契约 → 具体使用路径的验收标准 → 当前能力缺口 → 现有 M/AP/NR/Windows 计划与依赖。目标确认不替代具体方案定稿或实际验收。
- **维护平台**：根 README → 主约定 → 里程碑与待定项 → 当前任务的配套契约 → 相关实现 → 对应验证记录。改窗口先读原生应用窗口约束；改数据／迁移先读相应保护契约。
- **接管已有 project-v1 业务项目**：平台复制的真实交接信息 → 项目自己的 skill、当前安装环境／窗口约束、README、`project.json`、数据库说明、匹配版本的平台说明 → 当前实际 schema 和 CLI 帮助。新选型参考应用开发环境与交付要求；无需读完本仓库全部历史记录，更不能把平台源码目录当业务项目根。
- **适配 Node 应用源码**：应用开发环境与交付要求 → 原生应用窗口约束 → 本机 Node 配置与运行 → 作者的 `local-app.json` 与用户／开发说明 → [配置声明实验规范](node-configuration-v1/README.md) → Node 启动协议 → NR 当前小步及实际验证。先核对原生源码槽与 project-v1 的边界。
- **制作未来可分发应用包**：应用开发环境与交付要求 → 格式 v1 → 用户／AI 文档分工 → AP 计划及仍待确认的安装设计；Node 格式还需 NR 的依赖、数据、凭据和生命周期前置条件。不能把本机源码登记称为完整安装。

## 平台实际交付给 AI 的说明来源

维护源是 [`skills/local-projects-app/SKILL.md`](../skills/local-projects-app/SKILL.md)及其 `references/`。准备资源时复制整个 skill 目录到包内；新 project-v1 创建时再整体复制到项目 `.agents/skills/local-projects-app/`。项目 README、AGENTS、CLAUDE 和平台“复制 AI 接手说明”指向这个真实文件；缺少项目自带 skill 时可指向安装资源中的后备副本。`references/app-window-runtime.md` 因而可以随新项目和后备 skill 被直接读取，不要求接手 AI 拥有平台仓库；本次同路径加入环境与交付摘要，覆盖前端／SQL 优先、运行与构建分工及外部依赖说明。

既有 project-v1 的交接仍优先选择项目自己的 skill，但每次“复制 AI 接手说明”／`project handoff` 都额外要求阅读当前安装的窗口约束，`runtimeConstraintsPath` 返回包内 `references/app-window-runtime.md` 的实际路径，文本同时明确单窗口、开窗后台启动 Node、关窗停止和禁止弹新窗口。这样旧项目无需被覆盖，也能获得当前平台约束补充；新项目 README 默认说明原生窗口运行，HTTP／浏览器入口仅供必要诊断。安装资源缺失或被替换为链接时不伪造可读取路径，交接明确提示修复安装资源。

已有项目的 skill、SDK 和文档保持原样，不为发布新约束覆盖用户资料。当前独立 Node 源码槽不自动生成 project-v1 的 AGENTS、SQLite 文档或交接命令；作者先维护自己的 `documentation.readme`／`documentation.development`，不要虚构已经实现的 Node CLI／导出／迁移接口。
