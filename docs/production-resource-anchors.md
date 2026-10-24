# 正式签后资源锚协议：第一步

本步骤只闭合摘要、文件系统规则和编译绑定。固定开发资源来源清单与开发准备入口保持原样。真实 Team ID、Node/Git 权限配置摘要尚未配置；正式资源 feature 默认不能构建。即使使用一次性合成策略验证编译注入，`releaseReady`、`signatureEvidenceVerified` 和 `sourceTransformationVerified` 仍为 false，生产运行继续拒绝。

## 输入和固定策略

`production-resource-anchors` 是依赖 `production-profile` 的编译 feature，CLI/桌面同名转发。构建脚本只从固定源码路径读取生产 profile、`macos-signing-policy.json` 和现有三份 Node/Git 来源清单。Team ID 为明确的 10 位大写字母/数字，Node/Git 权限配置分别绑定完整 SHA-256；未配置、未知 schema/profile 或空摘要均拒绝，不从账号、环境或包内元数据猜测。

两个环境输入仅用于编译：`PANDA_DRAWER_COMPILED_TOOL_MANIFEST` 指定完整清单的绝对普通文件路径；`PANDA_DRAWER_COMPILED_TOOL_ROOT` 指定工具资源根。它们不能选择运行身份或用户数据根。构建脚本校验并逐文件扫描后，将完整清单 bytes 与其 SHA-256 放入 OUT_DIR，再编译到只读二进制；绝对 stage 路径不进入清单。缺少、旧版、未知字段、错误来源/目标/策略、缺文件或字节不符均使该 feature 编译失败。清单及资源输入变化会使构建检查失效并重新执行。所有编译路径在输出 Cargo 指令前拒绝控制字符、非原生分隔反斜线、非 UTF-8 和越界路径，避免路径拼接注入；正常 Unicode 安装父目录仍允许。

普通 `production-profile` 仍支持离线 build-info；未启用签后锚 feature 时不接受任何外部清单。所有生产运行入口在用户数据、发现记录或凭据读取前停止。锚已编译时错误为 `PRODUCTION_DISTRIBUTION_PROOF_UNCONFIGURED`，准确区分摘要绑定与尚未完成的真实签名分发证明。

## 清单 schema 1

所有对象拒绝未知字段，重复 JSON 字段也拒绝。顶层字段固定为：

- `schemaVersion: 1`、`digestRuleVersion: 1`。
- `profile`：固定 profile 原始 bytes 的 `sha256`，以及 `version`、`identifier`、`environment`、实际 Cargo `target` 和 `minimumSystemVersion`；全部与源码 profile 一致。
- `signingProfile`：`macos-developer-id-hardened-runtime-v1`；`signingPolicySha256` 绑定固定策略原始 bytes；`expectedTeamId` 与策略逐字匹配。
- `sourceAnchors`：`nodeRuntime`、`developmentNode`、`developmentGit` 分别为三份现有源码发行清单原始 bytes 的 SHA-256。
- `payloads`：只允许固定的 `nodeRuntime`、`developmentNode`、`developmentGit` 三项；对应逻辑根只能为 `node-runtime`、`development-node`、`development-git`，没有 stage 绝对路径。

每个 payload 固定字段为 `root`、`rootMode`、`toolVersion`、`npmVersion`、`entryCount`、`fileCount`、`totalBytes`、`sha256` 和 `entries`。Node/npm/Git 版本与各自现有来源锚一致。目录也计入 entries，不允许空目录、缺失父目录或文件/目录混用。

entry 只有两种类型：

```json
{"type":"directory","path":"bin","mode":493}
{"type":"file","path":"bin/node","mode":493,"size":123,"sha256":"64位小写hex","signature":null,"source":null}
```

路径仅允许 ASCII 字母、数字和 `-._@+`，以 `/` 分隔，拒绝空段、`.`、`..`、反斜线、控制字符、非 ASCII 及大小写归并冲突。此保守规则只用于内置工具树，不限制用户应用文件或正文。组件最长 255 bytes、总路径最长 1024 bytes、最多 64 层目录；所有 entry 按原始路径字节严格升序且唯一。

目录模式只允许 0555/0755；文件模式严格匹配来源清单的可执行角色，允许对应的 0555/0755 或 0444/0644。JSON 中 mode 用十进制表示。拒绝符号链接、硬链接、多链接文件和其他文件类型，使用 no-follow 打开并复核打开前后及整个扫描结束时的文件/目录身份与修改状态。

每个 Mach-O 必须具有完整 `signature` 和 `source` 绑定。signature 固定包含 `teamId`、`identifier`、`hardenedRuntime:true`、`entitlementsSha256`；角色权限摘要与固定策略一致，identifier 固定派生为 `生产identifier.tools.逻辑根.SHA256(相对路径)`。source 包含签前声称的 `size` 与 `sha256`。非 Mach-O 不允许声称签名或替换来源记录。当前目标仅接受薄 ARM64 Mach-O，结构性检查 macOS load command，最低系统须明确且不高于 profile 的 15.0；这些结构检查不验证代码签名。

