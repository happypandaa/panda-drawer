// Generates the constants shared by every prompt copied to an external AI.
// Sources (the only maintained copies):
// - docs/project-compatibility-review-prompt.md: platform requirements.
// - docs/ai-prompt-rules.json: rules repeated across handoff and help prompts.
// The TS module also works in the desktop's standalone DOM test compiler.
import { readFile, writeFile } from 'node:fs/promises';

const source = new URL('../docs/project-compatibility-review-prompt.md', import.meta.url);
const rulesSource = new URL('../docs/ai-prompt-rules.json', import.meta.url);
const tsTarget = new URL('../apps/desktop/src/project-ai-requirements.ts', import.meta.url);
const rustTarget = new URL('../crates/platform-core/src/ai_prompt_rules.rs', import.meta.url);

const document = await readFile(source, 'utf8');
const prompt = document.match(/^```text\n([\s\S]*?)\n```/m)?.[1];
const marker = '【平台要求：';
if (!prompt || !prompt.includes(marker)) throw new Error('The complete compatibility prompt has no platform requirements.');
const requirements = prompt.slice(prompt.indexOf(marker));
const protocolStart = requirements.indexOf('\n3. Node 协议');
const protocolEnd = requirements.indexOf('\n4. ', protocolStart + 1);
if (protocolStart < 0 || protocolEnd < 0) throw new Error('The platform requirements have no separate Node protocol section.');
const protocol = requirements.slice(protocolStart + 1, protocolEnd).trimEnd();

const rules = JSON.parse(await readFile(rulesSource, 'utf8'));
const ruleNames = { gitSave: 'GIT_SAVE', dataConsent: 'DATA_CONSENT', secrets: 'SECRETS', stdout: 'STDOUT' };
for (const key of Object.keys(ruleNames)) {
  if (typeof rules[key] !== 'string' || !rules[key].trim()) throw new Error(`docs/ai-prompt-rules.json is missing ${key}.`);
}
// JSON string literals are valid Rust literals once \u00XX escapes use braces.
const rustString = (value) => JSON.stringify(value).replace(/\\u([0-9a-fA-F]{4})/g, '\\u{$1}');

const header = '// Generated from docs/project-compatibility-review-prompt.md and docs/ai-prompt-rules.json.\n// Run node scripts/generate-project-ai-requirements.mjs after editing either source.\n';
const ts = `${header}export const PROJECT_AI_REQUIREMENTS = ${JSON.stringify(requirements)};\nexport const PROJECT_AI_NODE_PROTOCOL = ${JSON.stringify(protocol)};\nexport const AI_PROMPT_RULES = {\n${Object.keys(ruleNames).map(key => `  ${key}: ${JSON.stringify(rules[key])},`).join('\n')}\n} as const;\n`;
const rust = `${header}//! Rules shared by prompts that the platform copies for external AI.\n${Object.entries(ruleNames).map(([key, name]) => `pub const ${name}: &str = ${rustString(rules[key])};`).join('\n')}\n`;

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
