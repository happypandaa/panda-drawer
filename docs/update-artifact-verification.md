# Mac 更新产物：离线实际字节验签

本步把真实产物密码学验签接到已有[清单工具](update-manifest-protocol.md)，不新增运行时服务或另一套清单格式。`generate`/`validate` 继续只处理格式；新增 `verify` 用明确指定的公开公钥文件，验证磁盘上实际读取的压缩包字节及签名内完整 trusted comment。没有下载、解压、执行、安装或重启，也不连接运行中的应用和父/helper交接。

```text
node scripts/update-manifest.mjs verify --input /绝对路径/latest.json --artifact /绝对路径/熊猫小抽屉.app.tar.gz --public-key /绝对路径/operator.pub --current-version 0.1.0
npm run test:updates
```

`--input` 复用现有有界严格 JSON、固定发行仓库/arm64目标、SemVer 与签名格式校验；不接受 URL、身份、目标或输出位置覆盖。`--artifact` 的 basename 必须同时匹配清单 URL 和签名中的文件名。版本必须存在于签名的 trusted comment，并匹配清单版本；旧版本和相同版本的 assessment 不会成为更新结果，也不会获得安装许可。版本说明和发布日期仍不是签名认证的数据。

## 实际密码学验证

公钥采用 Tauri 的 Base64 编码 minisign 公钥文本，内含42字节包（算法2字节、key ID 8字节、Ed25519公钥32字节）。签名采用已有工具要求的 Base64 编码四行 minisign 文本。严格核对 canonical Base64、算法原始字节、包长度和 key ID，不把 ASCII 高位别名或不匹配的 ID 当有效值。

使用 Node 内置 OpenSSL Ed25519 验证，不自行实现椭圆曲线。实际产物签名只接受锁定 CLI 所使用的 `ED` 模式：对产物流式计算 BLAKE2b-512，再验证该64字节预哈希的 Ed25519 签名。另一份 Ed25519 全局签名必须覆盖 `signature64 || 完整trusted-comment UTF-8`，不包括文本前缀 `trusted comment: `。通过这个检查后，文件名与版本才得到认证；仅有符合格式的 `version:` 文本不能阻止旧包配新版本。模式匹配不能证明生成工具的来源。

旧 `Ed` 原文签名明确拒绝，不回退兼容。算法标签本身不在全局签名覆盖范围内：若同时接受两种消息域，合法 ED 签名可被改标签为 Ed，并把产物换成原文件的64字节摘要，仍通过签名检查。本轮在私有人工样本实际复现了这个替换，再加入明确拒绝的回归。本项目尚无已发布旧格式需要兼容；纯格式 validator 仍可识别 Ed，不表示实际验证器接受。公钥包中的 `Ed`/`ED` 算法字节继续按 Minisign 公钥格式识别，不能与产物签名模式混淆。

互操作依据是[锁定 Tauri CLI 2.12.3 的签名实现](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.3/crates/tauri-cli/src/helpers/updater_signature.rs)与 updater 使用的[Minisign 验证约定](https://github.com/jedisct1/rust-minisign-verify/blob/master/src/lib.rs)。[Tauri updater 验证路径](https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/updater.rs)也先验证字节和全局签名，再使用 signed version；本步固定要求该版本存在，不提供关闭选项。本工具拒绝旧 Ed 比该通用 updater 当前允许 legacy 的路径更严格，不宣称框架默认已拒绝它。

## 输入与资源预算

清单继续最多64 KiB；公开公钥文件最多8 KiB。`ED` 沿用现有压缩产物1 GiB上限，以64 KiB块同时计算预哈希和实际 SHA-256，不把整个产物载入内存。不会自动降级成 Ed、只核对 SHA-256 或签名格式。

所有输入要求绝对路径、普通单链接文件；产物和公钥还必须非空。拒绝符号链接、路径中的父级跳转及符号链接父目录。清单、产物和公钥均以 NOFOLLOW/NONBLOCK 打开，核对打开前的命名 inode 和实际描述符，并在有界读取前后比较 BigInt 身份、大小、权限、链接数和纳秒修改/变更时间，最后再次核对命名 inode。打开前被替换为 FIFO 等特殊文件不能造成阻塞或成为有效输入。文件读取结束后仍可能被其他程序修改；结果不是持有该文件的稳定安装句柄，未来下载缓存和安装路径必须重新核对实际使用的相同字节。

## 公钥信任与结果范围

明确指定的公钥只代表操作者本次选择的信任输入，不能从远端清单获得，也不等同于将来编译绑定的生产信任根。报告可以给出 `cryptographicSignatureVerified:true`、`artifactBytesVerified:true`、实际压缩字节数/SHA-256及公钥指纹，但始终为 `productionTrustBound:false`、`packageIdentityVerified:false`、`runtimeAdmission:false`、`installAuthorized:false`。公钥通过不证明 `.app` Bundle ID、Developer ID、公证、内部资源、解压安全或最低系统兼容。

未读取或配置生产私钥、账号、证书、钥匙串或正式更新公钥；不生成可发布清单或正式包。测试仅使用私有临时目录和人工 Ed25519 钥/数据，其中锁定的实际 Tauri CLI 生成测试钥并签人工文件，输出被捕获丢弃，不将私钥写入日志或交付补丁。独立检查结果另记，不把测试签名当生产发行证据。

2026-10-25 独立源码副本通过：实际验签13项、原清单18项与新参数1项，共32项，0失败/忽略；另生产预检、既有包输出及环境42项通过，两个实现脚本语法检查通过。互操作使用项目锁定的 Tauri CLI **2.12.3** 实际生成临时测试钥、签带版本的人工文件，再由真实新命令验证；也执行新 CLI 子进程的成功和篡改失败。清单测试用受控的文件系统调用边界，在真实 open 前替换成 FIFO/另一个普通文件，以及真实 open 后换名，确认拒绝并保留原件；产物读中只测试了8 MiB人工文件的覆盖写入，未把它记作产物读中 rename 验收。上述测试不检查真实 gzip/tar 归档结构或正式 `.app` 内容。

主工作区集成后另实际复跑上述32项，全部通过、0失败/取消/忽略。测试使用临时人工密钥，没有配置正式信任根或接入运行中的平台。

后续最小联网步骤可以复用此入口核验固定源下载的实际暂存字节，但须先补齐 HTTPS 重定向、超时、大小/磁盘预算、取消和失败暂存保留策略。生产公钥绑定/轮换仍待后续实施；正式签名配置由用户自行处理。自动检查门禁和生产运行门禁保持拒绝。[父/helper交接](macos-update-handoff.md)继续单独验证，不由此处密码学成功直接接安装。
