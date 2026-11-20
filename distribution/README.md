# 下载与发布

平台仓库为 GitHub `happypandaa/panda-drawer` 和 Gitee `mycolorfuldays/panda-drawer`。代码、分支和标签同步推送到两端。日常实现与用户验证使用 `develop`；通过现有集成检查后再合入 `main`。

## 推荐应用与线路

`download-sources.json` 是构建时嵌入的固定来源注册表。推荐列表读取两端的 `stable/catalog/apps.json`。在 `develop` 上生成并审定列表后，将该文件原样更新到推荐目录专用的 `stable` 分支，把同一提交推送到 GitHub 和 Gitee。匿名读取注册表中的两个地址，确认响应为 200，且两份正文的 SHA-256 均与审定文件相同。

设置 → 通用 → 下载线路，支持自动、国内、国外。国内推荐应用走 Gitee，国外推荐应用走 GitHub。自动模式并发读取两端推荐列表，每端限时 5 秒，先返回有效内容的一端胜出。失败时沿用上次成功结果。尚无成功结果时，系统首选语言为简体中文且时区为中国大陆才选国内。结果保存到本机，超过 7 天后台重新检测。

推荐列表或推荐源码下载失败，自动模式只尝试另一个来源一次。推荐源码连续 30 秒没有进度视为失败；先等待原进程及 I/O 收尾，再在同一添加任务的独立临时目录尝试镜像，最后统一清理。手动模式保留选择，失败时提供“换线路并重试”。用户粘贴的链接不跟随线路切换。

推荐输入维护在 `scripts/app-catalog-input.json`，名称和所需配置从各应用的 `panda.json` 生成：

```sh
pnpm generate:app-catalog --output catalog/apps.json
# 使用已核对的本地作者仓库生成，不修改作者目录：
pnpm generate:app-catalog --manifest-root /absolute/panda-project --output catalog/apps.json
```

四项推荐均已启用，各自维护源码、根清单和版本：

| 应用 | GitHub | Gitee |
| --- | --- | --- |
| 拾闻（Gleaner） | happypandaa/gleaner | mycolorfuldays/gleaner |
| Clack | happypandaa/Clack | mycolorfuldays/clack |
| Readio | happypandaa/Readio | mycolorfuldays/readio |
| Radio Bob | happypandaa/radio-bob | mycolorfuldays/radio-bob |

每条 `repo` 同时登记 `github` 和 `gitee` 公开 HTTPS 地址。推荐 ID 随导入记录保存，换源后继续识别已添加的同一应用；早期无 ID 记录比对两个仓库地址。读取旧缓存仍兼容单地址格式。

## Mac 候选安装包

在干净的发行工作树执行 `pnpm release:mac:candidate`。该命令使用本机 Developer ID Application 证书为应用和内置工具签名，通过钥匙串描述文件 `panda-drawer-notary` 向 Apple 公证应用和 DMG，并装订票据。安装包和收据保存在该工作树的 `target/production-macos/` 下。

运行 `node scripts/production-macos-candidate.mjs verify-latest` 可核对当前提交的候选包。它会复验应用签名、公证票据、内置工具和 DMG 摘要，并只读挂载 DMG 核对其中的应用。命令成功时会输出提交、版本、安装包路径和 SHA-256。若在应用签名完成后中断，可用 `node scripts/production-macos-candidate.mjs finish <候选目录绝对路径>` 继续公证。候选包验证完成后，正式更新包和发布清单仍需单独生成、验签和发布。

## 平台更新清单

Mac 首次安装使用 DMG，更新使用签名 `.app.tar.gz`；Windows x64 使用签名 NSIS `.exe`。清单生成支持单个平台记录，或版本、说明、日期一致的多平台记录数组（`darwin-aarch64`、`windows-x86_64`）。同一批签名产物分别生成两个来源的清单：

```sh
node scripts/update-manifest.mjs generate --input /absolute/signed-artifacts.json --output /new/github/latest.json --source github
node scripts/update-manifest.mjs generate --input /absolute/signed-artifacts.json --output /new/oss/latest.json --source oss
node scripts/update-manifest.mjs validate --input /absolute/oss/latest.json
node scripts/update-manifest.mjs verify --input /absolute/oss/latest.json --artifact /absolute/Panda_0.1.0_x64-setup.exe --public-key /absolute/operator.pub --target windows-x86_64
```

国外更新走 `happypandaa/panda-drawer-releases` 的稳定 Release，读取 `releases/latest/download/latest.json`，从版本标签下载产物。国内更新走阿里云 CDN 的 `https://panda-updates.zhimibuhui.com/latest.json`，产物存放在 CDN 对应的 `/v<版本>/<文件名>` 下。 [正式来源注册表](download-sources.json)

