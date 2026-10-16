# 初始应用与 AI 接管 skill 补充验收

日期：2026-10-15。范围：M2 增补，不替代此前的 M0/M1/M2 历史记录。所有实际项目位于固定开发身份 `dev.localprojects.platform.m0` 的工作区；未定义或访问正式环境数据目录。

## 交付行为

新建项目同时得到 React 源码、预编译欢迎页、固定 SDK 0.1.0、空 SQLite 数据库和接管文档。模板版本为 `react-ts-vite-0.1.1`。初始页面由平台打包时构建，新建和正常运行不执行 Node/npm，也不下载依赖。

来源为 `buildInfo.origin: bundledTemplate`，带模板版本及真实源码／产物摘要，初始 `sourceState: matched`；没有伪造的成功构建任务。用户修改源码后通过正常平台构建发布，成功后来源为 `projectBuild`。失败保留旧网页；单独提交的数据库修改不随构建失败回滚。

项目 `.agents/skills/local-projects-app/SKILL.md` 保存接管流程，README、AGENTS.md 和 CLAUDE.md 指向它。“复制给 AI”和 `project handoff` 给出项目、skill 和当前 CLI 的绝对路径。旧项目不自动改写，缺少 skill 时交接只读引用安装包中的后备文件。

## 自动验证

- 工作区 Rust 测试：60 项通过（core 50、CLI 8、desktop 2）；Clippy 全工作区、全部 targets、`-D warnings` 通过，Rust 格式检查通过。
- 桌面 TypeScript 检查与生产构建通过。平台打包流程真实执行模板 `npm ci` 和构建，并验证构建前后源码未变；安装时再次核对源码／产物摘要。
- 核心测试覆盖：无可用 Node 仍可创建初始应用、没有业务表、来源真实、首次构建失败保留初始页、成功后旧 `starter_` 资源仍可读取、数据库保留、篡改模板时原子创建失败且不留半项目；已登记的产物目录意外丢失会显示文件异常，不冒充尚未构建。
- skill 通过 skill-creator 官方 `quick_validate`；包内文件与仓库 skill 相同。生成 README、AGENTS.md、CLAUDE.md 的入口和旧项目后备引用均有检查。
- `npm run test:m2:live` 通过：真实模板构建、来源检测、更新事件、旧资源、失败及输入改变拒绝发布、无 Node 下已发布网页／数据库／附件继续使用；新增检查初始来源与 skill 文件。
- `npm run test:m1:live` 通过：两个本轮新建的可丢弃项目，验证会话、协议、数据库／文件隔离。当前运行时通过正式构建流程发布测试页面，没有直接覆盖 starter 或已有项目。

## 安装包与界面验证

实际运行 `.app`，以受限 PATH 启动；随后使用可恢复的临时 Node 路径令 `env doctor` 返回不可用，没有移除系统 Node。

项目 `prj_7b491d00976f4183bab0d5d062d2588e` 在该受控状态创建成功，立即为 `ready + matched + bundledTemplate`。无 `app/node_modules`，数据库零业务表，带接管 skill 和三个说明入口。通过真实会话访问入口及 `starter_` 脚本资源均为 200。测试后恢复 Node 选择。

桌面独立项目窗口和普通 Chrome 均实际显示欢迎页、“本地已连接”和“暂无业务数据表”；平台“复制给 AI”实际展示项目内 skill 的绝对路径。另从管理窗口创建 `prj_2869c62dd6474f7fb00533fca62a3533`（空白起点 · 界面验证），创建成功即显示“初始应用 · 可直接打开”，随后点击按钮成功打开欢迎页。

升级前记录的 7 个已有项目共 92 个持久文件，在升级、新建及回归测试后摘要全部一致，包括已有源码、说明、SDK、数据库与附件。旧项目 `prj_ac10c84d9d63493f986f2636316b13af` 的交接能读取安装包后备 skill，没有新增项目 skill 或覆盖 README。

## 独立 AI 接管验证

使用未继承平台开发对话的新 AI 上下文，只提供上述可丢弃业务项目路径和功能需求，要求先读项目 skill 与文档，不读取平台源码。需求为在欢迎页增加单条便签编辑和 SQLite 保存。AI 读取并核对实际 schema，新增单行表 `personal_note`、编辑及保存界面，更新数据库说明与 README，保持原欢迎页和黑白组件；未添加依赖。构建任务 `task_d54b22ab3dc841a3b6effa5e6426e3a8` 为 `succeeded`，源码状态 `matched`，CLI 写入和重新读取测试便签成功。

主代理在桌面看到新构建提示并明确重载。在独立浏览器会话中保存便签，重载仍可读取；桌面同步显示该文本。随后从桌面保存“桌面保存：初始应用和 AI 接管验证通过。”，浏览器同步显示同一内容。初始欢迎页另已通过普通 Chrome 验证；便签交互使用内置浏览器工具与真实桌面窗口验证，没有把 CLI 读回冒充页面交互。

新上下文反馈 skill 足够接管、没有阻塞。它发现固定 SDK 0.1.0 指南内的旧模板示例版本容易与当前模板 0.1.1 混淆，已在新项目生成说明中分别标注创建模板版本和固定 SDK 指南，并验证生成结果；没有改写同版本 SDK tarball。

最终开发包已完整退出并重启：桌面重新打开便签项目，仍显示最后保存的内容；CLI 核对便签与初始应用来源均保留。`npm run test:m1:restart` 通过，旧会话失效、数据库和附件保留、项目隔离与新会话有效。升级前的 92 个持久文件再次核对一致。

最终产物：`target/release/bundle/macos/Local Projects Dev.app`（14.02 MiB）与 `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg`（6.14 MiB）。`codesign --verify --deep --strict` 通过，包内开发说明与 skill 和当前仓库一致。DMG SHA-256：`0346b8a8d713aa074abc8d2e88b59249bd114825918c0b14f4928a020022dfb1`。

## 边界

仍为内部开发包，采用 ad-hoc 签名，尚非 Developer ID 签名或公证的正式发布。没有把 Node、AI 客户端或业务后端装进平台。没有全局安装 skill，也没有宣称已在所有外部 AI 客户端测试自动发现。备份、维护、迁移与完整空间管理仍在后续里程碑。

本轮详细本机证据保存在忽略目录 `.test-artifacts/starter-project.json`、`starter-before.json`、`starter-preserved.json`、`starter-final.json`、`m2-live.json`，M1 协议报告为 `artifacts/m1-live-test.json`。报告不保存入口票据或控制令牌。
