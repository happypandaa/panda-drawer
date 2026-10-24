# 本机配置 Mac 发行证书与公证

熊猫小抽屉通过 GitHub Releases 站外发行，使用 Developer ID Application 签名和 Apple 公证，无需提交 App Store。本文供账号持有人自行配置；打包、签名、公证和发行仍由后续发行流程完成。

## 1 确认会员和团队

登录 [Apple Developer 账号](https://developer.apple.com/account)，在会员信息中确认 Apple Developer Program 会员有效，并记下 10 位 **Team ID**。只有免费 Apple 账号不够；已有有效付费会员无需为本应用另买证书。Team ID 与本应用 Bundle ID `com.happypandaa.pandadrawer` 是两个不同值。

## 2 创建 Developer ID Application 证书

优先在这台用于打包的 Mac 上操作：

1. Xcode → Settings → Apple Accounts，选择你的账号及正确团队。
2. 打开 Manage Certificates，点「＋」，选择 **Developer ID Application**。
3. 完成后确认该证书出现在本机钥匙串的“我的证书”中，展开时能看到对应私钥。

不要选 Apple Development、Apple Distribution 或 Developer ID Installer；当前应用的 DMG 发行使用 Developer ID Application。

如果创建选项是灰色，先核对会员和团队权限。Apple 网页创建 Developer ID 证书要求 **Account Holder**；仅看到 Admin 不足以确认这个权限。具有云端 Developer ID 证书权限的 Admin 还可使用云端证书，但当前本机打包准备的是本机持有私钥的证书。参见 [Apple 的证书说明](https://developer.apple.com/help/account/certificates/create-developer-id-certificates)。

有 Account Holder 权限时，也可以按网页流程创建：

1. 打开“钥匙串访问”，菜单“证书助理 → 从证书颁发机构请求证书”，填写账号邮箱和名称，选择“存储到磁盘”，生成 CSR 文件。私钥留在本机。
2. 在开发者网站进入 Certificates, Identifiers & Profiles → Certificates →「＋」，选择 Developer ID Application，按页面提示上传 CSR。
3. 下载生成的 `.cer`，在生成 CSR 的这台 Mac 上双击安装，再检查“我的证书”下的私钥。

本机只读检查命令：

```sh
security find-identity -v -p codesigning
```

输出应包含有效的 `Developer ID Application: … (TEAMID)`。仅有 Apple Development／Apple Distribution 不代表此项完成。

## 3 保存公证凭据

在 [Apple 账号](https://account.apple.com)的“登录与安全 → App 专用密码”中创建一个专用于公证的密码。随后在本机终端执行，替换邮箱和 Team ID：

```sh
xcrun notarytool store-credentials "panda-drawer-notary" \
  --apple-id "你的Apple账号邮箱" \
  --team-id "你的TeamID"
```

按终端的安全提示输入 **App 专用密码**。不把密码写进命令参数；命令会验证并保存在本机钥匙串的 `panda-drawer-notary` profile 中。参见 [Apple 的 notarytool 凭据说明](https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool)。

可用只读命令验证该 profile 能登录；没有历史公证记录也正常：

```sh
xcrun notarytool history --keychain-profile "panda-drawer-notary"
```

## 4 配置完成后

告诉协作者“Developer ID Application 证书和 panda-drawer-notary 已配置”，需要时提供公开的证书名称和 Team ID 即可。密码、私钥和导出的 `.p12` 都只留在本机，不放入聊天、源码或 GitHub。

这一步不签署应用、不上传安装包、不改变源码仓库可见性。后续还须验证嵌套工具与整个应用的签名、完成公证和票据、测试实际安装；自动更新的产物签名另行配置，不与 Apple 证书混用。
