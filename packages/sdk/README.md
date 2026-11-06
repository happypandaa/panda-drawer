# Panda 网页 SDK（API 1）

平台提供普通脚本，不需要 npm 依赖、打包器或内置压缩包：

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

项目根 `panda.json` 声明 `platform: {"sdkApiVersion": 1}` 才能调用 SQL、附件和事件接口。没有启用时明确返回 `SDK_NOT_ENABLED`，不创建数据库。配置读取独立于此开关，也可直接 `fetch('/_platform/config.json')`：返回 `{变量名:值}`，文字／网址／选项为字符串，整数为数字，开关为布尔值，未填写的选填项省略。网页配置不允许秘密值，保存后刷新或重新打开生效。

`packages/sdk/sdk.js` 是唯一运行实现，作为 `/_platform/sdk.js` 随平台提供。所有数据请求固定访问 `/_platform/api/v1/`，使用同源 HttpOnly 会话；SDK 不从 URL 提取实例 ID，也不需要页面 meta。首次认证 context 固定本页的 projectId、documentEpoch 和 instanceId，后续身份变化要求重新打开。不同应用拥有不同固定端口；SDK 只使用当前 origin，不选择或修改端口。

## 接口

- `panda.context.get()`：读取应用名称、应用根目录绝对路径 `projectRoot`、实例、API 版本、能力和实际限额。`projectRoot` 只指向当前应用，保留系统原生路径格式（Windows 使用 Windows 路径），可用于生成交给 AI 的工作目录提示。
- `panda.config()`：读取裸类型化公开配置，不先请求 context。
- `panda.db.execute({sql,params,options})`：单条 SQL。
- `panda.db.transaction({statements,options})`：有序语句整体提交／回滚。
- `panda.db.schema()`：真实表、列、索引和外键。
- `panda.db.subscribe(listener)`：数据失效／状态事件，返回取消订阅函数；收到事件后重新查询，不自动重放写入或重载页面。
- `panda.files.put({path,content,overwrite})`、`get(path)`、`list(prefix)`、`remove(path)`、`url(path)`：项目附件，路径为相对逻辑路径。
- `panda.PlatformError`：包含 `code`、`details`、`retryable`、可选 `requestId`。

SQL 的 bigint 与 Uint8Array 无损编码，boolean 按 1／0 处理，unsafe integer 直接拒绝。SQL 与附件上传不是一个事务；失败或断线可能表示写入结果未知，先核对，不自动重放。文件数据在项目 `data/main.sqlite`、`data/attachments/`，按需创建；localStorage、IndexedDB、Cookie 是窗口浏览器数据。

## 类型与可选 npm 封装

`panda.d.ts` 是共享的全局类型声明，随平台资源安装；交接中的 `sdkTypesPath` 给出真实路径。默认模板在 `src/panda.d.ts` 放置相同声明，代码使用 `window.panda` 和 `Panda.ProjectContext`。`pnpm package:sdk` 同步模板声明，`node scripts/package-sdk.mjs --check` 检查一致性，不安装依赖或生成 tgz。

可选包 `@local-projects/sdk` 0.2.0 只给出类型及全局对象的 ESM 别名：

```js
// HTML 中先加载 /_platform/sdk.js。
import { platform, PlatformError } from '@local-projects/sdk';
// platform === window.panda，没有第二份客户端。
```

该包不是模板依赖，运行时仍由平台脚本提供。不重新引入 URL 身份解析或作者内置 SDK。自动 SDK 测试使用受控 fetch／EventSource，不能证明真实服务、原生窗口、设备或其他操作系统兼容。
