# 开发环境重置与新身份

确认日期：2026-10-25。状态：按用户最终明确授权完成旧受管资料清理，新身份空环境已安装、启动并核对。

用户最终明确选择：现有平台管理的项目和业务数据也全部删除，重新开始。该选择替代此前保留资料、整体改名及兼容旧引用的迁移计划，也覆盖本次旧开发实例的固定根与恢复证据保留约定。不交付开发身份迁移命令或旧路径兼容映射。

新固定开发身份为 `dev.happypandaa.pandadrawer`，环境为 `development`，数据根为 `~/Library/Application Support/dev.happypandaa.pandadrawer/`。CLI 文件名仍为 `local-projects-dev`，开发版本仍为 `0.0.0`；正式身份 `com.happypandaa.pandadrawer` 继续独立。

本次明确授权清空：

- 旧 `~/Library/Application Support/dev.localprojects.platform.m0/` 平台管理的数据，包括项目源码、SDK 副本、数据库及 WAL/SHM、附件、已安装 Node 应用、配置／Key、备份、旧回收项、维护及恢复资料。
- 旧系统状态 `~/Library/WebKit/dev.localprojects.platform.m0/`、`~/Library/Caches/dev.localprojects.platform.m0/`、`~/Library/Preferences/dev.localprojects.platform.m0.plist`。
- 此前为本轮身份切换生成的临时迁移证据及候选旧副本；不保留旧副本、校验清单或恢复旧开发实例的材料。

平台工程 `/Users/ethan/dev/panda-drawer/` 和外部原作者源码目录保留。清理只针对本次明确的旧平台受管范围及临时切换材料，不搜索全盘删除同名文件，不将外部源码引用当成受管副本删除。

完成清理后，以新身份安装并启动空环境。实际记录须核对旧指定位置和临时切换材料的清理结果、新应用身份及数据根、项目列表为空、外部原作者源码与平台工程保留。不能将用户授权、源码改动或自动测试写作这些真实动作已经完成。

这次授权仅用于重置旧开发实例。以后的普通升级、业务项目修改及开发验证继续遵守资料保护规则；普通工作区位置变化仍走公开迁移流程，不从本次重置推导日后任意清空权限。历史验收记录保留其原身份、路径、样本和实际结果，不改写为新身份通过记录。

## 实际结果（2026-10-25）

旧平台数据根及旧 WebKit、Caches、Preferences 指定位置已永久删除；旧安装应用、旧 debug 应用及本轮临时迁移清单／校验材料已删除。没有执行资料保留迁移，没有保留旧受管副本或迁移兼容回执。平台工程及外部原作者源码未纳入删除。

新应用安装在 `/Applications/熊猫小抽屉.app`，包标识及包内 CLI 为 `dev.happypandaa.pandadrawer`，版本 `0.0.0`。实际 `.app` 构建、完整严格 ad-hoc 签名校验通过；本轮未生成 DMG，未公证，不称正式分发。实际启动后通过包内 CLI 读取新身份、安装位置和已认证服务，后续再次启动仍返回空项目列表。

通过包内 CLI 及实际目录核对：数据根为 `~/Library/Application Support/dev.happypandaa.pandadrawer/`，工作区为其 `workspace/`，`migrationId` 为 null；网页项目、Node 登记、备份、项目登记、迁移与操作记录均为空。清理后再次核对，旧数据根和上述旧系统位置没有重新出现。

当前身份测试 4 项、打包／正式预检／更新清单检查 58 项及 core／CLI 全目标严格 Clippy 通过；TypeScript／Vite 构建、网站构建及 14 页／467 条链接检查通过。CUA 清单识别新应用正在运行，但本次工具无法绑定新 bundle id；因此未把服务和目录核对写作原生空列表截图验收，也未重新验收相机／麦克风、系统权限、macOS 15 或 Windows。
