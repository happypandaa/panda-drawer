// Generates the constants shared by every prompt copied to an external AI.
// Source: scripts/project-ai-requirements.json (summary, shared rules and handoff templates).
// The TS module also works in the desktop's standalone DOM test compiler.
import { readFile, writeFile } from 'node:fs/promises';

const source = new URL('./project-ai-requirements.json', import.meta.url);
const tsTarget = new URL('../apps/desktop/src/project-ai-requirements.ts', import.meta.url);
const enTarget = new URL('../apps/desktop/src/locales/en-prompt-templates.ts', import.meta.url);
const rustTarget = new URL('../crates/platform-core/src/ai_prompt_rules.rs', import.meta.url);

const { summary, rules, templates } = JSON.parse(await readFile(source, 'utf8'));
const marker = '【平台要求摘要】';
if (typeof summary !== 'string' || !summary.startsWith(marker)) throw new Error('scripts/project-ai-requirements.json has no platform summary.');
for (const retired of ['下一批', '后续批次', 'project.json', 'local-app.json', '.platform']) {
  if (summary.includes(retired)) throw new Error(`The platform summary still mentions ${retired}.`);
}

const ruleNames = {
  gitSave: 'GIT_SAVE', dataConsent: 'DATA_CONSENT', secrets: 'SECRETS', stdout: 'STDOUT',
  changeSize: 'CHANGE_SIZE', tools: 'TOOLS', reload: 'RELOAD', continueChat: 'CONTINUE_CHAT', schema: 'SCHEMA',
};
for (const key of Object.keys(ruleNames)) {
  if (typeof rules[key] !== 'string' || !rules[key].trim()) throw new Error(`scripts/project-ai-requirements.json is missing ${key}.`);
}
const templateNames = {
  modifyWeb: 'TEMPLATE_MODIFY_WEB', modifyNode: 'TEMPLATE_MODIFY_NODE',
  modifyAgentWeb: 'TEMPLATE_MODIFY_AGENT_WEB', modifyAgentNode: 'TEMPLATE_MODIFY_AGENT_NODE', create: 'TEMPLATE_CREATE',
};
const placeholders = text => [...new Set([...text.matchAll(/\{([A-Za-z][A-Za-z0-9_]*)\}/g)].map(match => match[1]))].sort();
for (const key of Object.keys(templateNames)) {
  const template = templates?.[key];
  if (typeof template?.zh !== 'string' || typeof template?.en !== 'string') throw new Error(`scripts/project-ai-requirements.json is missing template ${key}.`);
  if (placeholders(template.zh).join() !== placeholders(template.en).join()) throw new Error(`Template ${key} has different placeholders in zh and en.`);
}
// JSON string literals are valid Rust literals once \u00XX escapes use braces.
const rustString = (value) => JSON.stringify(value).replace(/\\u([0-9a-fA-F]{4})/g, '\\u{$1}');

const header = '// Generated from scripts/project-ai-requirements.json.\n// Run node scripts/generate-project-ai-requirements.mjs after editing the source.\n';
const ts = `${header}export const PLATFORM_AI_SUMMARY = ${JSON.stringify(summary)};
export const AI_PROMPT_RULES = {
${Object.keys(ruleNames).map(key => `  ${key}: ${JSON.stringify(rules[key])},`).join('\n')}
} as const;
/** Handoff scaffolds. Rule placeholders are filled with AI_PROMPT_RULES; the rest are per app. */
export const PROMPT_TEMPLATES = {
${Object.keys(templateNames).map(key => `  ${key}: ${JSON.stringify(templates[key].zh)},`).join('\n')}
} as const;
`;
const en = `${header}// English messages for the complete handoff scaffolds in PROMPT_TEMPLATES.
const messages: Record<string, string> = {
${Object.keys(templateNames).map(key => `  ${JSON.stringify(templates[key].zh)}: ${JSON.stringify(templates[key].en)},`).join('\n')}
};
export default messages;
`;
const rust = `${header}//! Rules and handoff templates shared by prompts that the platform copies for external AI.
pub const PLATFORM_AI_SUMMARY: &str = ${rustString(summary)};
${Object.entries(ruleNames).map(([key, name]) => `pub const ${name}: &str = ${rustString(rules[key])};`).join('\n')}
${Object.entries(templateNames).map(([key, name]) => `pub const ${name}: &str = ${rustString(templates[key].zh)};`).join('\n')}

/// Rule placeholders available to every template.
pub const RULES: [(&str, &str); ${Object.keys(ruleNames).length}] = [
${Object.entries(ruleNames).map(([key, name]) => `    (${JSON.stringify(key)}, ${name}),`).join('\n')}
];

/// Fill \`{name}\` placeholders in one pass. Rules come first; unknown names stay as written,
/// and values are inserted verbatim, so a value containing braces is never expanded again.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_end = after.find('}');
        let name = name_end.map(|end| &after[..end]);
        let valid = name.is_some_and(|name| {
            name.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        let value: Option<&str> = match name {
            Some(name) if valid => {
                let rule: Option<&str> = RULES
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| *value);
                rule.or_else(|| {
                    values
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| *value)
                })
            }
            _ => None,
        };
        match (value, name_end) {
            (Some(value), Some(end)) => {
                output.push_str(value);
                rest = &after[end + 1..];
            }
            _ => {
                output.push('{');
                rest = after;
            }
        }
    }
    output.push_str(rest);
    output
}
`;

if (process.argv.includes('--check')) {
  for (const [target, expected] of [[tsTarget, ts], [enTarget, en], [rustTarget, rust]]) {
    let actual = '';
    try { actual = await readFile(target, 'utf8'); } catch { /* Reported below. */ }
    if (actual !== expected) throw new Error('AI prompt constants are out of date. Run node scripts/generate-project-ai-requirements.mjs.');
  }
  process.stdout.write('AI prompt constants match their source configuration.\n');
} else {
  await writeFile(tsTarget, ts);
  await writeFile(enTarget, en);
  await writeFile(rustTarget, rust);
  process.stdout.write('Generated project-ai-requirements.ts, en-prompt-templates.ts and ai_prompt_rules.rs.\n');
}
