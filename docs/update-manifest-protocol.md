# Mac 自动更新清单：离线协议与生成工具

本步提供可运行的离线生成和校验工具，不接入运行中的平台，不联网、签名、发布、安装或重启。开发身份和数据根不变；`automaticCheckPolicy` 对开发、未知和当前生产候选都返回关闭，调用方 JSON 不能开放运行门禁。

格式依据 [Tauri 2 官方静态更新协议](https://v2.tauri.app/plugin/updater/)。本项目使用其固定子集：`version`、`notes`、`pub_date`、`platforms.darwin-aarch64.url/signature`，逐个拒绝缺失和未知字段；不把大小、摘要或应用身份伪装成插件会验证的标准字段。首次安装 DMG 与更新用的 `.app.tar.gz` 分开。

固定生产身份为 `com.happypandaa.pandadrawer`，profile 为 `production`，唯一仓库为 `happypandaa/panda-drawer-releases`。资产地址由仓库、`v<完整版本>` 和普通 `.app.tar.gz` 文件名生成；不接受调用方 URL、身份、目标或来源路径覆盖。校验拒绝 HTTP、其他源、滚动下载地址、查询参数、凭据、片段、路径穿越和不同版本 tag。清单发现地址固定为该仓库 `releases/latest/download/latest.json`，本步不访问它。

## 输入与命令

生成器消费前序发行事务提供的单个 JSON，固定字段如下：

```json
{
  "schemaVersion": 1,
  "profile": "production",
  "identifier": "com.happypandaa.pandadrawer",
  "target": "darwin-aarch64",
  "releaseRepository": "https://github.com/happypandaa/panda-drawer-releases",
  "version": "0.1.0",
  "tag": "v0.1.0",
  "notes": "本次变化与已知限制",
  "pub_date": "2026-10-25T00:00:00Z",
  "artifact": {
    "name": "熊猫小抽屉.app.tar.gz",
    "bytes": 123,
    "sha256": "产物实际的64位小写SHA-256",
    "signature": "由签名步骤提供的完整.sig文件内容"
  }
}
```

以上摘要和签名是说明占位，不能通过校验；没有随工程提供可发布的假签名或密钥。字段 `bytes/sha256` 仅校验元数据形状，不证明磁盘中的真实产物。前序事务仍须独立绑定实际字节、Developer ID、公证、包身份和编译资源锚。

```text
node scripts/update-manifest.mjs generate --input /绝对路径/signed-artifact-metadata.json --output /新文件/latest.json
node scripts/update-manifest.mjs generate --input /绝对路径/signed-artifact-metadata.json --output /新文件/latest.json --previous-version 0.1.0
node scripts/update-manifest.mjs validate --input /绝对路径/latest.json --current-version 0.1.0
node --test scripts/update-manifest-test.mjs
```

输入至多 64 KiB，拒绝重复 JSON 键、错误 UTF-8、深层输入和符号链接；输出独占创建，不覆盖已有文件。说明至多 16 KiB，签名内容至多 8 KiB，声明的压缩产物大小须为 1 字节至 1 GiB。它们是本协议预算，不是实际下载流量、解压大小或运行内存的保证；不把清单验证通过等同于已验证产物。

## 版本与签名边界

使用 SemVer 优先级，数字逐段比较，构建后缀不提高版本；稳定版判断跳过预发行，旧版本和相同优先级不给更新结果。生成稳定版要求无 `v` 前缀、无构建后缀、至少 0.1.0；传入上版号时必须严格更新。只读清单校验可接受官方支持的单个 `v` 前缀和合法预发行，以便准确报告不适用于稳定更新。日期必须带时区且日历合法，当前拒绝闰秒等不能用普通 Date 验证的表示。

[锁定的 Tauri CLI 2.12.3 签名实现](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.3/crates/tauri-cli/src/helpers/updater_signature.rs)把 minisign 文本整体编码为 Base64，并在 trusted comment 中记录文件名和版本。工具检查四行文本、算法及签名长度、文件名与版本绑定，拒绝把路径或 URL 作为签名。

**以上仅为签名格式检查，尚无密码学验签。** 结果始终声明 `cryptographicSignatureVerified:false`、`artifactBytesVerified:false`、`installAuthorized:false`。trusted comment 在完成密码学验证前也是未认证输入；单靠 tag 和文本不能证明发行身份或阻止恶意重放。后续运行实现须用编译绑定的公钥、实际产物和 `requireSignedVersion` 完成验证，依据 [Tauri updater 的版本验证代码](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/updater.rs)，再独立满足关闭、安装和恢复条件。

验证只使用临时目录、人工格式字节和本地 CLI；不生成密钥，不声称人工签名能验证，不发布 `latest.json`，不扩大正式发行或原生升级的完成范围。
