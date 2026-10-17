# M5 权限拒绝、设备释放与断网恢复实测

日期：2026-10-17（Asia/Shanghai）。接续 [浏览器实测续记](m5-browser-real-verification.md)，使用签名开发包的真实原生 WKWebView；系统为 macOS `27.0.1 (26A434)`、arm64。没有通过其他工具绕过普通浏览器 URL 访问限制，也没有用合成 offline、虚拟设备或模拟权限替代本记录中的实际操作。Safari/Chrome 及摄像头本轮未完成项仍单独保留。

## 样本与保护基线

只使用已有可丢弃样本「M5 可丢弃 · 浏览器真实验收 · 20261017」，ID `prj_508739f6cc624a16a477b534cdcb8dba`。固定开发身份、数据根及 `workspaces/current` 未变，没有迁移或清理。原项目“打僵尸”没有源码、业务数据或维护变更。

开始先复核上轮保护证据，再建立本轮 14 个保护范围：两个项目共 4513 个普通文件、236750383 字节，48 个备份文件、7382412 字节，5 个可恢复回收项，以及 402 个历史 final 任务根和全部恢复、迁移、清理证据。上轮样本三行业务数据、三份附件及 SDK 也纳入基线。内容 SHA-256、类型、权限和链接目标逐项检查，不重建基线来掩盖变化；本轮明确的样本源码、构建产物、维护状态及新测试文本另做精确来源审计。

私有证据位于忽略目录 `.test-artifacts/m5-browser-resilience-real-2026-10-17/`（0700），不提交设备素材、会话资料、生成包或本地测试证据。

## 实际权限拒绝与摄像头不可用

在样本点击开始录音，实际出现 `Allow “127.0.0.1” to use your microphone?`，选择 `Don’t Allow`。页面显示“未获得设备权限……或选择已有文件”，开始按钮恢复、停止按钮禁用；没有生成素材或保存记录。再次明确点击开始录音，真实许可框再次出现，没有自动申请或重试。证据 `01`—`04` 的 AX 记录。此结果是 WKWebView 来源许可拒绝，不扩张为 macOS 宿主权限撤销或其他浏览器的拒绝结果。

摄像头连续在旧页、新窗口和明确重载后，于许可框前返回 `Invalid constraint`。样本仅增加原有 action catch 的白名单错误观察，真实得到 `name=OverconstrainedError`，未提供具体约束字段。独立只读 AVFoundation 观察同时报告有连接摄像头、`isSuspended=true`；这不能证明是某个约束、宿主权限或硬件原因，也没有据此调整系统隐私设置。

模板 0.1.4 为这个错误增加中文提示：“当前设备无法满足采集要求，也可能暂时不可用。请检查设备，或选择已有文件。”不改变相机约束，不自动降约束、重新申请权限或重试。18 项助手模拟回归、16 项组件回归、隔离 TypeScript/Vite 构建及独立审查通过；固定 SDK 0.1.0 未变。源码修复已提交推送 `cd9b951`。

仅向样本显式复制这个已审查助手补丁，公开构建后明确重载，在实际 WKWebView 再次得到同一真实错误名和上述中文提示，见 `10-camera-raw-error-ax.txt`、`19-camera-fixed-Chinese-feedback-ax.txt`。不能把友好提示修复记为摄像头恢复可用；本轮摄像头拒绝、真实采集及释放仍未通过。

## 真实轨道及独立系统观察

样本专用被动诊断面板仅读取现有媒体组件的真实 `video.srcObject` 和轨道属性，保留真实轨道引用以观察移除后的 `readyState`，记录原生可见性事件及白名单运行状态。没有替换浏览器 API、自行请求或停止设备、读取设备 ID/名称、保存或传送媒体。通过公开构建 `task_9c94c46ca92646ddb8894ca0b65796b8` 发布；诊断及安全错误事件不进入平台模板。

另用本机 SDK 编译只读 CoreAudio/AVFoundation 观察器，不申请设备权限、不创建采集 session 或读取媒体帧。CoreAudio 查询默认输入设备的 `DeviceIsRunningSomewhere`，相机查询连接设备的占用和 suspended 聚合值。它们是独立系统属性，不能归因某个项目或排除其他应用的影响；轨道 ended 也不能单独证明所有系统或物理硬件已释放。

