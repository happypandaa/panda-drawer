// The copyable compatibility document is the single maintained source. This
// checked-in module also works in the desktop's standalone DOM test compiler.
import { readFile, writeFile } from 'node:fs/promises';

const source = new URL('../docs/project-compatibility-review-prompt.md', import.meta.url);
const target = new URL('../apps/desktop/src/project-ai-requirements.ts', import.meta.url);
const document = await readFile(source, 'utf8');
const prompt = document.match(/^```text\n([\s\S]*?)\n```/m)?.[1];
const marker = '【平台要求：';
if (!prompt || !prompt.includes(marker)) throw new Error('The complete compatibility prompt has no platform requirements.');
const requirements = prompt.slice(prompt.indexOf(marker));
const generated = `// Generated from docs/project-compatibility-review-prompt.md.\n// Run node scripts/generate-project-ai-requirements.mjs after editing its platform requirements.\nexport const PROJECT_AI_REQUIREMENTS = ${JSON.stringify(requirements)};\n`;
if (process.argv.includes('--check')) {
  if (await readFile(target, 'utf8') !== generated) throw new Error('Project AI requirements are out of date. Run node scripts/generate-project-ai-requirements.mjs.');
  process.stdout.write('Project AI platform requirements match their maintained document.\n');
} else {
  await writeFile(target, generated);
  process.stdout.write('Generated project-ai-requirements.ts from the compatibility document.\n');
}
