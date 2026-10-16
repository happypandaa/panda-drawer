# @local-projects/sdk 0.1.0

浏览器 SDK，接口主版本为 **API v1**。ES module 和 TypeScript 类型随项目打包，没有运行时依赖，也不连接 Node、Tauri IPC 或管理凭据。标准模板版本为 `react-ts-vite-0.1.0`。这是内部开发版本，产品正式名称尚未确定。

项目依赖使用 `"@local-projects/sdk": "file:vendor/local-projects-sdk-0.1.0.tgz"` 并由 `package-lock.json` 锁定。`vendor` 包是项目源码材料；保留它用于备份、迁移和再次构建。平台升级不会自动替换现有项目的包或锁文件。不要用新内容覆盖已分发的同版本包；明确升级时更新包版本、锁文件和项目说明。

## 项目连接

```ts
import { platform, PlatformError } from '@local-projects/sdk'

const context = await platform.context.get()
console.log(context.projectId, context.name, context.capabilities)
```

从平台独立项目窗口打开，或通过 CLI `project url --project <ID> --json` 取得一次性浏览器入口。不要写死端口或长期保存入口 URL。直接双击 HTML、直接打开 Vite 开发地址没有项目会话，SDK 会明确报错。浏览器和桌面访问同一项目数据库；Node 只在构建时使用。

服务注入 `platform-document-epoch`，SDK 创建时读取一次并固定持有。所有请求都携带该代次；上下文响应不能改变它。`PROJECT_RELOAD_REQUIRED` 表示用户需要明确重新打开项目，不能重放原请求或静默刷新表单。SDK 会检测 API 主版本与稳定能力字符串 `db.execute`、`db.transaction`、`db.schema`、`files`、`events`；缺少能力报 `CAPABILITY_UNSUPPORTED`。

## SQL 和数据类型

```ts
const result = await platform.db.execute({
  sql: 'SELECT id, word FROM words WHERE word LIKE ? ORDER BY word',
  params: ['a%'],
})
await platform.db.transaction({ statements: [
  { sql: 'INSERT INTO words(id, word) VALUES (?, ?)', params: ['word-1', 'apple'] },
  { sql: 'INSERT INTO practice(id, word_id) VALUES (?, ?)', params: ['practice-1', 'word-1'] },
] })
const schema = await platform.db.schema()
```

这只是接口用法，模板不创建上述业务表。先读取项目 `docs/database.md` 和真实 schema，再执行明确的数据操作。用户输入用参数绑定；一次 execute 只提交一条语句。多语句事务统一提交或回滚，不支持 JavaScript 回调或跨请求 BEGIN/COMMIT。表结构变更不放在构建脚本中，构建不初始化或清空数据库。

SQL 参数支持 `null`、字符串、有限 number、bigint、布尔值和 Uint8Array。布尔值写为 0/1，读出仍是 number。整数 number 必须安全；大整数用 bigint，范围为 SQLite 有符号 64 位。结果含 `columns`、对象 `rows`、`changes`、适用时的 `lastInsertRowid`；大整数返回 bigint，BLOB 返回 Uint8Array。重复列名应使用别名。

传输标记 `$int64` / `$bytes` 只在 SQL 值位置编解码；不会递归改写上下文、错误详情或其他业务对象。不要对包含 bigint 的结果直接 JSON.stringify。SQL 可选 `options: { maxRows, timeoutMs }` 只能在服务允许范围内选择；从 `context.get().limits` 查询当前数值，`limitsStatus: internalTest` 表示内部测试参数，SDK 不冻结或承诺这些数值。

## 附件

```ts
await platform.files.put({ path: 'notes/example.txt', content: new Blob(['hello'], { type: 'text/plain' }) })
const blob = await platform.files.get('notes/example.txt')
const files = await platform.files.list('notes/')
const url = await platform.files.url('notes/example.txt')
await platform.files.remove('notes/example.txt')
```

逻辑路径相对 `data/attachments/`；保存路径，不在数据库保存临时 URL。默认不覆盖，确需覆盖传 `overwrite: true`。上传使用服务原子暂存与发布；SDK 不自动重试写入。读文件返回 Blob；`files.url` 是异步方法，返回带当前文档代次的会话 URL，适用于图片、音频、下载，不应跨会话保存。上传大小按当前 context 限制检查；SQL 与附件不是同一事务，业务代码需处理部分成功。

## 通知、重载与错误

```ts
const unsubscribe = platform.db.subscribe(event => {
  if (event.type === 'data.changed') void reloadFromDatabase()
  if (event.type === 'project.updated') showExplicitReloadButton()
  if (event.type === 'error') showError(event.error)
})
// React effect 清理或页面释放：
unsubscribe()
```

`data.changed` 是项目级失效通知，不是行级同步。SSE 重连、重新获得焦点、恢复可见时，SDK 先核对会话和文档状态，再通知页面重新查询。通知不持久化，不保证精确一次。直接外部 SQLite 写入后，用 CLI `project refresh` 通知其他页面。

`project.updated` 表示新构建已发布，网页应提示用户明确重载，不能自动丢弃表单。`error` 事件和方法失败均使用 `PlatformError`，含 `code`、`message`、`details`、`retryable` 和可选 `requestId`。维护或会话过期时停止自动 SSE 重连。网络中断后的写入可能已经提交，`details.operationOutcome` 可为 `unknown`；先重新查询再决定操作，不自动重复 SQL、上传或删除。`retryable` 仅说明问题可能恢复，不是允许重放写入的指令。

## 开发包验证

SDK 维护者执行 `npm test --prefix packages/sdk`。安装包内包含版本固定的 tarball；模板使用它构建出浏览器资源。项目使用者不需要运行 SDK 自身测试或安装平台工程依赖。
