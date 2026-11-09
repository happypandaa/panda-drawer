# Panda 应用网页 SDK（API 1）

`panda.json` v2 的应用有必需的网页和可选的后台。有无 `backend` 都使用同一网页 SDK；`/_platform/*` 始终由平台处理，只有声明的业务 API 前缀转发给后台。平台提供普通脚本，不需要 npm 依赖、打包器或内置压缩包：

```html
<script src="/_platform/sdk.js"></script>
<script>
  async function load() {
    const configuration = await window.panda.config();
    const context = await window.panda.context.get();
    console.log(configuration, context.name);
  }
  load().catch(error => console.error(error.code, error.message));
</script>
```

项目根 `panda.json` 声明 `platform: {"sdkApiVersion": 1}` 才能调用 SQL、附件和事件接口。没有启用时明确返回 `SDK_NOT_ENABLED`，不创建数据库。配置读取独立于此开关，也可直接 `fetch('/_platform/config.json')`：返回 `{变量名:值}`，文字／网址／选项为字符串，整数为数字，开关为布尔值，未填写的选填项省略。网页只能读取公开配置；密钥不出现在配置响应、context 或网页中。只有声明 `backend` 才允许 `configuration` 的 `secret: true` 字段，密钥仅注入该后台。保存后重新加载应用生效。

`packages/sdk/sdk.js` 是唯一运行实现，作为 `/_platform/sdk.js` 随平台提供。所有数据请求固定访问 `/_platform/api/v1/`，使用同源 HttpOnly 会话；SDK 不从 URL 提取实例 ID，也不需要页面 meta。首次认证 context 固定本页的 projectId、documentEpoch 和 instanceId，后续身份变化要求重新加载。不同应用拥有不同固定端口；SDK 只使用当前 origin，不选择或修改端口。加上或移除 `backend` 后执行 `app reload --app <id>`，应用 ID、地址、文件夹、公开配置和浏览器数据保持不变，旧页面的会话失效。

## 接口

- `panda.context.get()`：读取应用名称、应用根目录绝对路径 `projectRoot`、实例、API 版本、能力和实际限额。`projectRoot` 只指向当前应用，保留系统原生路径格式（Windows 使用 Windows 路径），可用于生成交给 AI 的工作目录提示。
- `panda.config()`：读取裸类型化公开配置，不先请求 context。
- `panda.db.execute({sql,params,options})`：单条 SQL。
- `panda.db.transaction({statements,options})`：有序语句整体提交／回滚。
- `panda.db.schema()`：真实表、列、索引和外键。
- `panda.db.subscribe(listener)`：数据失效／状态事件，返回取消订阅函数；收到事件后重新查询，不自动重放写入或重载页面。
- `panda.files.put({path,content,overwrite})`、`get(path)`、`list(prefix)`、`remove(path)`、`url(path)`：项目附件，路径为相对逻辑路径。
- `panda.ai.open(text)`：请平台用用户自己选的 AI 工具打开这个应用的文件夹，并预填 `text`（不超过 2000 字）。只能在用户点击时调用；平台只预填、不替用户发送，打不开时会复制文字并提示用户去哪粘贴。有无后台均可使用。
- `panda.PlatformError`：包含 `code`、`details`、`retryable`、可选 `requestId`。

SQL 的 bigint 与 Uint8Array 无损编码，boolean 按 1／0 处理，unsafe integer 直接拒绝。SQL 与附件上传不是一个事务；失败或断线可能表示写入结果未知，先核对，不自动重放。文件数据在项目 `data/main.sqlite`、`data/attachments/`，按需创建；localStorage、IndexedDB、Cookie 是窗口浏览器数据。

SDK、后台和 agent 可读写同一个 `data/main.sqlite`。外部 SQLite 连接设置 5000 ms busy timeout，并保持 WAL 与外键设置；平台锁等待同为 5 秒。使用短事务，网络请求、下载和其他耗时工作放在事务外，避免长时间占用写锁。持锁超过等待上限会使 SDK 写入报 `SQL_BUSY`；先核对结果再决定下一步，不自动重放写入。不要替换正在使用的数据库或删除其 `-wal`、`-shm`；后台或 agent 的外部写入不会自动触发 SDK `data.changed`，页面需自行刷新查询。

## 类型与可选 npm 封装

`panda.d.ts` 是共享的全局类型声明，随平台资源安装；交接中的 `sdkTypesPath` 给出真实路径。默认模板在 `src/panda.d.ts` 放置相同声明，代码使用 `window.panda` 和 `Panda.ProjectContext`。`pnpm package:sdk` 同步模板声明，`node scripts/package-sdk.mjs --check` 检查一致性，不安装依赖或生成 tgz。

可选包 `@local-projects/sdk` 0.2.0 只给出类型及全局对象的 ESM 别名：

```js
// HTML 中先加载 /_platform/sdk.js。
import { platform, PlatformError } from '@local-projects/sdk';
// platform === window.panda，没有第二份客户端。
```

该包不是模板依赖，运行时仍由平台脚本提供。不重新引入 URL 身份解析或作者内置 SDK。自动 SDK 测试使用受控 fetch／EventSource，不能证明真实服务、原生窗口、设备或其他操作系统兼容。
