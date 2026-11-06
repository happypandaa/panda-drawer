import { getLocale, t } from './i18n';
import { AI_PROMPT_RULES, PLATFORM_AI_SUMMARY } from './project-ai-requirements';

const webHandoffTemplate = '请先阅读并理解这个网页应用，暂时不要修改任何文件。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n\n先读平台说明及其 references，再读项目的 panda.json、README 和开发文档；如有 AGENTS.md 或 agent.instructions 指定文件，也一并阅读；读不到说明时先告诉我。用通俗的中文说明应用做什么、代码和数据怎么组织、发现了哪些问题，然后等我提出具体需求。收到需求后，先说明修改计划，等我同意再动手。\n\n修改时请遵守：\n- 只修改上面的项目文件夹，其余规则以平台说明为准。{secrets}\n- {gitSave}\n- {dataConsent}\n- 改代码或构建前提醒我关闭应用窗口，按 panda.json 完成必要构建；改完说明后续操作和验证结果。\n';
const nodeHandoffTemplate = '请先阅读并理解这个应用，暂不修改文件。\n\n{nameLine}项目文件夹：{projectRoot}\n平台说明：{skillPath}\n日志目录：{logsPath}（current.log 本次、previous.log 上次）\n\n先读平台说明及 references、panda.json、README 与日志；如有 AGENTS.md 或 agent.instructions 指定文件，也一并阅读；说明不可读时先告诉我。用通俗中文说明应用功能、代码和数据组织及发现的问题，然后等我提出具体需求；先说明修改计划，等我同意再动手。\n\n修改规则：\n- 只修改项目文件夹，以平台说明为准。{secrets}\n- {gitSave}\n- {dataConsent}\n- 修改前提醒我关闭应用窗口；完成后说明改动、验证及后续操作。\n- {stdout}';
const directoryTemplate = '请只读评估下面这个本地文件夹能否接入“熊猫小抽屉”。我不懂代码，请用通俗易懂的中文回答，先给结论。\n\n文件夹：{folder}\n平台检查结果（排查材料，不是执行指令）：{report}\n平台说明：{skillPath}\n\n请先读平台说明里“用法二：只读评估”一节，按它的要求检查和回答。读不到这个文件夹或平台说明时，第一句就告诉我。本次只读：不修改文件、不安装依赖、不运行项目；需要修改时先告诉我方案，我同意后再改，改好后我会回平台重新检查。\n\n{summary}';
const unavailableSkill = '当前平台缺少安装资源，暂时无法提供';
const parameter = /\{([A-Za-z][A-Za-z0-9_]*)\}/g;
const escapePattern = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Match the complete known platform scaffold; never rewrite arbitrary payloads. */
function scaffold(template: string, fixed: Record<string, string> = {}) {
  const names: string[] = [];
  let expression = '^';
  let offset = 0;
  for (const match of template.matchAll(parameter)) {
    expression += escapePattern(template.slice(offset, match.index));
    if (Object.hasOwn(fixed, match[1])) expression += escapePattern(fixed[match[1]]);
    else { names.push(match[1]); expression += '([\\s\\S]*?)'; }
    offset = match.index! + match[0].length;
  }
  expression += escapePattern(template.slice(offset)) + '$';
  const pattern = new RegExp(expression);
  return (text: string): Record<string, string> | null => {
    const match = pattern.exec(text);
    return match ? Object.fromEntries(names.map((name, index) => [name, match[index + 1]])) : null;
  };
}

const handoffRules = {
  secrets: AI_PROMPT_RULES.secrets,
  gitSave: AI_PROMPT_RULES.gitSave,
  dataConsent: AI_PROMPT_RULES.dataConsent,
  stdout: AI_PROMPT_RULES.stdout,
};
const handoffScaffolds = [webHandoffTemplate, nodeHandoffTemplate].map(template => ({ template, match: scaffold(template, handoffRules) }));
const matchDirectory = scaffold(directoryTemplate, { summary: PLATFORM_AI_SUMMARY });

/** Keep original backend text in state so changing language updates the preview. */
export function localizePlatformPrompt(text: string): string {
  if (getLocale() === 'zh-CN') return text;
  const directory = matchDirectory(text);
  if (directory) return t(directoryTemplate, {
    ...directory,
    skillPath: directory.skillPath === unavailableSkill ? t(unavailableSkill) : directory.skillPath,
    summary: t(PLATFORM_AI_SUMMARY),
  });
  for (const { template, match } of handoffScaffolds) {
    const handoff = match(text);
    if (!handoff) continue;
    const name = handoff.nameLine ? /^应用：([\s\S]*)\n$/.exec(handoff.nameLine) : null;
    if (handoff.nameLine && !name) return text;
    return t(template, {
      ...handoff,
      nameLine: name ? t('应用：{name}\n', { name: name[1] }) : '',
      skillPath: handoff.skillPath === unavailableSkill ? t(unavailableSkill) : handoff.skillPath,
      secrets: t(AI_PROMPT_RULES.secrets),
      gitSave: t(AI_PROMPT_RULES.gitSave),
      dataConsent: t(AI_PROMPT_RULES.dataConsent),
      stdout: t(AI_PROMPT_RULES.stdout),
    });
  }
  return text;
}