每棵树最多 30,000 项、256 MiB，单文件最多 180 MiB，三棵树合计最多 640 MiB；清单自身最多 8 MiB。清单限制独立于旧 development metadata 的 64 KiB 限制，旧 metadata 必须逐字节保持来源清单原样。

## 摘要规则 1

SHA-256 初始输入为 UTF-8 `signed-tools-tree-v1\0逻辑根\0rootMode十进制\n`。之后按路径升序，对每个 entry 序列化为紧凑 JSON，再追加 LF。字段顺序固定为：目录 `type,path,mode`；文件 `type,path,mode,size,sha256,signature,source`；signature `teamId,identifier,hardenedRuntime,entitlementsSha256`；source `size,sha256`。空 signature/source 序列化为 null。profile、来源和签名策略绑定另外由顶层逐字段核验，完整清单原始 bytes 也具有编译 SHA-256。跨语言固定向量见定向测试，改变规则必须提升 `digestRuleVersion`。

开发 Node/Git 的 source 声明回填到旧 v1 路径/大小/摘要记录后，须匹配旧整树摘要、总字节和文件数；未签文件用其实际签后值，旧 development metadata 保持原 bytes。运行 Node 的 source bin/node 记录须匹配官方固定摘要和大小，LICENSE 须匹配固定许可摘要。runtime.json 必须逐字节匹配原 `package-node-runtime.mjs` 的固定来源投影（字段顺序、两格缩进、结尾 LF 均保持），其 size/hash 一并核对；仍不从它读取替换的签后 hash。

这些检查证明来源声明与旧锚一致，并限制未签文件的变动，**不能单独证明新 Mach-O 是由旧字节重签而成**。任意新 Mach-O 仍可携带同一 source 声明；真正的转换关系必须由后续独占 stage 的来源校验和签名事务证明，或由可验证的签名前后 Mach-O 内容比较证明。清单中的 Team ID、权限摘要和 hardened-runtime 字段在此步骤也只是绑定值，不是 codesign 验证证据。

## 后续打包顺序

1. 以现有严格来源锚校验上游/准备资源，再独占复制到新私有 stage；记录输入与操作归属。
2. 按已确认 Team ID 和逐角色权限配置签署全部 Mach-O 叶子；实际读取并核验 codesign 的团队、identifier、hardened runtime 和权限值。
3. 在同一受控事务中核对签前来源与签后变化，只允许预期签名变化，再产生此协议的签后清单。
4. 编译 core/CLI/桌面，严格扫描 stage 并嵌入清单；单独生产输出，生成实际生产 Tauri 配置，防止复用开发 sidecar、缓存或资源。
5. 签 CLI、主程序和 app 外层；再次按编译锚逐字节复验内嵌工具，避免打包器或后续签名改变它们。主程序/CLI 自身 hash 不进入待编译工具清单，避免循环。
6. 完成公证 accepted、票据、系统分发检查和实际安装验收；经单独实现的运行准入规则确认后才可开启生产运行与真实发行就绪状态。

本步骤不实现上述完整事务，不创建证书、正式密钥、安装包或 GitHub Release，不把 Tauri 命令成功当作已签名公证。

路径式祖先链前后复核加叶子 no-follow 不能单独证明不存在瞬时祖先替换期间的越界读取。当前校验只用于显式资源路径，且生产运行保持拒绝；后续签名事务必须使用真正独占的私有 stage 和受控目录持有，必要时采用 FD-relative 父目录链，不能把本步骤当作完整抗替换证明。

配置、schema 与编译输入路径检查保持跨平台；Windows 原生绝对路径的分隔符允许使用。Unix 文件身份/no-follow 校验独立于可移植模块，仅在 Unix 宿主编译；非 Unix 宿主明确拒绝正式资源 feature 和其文件验证，默认开发构建不加载 Unix 接口。此兼容边界不表示 Windows 运行、安装或发行已经验收。

## 实际验证（2026-10-25）

独立一次性源码中的默认开发／基础 production 检查共 99 次测试执行通过，另选定编译锚 feature 的合成策略检查共 25 次通过；这些跨 profile 的重复执行不按独立测试数计算。三模式的 core／CLI 严格 Clippy、桌面编译检查和格式检查通过。10 个实际构建拒绝场景覆盖缺少清单、未配置策略、不安全 Cargo 输入路径、旧 schema、错误目标／来源、未知 payload、缺失或额外文件，以及恢复真实 null 策略后再次拒绝。合成编译锚还核对资源变更拒绝和生产运行仍拒绝；没有执行或重新签署复制的工具。另 9 次非 Unix 分支模拟通过，在 Mac 上编译实际可移植协议／buildscript／runtime 模块且不加载 Unix 文件模块；没有 Windows 宿主或目标验证。

集成到当前开发分支后，资源协议 16 项通过、1 项显式工具复制试验按要求忽略；离线开发 CLI 元数据 1 项、基础 production profile 4 项及 CLI 2 项通过，全工作区全目标严格 Clippy 与格式检查通过。固定策略三个待配置值仍为 null，现有三份开发来源清单和准备脚本未变。本步骤没有创建或读取账号秘密，没有正式签名、公证、原生窗口、安装包、GitHub Release 或自动升级验收。
