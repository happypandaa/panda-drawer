// Prepare the platform script and the template's copy of its canonical types.
// No dependency install, npm pack, vendor tarball or second SDK implementation.
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { Script } from 'node:vm';
const sdk = new URL('../packages/sdk/', import.meta.url);
const script = await readFile(new URL('sdk.js', sdk), 'utf8');
new Script(script, { filename: 'panda-sdk.js' });
const declaration = await readFile(new URL('panda.d.ts', sdk), 'utf8');
const templateTypes = new URL('../templates/react/app/src/panda.d.ts', import.meta.url);
if (process.argv.includes('--check')) {
  if (await readFile(templateTypes, 'utf8') !== declaration) throw new Error('Template SDK types differ; run pnpm package:sdk.');
} else {
  await writeFile(templateTypes, declaration);
}
console.log(`Panda SDK API 1: ${fileURLToPath(new URL('sdk.js', sdk))}`);
