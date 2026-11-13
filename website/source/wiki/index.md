---
title: Wiki
description: 熊猫小抽屉的使用指南，以及给 AI 和应用作者的平台规则与格式说明。
---

# Wiki

先选一个应用用起来，想改哪里，就交给你自己的 AI。这里既有跟着做的使用指南，也有 AI 修改应用时需要读的规则。

## 使用指南

- [入门指南](/wiki/getting-started/)：添加、配置和打开第一个应用。
- [交给你的 AI](/wiki/ai-workspace/)：选择工具、打开项目文件夹，告诉 AI 想改什么。
- [常见问题](/wiki/faq/)：准备应用、保存内容、删除和下载线路。
- [教程](/tutorials/)：先改一个现成应用，再从零做一个。
- [应用](/apps/)：拾闻、Clack、Readio 和 Radio Bob，需要什么、怎么开始。

## 规则与格式

下面五页写给你的 AI 和应用作者。可以把[平台规则](/wiki/platform-rules/)发给 AI，让它从这里开始。

- [平台规则](/wiki/platform-rules/)：AI 接手、修改和评估应用时要遵守的规则。
- [panda.json 完整说明](/wiki/panda-json/)：字段、路径、配置声明和完整示例。
- [应用类型与运行方式](/wiki/app-types/)：网页、Node、SDK、控制协议和窗口行为。
- [数据、配置、日志和删除](/wiki/data-and-config/)：保存位置、操作边界和删除范围。
- [添加方式与要求](/wiki/adding-apps/)：链接、本机文件夹、起始应用及接入评估。

AI 也可以从 [llms.txt](/llms.txt)取得这些页面的完整地址。交接提示词会给出本机安装的平台说明位置，以那份说明为准。

## 当前版本

更新于 2026-11-13。熊猫小抽屉目前是内部开发版，正式安装包仍在准备中。开发包的获取方式请联系提供此版本的人。

- 平台可切换中文和英文。各应用自行决定界面语言。
- 推荐列表已加入拾闻。用「交给我的 AI」让自己的 AI 收集资讯，收藏和反馈留在本机。
- 应用更新后可以重新加载。作者可给现有应用增加或移除后台，同一个应用保留原有数据和本机地址。详见[运行方式](/wiki/app-types/)。

| 系统 | 开发版要求 |
| --- | --- |
| macOS | macOS 15 及以上、Apple Silicon，工作区位于本机或直连磁盘的 APFS 卷 |
| Windows | Windows 10 22H2 或 Windows 11、x64、WebView2 Evergreen，工作区位于本机内置 NTFS 磁盘 |

macOS 提供 AI 工具检测、一键交接、窗口标题栏操作和工作区迁移。Windows 开发版可从详情复制提示词和打开项目文件夹。

[平台应用说明](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/SKILL.md)与页面列出的来源按当前 develop 分支维护；本机安装的说明优先。网站主题和字体许可见[第三方许可](/licenses/)。
