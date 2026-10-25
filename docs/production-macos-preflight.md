# Mac 正式发行：固定元数据预检与隔离计划

本步骤完成可执行的配置预检入口和计划文件生成，不构建正式应用。固定首版为 `0.1.0`、`com.happypandaa.pandadrawer`、macOS 15+ arm64，公开发行仓库为 `happypandaa/panda-drawer-releases`。开发构建入口、固定开发身份、开发资源和数据根不变。

## 使用

```sh
npm run release:mac:preflight
npm run release:mac:preflight -- prepare
```

默认 `check` 只读固定源码元数据，标准输出返回 JSON，不创建 `target` 或其他输出。`prepare` 在脚本所属工程的 `target/production-macos/preflight-<UUID>/` 独占创建本轮计划，输出相同 JSON，并写入 `preflight.json` 和 `tauri.production-plan.json`。从其他当前目录或工程原有路径别名启动也仍使用脚本所属工程；没有选择其他 profile、身份、版本、工程根、数据根、配置文件或输出目录的命令参数。

配置有效且预检完成时返回成功；这不表示可发行。当前真实策略仍缺 Team ID、Node 权限摘要和 Git 权限摘要，报告逐项列出，并始终包含 `productionBuildAvailable: false`、`runtimeAdmission: false`、`releaseReady: false`。显式配置三个公开策略字段也不消除实际签名、构建、公证和安装交接待办。`check` 的输出路径是尚未创建的本轮规划路径。

## 固定输入和生成内容

入口只读取固定的生产 profile、Mac 签名策略、开发 Tauri 配置和三份 Node/Git 来源元数据。校验已确认的生产字段、开发构建默认值、输入大小、严格 UTF-8、重复 JSON 字段和未知配置字段；来源元数据只核对 schema、平台、架构、版本格式及原始文件摘要，不扫描或宣称验证实际工具资源。额外 Mac/替代 Tauri 配置、链接文件／祖先目录、不安全路径及会覆盖输出、目标、配置或资源锚的环境变量均拒绝。

生成的 Tauri 文件是 **JSON Merge Patch**，合并到固定开发配置后才是完整配置，不能当作独立 schema 文档。它替换生产 ID、版本、管理窗标题及说明，删除开发前置命令和 dev URL，保留既有安全配置及窗口尺寸；品牌图标使用固定源码路径。`resources/` 的 null 项用于删除继承的开发资源映射，实际资源映射只指向本轮私有 stage；sidecar、网页产物、权限文件也只指向本轮规划位置。

计划列出独立 Cargo target、未签来源工具槽、正式资源槽、frontend、sidecar、签名权限和收据目录，以及后续编译需要的清单路径与固定环境。`prepare` 只创建空目录和两个 JSON 文件，不复制工具，不填充 SDK／模板，不编译或启动 CLI，不创建签后清单。sidecar 暂保留代码当前要求的 `local-projects-dev` 内部文件名，生产编译身份由现有 Cargo feature 决定；本步骤没有更改产品展示名称或 CLI 查找规则。

覆盖配置要求 `production-resource-anchors` feature，保留当前生产运行拒绝；删除开发 ad-hoc `signingIdentity: "-"`，将主程序权限指向本轮尚待准备的权限文件。`createUpdaterArtifacts` 明确为 false，不配置假的更新公钥。配置文件及计划没有执行入口，不能单独替代未来的正式签名构建事务，也不应交给开发版标准清理入口执行。

## 输出和边界

每轮输出用独占目录与排他文件创建，目录 `0700`、JSON `0600`。已有预检槽、开发 `.app`／DMG 和其他文件均不覆盖、不删除；部分写入失败时保留本轮槽供检查。现有 `target/production-macos` 若不是本用户拥有的私有普通目录则拒绝。

输入读取使用 no-follow、打开前后及父目录元数据复核，输出限制在独占私有槽。本步骤是本用户控制的源码元数据生成器，不将这些路径检查描述为面对同权限恶意进程的完整 FD-relative 防替换证明；未来签名与安装事务仍须建立自己的受控目录所有权。

此入口不导入子进程或网络模块，不读取任何 Apple／更新签名环境值、钥匙串、证书、密码或私钥，不生成密钥，不执行签名、公证、安装、服务发现或 GitHub 发布。报告中的签名策略是公开配置绑定，`evidenceVerified` 保持 false。

自动更新仍需单独实现：固定 HTTPS 检查、受管下载和签名校验、用户选择安装、排空、管理窗最终销毁、旧主进程实际退出和安装 helper 交接。`ReadyForNativeExit` 继续仅供正常退出，不授予安装许可。真实正式包、下载资产、最低系统和升级安装尚未完成。

## 验证（独立源码副本，2026-10-25）

定向 Node 测试 14 项通过、0 失败；包括严格 JSON／每个固定 profile 字段拒绝、公开签名策略类型、开发配置漂移、从其他当前目录和工程别名启动、零输出的 check、prepare 私有空槽、重复执行保留旧槽、真实开发输出哨兵保全、Tauri 合并补丁隔离映射、配置与环境覆盖拒绝、人工签名环境标记不输出、额外配置／输入软硬链接／祖先链接及输出链接拒绝。测试仅在 TempDir 放置人工元数据与哨兵，未读写真实项目根。

独立源码副本另实际运行 `prepare`，使用当前真实仓库元数据生成计划与覆盖配置，确认九项待办及全部就绪字段为 false，stage 均为空；该结果不是 Tauri 编译、签名或包校验。开发默认配置及三个来源元数据原始 bytes 未修改。配套回归入口：`npm run test:production-macos-preflight`。
