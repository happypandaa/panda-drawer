# 熊猫小抽屉桌面版正式发布手册

本流程适用于 Apple 芯片 Mac 和 Windows x64。两端同时上线时，各自在本机验收同一提交，发布机汇总原件，再发布 GitHub 和国内 OSS/CDN 两条线路。发布负责人指定 macOS 先上线时，按第 6 节分平台发布。

## 1. 确定版本和源码

1. 分别匿名读取 [GitHub 稳定清单](https://github.com/happypandaa/panda-drawer/releases/latest/download/latest.json) 和 [国内稳定清单](https://panda-updates.zhimibuhui.com/latest.json)。将两份原文分别保存在本次本地发行目录，记录摘要、旧版本和 GitHub 最新稳定 Release。两份清单应包含相同的版本、平台和签名，下载 URL 按线路不同。二次发布读到 404、清单不一致或无法访问时，先查明当前线上状态。
2. 确定更高的稳定版本号和中英文发布说明。以 `0.1.0 → 0.1.1` 为例，更新 `build-profiles/production-version.txt`、`production.json` 和 `production.windows-x64.json` 的正式版本。核对固定生产标识、更新公钥和两条线路。[发行配置](../build-profiles/) · [正式来源注册表](download-sources.json)
3. 在 `develop` 提交更改，同步 GitHub 和 Gitee。记录待发行提交的 **完整 SHA**，发给 Windows 端。两端从干净的同一提交构建，通常的开发目录保持在 `develop`。按项目已有检查要求将该提交集成到 `main`，发布时核对两个远端的 `main` 和 `v<版本>` 标签都指向这一个 SHA。若集成产生新的发行 SHA，应以新 SHA 重新构建。已有同名标签须核对归属，保留已发布标签。

## 2. Mac 构建、签名和公证

以下命令在干净的发行源码根目录中逐条执行，每步成功后再继续。两台机器均按项目约定分开运行 CI 和候选构建。

```sh
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
git status --short
git rev-parse HEAD
pnpm install --frozen-lockfile
node scripts/ci.mjs
pnpm release:mac:preflight
pnpm release:mac:candidate
node scripts/production-macos-candidate.mjs verify-latest
```

候选构建使用 Developer ID Application 签署应用和内置工具，通过钥匙串配置 `panda-drawer-notary` 完成公证并装订票据。记录复验输出中的提交、版本、DMG 的实际位置和摘要。如已完成应用签名而公证中断，用 `node scripts/production-macos-candidate.mjs finish <候选目录绝对路径>` 续跑。[Mac 候选包说明](README.md#mac-候选安装包)

设置 `MAC_OUT` 为本次尚不存在的输出目录的绝对路径（父目录须已存在）、`VERSION` 为本次版本号。运行以下命令，从当前提交的已复验候选生成应用内更新包。DMG 用于首次安装，更新包使用最终文件名签署。由本机已配置的签名环境提供正式密钥。需要解锁时在 Mac 本机的隐藏输入框填写密码。

```sh
node scripts/production-update-artifact.mjs archive "$MAC_OUT"
MAC_UPDATE="$MAC_OUT/PandaDrawer_${VERSION}_aarch64.app.tar.gz"
node scripts/production-update-artifact.mjs sign "$MAC_UPDATE" "$MAC_OUT/archive-receipt.json"
node scripts/production-update-artifact.mjs verify "$MAC_UPDATE" "$MAC_OUT/archive-receipt.json"
```

在测试机上打开本次候选，核对管理主窗口、小应用窗口、菜单和「关于与更新」显示的产品名和版本。正式版使用「熊猫小抽屉」或 “Panda Drawer”，并在两种界面语言下核对标题。

## 3. Windows 端如何交付

将第 1 步的完整 SHA 和版本号交给 Windows x64 机器的负责人或代理。在 Windows 的干净开发目录用 `git switch develop` 和 `git pull --ff-only origin develop` 取得源码。随后在 PowerShell 逐条执行下列命令。先确认 `git rev-parse HEAD` 与交接 SHA 逐字相同。若已有新提交，在干净的发行工作树签出约定 SHA 后再构建。使用项目 `package.json` 固定的 pnpm 版本。[Windows 工具链与验收约定](../docs/windows-x64-port.md)

```powershell
git status --short
git rev-parse HEAD
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_INCREMENTAL = '0'
pnpm install --frozen-lockfile
pnpm env:windows
pnpm check:windows:toolchain
node scripts/ci.mjs
pnpm release:windows:candidate check
pnpm release:windows:candidate
pnpm release:windows:candidate verify-latest
```

以构建结果中的 `candidateDirectory` 为准，从同一目录交付以下文件，可保留原目录结构打成 ZIP。记录安装包的 SHA-256 时可使用 PowerShell 的 `Get-FileHash -Algorithm SHA256 <EXE路径>`。

| 交回发布机 | 内容 |
| --- | --- |
| 原始 NSIS 安装包 | `candidate/PandaDrawer_<版本>_x64-setup.exe`，记录实际字节数和 SHA-256 |
| 原始构建收据 | `receipts/candidate.json`，含完整源码 SHA、版本和产物摘要。已有更新签名时连同 EXE 旁的 `.exe.sig` 交付 |
| Windows 实测记录 | 写明实际系统版本、CI 和候选复验结果、安装和重开、项目与配置保留情况、WebView2、Authenticode 和 SmartScreen 实测结果，以及任何尚未完成或失败的项 |

若 Windows 有匹配的正式更新私钥，候选命令会签署并复验 EXE。若私钥在 Mac，Windows 直接交回同一 EXE 和原始收据。将 `WINDOWS_EXE` 和 `WINDOWS_RECEIPT` 分别设为收到的 EXE 和原始收据绝对路径。Mac 核对版本、SHA 与 EXE 摘要后执行下列命令。随包已有签名时直接运行 `verify`。首发 `0.1.0` 已在 Mac 对 Windows 交付的原始 EXE 补签并按两条线路验签。Tauri 更新签名和 Windows Authenticode 发布者签名分别记录。

```sh
node scripts/production-update-artifact.mjs sign "$WINDOWS_EXE" "$WINDOWS_RECEIPT"
node scripts/production-update-artifact.mjs verify "$WINDOWS_EXE" "$WINDOWS_RECEIPT"
```

## 4. 收齐两端产物，核对两份清单

发布机核对两端候选与待发布标签的完整 SHA、版本和 SHA-256。准备 DMG、Mac 更新包及其 `.sig`、Windows EXE 及其 `.sig`、`SHA256SUMS.txt` 和中英文说明。把本次验收结果交给发布负责人，明确哪些来自真实安装，哪些是离线验证。

生成一份 `signed-artifacts.json`，内容是两个平台的记录数组。每条记录填写 `schemaVersion: 1`、`profile: "production"`、`identifier: "com.happypandaa.pandadrawer"`、`releaseRepository: "https://github.com/happypandaa/panda-drawer"`，以及 `target`、`version`、`tag`、`notes`、含时区的 `pub_date`。两个目标分别是 `darwin-aarch64`、`windows-x86_64`，版本、标签、说明和日期相同。`artifact` 的字段为最终文件名 `name`、字节数 `bytes`、小写十六进制 `sha256` 和相应 `.sig` 的内容 `signature`（去除首尾空白）。[生成和校验规则](../scripts/update-manifest-protocol.mjs)

设置 `STAGE` 为本次发行目录绝对路径、`PREVIOUS` 为第 1 步核实的旧版本。`MAC_UPDATE` 和 `WINDOWS_EXE` 均指向最终原件。以下命令在平台仓库根目录执行，两份 `latest.json` 写入各自的新路径。

```sh
mkdir -p "$STAGE/github" "$STAGE/oss"
PUB="$PWD/build-profiles/production-updater.pub"
for source in github oss; do
  manifest="$STAGE/$source/latest.json"
  node scripts/update-manifest.mjs generate --input "$STAGE/signed-artifacts.json" \
    --previous-version "$PREVIOUS" --source "$source" --output "$manifest" || exit 1
  node scripts/update-manifest.mjs verify --input "$manifest" --current-version "$PREVIOUS" \
    --artifact "$MAC_UPDATE" --public-key "$PUB" --target darwin-aarch64 || exit 1
  node scripts/update-manifest.mjs verify --input "$manifest" --current-version "$PREVIOUS" \
    --artifact "$WINDOWS_EXE" --public-key "$PUB" --target windows-x86_64 || exit 1
done
```

## 5. 暂存，确认发布，回验

1. **暂存同一批字节。** 在 GitHub 主仓库为已核对的标签准备 Release 草稿，附上两端产物、签名、摘要和 GitHub 清单。在国内发行 OSS 的 `v<版本>/` 暂存同一批二进制和签名，并放 OSS 版本清单。核对草稿全部附件。遇到已存在的版本对象时，仅复用能证实完全相同的字节。同名不同内容时停止处理冲突。
2. **核对公开的版本地址。** 发布附件齐全的 GitHub 版本时显式指定 `draft=false`、`prerelease=false`、`make_latest="false"`，回读原有稳定地址仍指向旧版。匿名下载 GitHub 的 `releases/download/v<版本>/` 和国内 CDN 的 `v<版本>/` 下各自的清单、包、签名及摘要文件。与批准的本地文件逐一核对大小和 SHA-256，并对两个更新包验签。保存两条线路的公开验证结果后，才切换稳定入口。[GitHub Release API](https://docs.github.com/en/rest/releases/releases#update-a-release)
3. **负责人亲自确认。** 核对版本、完整 SHA、两个平台的文件名和摘要、实测限制及公开下载结果。由用户按发布确认键。熊猫发布台目前的「上线 / 更新」汇总检测结果。尚无发布按钮时，用户可按下步直接在 GitHub 发布页和阿里云控制台亲自完成切换。
4. **切换已有稳定清单。** 只由一名负责人操作稳定入口。切换前从 OSS 源站读回根 `latest.json`，与第 1 节保存的旧版正文核对，再查 GitHub 当前稳定 Release。任一已变化时重新核对。先到主仓库 Releases 编辑已验证的新版本，选择「Set as latest release」，点击「Update release」，从稳定 URL 回读 GitHub 清单。紧接着到 OSS 控制台的发行 Bucket，在根目录上传刚核验过的 `oss/latest.json`，人工确认覆盖这一份指针并保留根清单短缓存。读回源站确认摘要，在 CDN 控制台只刷新该清单 URL，再从国内稳定地址回验。版本目录和旧 Release 附件继续保留。[GitHub 发布页](https://github.com/happypandaa/panda-drawer/releases) · [编辑 Release](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository#editing-a-release) · [OSS 控制台](https://oss.console.aliyun.com/) · [CDN 控制台](https://cdn.console.aliyun.com/)

   首发临时脚本固定了版本，且拒绝覆盖已有对象。二次发布直接替换核准的根清单，跳过「先删除旧清单」的操作。普通 `PutObject` 会替换同名对象，操作范围须精确到本次根清单。不要把 `PutObject` 的 `If-Match` 用作并发写保护。[OSS 对象写入说明](https://www.alibabacloud.com/help/en/oss/developer-reference/putobject)
5. **处理中断。** 若切换一条线路后中断，先从两处重新读取稳定清单并对照本地新旧摘要。已正确切换的线路保留现状，续做尚未完成的切换。发现未知内容时保留现场，交负责人决定。需要撤回时复核已保存的旧清单和旧 Release，另行确认后恢复旧版本入口并回验。已安装新版的用户需通过更高版本的修复包升级。
6. **公开回验。** 分别匿名下载两个稳定清单，核对版本、两个目标、对应源的 URL 和签名。从国内与海外线路完整下载并核验摘要，使用正式公钥复验更新包，检查根清单短缓存、版本文件长缓存及安装包分段请求。用上一正式版本在相应平台实际执行「检查更新 → 下载 → 安装并重启」，核对版本和已有数据。分别记录网络回验和实际升级结果。
7. **同步官网。** 两条稳定线路均完成回验后，在独立官网仓库更新中英文下载页、首页和版本说明中的版本、主下载与备用链接，同步对应站点检查，依次运行 `pnpm run build`、`pnpm run check`，发布通过检查的页面并刷新相关页面缓存。回读官网入口及两端安装包链接。最终记录版本、完整 SHA、标签、Release 和两条清单、各平台验收结果及遗留项。[官网下载页](https://panda.zhimibuhui.com/download/)

## 6. macOS 先上线，Windows 随后交付

1. **固定发行提交。** 完成第 2 节的 Mac 验收，将同一标签推到 GitHub 和 Gitee，回读完整 SHA。将标签和 SHA 交给 Windows 端。若共享 `main` 尚待 Windows CI，先从已验收提交发布标签，待两端检查齐备后再集成 `main`。补发 Windows 时继续使用原标签和原提交。
2. **单独发布 Mac。** 第 4 节的签名记录只填 `darwin-aarch64`，为 GitHub 和 OSS 各生成一份清单，并分别对 Mac 归档验签。按第 5 节上传和公开回验 DMG、更新归档及签名。分批发布时可用 `SHA256SUMS-macOS.txt` 和 OSS 的 `v<版本>/macos-latest.json` 保存这批的摘要和清单快照，给完整双端清单留出独立文件名。核对版本化下载后，按发布授权切换两条稳定入口。
3. **分平台验收。** 已发行的 0.1.0 客户端读取有效清单后，只在含本机平台且版本较高时提供更新。macOS 应在真实旧版执行检查、下载、安装和重启。Windows 继续使用原版本；中英文官网分别标明两端版本，并保留 Windows 原有的 EXE 下载地址。[客户端清单处理](../apps/desktop/src-tauri/src/platform_update/transport.rs)
4. **补发 Windows。** Windows 按第 3 节交回 EXE、原始收据和实测记录。发布机在标签对应的干净发行工作目录验签，核对已上线 Mac 文件的摘要和签名。从原记录补入 `windows-x86_64`，两平台使用一致的版本说明和发布日期。生成时 `--previous-version` 填 Windows 当前稳定版（本次为 `0.1.0`）。把 EXE 和签名追加到相同的版本路径，用包含两端的清单分别验签两份安装产物。
5. **切换双端清单。** 保存当前仅含 Mac 的稳定清单作为切换基线，先验证新的版本化文件再替换稳定清单。此时 GitHub 的同一 Release 已是 Latest，需更新其 `latest.json` 附件并回读正式地址；操作前核对 Release 允许修改附件。按第 5 节同步国内根清单并回验两条线路，验证 Windows 旧版实际升级，再更新官网 Windows 下载。已上线的 Mac 安装包、更新归档和签名沿用原件。
