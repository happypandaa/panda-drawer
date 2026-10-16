# M5 浏览器能力真实验证续记

日期：2026-10-17（Asia/Shanghai）。本记录补充 [M5 原验收](m5-verification.md)，仅将实际操作通过的原生 WKWebView 项目窗口流程记为通过。Safari、Chrome、拒绝权限、独立系统设备释放测量及物理断网仍未完成；此前普通浏览器的工具 URL 访问限制没有通过其他工具绕过。不能据此将 M5 三端浏览器验收标成全部完成。

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

- 录音停止后得到 `audio/webm; codecs=opus`、373852 字节、15120 毫秒；计划约 2 秒，授权弹窗与窗口切换造成停止延后，实际为约 15.1 秒，已向用户说明。文件以 `.webm` 保存，没有假定 MP4 或改写扩展名。
- 原生音频控件点击播放后从 0 秒推进至结尾 14 秒，见 `10-real-recording-stopped` 和 `11-real-recording-playback`。这是浏览器实际解码和播放进度，不代表测量了扬声器物理输出。
- 摄像头许可后出现本地预览与拍摄按钮；仅点击拍下一张照片，预览转为 JPEG，停止采集按钮禁用；点击保存得到 `.jpg` 附件和业务行。为减少额外保存设备画面，照片过程仅记录 AX，见 `12-real-photo-stopped-ax.txt`、`13-real-photo-saved-ax.txt`，没有将照片放入仓库或交付文档。

`real-capture-files.json` 核对总共 3 行：原文本、录音和一张照片；每行附件长度、路径、MIME 与实际文件一致，录音具有 WebM/EBML 文件头，37595 字节照片具有完整 JPEG 起止标记，另存 SHA-256。设备仅在明确点击后申请，停止/拍摄后界面退出采集模式；尚未独立测量系统设备占用或验证维护、隐藏、拒绝权限的硬件分支，不把界面提示或模拟 track.stop 断言扩张为这些结论。
