# M1 项目、数据和浏览器访问验证

日期：2026-10-15。平台版本仍为 `0.0.0`，开发身份保持 `dev.localprojects.platform.m0`，不会因进入 M1 改目录或收养正式数据。

状态：核心功能已实现，自动测试、真实桌面／浏览器操作、内部打包和重启复验通过。资源限额继续作为用户明确选择的内部测试参数；正式默认值与完整资源验收未定稿。M2 构建、正式 SDK、备份恢复和对外发行均不包含在本次交付中。

## 实际交付

- 管理窗口创建项目：名称、简介、固定 ID、标准清单、AI README 和空 SQLite 数据库；每个项目拥有独立目录。列表显示未构建、缺失、错误或可打开，不虚构空间统计。
- 每项目一个 SQLite 工作线程和有界串行队列；实际表结构、参数化 SQL、整体事务、整数／字节无损传输。WAL、外键和 FULL 同步配置有读回检查；数据库缺失或身份被替换时拒绝操作。
- 原生 CLI 支持项目创建／列表／详情／入口／刷新、schema、SQL 和事务。固定开发身份握手、无代理、无重定向、写入不自动重试；JSON 输入有界，返回业务响应和请求 ID。
- 管理 Bearer 与项目浏览器会话分开；60 秒单次 launch ticket、HttpOnly Cookie、Host／Origin 检查和文档代次。静态文件仅来自产物目录；附件使用受控路径、流式暂存与原子发布。
- 基础 SSE 数据变更通知；事务提交之后才通知。项目网页在独立窗口或 Safari 中直接使用同一个 Rust 服务，无项目 Node 后端。
- 同项目窗口复用；管理窗口关闭隐藏，项目窗口关闭销毁，`⌘⇧H` 返回项目管理。明确退出排空已接纳的数据库操作和创建，取消未发布上传。慢速只读下载可能在 3 秒退出宽限后断开。
- 共享任务状态类型和项目操作协调锁已建立。持久任务执行器、构建任务和任务查询将在 M2 接入，未实现的任务接口不会返回伪造成功。

## 自动验证

命令：

```sh
node scripts/rust.mjs test --locked --workspace
node scripts/rust.mjs clippy --locked --workspace --all-targets -- -D warnings
node scripts/rust.mjs fmt --all -- --check
npm run build --workspace @local-projects/desktop
```

38 项测试通过：核心 31、CLI 5、桌面导航边界 2。覆盖开发身份与目录隔离、两项目持久化、事务及 RETURNING 超限回滚、查询和队列超时、后续连接可用、外键与 schema、64 位整数与 BLOB、受管连接配置、跨库／事务控制拒绝、符号链接／硬链接、数据库缺失／替换、Host／Origin、项目会话／旧代次／过期 ticket、重启失效、附件中断／冲突／范围读取／路径边界、HTML MIME 与响应策略，以及仅提交后发送 SSE。

完整实例的接口验证使用 `scripts/m1-live-test.mjs`，已在实际 `.app` 内运行的 Rust 服务上通过。脚本明确创建两个新测试项目，使用包内 CLI 创建表与记录，再安装 `examples/m1-runtime` 的受控网页产物。它没有调用尚未实现的构建服务，也没有更改普通新项目的空数据库规则。

## 真实界面验证

环境：Apple Silicon、macOS 27.0.1；实际 LaunchServices 启动的开发 `.app`、Safari 27.0.1、Google Chrome 155.0.8059.80。Chrome 浏览器扩展自动化连接不可用，改用原生应用界面完成真实操作；macOS 15 实机仍留待发行阶段。

已确认：

