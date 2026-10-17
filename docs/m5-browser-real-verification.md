# M5 浏览器能力真实验证续记

日期：2026-10-17（Asia/Shanghai）。本记录补充 [M5 原验收](m5-verification.md)，仅将实际操作通过的原生 WKWebView 项目窗口流程记为通过。Safari、Chrome、拒绝权限、独立系统设备释放测量及物理断网仍未完成；此前普通浏览器的工具 URL 访问限制没有通过其他工具绕过。不能据此将 M5 三端浏览器验收标成全部完成。

本机实际系统为 macOS `27.0.1 (26A434)`、arm64；本轮不重用最低 macOS 15 的配置检查充当实机验收，也没有获得 Safari/Chrome 的新版本运行记录。

## 样本与原数据保护

固定身份为 `dev.localprojects.platform.m0`，工作区仍为固定开发数据根下 `workspaces/current`。通过原生管理界面新建唯一样本「M5 可丢弃 · 浏览器真实验收 · 20261017」，ID 为 `prj_508739f6cc624a16a477b534cdcb8dba`，只对该样本写入测试资料。

创建前建立 14 个只读保护范围：原有项目全树、全部备份、全部现存 trash、设置、维护与项目状态、操作和任务、迁移、清理回执及诊断证据。原项目全树为 3922 个普通文件、170651946 字节；全部备份为 46 个普通文件、6692404 字节，5 个既有回收项可恢复。记录内容 SHA-256、路径类型、权限及链接目标，不跟随链接。运行发现、令牌、ticket、缓存和有期限的清理计划不纳入不变性断言。

私有证据在忽略目录 `.test-artifacts/m5-browser-real-2026-10-17/`（0700），不提交测试数据、会话材料或生成包。`initial-protected-check.json` 和 `browser-file-phase-protected.json` 均为 14/14 通过。文件阶段新增只属于该样本：项目、对应 project-state 及两个成功的 storageScan 任务；未修改原有项目、46 个备份、5 个保留回收项及历史操作证据。

## 已完成的真实文件流程

使用当前严格签名核验通过的开发包及其原生项目窗口，实际 URL 为服务签发的该样本项目入口，未使用独立 Vite 页面、HTTP 测试或 jsdom 代替 GUI。

- 初始页面显示本地已连接、暂无业务数据表。展开媒体与文件组件，录音及摄像头按钮均禁用，说明设备未声明；没有申请系统设备权限。
- 从实际 macOS 文件选择框选取本轮专用 `m5-browser-note.txt`（110 字节，含中文）。页面显示文件名、`text/plain` 和准确大小；只读 schema 检查确认选择和下载前后仍无业务表。
- 点击“下载本地副本”实际弹出“保存项目下载”原生保存框。取消后预览和文字草稿保留，只删除平台私有下载暂存，未生成业务附件。
- 再次下载并明确保存为测试目录中的 `m5-native-saved-copy.txt`。独立读取确认内容与 110 字节原文件完全一致，`runtime/native-downloads` 已空。
- 手动粘贴包含中文的测试文字、点击“复制文字”，界面显示已复制。随后清空输入，再用原生 `⌘V` 粘贴，准确恢复测试文字；没有静默读取用户剪贴板来验证。
- 点击“保存素材示例”后，界面才显示“附件和业务记录均已保存”。只读 SQL 核对 `media_samples` 恰有 1 行，附件路径、MIME、原文件名和 110 字节大小一致；附件内容与原文件完全一致。保存按钮禁用，未重复写入。

证据为 `01-initial-project`、`02-file-picked`、`03-native-save-dialog`、`04-clipboard-roundtrip`、`05-file-and-record-saved` 的 PNG/AX，以及 `fixture.json`、`native-download-before-cancel.json`、`native-download-cancel.json`、`file-roundtrip.json`。只读来源审计另见 `browser-file-phase-provenance.json`。

这些文件流程结果尚不覆盖文件拖入、复制拒绝降级或受保护保存路径，不使用已有模拟测试结论冒充实测。

## 实际维护门槛问题及修复

只对样本进入维护，任务 `task_c54535ff6e2f4a8db0d4ac2b5aa8aad1` 成功，维护 ID `maintenance_2ef8ce13e2c24fb1bf91bbcdd52a2888`、基线备份 `backup_a1c98ea476444a43ae8a9b5227fb71c2` 均保留。真实页面显示维护提示，文字草稿仍在；重新选择本轮 PNG 后，“保存素材示例”仍可点击，见 `06-maintenance-before-fix` 和 `07-maintenance-save-enabled-before-fix`。

