// Explicit real-WebKit verification. The fixture has a unique real NSBundle;
// this script never starts the development app or initializes its data root.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { root, run, rustEnvironment } from './process.mjs';

if (process.platform !== 'darwin') throw new Error('The native store fixture requires macOS.');

await run('cargo', ['build', '--locked', '-p', 'platform-desktop', '--features', 'native-store-fixture', '--bin', 'native-store-fixture']);
const temporary = await mkdtemp(path.join(os.tmpdir(), 'panda-native-store-fixture-'));
const identifier = `dev.happypandaa.pandadrawer.webkit-fixture.${randomUUID().replaceAll('-', '')}`;
if (!/^dev\.happypandaa\.pandadrawer\.webkit-fixture\.[0-9a-f]{32}$/.test(identifier)) throw new Error('Invalid fixture identity.');
const bundle = path.join(temporary, 'DisposableWebKitFixture.app');
const executable = path.join(bundle, 'Contents', 'MacOS', 'native-store-fixture');
const reportPath = path.join(temporary, 'native-report.json');
await mkdir(path.dirname(executable), { recursive: true, mode: 0o700 });
await copyFile(path.join(root, 'target', 'debug', 'native-store-fixture'), executable);
await writeFile(path.join(bundle, 'Contents', 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${identifier}</string>
<key>CFBundleExecutable</key><string>native-store-fixture</string>
<key>CFBundleName</key><string>Disposable WebKit Fixture</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsArbitraryLoadsInWebContent</key><true/></dict>
</dict></plist>`, { mode: 0o600 });
await run('/usr/bin/codesign', ['--force', '--sign', '-', bundle]);

let passed = false;
let output = '';
try {
  const code = await new Promise((resolve, reject) => {
    const child = spawn(executable, [identifier, reportPath], { cwd: temporary, env: rustEnvironment(), stdio: ['ignore', 'pipe', 'pipe'] });
    const collect = data => { output = (output + data.toString()).slice(-128_000); };
    child.stdout.on('data', collect); child.stderr.on('data', collect);
    const timer = setTimeout(() => { child.kill('SIGKILL'); }, 180_000);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('exit', (code, signal) => { clearTimeout(timer); if (signal) reject(new Error(`Fixture terminated: ${signal}`)); else resolve(code); });
  });
  const report = JSON.parse(await readFile(reportPath, 'utf8'));
  if (code !== 0 || report.ok !== true || report.bundleIdentifier !== identifier) throw new Error(`Native fixture failed: ${JSON.stringify(report)}`);
  passed = true;
  process.stdout.write(`${JSON.stringify({ ok: true, fixtureIdentifier: identifier, checks: report.checks })}\n`);
} catch (error) {
  process.stderr.write(`${output}\nFixture evidence: ${temporary}\n`);
  throw error;
} finally {
  // These exact namespaces were freshly allocated by this invocation. They
  // cannot name the fixed development identity, a user's project, or a parent.
  const library = path.join(os.homedir(), 'Library');
  for (const parent of ['WebKit', 'Caches', 'Application Support', 'HTTPStorages']) {
    await rm(path.join(library, parent, identifier), { recursive: true, force: true });
  }
  await rm(path.join(library, 'Saved Application State', `${identifier}.savedState`), { recursive: true, force: true });
  await rm(path.join(library, 'Preferences', `${identifier}.plist`), { force: true });
  if (passed) await rm(temporary, { recursive: true, force: true });
}
