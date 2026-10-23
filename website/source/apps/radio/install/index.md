---
title: 安装与配置 Radio Bob
description: 已有可信源码时，通过熊猫抽屉准备独立副本、安装并配置 Radio Bob。
---

# 安装与配置 Radio Bob


Radio Bob 的公开源码地址和下载包尚未提供。下面介绍**已有作者提供的可信源码**时，当前内部版的安装与配置方法。

## 开始之前

- 使用当前 macOS 15+、Apple Silicon 的熊猫抽屉内部版。正式发行与其他系统尚未验收。
- 准备作者适配好的源码目录，包含有效的 `local-app.json`、前端页面、可直接运行的 Node 后端，以及 `package.json` 和 npm 锁文件。
- 准备自己的 AI 服务 API 密钥与可用额度。安装依赖、生成节目和获取音乐都需要网络；语音服务另行配置。

当前安装入口接受受支持的 Node 服务源码。任意 GitHub 项目都不保证能直接安装；遇到“需要适配”时，先让 AI 按[应用开发要求](https://github.com/happypandaa/postgresUI/blob/main/docs/app-development-environment.md)检查和修改源码。

## 1. 获取一个独立源码副本

在“我的项目”点击“添加 → 获取项目源码”。已有本机目录时，先检查该目录，再明确选择“接入此目录的源码副本”。

检查本身不会复制或运行代码。接入后，平台保存独立来源记录和源码副本，原作者目录保持原样。不要把自己的密钥、登录 Cookie 或业务数据库放进准备交付的源码。

如果作者以后提供公开 GitHub 或 Gitee HTTPS 仓库，也可以从这里获取。当前不要使用猜测的 Radio 仓库地址。

## 2. 准备安装材料

进入“查看安装候选与准备安装”，选择刚才完成检查的 Node 源码候选，点击“准备安装材料”。

平台会在独立目录准备程序和生产依赖，跳过项目与依赖的安装脚本。这个入口不替作者编译前端，也不会启动 Radio。等待任务完成，核对应用名称、版本和来源。

如果准备失败，先查看任务原因。保留失败现场，不要把“已获取源码”或“任务已接受”当作安装成功。

## 3. 明确确认安装

核对候选后点击“确认安装”。成功后会出现新的独立应用实例，运行不再依赖作者目录。**确认安装不会自动启动应用**；还需要完成配置。

当前安装的是用户权限下运行的可信本机代码。只接入你信任、允许使用的来源。

## 4. 填写应用配置

打开 Radio Bob 的详情，进入“应用配置”。普通默认值通常可以保留；API 密钥使用你自己的账号创建。

| 设置 | 怎么填 |
| --- | --- |
| AI 服务地址 | 默认 `https://api.deepseek.com/anthropic`。这是 Anthropic Messages 兼容接口地址，不是聊天网页。 |
| AI API 密钥 | 必填。在 [DeepSeek API keys](https://platform.deepseek.com/api_keys) 创建，确认账号服务与额度后粘贴。 |
| AI 模型 | 默认 `deepseek-flash`。更换前核对[官方模型说明](https://api-docs.deepseek.com/quick_start/pricing)与账号权限。 |
| 语音生成密钥 | 可选。在 [Fish Audio API Keys](https://fish.audio/app/api-keys/) 创建。不填仍能看文字串词、播放音乐。 |
| 主持人音色 ID | 可选。按 [Fish 快速入门](https://docs.fish.audio/developer-guide/getting-started/quickstart)选择有权使用的音色，填写 reference ID。 |
| 语音生成模型 | 默认 `s2.1-pro-free`。型号名称不保证免费额度或使用权限，更换前核对 [Fish 接口说明](https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech)。 |

AI 地址、模型和密钥要对应同一服务商。当前使用 [DeepSeek 的 Anthropic 兼容协议](https://api-docs.deepseek.com/guides/anthropic_api/)，不能只把另一个服务的 OpenAI 接口地址填进来就当作兼容。

点击保存后，界面只显示密钥“已设置”。密钥保存在该应用的本机私有文件中，以当前用户的文件权限保护；不要写进源码、Git 或截图。保存成功表示本机配置已保存，服务商是否接受密钥、账户是否有额度，要在实际使用时确认。

## 5. 打开并试一次点歌

在 Radio 详情点击“进入项目”。缺少必填项时，先回到配置页补齐；保存配置不会自动启动。进入 Radio 后，可以先直接搜索一首歌，再试着输入：

> 请播放 Ludwig van Beethoven 的《Moonlight Sonata》，只放一首，不需要语音。

音乐推荐仍需匹配曲库；搜索到歌曲也不保证有完整播放权限。当前受管版为游客模式，不开放音乐账号登录。若只有试听，按界面标记理解即可。

关闭 Radio 窗口会停止它的后端；以后重新打开使用已保存配置和本机数据。运行中修改配置后，需要明确“重启并应用配置”，或关窗后再打开。在 macOS 上，关闭熊猫抽屉管理窗口只会隐藏窗口，使用菜单退出或 `Cmd+Q` 才退出整个平台。

## 目前的范围

这套源码副本安装流程已用独立样本验收，Radio 的新副本安装链路尚未单独完成验收。Radio 既有实例已验证真实 AI 点歌、原生窗口试听及关窗停止；Fish 语音和完整新电脑交接仍未验收。

Radio 的普通更新与数据兼容性尚未单独完成验收。更新前先核对当前版本的支持范围，不要手工修改已安装程序或登记文件来绕过检查。完整规则见[当前安装与更新说明](https://github.com/happypandaa/postgresUI/blob/main/docs/node-managed-installation.md)。

[返回 Radio Bob](/apps/radio/) · [了解如何获取和修改应用](/tutorials/remix-an-app/)