实际点击后，服务正确拒绝上传，界面显示“附件写入未得到成功确认，未请求写入记录”，保留失败路径且不自动重试，见 `08-maintenance-upload-failure`。只读核对该 PNG 路径不存在、已有业务行仍只有 1 行，证据 `maintenance-upload-failure-check.json`。这不是数据保护失效，但按钮反馈与实际运行门槛不一致。

修复后的模板 0.1.3 同步 `platform:state` 到渲染状态和即时 ref：采集与保存共用门槛，迟到 context 不重新启用；保存起点及上传完成后写 SQL 前再次核对；同轮重复点击也只接受一次保存。暂停时保留选择、预览、下载及手动复制，文件选择提示不再误称设备已释放。固定 SDK 0.1.0、JS 媒体助手和 Rust 未变；只新项目使用新模板，原有项目不注入源码。

新增 `npm run test:media-panel` 的 16 项真实 React/jsdom 组件回归、原有 15 项助手模拟及隔离模板 TypeScript/Vite 构建通过。独立协作审查另行运行组件回归及 strict TypeScript 检查通过。它们覆盖运行门槛、事件与渲染竞态、迟到 context、恢复但不重放、上传与 SQL 间中断和重复点击，不能代替修复后的 WKWebView 复测。源码修复阶段尚未将该复测或新的 app/DMG 记为通过。

## 本轮实际设备与回放

在样本维护期间仅修改该样本的摄像头和麦克风具体用途声明，外网声明保持为空，并将用途和保留规则写入该样本 README。结束维护任务 `task_f8ebbec56042468bb8b43d34cbe33049` 成功。旧页显示运行需求变化及明确重载入口；两步确认取消后仍保留 PNG 草稿和文字，见 `09-policy-cancel-preserves-draft`。随后明确确认重载，新页显示两项声明用途、启用设备按钮，没有自动申请设备。

用户明确允许仅在该样本中启用麦克风和摄像头，素材仅留本机。实际出现 WKWebView 的麦克风和摄像头 Allow / Don’t Allow 提示，许可后调用真实浏览器采集 API，没有注入虚拟 getUserMedia 或模拟 MediaRecorder。

- 录音停止后得到 `audio/webm; codecs=opus`、373852 字节、15120 毫秒；计划约 2 秒，本轮操作中停止延后，实际为约 15.1 秒，已向用户说明。未独立测量延后的具体原因。文件以 `.webm` 保存，没有假定 MP4 或改写扩展名。
- 原生音频控件点击播放后从 0 秒推进至结尾 14 秒，见 `10-real-recording-stopped` 和 `11-real-recording-playback`。这是浏览器实际解码和播放进度，不代表测量了扬声器物理输出。
- 摄像头许可后出现本地预览与拍摄按钮；仅点击拍下一张照片，预览转为 JPEG，停止采集按钮禁用；点击保存得到 `.jpg` 附件和业务行。为减少额外保存设备画面，照片过程仅记录 AX，见 `12-real-photo-stopped-ax.txt`、`13-real-photo-saved-ax.txt`，没有将照片放入仓库或交付文档。

`real-capture-files.json` 核对总共 3 行：原文本、录音和一张照片；每行附件长度、路径、MIME 与实际文件一致，录音具有 WebM/EBML 文件头，37595 字节照片具有完整 JPEG 起止标记，另存 SHA-256。设备仅在明确点击后申请，停止/拍摄后界面退出采集模式；尚未独立测量系统设备占用或验证维护、隐藏、拒绝权限的硬件分支，不把界面提示或模拟 track.stop 断言扩张为这些结论。

## 修复后的真实窗口及网络补验

仅向该样本显式复制已经审查的 MediaToolsPanel 修复，固定 SDK 摘要保持不变；另加该样本专用的只读验收面板，不进入平台或新项目模板。它只在点击后查询现有业务行、取得当前会话持久附件 URL 或执行两个固定公开 GET，不采集设备、上传或建表。样本 README 和联网声明同步说明发送范围；仅声明 `network.connect` 的 `https://httpbin.org`，设备素材不传往公网。

首个样本构建 `task_ce7d1e3d1f494b7fa0807e32a71bd360` 因验收面板 TS2783 重复字段失败，原欢迎页及 3 行资料保留；修正样本测试代码后，平台构建 `task_7de3eb27c5cb406b87f123b18c4a011a` 成功，严格 TypeScript/Vite 完成。此失败属于新增的样本验收工具，不是把未通过构建的内容发布为产品。`fixture-build.json`、`fixture-build-2.json` 保留两次真实任务。

