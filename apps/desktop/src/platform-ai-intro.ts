import { t } from './i18n';
import { PLATFORM_AI_SUMMARY } from './project-ai-requirements';
import { defaultPlatformAiDescription } from './frontend-platform';

/**
 * Platform guidance for adapting open-source software, creating an app, or
 * assessing an existing project. Only the optional software URL is specific
 * to the source the user wants to review.
 */
export function buildPlatformAiIntroPrompt(platformVersion?: string, platformDescription = defaultPlatformAiDescription, skillPath?: string, cliPath?: string, sourceUrl?: string): string {
  const version = platformVersion?.trim() || t('未读取');
  const intro = t(`下面是“熊猫小抽屉”平台的介绍。请读完后帮我：
- 我发来开源软件地址：判断能否改成小抽屉应用。我同意后再改并添加。
- 我想做新应用：按平台要求帮我规划和开发。
- 我已有项目：判断能否接入、要改什么。
请用通俗中文先给结论。给出方案并经我同意之前，不要修改文件。下载和改造需要能读写本机文件、运行命令的 AI 工具。做不到时，第一句就告诉我。

熊猫小抽屉是{platformDescription}（当前版本：{version}）。各应用有独立窗口，数据存本机。平台管启停、配置、密钥和日志，AI 管代码、构建和版本。

{summary}

完整的平台说明：{skill}
本机命令行：{cli}
改造开源软件：先用 app reserve --json 预留文件夹，用平台 Git 下载进去供评估，此步骤可在我同意前做。保留 Git 历史和许可证。读完先给方案，我同意再改。用户数据默认存 data/main.sqlite。改好后用 app check --dir 检查，再用 app add --reservation 添加，由小抽屉管理。
新应用可用 app create --name 创建。已有文件夹用 app add --dir 添加，文件夹归用户所有。

如果是开源软件，请告诉我：一句话结论（可以改／改动较大／当前不推荐／信息不足）、要改什么、会失去哪些功能、数据存在哪、需要我填哪些配置、原有数据要不要导入。
如果是新项目，请告诉我：是否需要后台、怎样直接创建应用、大致怎么拆功能和存数据、需要哪些配置项、会用到的外部服务和费用、平台做不到的部分怎么替代。
如果是已有项目，请告诉我：一句话结论（可以接入／需要修改／当前不推荐／信息不足）、需要改什么、会失去什么、我现在只需做的下一步。静态判断不能说成“已经验证能运行”。`, {
    cli: cliPath || t('未找到包内 CLI'), platformDescription: t(platformDescription), version, summary: t(PLATFORM_AI_SUMMARY),
    skill: skillPath ? t('{path}（能读取本机文件时请先读。改造开源软件时再读 references/porting.md）', { path: skillPath }) : t('随平台安装，复制时未读取到路径'),
  });
  const url = sourceUrl?.trim();
  return url ? `${intro}\n${t('软件地址：{url}', { url })}` : intro;
}
