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

新 0.1.4 模板 app/DMG、严格签名与内容校验、真实新包启动以及最终保护审计将在完成后补充。本段不将已保留旧包或样本公开构建称作新原生包。

本轮尚未完成摄像头的拒绝和释放、macOS 宿主授权撤销、Safari/Chrome，以及最低 macOS 15 实机等验收；不标记 M5 三端设备矩阵全部完成，也不改变 M6 未开始的状态。