明确重载后，通过只读按钮从数据库读取 3 份保存资料；附件地址是当前会话的 HTTP 文件接口，不是旧页面 blob。JPEG 实际解码为 640 × 480，持久 WebM 在点击播放后推进到 14.04 / 14.04 秒并收到 ended，见 `14-persisted-assets-loaded-ax.txt`、`15-declared-connect-ax.txt`。原生控件舍入显示 14 秒与业务记录的采集墙钟 15120 毫秒分开记录，没有将两者伪作相同。

在真实 WKWebView 点击已声明 `https://httpbin.org/json` 请求按钮，返回 HTTP 200、type=cors、application/json、429 个可读取字符且可解析 JSON。点击故意未声明的 `https://example.com/` 后，fetch 失败并收到 `securitypolicyviolation`：`directive=connect-src`、`blockedURI=https://example.com/`、`disposition=enforce`。这是实际浏览器执行结果，不只是检查 HTTP 头，也没有仅凭 fetch 失败臆断 CORS 原因。证据 `15-declared-connect-ax.txt`、`16-undeclared-csp-ax.txt`；另存 `public-endpoint-readiness.json` 只是先行 HTTP 可用性探测，不替代 GUI 证据。尚未实测 images/media 分组或外站故障矩阵。

随后再次只对该样本进入维护：`task_59ef58d5be764e9b949ef3382859f2e0` 成功、维护 ID `maintenance_a09493b13af5448696455afc815a2f93`、新基线 `backup_0e9b658405714fb7b0a70a7ed5bd8981` 均保留。已声明的两个采集按钮和保存立即禁用，本地预览及草稿仍在；再次用原生选择器选择同一 PNG 后仍不能采集或保存，预览、下载及文字工具保持可用，准确覆盖原触发条件。证据 `17-fixed-maintenance-gate`、`18-fixed-maintenance-reselect` 的 PNG/AX。

结束维护 `task_d4f576480303483a8dc30087d69051fe` 成功后，旧页仍显示需明确重载，采集与保存没有自动恢复。明确处理测试草稿并重载后，新页恢复采集按钮、停止仍禁用，没有自动申请设备或保存。见 `19-fixed-reload-gate-ax.txt`、`20-fixed-normal-after-explicit-reload-ax.txt`。`maintenance-fixed-ended.json` 核对仍恰好 3 行、3 份附件 SHA-256 不变，样本组件字节与已审查模板修复一致。本轮没有再次采集照片或录音。

## 打包前保护复核

`browser-before-package-protected.json` 的 14 个原保护范围全部不变，来源审计 `browser-before-package-provenance.json` 通过。新增只有唯一样本、3 份对应状态、2 个 maintenanceBaseline ZIP（690008 字节合计）、2 个基线 snapshot 操作和 10 个 final 任务。10 个任务包括 4 次 storageScan、2 次 maintenanceBegin、2 次 maintenanceEnd、2 次 build；唯一失败为上述已记录的首个样本构建，其他 9 个成功。两份 ZIP 的归档清单 owner 均为该样本。原 46 个备份继续不变，加上新样本两份基线共 48 个备份文件、7382412 字节；5 个既有回收项全部保留且可恢复。样本为 ready，busy、recoveryRequired 为 false，activeTask 为空。

此前交付 app/DMG 在覆盖输出前完整保留到本轮私有 `prior-package/`：DMG 为 8282103 字节，SHA-256 `c08f7b51bf868bea3dd3795df2d6c97bf48866879508719a28980b6e8b2f3909`。源与副本的 55 个 app 条目（含根）类型、权限、长度、SHA 和链接目标一致，复制后再次扫描源确认未变。证据为 `prior-package-manifest.json`、`prior-package-summary.json`。新包构建、严格签名/DMG 校验及最终启动复核记录如下，保留的旧包不作为本轮新交付。

## 新包构建与校验

实测记录及修复分步提交推送为 `cd91bad`、`806b54c`、`48b2e8a`，打包检查记录为 `dce18b5`。旧原生进程 PID `48728` 通过 CUA 原生 `⌘Q` 退出；随后 AX 返回 procNotFound，独立检查旧进程已不存在、CLI 返回 `SERVICE_NOT_RUNNING`。证据为 `before-new-package-restart.json`、`new-package-gui-quit.json`。没有用 SIGTERM 代替这次正常退出。

`npm run build` 退出码为 0，新 app 与 DMG 生成，日志 `build-dmg.log`。`node scripts/m5-package-verify.mjs --output …/package-verification` 为 passed，报告和完整日志分别为 `package-verification/verification.json`、`package-verification.log`：

