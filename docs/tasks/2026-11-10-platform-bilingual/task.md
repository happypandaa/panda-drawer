# 平台中英双语补全

用户于 2026-11-10 确认实施。平台和应用各自管理语言；平台语言只影响平台界面、原生操作、平台交接模板以及应用在平台中展示的元信息。应用自行跟随系统或提供语言设置，不新增 SDK locale、PANDA_LOCALE 或同步机制，不修改应用内部语言策略。

## 实施范围

1. 未保存平台语言时，操作系统首选界面语言以 zh 开头使用 zh-CN，其余使用 en；原生结果作为桌面权威值，启动弹窗与管理器一致。手动保存后不跟随系统改变。
2. catalog 应用可选 i18n.zh-CN/en，支持 name、summary、needs、note，逐字段回退原文。生成输入保留人工译文，名称可回退清单译文；needs 从必填配置字段逐项取译文 label 后回退原 label，再完整生成。解析和缓存保留全部语言数据。四个应用补英文草稿供用户审核。
3. panda.json 可选 localization.zh-CN/en，仅覆盖 app.name/description、configuration 按字段 name 的 label/description/placeholder/howToGet/options（选项 value → label）、runtimeRequirements.network 按 connect/images/media 和 origin 的 purpose/dataSent、devices.camera/microphone.purpose。
4. 未提供译文字段回退原文；howToGet 整数组替换，options 逐 value 覆盖。拒绝未知字段/语言、null、错误类型、无效引用及不满足原文字段约束的内容。只改变展示文字，不改变标识、配置值、策略或权限。源定义由 Rust 生成 Schema 和前端类型。
5. 卡片、详情、配置、添加前预览和原生窗口标题切换语言立即更新；原始清单和配置保留，不将翻译投影保存回登记或应用。SDK context、应用业务内容和运行进程保持应用自主语言。
6. 同步格式文档、随安装 skill 和 React 模板清单示例；不恢复已删除的 local-node 模板，不要求起始应用内部全面双语。
7. 增加 README.en.md 与中文 README 顶部互链，内容与实际状态一致。

## 官网待确认范围

增加 /en/ 英文入口页，包含平台简介、系统要求、实际安装包/下载状态、中英文互跳；沿用主题、站点身份与权限。Wiki、教程和应用详情页保留现状。用户在本轮确认此范围，并授权完成后更新现有站点。

## 验证与交付

本机负责 macOS 本地 CI；Windows 由 Windows 机器验证，不等待远端 CI。串行套件、Rust 最多两个构建作业，关闭本地 dev/test debug 和增量输出。用一次性夹具和干净测试环境验证默认语言、已保存选择、部分翻译回退、引用校验及 SDK 开关与展示翻译无关；不删除既有开发数据。自动化不代替英文系统干净安装、真实窗口和 Windows 实机验收。

在通常使用的项目目录 develop 完成集成，保存原有工作；完整步骤通过检查后提交并核对 GitHub/Gitee。报告最终格式、实际检查、文案审核状态、未覆盖项目及目录/分支/提交。
