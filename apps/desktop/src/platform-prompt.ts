import { getLocale, t } from './i18n';
import { AI_PROMPT_RULES, BACKEND_CONTEXT, DIRECTORY_PROMPT, PLATFORM_AI_SUMMARY, PROMPT_TEMPLATES } from './project-ai-requirements';

const directoryTemplate = DIRECTORY_PROMPT;
const unavailableSkill = '当前平台缺少安装资源，暂时无法提供';
const parameter = /\{([A-Za-z][A-Za-z0-9_]*)\}/g;
const escapePattern = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Match the complete known platform scaffold; never rewrite arbitrary payloads. */
function scaffold(template: string, fixed: Record<string, string> = {}, patterns: Record<string, string> = {}) {
  const names: string[] = [];
  let expression = '^';
  let offset = 0;
  for (const match of template.matchAll(parameter)) {
    expression += escapePattern(template.slice(offset, match.index));
    if (Object.hasOwn(fixed, match[1])) expression += escapePattern(fixed[match[1]]);
    else { names.push(match[1]); expression += `(${patterns[match[1]] ?? '[\\s\\S]*?'})`; }
    offset = match.index! + match[0].length;
  }
  expression += escapePattern(template.slice(offset)) + '$';
  const pattern = new RegExp(expression);
  return (text: string): Record<string, string> | null => {
    const match = pattern.exec(text);
    return match ? Object.fromEntries(names.map((name, index) => [name, match[index + 1]])) : null;
  };
}

// Optional single-line prefixes stay apart even when two of them are adjacent.
const linePatterns = { nameLine: /(?:应用：[^\n]*\n)?/.source, descriptionLine: /(?:简介：[^\n]*\n)?/.source };
const handoffScaffolds = Object.values(PROMPT_TEMPLATES).map(template => ({ template, match: scaffold(template, AI_PROMPT_RULES, linePatterns) }));
const localizedRules = () => Object.fromEntries(Object.entries(AI_PROMPT_RULES).map(([name, rule]) => [name, t(rule)]));
const matchDirectory = scaffold(directoryTemplate, { summary: PLATFORM_AI_SUMMARY });
const matchBackendContext = scaffold(BACKEND_CONTEXT);

/** Keep original backend text in state so changing language updates the preview. */
export function localizePlatformPrompt(text: string): string {
  if (getLocale() === 'zh-CN') return text;
  const directory = matchDirectory(text);
  if (directory) return t(directoryTemplate, {
    ...directory,
    skillPath: directory.skillPath === unavailableSkill ? t(unavailableSkill) : directory.skillPath,
    cliPath: directory.cliPath === unavailableSkill ? t(unavailableSkill) : directory.cliPath,
    summary: t(PLATFORM_AI_SUMMARY),
  });
  for (const { template, match } of handoffScaffolds) {
    const handoff = match(text);
    if (!handoff) continue;
    const name = handoff.nameLine ? /^应用：([\s\S]*)\n$/.exec(handoff.nameLine) : null;
    if (handoff.nameLine && !name) return text;
    const description = handoff.descriptionLine ? /^简介：([\s\S]*)\n$/.exec(handoff.descriptionLine) : null;
    if (handoff.descriptionLine && !description) return text;
    const unavailable = (value: string | undefined) => value === unavailableSkill ? t(unavailableSkill) : value;
    return t(template, {
      ...handoff,
      ...localizedRules(),
      nameLine: name ? t('应用：{name}\n', { name: name[1] }) : '',
      backendContext: handoff.backendContext ? (() => { const context = matchBackendContext(handoff.backendContext); return context ? t(BACKEND_CONTEXT, { ...context, ...localizedRules() }) : handoff.backendContext; })() : '',
      descriptionLine: description ? t('简介：{description}\n', { description: description[1] }) : handoff.descriptionLine,
      skillPath: unavailable(handoff.skillPath),
      cliPath: unavailable(handoff.cliPath),
    });
  }
  return text;
}