| 实际分支 | WKWebView 结果 | 独立系统观察 |
| --- | --- | --- |
| 主动停止麦克风 | 真实 audio 轨道 live → ended，移除 srcObject，停止禁用，保留未保存预览 | 默认输入运行值在基线为 false、录音时 true、停止后 false |
| 最小化项目窗口 | 收到 `isTrusted=true`、visibility=hidden 的事件，同一真实轨道 ended；恢复窗口仍停止，没有自动录制 | 最小化后默认输入运行值 false |
| 录音时进入维护 | 实际平台事件进入 maintenance/busy 时轨道 ended、采集与保存禁用；维护结束仍要求明确重载 | 进入维护后默认输入运行值 false |
| 许可等待时进入维护，再处理迟到许可 | maintenance 事件早于许可处理，实际返回 AbortError，显示“操作已停止，未保存”，没有进入录制或产生预览 | 迟到许可处理后默认输入运行值 false；不声称测量了可能的瞬时设备占用 |

三段实际临时录音分别约 33.4、9.5、16.0 秒，均在 60 秒内部保护范围内；没有把计划的短测试写成固定 2 秒。这些录音只在页面内存，未点击保存、下载或上传，随后明确清除或重载；本轮未新增照片。证据为 `11`—`18`、`25`—`27` 的 AX、对应 `device-*.json` 和两轮维护任务。

第一轮维护为 `task_78b269312cb74404956f12ad3b49a8a9` / `maintenance_fc2d10f1198f4b879c1a10cc256f5fe9`，基线 `backup_5d38b257641f48c0b5a59161b2d36781`。维护内公开构建 `task_0e750271b4f34987a9ab2eca97193d61` 发布上述显式助手补丁，结束任务 `task_ab34467efd4a4133a3a0e0b7f518dc58` 成功。

第二轮在真实许可框等待时进入维护，任务 `task_a237703d04bf485196038f36c8f04325` / `maintenance_6c51a84bf14842e1b6ddfa623e2ed622`，基线 `backup_ad8a145594b040e08183510f9b003024`，结束任务 `task_2ba6c078814645a3bcdff220f37c903e` 成功。两份基线和操作证据均保留；没有恢复旧快照或重放失败操作。

## 真实外网中断及恢复

先试短暂关闭 Wi-Fi 电源并自动恢复，但这次未观察到公网失败，不计为离线失败分支通过。后续改为临时禁用原有 Wi-Fi 网络服务，保留原配置并在 finally 自动恢复；系统连续报告服务 Disabled、接口 inactive、无可达 IPv4，恢复后为 Enabled、接口 active、IPv4 Reachable。没有修改代理、DNS、凭据、防火墙或安全策略。

为避免控制工具在断网时无法下发新命令，先在本机部署 CUA 动作，等私有系统快照确认实际断网后才操作现有项目窗口。真实断网区间内，固定公开 httpbin GET 显示请求失败，同时本地 SQL 与当前会话附件接口仍读出三份原素材，JPEG 解码为 640 × 480。独立 CLI SELECT 同时成功，额外公开 HTTP 探测为 ENOTFOUND；后者只作网络旁证，不代替 GUI。证据 `network-service-coordinated-cycle.json`、`21-coordinated-physical-offline-gui-ax.txt`，GUI 记录时刻确在无路由的系统快照区间内。

本机 WKWebView 在该次外网中断期间仍报告 `navigator.onLine=true`，因此没有把这个提示当作互联网可达证明。平台事件 offline=false 表示本地 loopback 状态检查成功，与外网断开不矛盾；本轮没有合成 offline 事件。恢复外网后，公开请求仍保留失败提示，没有自动重试；手动点击后返回 HTTP 200、type=cors、429 字符可解析 JSON，见 `22`—`24` AX。

