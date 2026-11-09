import { t } from './i18n';
import { PLATFORM_AI_SUMMARY } from './project-ai-requirements';
import { defaultPlatformAiDescription } from './frontend-platform';

/**
 * A project-independent introduction for any external AI: used before a new
 * project starts, or to judge whether a project built elsewhere can join.
 * It never carries a project path, ID or diagnostics. The receiving AI may be
 * a web chat that cannot read local files, so it carries the short platform
 * summary and points file-capable AIs to the installed skill.
 */
export function buildPlatformAiIntroPrompt(platformVersion?: string, platformDescription = defaultPlatformAiDescription, skillPath?: string, cliPath?: string): string {
  const version = platformVersion?.trim() || t('未读取');
  return t(`下面是“熊猫小抽屉”平台的介绍。请先读完，再根据我接下来说的情况帮我：
- 我准备做一个新项目：按平台的要求帮我规划和开发；
- 我已经有一个项目：判断它能不能放进平台，需要改什么。
我不懂代码，请用通俗易懂的中文回答，先给结论。在我说清需求之前，不要创建或修改任何文件。

熊猫小抽屉是{platformDescription}（当前版本：{version}）。用户在平台里打开、配置和运行一个个小应用，每个应用有自己的窗口，数据保存在这台电脑上。平台负责启动和停止应用、保存配置和密钥、记录运行日志；写代码、构建、用 Git 管理版本由 AI 负责。

{summary}

完整的平台说明：{skill}
本机命令行：{cli}
需求明确后，可以直接运行 app create --name 创建应用；用 app add --dir 添加已有文件夹时，该文件夹归用户所有。下载应用并希望由平台管理时，先用 app reserve 预留文件夹，再用 app add --reservation 登记。需要后台或密钥时，在 panda.json 中加 backend 段，再运行 app reload；用 app logs 查看后台日志。删除应用、清理存储和更改运行环境会在平台主窗口请求用户确认。

如果是新项目，请告诉我：是否需要后台、怎样直接创建应用、大致怎么拆功能和存数据、需要哪些配置项、会用到的外部服务和费用、平台做不到的部分怎么替代。
如果是已有项目，请告诉我：一句话结论（可以接入／需要修改／当前不推荐／信息不足）、需要改什么、会失去什么、我现在只需做的下一步。静态判断不能说成“已经验证能运行”。`, {
    cli: cliPath || t('未找到包内 CLI'), platformDescription: t(platformDescription), version, summary: t(PLATFORM_AI_SUMMARY),
    skill: skillPath ? t('{path}（能读取本机文件时，请先读它的“用法二：只读评估”一节）', { path: skillPath }) : t('随平台安装，复制时未读取到路径'),
  });
}
