---
name: local-projects-app
description: 在熊猫小抽屉中接手业务项目，核对源码、数据和实际平台能力，按用户授权修改并验证。
---

# 熊猫小抽屉项目开发

先核对平台交接给出的实际项目文件夹、实例身份、CLI 路径和用户目标。平台实例和业务项目分开，不修改平台登记、私有配置、密钥或服务发现文件。

## 当前交付范围

2026-11-01 统一清单批次 3：网页和 Node 均读取根部 panda.json，runtime 分别为 web、node；直接使用登记的项目文件夹。网页使用按实例固定的独立端口，入口在根路径；普通脚本 /_platform/sdk.js 提供 window.panda，SDK API 1 固定访问 /_platform/api/v1/。不要从 URL 解析实例 ID。

平台不再提供源码／数据版本面板、版本取回、项目 ZIP 导入导出或复制。Git 版本由外部 AI 在项目自己的仓库完成。Git 获取、项目构建、SQL／附件和项目删除仍按实际平台能力提供。

## 开始工作

1. 阅读项目 README、声明的用户／开发说明和实际入口。
2. 检查真实 Git 状态，保留已有未提交修改；修改前用 Git 保存当前代码。Git 代码版本不等于业务数据备份。
3. 不读取或输出真实 .env、Key 和平台私有配置。任何会删除或清空业务数据、数据库或上传文件的操作，先说明影响并取得用户明确同意。
4. 只执行用户已授权的工作；只读评估不包括安装、构建、运行或改写。

## Node 项目

panda.json 使用 manifestVersion:1、runtime:node；app.name 必填，版本／简介／图标可选，没有作者 ID 或实例 ID。前后端入口相对各自 root；Node 24 和控制协议 1 保持。Node 禁止 development、platform、runtimeRequirements，运行文件及依赖须事先准备。

配置声明在 configuration 中，用户填写实际值由平台保存并注入。stdout 只输出 initialize/ready、ping/pong、shutdown/stopping 协议；日志写 stderr。应用从平台单个原生窗口打开，关窗要保存并停止服务及所属子进程，stdin 关闭也退出。网页走平台代理，不向前端提供控制 token 或秘密 Key。

文件数据通过 APP_DATA_DIR 保存到项目 data/，缓存和临时文件使用平台注入目录。修改源码前关闭应用窗口，改好重新打开。名称、版本、图标和有效配置声明下次打开生效；改变 runtime 要按平台提示处理，不能改平台登记绕过类型检查。

## 网页项目

panda.json 使用 manifestVersion:1、runtime:web、app.name 和 web.root／entry；入口相对网页根。无需 SDK、构建声明或特定框架。platform.sdkApiVersion=1 启用当前 SQL／附件 API，省略时不调用 SDK 接口。名称、版本、图标等只修改作者清单，平台不提供改名／图标编辑命令。不要向清单添加实例 ID、数据库路径或未知字段。

development 可声明 sourceDir（默认点）及 build.kind:npm、build.script。先关闭应用窗口，再使用平台 project build；它在原目录执行 npm ci 后运行指定脚本，不追加 outDir、不复制工程、不保存旧产物或回滚脚本改动。等待真实终态，确认 web.entry 已生成再重新打开。失败可能已经修改文件，先核对 Git 和实际输出；不宣称旧网页被保留或新旧源码匹配。

阅读项目的数据库文档，通过实际 CLI 帮助确认命令。SQL 使用 db 命令或启用的 SDK，附件使用 SDK files。SQL 和上传不是同一事务，未知结果不要自动重放。修改表结构前核对实际 schema，同步字段、单位、关系和删除规则。不替换正在使用的数据库及 WAL/SHM。

默认新建模板把源码和 package 放项目根，HTML 引入 /_platform/sdk.js；使用 window.panda，类型在交接给出的 sdkTypesPath，模板另有 src/panda.d.ts。不内置 tgz，不要求安装 SDK npm 包，不添加 Vite base。公开配置用 window.panda.config() 或 GET /_platform/config.json，返回类型化普通对象，选填未填项省略；无 SDK 声明也可读取配置。网页配置不允许 secret，保存后刷新或重开生效，无需构建。不调用已删除的 project metadata、project git、导入导出、复制或历史清理命令。回退代码用标准 Git，业务数据另行备份。

## 数据和删除

文件数据在项目 data/；启用 SDK 的网页数据库和附件按需生成。普通网页自己的 localStorage、IndexedDB、Cookie 在平台窗口存储，不能宣称它们在项目文件夹。删除实例一定清除浏览器数据。

用户选定的本地文件夹和在其中生成的代码归用户所有，删除应用保留该文件夹和 data/；平台下载或在平台项目目录新建的项目，删除时连同 Git、代码和文件数据全部删除。配置、密钥、缓存、日志和窗口存储始终按实例清除。用户完成的外部下载、附件另存和原始导入文件不归项目删除。

## 验证和交接

完成后说明改了哪些文件，实际执行的构建和功能检查，以及未验证项。至少验证受影响功能、保存和重开。模拟测试不证明物理设备、原生窗口、系统授权或另一操作系统已经通过。

应用环境和原生窗口的现行限制参见随附的 [运行约束](references/app-window-runtime.md)。工作区位置变更仍走平台实际提供的公开流程，不手动改平台根或清理旧数据。
