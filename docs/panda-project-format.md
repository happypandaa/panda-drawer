# 项目格式：panda.json

更新：2026-11-01，统一清单批次 2。当前实现及边界见[实施记录](unified-project-manifest-implementation.md)，确认的完整方向见[方案](unified-project-manifest-proposal.md)。本文描述当前可用格式。

## 一个项目文件夹，一份清单

项目根必须有 UTF-8 `panda.json`，使用 `manifestVersion: 1`。`runtime` 为 `web` 或 `node`，`app.name` 必填，`app.version`、`app.description`、`app.icon` 可选。图标为项目内 PNG；无效图标使用默认展示，不影响有效项目运行。没有作者应用 ID、本机实例 ID、数据库路径或平台私有目录字段。`package.json` 仅描述依赖及脚本，不能代替项目清单。

平台读取清单并生成自己的实例登记，不把登记、配置、密钥、日志、构建缓存或任务控制文件写入项目。源码、Git、网页和文件数据共用项目这一个文件夹；不保存程序副本或网页产物代次。不接受旧清单或旧登记，不自动迁移。

完整字段及禁止未知字段的校验以共享 [JSON Schema](../schemas/panda.schema.json) 和解析器为准。开发交接提供实际 Schema 和 CLI 路径。

## 网页项目

无需平台能力的普通网页最小声明：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "我的网页" },
  "web": { "root": ".", "entry": "index.html" }
}
```

`web.entry` 相对 `web.root`。可采用普通 HTML、现成网页或构建后的前端；不强制 React、PWA、构建声明或平台 SDK。`platform: {"sdkApiVersion": 1}` 启用当前 SQL／附件 API；省略后不能调用这些接口，也不会因此创建数据库。

需要构建的项目可声明：

```json
{
  "manifestVersion": 1,
  "runtime": "web",
  "app": { "name": "构建网页" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": {
    "sourceDir": ".",
    "build": { "kind": "npm", "script": "build" }
  }
}
```

构建在原项目的 `development.sourceDir`（省略为 `.`）中执行受管 `npm ci`，再执行 `npm run` 指定的 `development.build.script`。包声明和有效 npm v2/v3 锁文件必须匹配。脚本负责生成所声明入口，平台不追加 `--outDir`，不复制工程、不计算新旧匹配状态、不保存旧产物或回滚脚本改动。失败可能留下脚本已修改的文件。源码和数据版本由作者自己的 Git 和备份方式管理。

构建前关闭应用窗口；窗口打开时构建被拒绝。构建结束后重新打开，直接服务原目录文件。缺入口但有构建声明时显示 `needsBuild`；无构建声明则显示 `entryMissing`，均可完成本地登记。添加、只读检查和打开不隐式构建。

批次 2 保留现有 `/p/<实例ID>/` 地址、SDK npm 包和模板内 tgz。模板 `base: './'` 适应该地址；独立地址和 `/_platform/sdk.js` 尚未交付。网页可选 `runtimeRequirements` 继续按当前网络及设备策略处理。

## Node 项目

```json
{
  "manifestVersion": 1,
  "runtime": "node",
  "app": { "name": "我的服务" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": { "root": "backend", "entry": "server.mjs", "protocolVersion": 1, "nodeMajor": 24 }
}
```

前后端入口和运行依赖须提前准备；后端是固定 Node 24 可直接执行的 JavaScript，遵守 initialize／ready、健康检查、ping／pong、shutdown／stopping 协议。Node 不接受 `development`、`platform`、`runtimeRequirements`。本地添加不安装依赖，启动不构建。详情见 [Node 单目录运行](node-single-directory.md)。

## 配置、文档及数据

`configuration` 使用 `version: 1` 与 `fields`，声明变量用途及获取方法；`howToGet` 是非空字符串数组。真实值由平台表单保存；Node 秘密只注入目标后端，不写清单或网页。`documentation.readme`、`documentation.development` 可选，声明后文件必须存在。可选 `window` 控制初始窗口尺寸及记忆，详见 Schema。

文件数据在项目 `data/`：启用 SDK 的网页按需创建 `data/main.sqlite` 及 `data/attachments`，Node 通过 `APP_DATA_DIR` 写入。检查、登记和无数据操作的普通网页不会提前生成数据库。localStorage、IndexedDB、Cookie 是平台窗口的浏览器数据，与项目文件分开。

用户选择的本地目录，包括在空目录中生成的起始项目，归用户所有；删除应用保留代码、Git 和 `data/`。平台新建或 Git 下载的目录归平台所有，删除时整根删除。两类实例的私有配置、密钥、缓存、日志、任务和浏览器数据均删除。文件数据和浏览器数据在删除提示中分别说明。

## 路径和接入

路径必须在项目内。网页根按解析符号链接后的真实位置检查，不能等于或位于 `data/`、`node_modules/`、隐藏目录中；不能提供 `panda.json`、隐藏文件或共享硬链接。静态请求逐级核对边界和真实文件身份。清单通过不证明网页所有脚本或业务安全。

[本地目录添加](local-directory-onboarding.md)只读检查后登记原目录；[Git 添加](git-app-import.md)准备平台独占源码，网页入口缺失且有构建声明时先获得明确构建确认。平台不提供项目 ZIP 导入导出、复制、源码／数据版本及取回。
