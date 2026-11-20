# 改造现有项目或开源软件

## 先判断是否适合

- 网页小工具和带一个 Node 后台的本地工具通常适合。纯网页项目可能只需补清单。有后台时要适配 [后台控制协议](node-backend.md)。
- Next.js、Nuxt 等项目如果依赖服务端渲染，需要先改成可静态导出。Electron 或 Tauri 项目可以提取网页部分，评估桌面壳提供的功能能否用网页或平台支持的接口实现。
- 需要多个服务、Python、Java、Docker 或手机原生运行环境的项目，当前通常不推荐。先说明哪些功能依赖这些环境，再判断有无值得做的改造方案。

## 数据存放对照

| 原来的存放方式 | 改造方式 |
| --- | --- |
| localStorage 或 IndexedDB（Dexie、localForage 等）里的用户记录 | 用 SDK SQL 保存到 `data/main.sqlite`。主题等可丢弃的偏好可以继续用浏览器存储。 |
| IndexedDB 中的文件或 base64 编码文件 | 用 `panda.files` 写入 `data/attachments/`。 |
| better-sqlite3、sqlite3 或使用 SQLite 的 Prisma | 用 Node 24 自带的 `node:sqlite` 打开 `APP_DATA_DIR/main.sqlite`。 |
| Postgres、MySQL、MongoDB 或 Redis 中的业务数据 | 评估改写到 SQLite，结论标「改动较大」。 |
| Supabase、Firebase 等云服务 | 先问用户是否要保留账号和同步，再确定要保留的服务。 |
| 成批记录的 JSON 文件 | 迁到 SQLite。小配置文件可以继续放在 `data/`。 |

要让 CLI 能读写平台数据库，在 `panda.json` 里声明 `platform.sdkApiVersion: 1`。应用首次打开时建表并管理表结构版本，将表结构说明写到 `docs/database.md`。

## 改造后逐项核对

- **数据路径**。把 Application Support、`%LOCALAPPDATA%`、`~/.xxx` 等应用目录外的持久数据路径改到项目 `data/`。后台用 `APP_DATA_DIR`，缓存用 `APP_CACHE_DIR`。
- **依赖**。改用能在平台 Node 24 下运行、无需安装脚本或本机编译的依赖。平台从链接安装运行依赖时忽略安装脚本，其他 Node 版本编译出的原生模块也可能无法加载。
- **输出与启动**。stdout 只输出平台协议消息。将 `console.log`、Fastify 或 pino 的普通日志重定向到 stderr。后台收到 `initialize` 后监听 `APP_HOST` 和 `APP_PORT`，成功后发 `ready`。当 `APP_PORT=0` 时，从已监听的服务器读取实际分配的端口（例如 `server.address().port`），把它写入 `ready.port`。按协议处理 `ping`、`shutdown`、Bearer 校验与 `/_platform/health`。
- **相对路径与运行文件**。从代码文件自身的位置定位迁移 SQL 和其他运行资源，避免依赖当前工作目录。构建后从实际执行的 JS 文件解析一次路径，确认它能读到仓库里的迁移 SQL，或将 SQL 复制进相应的构建目录。核对 `web.root` 和 `backend.entry` 真正指向已准备好的文件。分享应用时须提供平台不会生成的后台运行文件及资源，检查 `.gitignore` 有无漏掉它们。
- **退出与启动器**。由平台负责启停，移除独立启动脚本、PID 文件、守护进程及业务接口的自行退出行为。平台提供网页，后台的业务路径通过 `backend.apiPrefixes` 代理。应用用户可以直接从平台打开应用。
- **备份恢复**。平台可能仍持有数据库连接，关闭后台自己的连接不足以安全替换文件。优先在原数据库中恢复：验证备份完整性与表结构版本，可用 `ATTACH` 读取备份，按约束顺序在单个事务中写回业务数据，失败时整体回滚。保留数据库文件和 `-wal`、`-shm`。只有能确认所有应用和平台连接都已关闭时，才评估离线恢复。在可丢弃的数据上实际验证：创建一条记录并备份，保持应用打开，用 `db execute` 修改记录，然后通过应用恢复。确认读到备份中的值，再写入一条记录并重新读取。
- **配置与外部请求**。将 `.env` 依赖改成 `panda.json` 的 `configuration`，密钥只注入后台。网页直接访问外部服务须在 `runtimeRequirements.network.connect` 声明；服务不允许跨域时改由后台转发。

## 原有数据和上游

- 代码方案与旧数据导入分别征得同意。先说明来源、目标及可能覆盖的数据，再在原应用和改造后的应用关闭时复制，保留原位置的数据。复制 SQLite 前确认写入已结束并使用一致的备份，避免漏掉 WAL 中尚未写回的记录。
- 保留 LICENSE、原作者署名和版权声明，在 README 标明改造来源。保留原始提交历史，将原仓库设为 `upstream`，例如克隆后用 `git remote rename origin upstream`。以后合并上游更新时，先向用户说明可能与适配改动冲突的地方。
