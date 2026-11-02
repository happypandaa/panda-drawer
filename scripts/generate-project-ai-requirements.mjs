// Generates the constants shared by every prompt copied to an external AI.
// Sources (the only maintained copies):
// - docs/project-compatibility-review-prompt.md: the short platform summary.
// - docs/ai-prompt-rules.json: rules repeated across handoff and help prompts.
// The TS module also works in the desktop's standalone DOM test compiler.
import { readFile, writeFile } from 'node:fs/promises';

const source = new URL('../docs/project-compatibility-review-prompt.md', import.meta.url);
const rulesSource = new URL('../docs/ai-prompt-rules.json', import.meta.url);
const tsTarget = new URL('../apps/desktop/src/project-ai-requirements.ts', import.meta.url);
const rustTarget = new URL('../crates/platform-core/src/ai_prompt_rules.rs', import.meta.url);

const document = await readFile(source, 'utf8');
const summary = document.match(/^```text\n([\s\S]*?)\n```/m)?.[1];
const marker = '【平台要求摘要】';
if (!summary || !summary.startsWith(marker)) throw new Error('docs/project-compatibility-review-prompt.md has no platform summary block.');
for (const retired of ['下一批', '后续批次', 'project.json', 'local-app.json', '.platform']) {
  if (summary.includes(retired)) throw new Error(`The platform summary still mentions ${retired}.`);
}

const rules = JSON.parse(await readFile(rulesSource, 'utf8'));
const ruleNames = { gitSave: 'GIT_SAVE', dataConsent: 'DATA_CONSENT', secrets: 'SECRETS', stdout: 'STDOUT' };
for (const key of Object.keys(ruleNames)) {
  if (typeof rules[key] !== 'string' || !rules[key].trim()) throw new Error(`docs/ai-prompt-rules.json is missing ${key}.`);
}
// JSON string literals are valid Rust literals once \u00XX escapes use braces.
const rustString = (value) => JSON.stringify(value).replace(/\\u([0-9a-fA-F]{4})/g, '\\u{$1}');

const header = '// Generated from docs/project-compatibility-review-prompt.md and docs/ai-prompt-rules.json.\n// Run node scripts/generate-project-ai-requirements.mjs after editing either source.\n';
const ts = `${header}export const PLATFORM_AI_SUMMARY = ${JSON.stringify(summary)};\nexport const AI_PROMPT_RULES = {\n${Object.keys(ruleNames).map(key => `  ${key}: ${JSON.stringify(rules[key])},`).join('\n')}\n} as const;\n`;
const rust = `${header}//! Rules shared by prompts that the platform copies for external AI.\npub const PLATFORM_AI_SUMMARY: &str = ${rustString(summary)};\n${Object.entries(ruleNames).map(([key, name]) => `pub const ${name}: &str = ${rustString(rules[key])};`).join('\n')}\n`;

if (process.argv.includes('--check')) {
  for (const [target, expected] of [[tsTarget, ts], [rustTarget, rust]]) {
    let actual = '';
    try { actual = await readFile(target, 'utf8'); } catch { /* Reported below. */ }
    if (actual !== expected) throw new Error('AI prompt constants are out of date. Run node scripts/generate-project-ai-requirements.mjs.');
  }
  process.stdout.write('AI prompt constants match their maintained documents.\n');
} else {
  await writeFile(tsTarget, ts);
  await writeFile(rustTarget, rust);
  process.stdout.write('Generated project-ai-requirements.ts and ai_prompt_rules.rs.\n');
}
