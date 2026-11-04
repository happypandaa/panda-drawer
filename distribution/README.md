# 下载与发布

平台仓库为 GitHub `happypandaa/panda-drawer` 和 Gitee `mycolorfuldays/panda-drawer`。代码、分支和标签同步推送到两端。日常实现与用户验证使用 `develop`；通过现有集成检查后再合入 `main`。

## 推荐应用与线路

`download-sources.json` 是构建时嵌入的固定来源注册表。当前开发版读取两端 `develop/catalog/apps.json`，保证两条线路对应同一批开发内容；正式发行切换到稳定分支时，应同时调整两个地址。

设置 → 通用 → 下载线路，支持自动、国内（Gitee）、国外（GitHub）。自动模式并发读取两端推荐列表，每端限时 5 秒，先返回有效内容的一端胜出。失败时沿用上次成功结果；尚无成功结果时，系统首选语言为简体中文且时区为中国大陆才选国内。结果保存到本机，超过 7 天后台重新检测。

推荐列表或推荐源码下载失败，自动模式只尝试另一个来源一次。推荐源码连续 30 秒没有进度视为失败；先等待原进程及 I/O 收尾，再在同一添加任务的独立临时目录尝试镜像，最后统一清理。手动模式保留选择，失败时提供“换线路并重试”。用户粘贴的链接不跟随线路切换。

推荐输入维护在 `scripts/app-catalog-input.json`，名称和所需配置从各应用的 `panda.json` 生成：

```sh
pnpm generate:app-catalog --output catalog/apps.json
# 使用已核对的本地作者仓库生成，不修改作者目录：
pnpm generate:app-catalog --manifest-root /absolute/panda-project --output catalog/apps.json
```

三项推荐均已启用，各自维护源码、根清单和版本：

| 应用 | GitHub | Gitee |
| --- | --- | --- |
| Clack | happypandaa/Clack | mycolorfuldays/clack |
| Readio | happypandaa/Readio | mycolorfuldays/readio |
| Radio Bob | happypandaa/radio-bob | mycolorfuldays/radio-bob |

每条 `repo` 同时登记 `github` 和 `gitee` 公开 HTTPS 地址。推荐 ID 随导入记录保存，换源后继续识别已添加的同一应用；早期无 ID 记录比对两个仓库地址。读取旧缓存仍兼容单地址格式。

## 平台更新清单

Mac 首次安装使用 DMG，更新使用签名 `.app.tar.gz`；Windows x64 使用签名 NSIS `.exe`。清单生成支持单个平台记录，或版本、说明、日期一致的多平台记录数组（`darwin-aarch64`、`windows-x86_64`）。同一批签名产物分别生成两个来源的清单：

```sh
node scripts/update-manifest.mjs generate --input /absolute/signed-artifacts.json --output /new/github/latest.json --source github
node scripts/update-manifest.mjs generate --input /absolute/signed-artifacts.json --output /new/gitee/latest.json --source gitee
node scripts/update-manifest.mjs validate --input /absolute/gitee/latest.json
node scripts/update-manifest.mjs verify --input /absolute/gitee/latest.json --artifact /absolute/Panda_0.1.0_x64-setup.exe --public-key /absolute/operator.pub --target windows-x86_64
```

GitHub 正式清单放在稳定 Release 的 `latest.json`；Gitee 正式清单地址为注册表指定的仓库 `distribution/latest.json`，产物放在对应标签的 Release 中。当前未发布正式签名清单，不用测试附件冒充正式更新。`routeUpdateEndpoints(route, mode)` 在自动模式返回当前来源、另一来源，在手动模式仅返回选定来源。

`verify` 核验实际产物字节、签名及签名中的版本与文件名。运行时自动更新接入属于后续任务，仍需绑定编译内置公钥并实现检查、下载、关闭、安装和重启；本次离线工具不表示已提供自动更新或增量补丁。

## 双端测试 Release

SSH 用于推送代码和标签。Release 创建及附件上传使用两端 API 令牌：GitHub fine-grained token 仅授权本仓库的 Contents 读写；Gitee 私人令牌授权 `projects`。在本机运行以下脚本隐藏输入令牌，保存到仓库外、仅当前用户可读的文件，不要提交或发送令牌：

```sh
python3 scripts/configure-release-credentials.py
```

先提交并同步 `develop`，创建唯一测试标签（如 `download-routes-test-20261104-1`），推送并核实两端标签指向同一提交。将待发布文件放在独立目录，再执行：

```sh
node scripts/publish-test-release.mjs --tag download-routes-test-20261104-1 --assets /absolute/test-assets --credentials /absolute/private/release-credentials.json
```

脚本仅接受专用测试标签，创建两端预发布，不替换正式最新版。相同附件上传后分别匿名下载，核对实际字节数与 SHA-256；可在部分完成后继续，不覆盖已有附件。测试报告应区分下载成功、真实系统安装和应用实际运行。

Gitee 普通仓库发行附件当前单文件上限为 100 MB（[官方说明](https://help.gitee.com/repository/release/create)）。脚本发布前检查此限制；超限的平台包需要 OSS 或其他实际可用的分发方案，不能把源码归档当作安装包。

## 后续 OSS

平台包分发与应用源码分开配置。OSS 固定域名确定后，在 `updates.sources` 注册 `oss` 的 `manifestUrl`、`artifactBaseUrl`，将 `routes.cn.updates` 改为 `oss`，国内应用源码继续使用 Gitee。产物路径为 `<artifactBaseUrl>/v<版本>/<文件名>`；`--source oss` 为相同签名产物生成 OSS 地址清单。推荐目录如需放到 OSS，再独立调整 `catalog.sources` 与 `routes.cn.catalog`。

网络清单、环境变量和命令参数不能注册新的受信任地址。每个更新地址精确绑定来源、版本和文件名；增加分发源继续执行相同的验签规则。