1. 管理窗口创建 `M1 界面创建验证`，随后显示真实项目路径、独立数据库、README，状态为“待生成网页”，打开按钮禁用。
2. 第一个运行测试项目在桌面显示 `alpha`，从桌面保存 `desktop-apple` 与中文释义，并把学习笔记保存为附件。
3. Safari 通过一次性入口进入干净项目 URL，读取上述桌面记录与附件，再保存 `browser-orange`；桌面通过通知显示浏览器新记录。
4. 项目窗口的测试按钮调用 `bootstrap_info` 被拒绝，没有取得管理窗口信息。
5. 最终包重启后，原项目仍显示 `alpha`、`desktop-apple`、`browser-orange` 和附件。填入未提交的 `keep-this-unsaved`，返回管理窗口后重复打开同项目，内容仍在。
6. 第二个桌面项目只显示自己的 `beta` 和资料；关闭后重新打开，未提交的测试表单消失，已保存记录仍在，验证窗口销毁行为。
7. 管理界面的浏览器按钮实际打开 Chrome 中的第二个项目；读取 `beta` 后保存 `chrome-beta` 成功。

首次测试脚本把已建立的 `dist` 目录再交给拒绝覆盖的目录复制操作，正确中止；脚本已修正为逐文件复制且不覆盖目标。该中断样本保留为“未构建状态验证”，没有删除数据或伪报产物成功。

## 开发数据和复现材料

所有项目保存在固定开发根下：

```text
~/Library/Application Support/dev.localprojects.platform.m0/workspace/projects/
```

本次运行验证项目：

| 项目 | ID | 初始记录 |
| --- | --- | --- |
| 运行验证 1 | `prj_559bd07cc85b4b80bf7d08a5a2a39da2` | alpha |
| 运行验证 2 | `prj_fea8d528f580479f8e80f04efbe8b64f` | beta |
| 界面创建验证 | `prj_bbec45f064b746baa2aecc7f3e137ae2` | 无业务表 |
| 未构建状态验证 | `prj_b0dd588c6f5a4d4184b7a3ebfa63b851` | 测试记录，未安装网页 |

仓库忽略的 `artifacts/m1-live-test.json` 保存接口验证结果；`artifacts/m1-restart-test.json` 保存重启复验结果。临时旧会话记录为 0600 权限，不写入项目 README。自动 Rust 测试只使用临时目录；没有清理实际工作区或触碰正式数据目录。

## 范围与剩余条件

- JSON、SQL、上传及队列限额全部属于 **internalTest**。用户在本轮明确要求保留这个状态；M1 不能据此宣称资源默认值已定稿或完整资源验收已结束。
- `VACUUM` 等不能放入托管事务的维护 SQL 暂不开放；正常业务查询、DDL、DML 和多语句托管事务可用。
- 当前网页来自受控测试产物；固定版本 SDK、React 模板、Node 检测与平台构建是 M2 工作。
- 当前没有维护、备份恢复、导入导出、删除和工作区迁移，不用于承载正式用户数据。
- 项目共用本地 origin，按用户信任的自建代码设计，不支持不可信第三方代码隔离。
- 包仍采用开发身份和 ad-hoc 签名；Developer ID、公证、最低系统实机、互联网下载首启尚未完成。

## 最终完成记录

`npm run build` 成功生成 arm64 内部 `.app`（约 11.91 MiB）与 DMG（约 5.31 MiB）。桌面及包内 CLI 均为 arm64，`codesign --verify --deep --strict` 通过；签名仍为 ad-hoc，不代表公证或发行验收。

输出位置：

- `target/release/bundle/macos/Local Projects Dev.app`
- `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg`

本次 DMG SHA-256：`435ce4d236df50cb772559a9581b16cce0ab674a79985d35bc1932fc1a02202b`。

重启脚本通过：实例由 `1c68966c-27cb-458b-9bb4-a7ff0518bb09` 变为 `814b57dc-19c3-4eb4-a8ac-e1f70ae51167`，两个旧项目 Cookie 均返回 401；初始数据库记录和附件完整保留，新 ticket 可建立有效会话。普通重启保持项目文档代次，实例身份仍足以使旧会话失效。

独立代码复核发现 `.HTML`／`.htm` 的 MIME 与原扩展名判断不一致，已改为按实际 HTML MIME 注入代次，并为全部项目静态响应添加 CSP 与 Permissions-Policy；相应回归测试已通过。没有遗留已知阻断 M1 核心使用的代码问题；正式限额定稿及后续里程碑范围仍按上节保留。