为验证离线本地写入，原生文件选择器选取本轮专用 `m5-offline-local-save-20261017.txt`（text/plain，138 字节，SHA-256 `7af487ef87382d8e8c3b50f0ad5f8be2eecd95446efb9a544ed278900c9a6d8c`）。首次保存虽成功，但时间复核发现已晚于自动网络恢复，明确归为在线预检；该记录保留，不冒充离线成功，也不盲目重放未知写入。随后明确重新选择同一源文件，使用在动作前再次检查当前离线时序的流程，重新点击一次保存。

严格离线保存的系统无路由快照为 16:38:46.324 和 16:38:56.391（UTC+8）；新行创建于 16:38:46.578，真实 GUI 在 16:38:47.159 显示“附件和业务记录均已保存”，完整落在该离线区间内。新 UUID 为 `1836ebd1-5396-4b20-9982-76bd3b1d3b04`，新附件长度、内容摘要、路径、MIME 与 SQL 行完全一致，见 `31`—`32` AX、`offline-save-receipt.json` 和 `network-offline-save-strict-cycle.json`。在线预检 UUID 为 `0abc7433-937d-4503-95bf-97bf31c2b32a`，另有独立 `online-save-precheck-receipt.json` 保留准确标签。

恢复后保存按钮保持禁用，独立 SELECT 恰有五行：三份原素材逐字段和文件摘要不变，仅上述两个已明确写入的 138 字节测试文本新增。没有自动重复保存、清理或删除测试证据，见 `33-restored-save-no-replay-ax.txt`。本轮未注入上传响应丢失或 SQL 应答丢失，也不把维护拒绝等价成这些失败矩阵。

## 打包及最终复核

打包前 `before-package-protected.json` 和来源审计通过：14 个保护范围中原用户数据、48 个原备份、402 个历史任务、原三份素材与全部回收/恢复证据不变。53 个样本精确路径变化逐项登记前后摘要和原因；两个公开构建的输入、产物及保留前代链均独立复算一致，没有整体豁免样本目录。新增 10 个成功任务仅属于样本（2 build、4 storageScan、2 maintenanceBegin、2 maintenanceEnd），两份新基线合计 1115276 字节，当前共 50 个备份文件、8497688 字节。五行资料严格等于原三行加两个精确文本回执，样本 ready、busy/recoveryRequired 为 false、activeTask 为空。证据另见 `before-package-protection-summary.json`、`before-package-build-artifact-provenance.json`、`exact-two-text-material-provenance.json`。

上轮 7b app/DMG 已完整保留到本轮私有 `prior-package/`，含根 55 个 app 条目的类型、权限、长度、摘要及链接目标与源一致。旧 DMG 为 8283876 字节、SHA-256 `7b72754295eeaa3401b57f0d4a537d9986efbb66a73201c7d02f5ccd3bca6680`；更早的 c08 保留包也再次通过原清单核对。

实测记录分步提交推送为 `8cb525a`。旧进程 PID `57930` 通过 CUA 原生 `⌘Q` 正常退出，随后 AX 为 procNotFound，独立核对旧 PID 不存在、CLI 为 `SERVICE_NOT_RUNNING`，见 `new-package-native-quit.json`。没有用强制终止代替正常退出。

`npm run build` 退出码 0，生成本轮新 app 和 DMG，日志 `build-dmg.log`。包验证首个命令因 `--output` 使用相对路径被参数检查拒绝，尚未执行校验；改为要求的绝对路径后，`package-verification/verification.json` 为 passed，两次命令日志均保留，没有更改检查门槛。

