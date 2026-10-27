# 构建任务 npm 日志目录（历史验证）

2026-10-27：本页日志保留方案已按[操作记录精简](operation-record-simplification.md)退役。新任务不再保存平台头尾输出或 npm debug 日志，日志查询接口同时移除；构建、安装及发布状态继续保留。以下内容仅是当时的修复与验证证据。

npm 的日志轮换会清理其日志目录里的旧文件。project-v1 构建原先共享 `.platform/cache/npm`，如果同时使用默认 `_logs`，一次新构建就可能删除之前任务的 npm 日志。

现在依赖安装和构建都显式传入 `--logs-dir <本任务目录>/npm-logs`。目录由平台以 0700 创建，CLI 参数优先于继承的环境和 `.npmrc` 配置。共享依赖缓存继续复用；已有 `_logs`、其他任务及恢复材料不清理。此修复没有使用 `logs-max=0`。

独立源码副本的实际验证使用本地配套 Node/npm、人工 TempDir 项目、离线 `npm ci` 与 `npm run build`，没有访问真实项目或下载依赖。测试显式使用人工 user/global npm 配置，执行进程从空环境按白名单建立：

- 项目和 user `.npmrc` 故意指向共享目录中的 20 条人工旧日志；两次真实命令后，20 条内容、inode、mtime 全部保持，新日志只出现在本任务目录。
- 继承的 `npm_config_logs_dir` 另指向 20 条人工旧日志；该目录同样保持 20 条，内容和元数据未变。
- 既有构建专项串行回归 30 通过、1 项原有 opt-in 忽略；核心 all-targets 严格 Clippy 与格式检查通过。

真实 npm 回归为 `builds::npm_logs_tests::real_npm_preserves_twenty_shared_logs_and_writes_only_task_logs`，默认忽略，运行时必须显式提供 `PANDA_DRAWER_TEST_NODE`、`PANDA_DRAWER_TEST_NPM_CLI` 的本地工具绝对路径。普通构建专项通过不代替这条真实 npm 证据。原生新包仍需在合并、重新打包后验收。

集成到开发分支后，再次用明确指定的本地配套 Node 24.22.0／npm 11.19.1 运行该真实离线回归，1 项通过；core 全目标严格 Clippy、格式和差异空白检查通过。日志保留在本地忽略证据目录。本修复防止后续默认轮换，不将 E22 已观察到的两份旧缓存日志移除改写为“未曾删除”。
