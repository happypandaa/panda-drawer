# M5 浏览器能力真实验证续记

日期：2026-10-17（Asia/Shanghai）。本记录补充 [M5 原验收](m5-verification.md)，仅将实际操作通过的原生 WKWebView 项目窗口流程记为通过。Safari、Chrome、真实录音拍照、系统设备许可及物理断网仍未完成；此前普通浏览器的工具 URL 访问限制没有通过其他工具绕过。不能据此将 M5 三端浏览器验收标成全部完成。

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

这些结果尚不覆盖文件拖入、复制拒绝降级、受保护保存路径、保存的失败分支、媒体回放或系统设备释放。后续修复和新包交付将在相应验证通过后补充，不使用已有模拟测试结论冒充实测。
