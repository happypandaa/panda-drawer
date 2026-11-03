---
title: Readio
description: 个人英语听读工具，逐句听音频、翻译查词，收藏想复习的句子与生词。
---

# Readio

把想读的英文文章放进来，一句一句听懂。Readio 把朗读、翻译、查词与收藏放在同一个学习窗口里。

[查看源码与使用说明](https://github.com/happypandaa/Readio)

## 能做什么

- 粘贴英文正文，自动分句并生成逐句音频。
- 继续上次的学习进度，跳句或调整播放速度。
- 翻译句子，点击单词查看语境释义。
- 收藏句子和生词，回到原文复习。

## 添加与配置

在熊猫小抽屉点击「添加应用」，粘贴 [Readio 仓库链接](https://github.com/happypandaa/Readio)，等待添加完成。已有本机项目时，可以选择包含 `panda.json` 的整个文件夹；使用包含当前后端接口支持的新版熊猫小抽屉。

打开应用详情，在「应用配置」填写两把自己的密钥：

| 设置 | 用途 |
| --- | --- |
| DeepSeek API Key | 句子翻译和查词；可在 [DeepSeek 平台](https://platform.deepseek.com/api_keys)创建。 |
| 豆包语音 API Key | 逐句英文朗读；按配置页说明在火山引擎语音控制台的「API Key 管理」中创建。 |

两项都必填，模型和音色使用应用内置默认值。生成音频、翻译和查词需要联网，服务可能按用量收费，先确认自己的账号权限与额度。保存配置后，再点击「打开」。

## 学习记录怎么保存

文章、学习进度和收藏保存在这个应用窗口的浏览器数据中。需要备份时，在 Readio 顶部「⋯」菜单导出 JSON；导入会要求确认覆盖当前数据，音频需要重新生成。

**删除应用会清除这些浏览器数据**，即使是从本机文件夹添加、项目文件夹仍保留，也一样。删除或更换实例前，先在 Readio 内导出需要的学习记录。

本站核对了[公开项目清单](https://github.com/happypandaa/Readio/blob/main/panda.json)和 [README](https://github.com/happypandaa/Readio/blob/main/README.md)，没有把源码检查当作真实服务或跨系统使用验收。

[返回应用列表](/apps/) · [把应用改成自己的](/tutorials/remix-an-app/)