自动线路优先使用当前线路。清单请求或包下载遇到网络故障时最多换源一次。备用清单提供不同版本时，应用等待用户确认该版本后再下载。手动线路固定使用所选来源。地址或签名校验失败时结束本轮，保留失败原因。

更新地址由构建时注册表固定，清单中的产物地址须匹配来源、版本和文件名。`verify` 核验实际产物字节、签名及签名中的版本与文件名。

### Windows 本机升级验证

在 Windows x64 上运行 `node scripts/local-update-test.mjs build`。输出目录中包含独立测试版的两个 NSIS 安装包、0.1.1 更新包的签名、公钥和本机清单。构建需要项目固定的 pnpm 版本。如果 pnpm 通过单独的 JS 入口安装，可用 `PANDA_CI_PNPM_CLI` 指向该入口。

运行 `node scripts/local-update-test.mjs serve <输出目录>`，安装该目录中的 `PandaDrawer_0.1.0_x64-setup.exe`，打开「熊猫小抽屉 · 更新测试版」。看到 0.1.1 提示后点击「下载」，完成后点击「安装并重启」。在重开的应用中通过「关于与更新」核对版本。该测试版使用固定标识 `test.happypandaa.pandadrawer.update` 和本机更新源 `127.0.0.1:18427`。

### Windows 与 macOS 双线路线上升级验证

在要验证的平台上，使用新的 `auto-update-dual-test-YYYYMMDD-HHMMSS` 标签运行 `node scripts/local-update-test.mjs build-online-dual <标签>`。构建会把标签和同一把测试公钥嵌入 0.1.0 和 0.1.1。每次构建使用独立签名密钥，为不同平台或重新构建的套件分配新标签。

输出目录的根部包含 0.1.0 初始程序和 `update-test.pub`。`assets/github` 与 `assets/oss` 各含同一份 0.1.1 更新包、签名及各自的 `latest.json`。在另一台机器发布时，把这两个目录和公钥一起复制过去，保持目录结构。将测试标签同步到 GitHub 和 Gitee，核对两端均指向构建时的提交。在有发布权限的机器上执行以下命令，替换标签、完整提交 SHA 和套件的绝对路径。

```sh
node scripts/publish-online-update-test.mjs --tag "$tag" --commit "$commit" --assets "$kit/assets/github" --source github
node scripts/publish-online-update-test.mjs --tag "$tag" --commit "$commit" --assets "$kit/assets/oss" --source oss
```

GitHub 使用项目仓库中对应测试标签的预发布。OSS 使用发布机已配置的 ossutil v2 身份及地域，通过 `PANDA_UPDATE_OSS_BUCKET` 指定该测试 CDN 的源存储桶，产物路径为 `_check/<标签>/`。脚本在发布清单前核对安装包和签名，并通过各自可公开读取的地址复核字节。添加 `--verify-only` 可以在上传前离线核验套件。

Windows 上安装套件根部的 `PandaDrawer_0.1.0_x64-setup.exe`，从桌面打开「熊猫小抽屉 · 双线路更新测试版」。在「设置 → 通用 → 下载线路」选择「国外线路」，检查更新，下载并安装 0.1.1，重启后核对版本。卸载该测试程序，重新安装同一份 0.1.0，选择「国内线路」重复测试。手动选择会固定来源，便于分别验证 GitHub 和 OSS。

## 代码仓库分发测试

SSH 用于推送代码和标签。Release 创建及附件上传使用两端 API 令牌：GitHub fine-grained token 仅授权本仓库的 Contents 读写；Gitee 私人令牌授权 `projects`。在本机运行以下脚本隐藏输入令牌，保存到仓库外、仅当前用户可读的文件，不要提交或发送令牌：

```sh
python3 scripts/configure-release-credentials.py
```

同一脚本也用于更新令牌：已有配置时，输入新值替换对应令牌，直接回车保留原值。文件仍为仅当前用户可读写，全部输入完成后原子替换，不保留旧令牌副本。GitHub 令牌的 Repository access 必须包含本仓库；仅能读取公开仓库的授权无法发布 Release。

先提交并同步 `develop`，创建唯一测试标签（如 `download-routes-test-20261104-1`），推送并核实两端标签指向同一提交。将待发布文件放在独立目录，再执行：

```sh
node scripts/publish-test-release.mjs --tag download-routes-test-20261104-1 --assets /absolute/test-assets --credentials /absolute/private/release-credentials.json
```

脚本仅接受专用测试标签，创建两端预发布，不替换正式最新版。相同附件上传后分别匿名下载，核对实际字节数与 SHA-256；可在部分完成后继续，不覆盖已有附件。测试报告应区分下载成功、真实系统安装和应用实际运行。

Gitee 普通仓库发行附件单文件上限为 100 MB。上述脚本会在上传前检查探针附件的大小。 [Gitee 发行说明](https://help.gitee.com/repository/release/create)
