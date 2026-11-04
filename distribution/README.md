# 下载与发布

平台源码、推荐列表和平台发布文件统一维护在 `happypandaa/panda-drawer`。开发使用 `develop`，通过现有集成检查后将公开内容合入 `main`。GitHub 是当前唯一启用的下载来源。

## 推荐应用

发布文件为仓库 `main` 分支中的 `catalog/apps.json`，固定读取地址记录在本目录的 `download-sources.json`。Mac 和 Windows 使用同一列表和本机缓存。读取失败时使用上次有效缓存。

手写的简介、地址及启用状态在 `scripts/app-catalog-input.json`，运行下面的命令读取应用的公开 `panda.json`，生成名称和所需配置。应检查生成结果，再提交列表。

```sh
pnpm generate:app-catalog --output catalog/apps.json
```

`repo` 是提供方到公开 HTTPS 地址的映射，当前填写 `github`，未来可以加入 `gitee`。读取旧缓存时兼容原单地址格式。推荐 ID 随导入记录保存，切换仓库地址后仍能识别已添加的应用。早期记录没有 ID 时，比对两个仓库地址。

Clack 和 Readio 已具有当前格式的 `panda.json`。Radio Bob 的输入记录保留为 `enabled: false`，待作者适配并验证后启用。三个应用各自维护源码和版本。

## 平台更新清单

每个版本在平台仓库的 GitHub Release 中发布各自签名的 Mac 更新归档、Windows NSIS 安装包和一份 `latest.json`。首次 Mac 安装使用 DMG，更新归档使用 `.app.tar.gz`，Windows 更新产物使用 `.exe`。

清单生成输入可以是原来的单个平台记录，或 Mac、Windows 两个平台记录的数组。两条记录必须使用相同版本、说明和发布日期。目标分别是 `darwin-aarch64` 与 `windows-x86_64`。

```sh
node scripts/update-manifest.mjs generate --input /absolute/signed-artifacts.json --output /new/latest.json --source github
node scripts/update-manifest.mjs validate --input /absolute/latest.json
node scripts/update-manifest.mjs verify --input /absolute/latest.json --artifact /absolute/Panda_0.1.0_x64-setup.exe --public-key /absolute/operator.pub --target windows-x86_64
```

生成和格式检查保持原有边界。`verify` 核验实际产物字节、签名及签名中的版本和文件名。正式运行时还需绑定编译内置公钥，并完成检查、下载、关闭、安装和重启流程。

## 后续接入 OSS 和 Gitee

平台包分发和应用源码分开配置，能够分别使用 OSS 与 Gitee。

- OSS 的固定域名确定后，在 `download-sources.json` 的 `updates.sources` 中登记 `oss`，填写 `manifestUrl` 与 `artifactBaseUrl`。产物路径保持 `<artifactBaseUrl>/v<版本>/<文件名>`。生成工具的 `--source oss` 使用这个已登记来源，为同一批产物生成对应清单，保留原签名。
- 推荐列表需要镜像时，在 `catalog.sources` 中登记固定读取地址并选择 `primary`。原生读取使用构建时嵌入的注册表。
- 应用源码镜像通过每条推荐的 `repo.gitee` 指定。现有 Git 导入支持这类公开地址。未来的线路选择、网络失败切换和同任务重试需要另行接入。

当前注册表只登记 GitHub，待真实固定地址到位后再启用对应国内来源。网络清单、环境变量和命令参数均不能注册新的受信任地址。更新地址仍精确绑定来源、版本和文件名，任何分发源都必须经过同样的产物验签。