| 项目 | 本轮交付结果 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 长度 / SHA-256 | `8284493` 字节 / `95b525bd3477a0cd676b664f551137feb3e09e48e4df439db02b360d9f1ca491` |
| 身份 / 版本 / 架构 | `dev.localprojects.platform.m0` / `0.0.0` development / arm64 |
| 签名与用途 | 源 app 与 DMG app 严格深度签名通过，内部 ad-hoc、runtime；相机/麦克风用途及 camera/audio-input 权利保留 |
| 模板 / SDK | `react-ts-vite-0.1.4` / 固定 SDK `0.1.0`，8 处 SDK 副本摘要仍为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9` |
| DMG / 资源 | `hdiutil verify` 通过，54 个包内条目与源 app 完全一致，只读挂载已卸载；包内资源 README 与本次打包的 `docs/development.md` 一致 |
| 最低系统 | 配置及 Mach-O 为 macOS 15.0；本轮实际仍是上述 macOS 27.0.1，没有冒充最低版本实机通过 |

从准确新 app 路径通过 CUA 实际启动，PID `67121`、instanceId `e845bf1c-b057-466e-b447-0d96a687f169`，均不同于旧进程/实例。`new-package-restart.json` 核对安装路径、固定工作区、模板 0.1.4、SDK 摘要、2 个项目和 5 个可恢复回收项，样本五行业务数据和五份附件摘要与退出前完全一致，ready、busy/recoveryRequired 为 false、无 activeTask；样本已实测助手与包内模板助手逐字节一致。

新包原生管理页实际显示原项目和样本，进入样本后明确点击读取，显示 5 份持久资料并解码原 JPEG 为 640 × 480，见 `34-new-package-manager-ax.txt`、`35-new-package-five-persisted-materials-ax.txt`。新宿主的麦克风复测仅停在“等待设备许可”，未观察到可操作的许可框；明确点击停止后采集按钮恢复、停止禁用，随后关闭该页面，没有把它记为新宿主拒绝或采集通过。重新打开干净样本页，相机仍给出修复后的中文不可用提示，见 `36`—`38` AX；本阶段没有新增录音、照片或保存记录。

本轮前述麦克风实际拒绝/采集/释放和物理断网 GUI 验证运行于已保留的 7b 宿主，最后五素材读取、取消许可等待和中文相机提示运行于本次 95 新宿主。两阶段分开记录；新宿主完整设备授权与采集复测仍需补齐。

最终 `final-protected-checkpoint-reviewed.json` 为 passed，pending 和 forbidden 均为空，`final-independent-data-package-audit.json` 独立总审计也通过。14 个范围继续保护原用户项目、全部 48 个既有备份、402 个历史任务、原三份素材及全部回收/恢复证据；只对已审查的样本源码、成功构建产物、维护状态和两个精确文本逐项登记差异，没有重建原基线。关闭/重启后的样本 SQLite checkpoint 改变主库摘要并使原 WAL/SHM 运行文件消失，首次审计正确要求复核；在只读副本中独立比较结构、完整性及原三行加两个回执恰好五行后，按实际前后状态精确登记。早先 review-required 报告保留，没有重建、替换或手工删除业务数据库与运行文件来消除差异。

全轮新增 12 个 final 任务全部成功：6 storageScan、2 build、2 maintenanceBegin、2 maintenanceEnd。重启后新增扫描 `task_6afdbbc2100343d4a6f25d92063062e6` 属于样本；`task_61e2783830514e7b83c0901afda71d63` 属于原项目“打僵尸”，已按精确 ID、owner、storageScan 类型及结果范围单项审计，仅为只读管理扫描，未默许任何原项目写操作。两份新基线及 snapshot 与打包前相同，当前仍共 50 个备份文件、8497688 字节，样本 ready、无 activeTask。原用户项目内容、原有历史证据及 5 个可恢复回收项不变。

最终另核对新 DMG 长度/摘要、固定新实例、SDK 和样本助手与包内模板的一致性，并再次复核 7b、c08 两份保留 app 各 55 个条目及 DMG 均与原清单一致。系统设备结论只引用先前 16:51:45（UTC+8）的匿名只读快照：默认输入未运行、摄像头 suspended；没有在锁屏期间重新操作设备或把此快照当作摄像头释放通过。

随后进一步通过 CUA 只读打开系统许可入口时，工具明确报告 Mac 已锁定、自动解锁失败，要求手动解锁；已向用户发出解锁请求并暂停 GUI，没有用 CLI 或其他 UI 工具绕过。没有据此反推此前许可等待的具体原因，也没有将尚未完成的系统许可检查写成通过。新宿主许可问题须在用户解锁后继续核对。

本轮尚未完成摄像头的拒绝和释放、macOS 宿主授权撤销、Safari/Chrome，以及最低 macOS 15 实机等验收；不标记 M5 三端设备矩阵全部完成，也不改变 M6 未开始的状态。