| 项目 | 新包结果 |
| --- | --- |
| app | `target/release/bundle/macos/Local Projects Dev.app` |
| DMG | `target/release/bundle/dmg/Local Projects Dev_0.0.0_aarch64.dmg` |
| DMG 长度 | `8283876` 字节 |
| DMG SHA-256 | `7b72754295eeaa3401b57f0d4a537d9986efbb66a73201c7d02f5ccd3bca6680` |
| 身份 / 版本 | `dev.localprojects.platform.m0` / `0.0.0`，development |
| 签名 / 架构 | 源 app 及 DMG app 严格深度签名通过，ad-hoc、runtime、arm64；最低系统配置 `15.0` |
| 模板 / SDK | `react-ts-vite-0.1.3`；固定 SDK `0.1.0` 的 8 处副本摘要仍为 `334022724d4799c44f6b8251c14190e7473882de541144f80698c4e7b77d3dc9` |
| DMG 内容 | `hdiutil verify` 通过；只读挂载后 54 个包内条目与源 app 完全一致，已卸载 |
| 配置 | 相机/麦克风用途、实际签名权利和固定开发身份均保留；源 app 内资源 README 与本次打包的 `docs/development.md` 一致 |

原 c08 包再次核对仍与保留清单一致，证据 `prior-package-final-verification.json`。额外离线保护 `browser-after-package-offline-protected.json` 为 14/14 通过；脚本不导入 CLI、不请求服务或启动程序，所有原数据文件、类型、权限和链接目标仍一致。它不替代重启后 API 身份与维护状态验证。

构建后首次尝试通过 CUA 从新 app 的准确路径启动时，工具返回 Mac 已锁定，要求用户手动解锁；没有改用 CLI 或其他 UI 方式绕过。用户回复“已解锁，继续验证”后，从同一路径正常启动新包，后续验证通过。

## 新包真实启动与最终数据复核

新进程 PID 为 `57930`，instanceId 为 `8b1a20de-2708-446f-a010-8eb2cbfb28bc`，与旧进程和实例均不同。`new-package-restart.json` 核对实际 installationPath 为本次 app、模板为 `react-ts-vite-0.1.3`，固定工作区路径未变。原生管理界面显示原项目“打僵尸”和唯一样本，共 2 个项目；5 个既有回收项均仍为可恢复状态，见 `21-new-package-manager` 的 PNG/AX。样本为 ready，busy、recoveryRequired 均为 false，activeTask 为空；数据库仍为同样的 3 行，三份附件长度及 SHA-256 与采集后一致，SDK 未变，样本中已实测的修复组件与包内模板字节完全一致。

只进入该样本，在新宿主的真实 WKWebView 再次明确点击读取持久资料，显示 3 份素材，JPEG 实际解码为 640 × 480；手动播放已保存录音至 ended，进度为 14.04 / 14.04 秒。再次点击公开只读请求，已声明 httpbin 返回 HTTP 200、type=cors、可读取且可解析的 429 字符 JSON；未声明 example.com 请求失败，并实际触发 `connect-src`、`blockedURI=https://example.com/`、`disposition=enforce` 的 CSP 事件。证据分别为 `22-new-package-persisted-assets-ax.txt`、`23-new-package-playback-connect-ax.txt`、`24-new-package-csp-ax.txt`。新宿主阶段没有再次采集设备、保存业务记录或改动项目源码。真实采集与修复后维护复测发生于此前 c08 宿主，重启后持久读取、解码、播放和网络限制发生于本次 7b 新包，两阶段分别记录。

最终 `final-protected.json`、`final-provenance.json` 和独立只读审计 `final-independent-data-package-audit.json` 均通过：14/14 原保护范围的内容、类型、权限和链接目标不变，原项目 3922 个普通文件、46 个原备份及 5 个既有回收项完整保留。新增仍只有唯一样本、3 份对应状态、2 份样本基线 ZIP 和 2 个 snapshot 操作；累计 12 个 final 任务为 6 次 storageScan、2 次 maintenanceBegin、2 次 maintenanceEnd、2 次 build，其中已披露的首次样本构建失败，其余 11 个成功。

重启管理界面新增的两个扫描中，一个属于样本，另一个 `task_7c6630ec72694807b90d17a2f711f59a` 属于原项目“打僵尸”。后者已按精确任务 ID、owner、storageScan 类型及结果范围单独审计，是只读管理扫描，仅增加扫描任务记录，没有改动原项目或既有维护状态；没有将其笼统纳入样本写入或允许未知新增。原有历史任务、恢复现场和回收证据均保持不变。

含样本两份基线后共 48 个备份文件、7382412 字节；本轮没有清理、迁移或覆盖原项目。三份素材及两份样本基线与打包前完全一致，新 DMG 和保留的旧 DMG 长度、SHA-256 再次通过。样本和私有证据保留供后续 M5 验证；Safari/Chrome、权限拒绝、系统设备释放及物理断网等未完成项仍按本记录开头的范围保留，不标记 M5 全部验收完成。
