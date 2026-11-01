// Run only after the final M5 package has been built. This never launches the app,
// changes developer settings, signs binaries, or writes to existing app/DMG files.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile, readdir, lstat, readlink, mkdtemp, rmdir, stat } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const IDENTIFIER = 'dev.happypandaa.pandadrawer'
const args = process.argv.slice(2)
if (args.includes('--help')) {
  console.log('Usage: node scripts/m5-package-verify.mjs [--app ABSOLUTE_APP] [--dmg ABSOLUTE_DMG] [--output ABSOLUTE_NEW_DIRECTORY]')
  process.exit(0)
}
const options = {}
for (let index = 0; index < args.length; index += 2) {
  const key = args[index]
  assert.ok(['--app', '--dmg', '--output'].includes(key), `Unknown option: ${key}`)
  assert.ok(args[index + 1] && path.isAbsolute(args[index + 1]), `${key} needs an absolute path`)
  assert.equal(options[key], undefined, `Duplicate option: ${key}`)
  options[key] = args[index + 1]
}
assert.equal(process.platform, 'darwin', 'Package verification requires macOS tools')
const app = options['--app'] ?? path.join(root, 'target/release/bundle/macos/熊猫小抽屉.app')
const dmg = options['--dmg'] ?? path.join(root, 'target/release/bundle/dmg/熊猫小抽屉_0.0.0_aarch64.dmg')
const output = options['--output'] ?? path.join(root, '.test-artifacts', `m5-package-${new Date().toISOString().replaceAll(':', '-').replaceAll('.', '-')}`)
await mkdir(path.dirname(output), { recursive: true })
await mkdir(output) // Never replace another verification run.
const report = { verifiedAt: new Date().toISOString(), status: 'running', output, app, dmg, limitations: ['Read-only package checks; no app launch, device permission, Safari/Chrome or native-dialog interaction is performed.'] }
const json = async (name, value) => writeFile(path.join(output, name), `${JSON.stringify(value, null, 2)}\n`, { flag: 'wx' })
const hash = bytes => createHash('sha256').update(bytes).digest('hex')
const fileHash = async file => hash(await readFile(file))
async function command(name, executable, commandArgs, input) {
  const result = spawnSync(executable, commandArgs, { encoding: 'utf8', input, maxBuffer: 32 * 1024 * 1024 })
  const record = { executable, arguments: commandArgs, status: result.status, signal: result.signal, stdout: result.stdout ?? '', stderr: result.stderr ?? '', error: result.error?.message }
  await json(`${name}.json`, record)
  await writeFile(path.join(output, `${name}.log`), `${record.stdout}${record.stderr}`, { flag: 'wx' })
  assert.equal(result.status, 0, `${name} failed: ${record.stderr || record.error || record.stdout}`)
  return record
}
async function plist(file, name) { return JSON.parse((await command(name, '/usr/bin/plutil', ['-convert', 'json', '-o', '-', file])).stdout) }
async function decodePlist(bytes, name) { return JSON.parse((await command(name, '/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], bytes)).stdout) }
async function inventory(directory) {
  const entries = {}
  async function visit(relative = '') {
    for (const name of (await readdir(path.join(directory, relative))).sort()) {
      const key = relative ? `${relative}/${name}` : name
      const absolute = path.join(directory, key)
      const info = await lstat(absolute)
      if (info.isSymbolicLink()) entries[key] = { kind: 'symlink', target: await readlink(absolute) }
      else if (info.isDirectory()) { entries[key] = { kind: 'directory' }; await visit(key) }
      else if (info.isFile()) entries[key] = { kind: 'file', bytes: info.size, sha256: await fileHash(absolute) }
      else throw new Error(`Unsupported bundle entry: ${key}`)
    }
  }
  await visit()
  return entries
}
async function signedBundle(bundle, prefix) {
  await command(`${prefix}-codesign-verify`, '/usr/bin/codesign', ['--verify', '--deep', '--strict', '--verbose=2', bundle])
  const details = await command(`${prefix}-codesign-identity`, '/usr/bin/codesign', ['-dvvv', bundle])
  const identityText = details.stdout + details.stderr
  assert.match(identityText, new RegExp(`^Identifier=${IDENTIFIER.replaceAll('.', '\\.')}\\s*$`, 'm'))
  assert.match(identityText, /^Signature=adhoc$/m)
  assert.match(identityText, /flags=.*\([^)]*runtime[^)]*\)/)
  assert.match(identityText, /^TeamIdentifier=not set$/m)
  const info = await plist(path.join(bundle, 'Contents/Info.plist'), `${prefix}-info-plist`)
  assert.equal(info.CFBundleIdentifier, IDENTIFIER)
  assert.equal(info.LSMinimumSystemVersion, '15.0')
  assert.equal(info.CFBundleShortVersionString, '0.0.0')
  assert.equal(info.CFBundleExecutable, 'local-projects-desktop-dev')
  assert.equal(info.CFBundleName, '熊猫小抽屉')
  assert.ok(typeof info.NSCameraUsageDescription === 'string' && info.NSCameraUsageDescription.trim())
  assert.ok(typeof info.NSMicrophoneUsageDescription === 'string' && info.NSMicrophoneUsageDescription.trim())
  const entitlementResult = await command(`${prefix}-entitlements`, '/usr/bin/codesign', ['-d', '--entitlements', ':-', bundle])
  const entitlementText = entitlementResult.stdout + entitlementResult.stderr
  const start = entitlementText.indexOf('<?xml') >= 0 ? entitlementText.indexOf('<?xml') : entitlementText.indexOf('<plist')
  const end = entitlementText.lastIndexOf('</plist>')
  assert.ok(start >= 0 && end >= start, 'codesign must return a readable entitlement plist')
  const entitlements = await decodePlist(entitlementText.slice(start, end + '</plist>'.length), `${prefix}-entitlements-plist`)
  assert.equal(entitlements['com.apple.security.device.camera'], true)
  assert.equal(entitlements['com.apple.security.device.audio-input'], true)
  for (const key of ['com.apple.security.app-sandbox', 'com.apple.security.get-task-allow', 'com.apple.security.cs.disable-library-validation', 'com.apple.security.cs.allow-unsigned-executable-memory']) assert.notEqual(entitlements[key], true, `Unexpected entitlement ${key}`)
  const binaries = {}
  for (const executable of ['local-projects-desktop-dev', 'local-projects-dev']) {
    const binary = path.join(bundle, 'Contents/MacOS', executable)
    const architecture = (await command(`${prefix}-${executable}-arch`, '/usr/bin/lipo', ['-archs', binary])).stdout.trim()
    assert.equal(architecture, 'arm64')
    const load = await command(`${prefix}-${executable}-load-commands`, '/usr/bin/otool', ['-l', binary])
    assert.match(load.stdout, /\bminos 15\.0(?:\.0)?\s/)
    binaries[executable] = { architecture, sha256: await fileHash(binary), minimumOS: '15.0' }
  }
  const resources = path.join(bundle, 'Contents/Resources/resources')
  const template = JSON.parse(await readFile(path.join(resources, 'template/template.json'), 'utf8'))
  assert.equal(template.templateVersion, 'react-ts-vite-0.3.0')
  assert.equal(template.sourceDirectory, '.')
  assert.equal(template.sdkApiVersion, 1)
  assert.equal(template.sdkScript, '/_platform/sdk.js')
  for (const relative of ['sdk/sdk.js','sdk/panda.d.ts','template/app/src/panda.d.ts']) assert.ok((await stat(path.join(resources,relative))).isFile(), `${relative} missing`)
  for (const relative of ['template/app/src/lib/browser-tools.js', 'template/app/src/lib/browser-tools.d.ts', 'template/app/src/components/MediaToolsPanel.tsx', 'template/app/BROWSER_TOOLS.md']) assert.ok((await stat(path.join(resources, relative))).isFile(), `${relative} missing`)
  return { bundleIdentifier: info.CFBundleIdentifier, minimumSystemVersion: info.LSMinimumSystemVersion, version: info.CFBundleShortVersionString, usageDescriptions: { camera: info.NSCameraUsageDescription, microphone: info.NSMicrophoneUsageDescription }, entitlements, binaries, templateVersion: template.templateVersion, sdkApiVersion: template.sdkApiVersion }
}

let mountPoint
let mounted = false
let detached = false
try {
  const appBefore = await inventory(app)
  report.bundle = await signedBundle(app, 'app')
  await json('app-inventory.json', appBefore)
  report.dmg = { path: dmg, bytes: (await stat(dmg)).size, sha256: await fileHash(dmg) }
  await command('dmg-verify', '/usr/bin/hdiutil', ['verify', dmg])
  mountPoint = await mkdtemp(path.join(os.tmpdir(), 'local-projects-m5-dmg-verify-'))
  const mountResult = await command('dmg-mount', '/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-noautoopen', '-mountpoint', mountPoint, '-plist', dmg])
  mounted = true
  const mount = await decodePlist(mountResult.stdout, 'dmg-mount-plist')
  assert.ok(mount['system-entities']?.some(entity => entity['mount-point']), 'DMG attach did not report a mounted filesystem')
  const mountedApp = path.join(mountPoint, path.basename(app))
  report.mountedBundle = await signedBundle(mountedApp, 'dmg-app')
  const mountedInventory = await inventory(mountedApp)
  assert.deepEqual(mountedInventory, appBefore, 'DMG app differs from built app')
  await json('dmg-inventory.json', { mountedApp, entriesCompared: Object.keys(appBefore).length, match: true, readonly: true, inventory: mountedInventory })
  const sdkHashes = {}
  for (const relative of ['sdk.js','panda.d.ts']) {
    const expected=await fileHash(path.join(root,'packages/sdk',relative))
    const copies=[path.join(root,'apps/desktop/src-tauri/resources/sdk',relative),...[app,mountedApp].map(bundle=>path.join(bundle,'Contents/Resources/resources/sdk',relative))]
    if(relative==='panda.d.ts') copies.push(path.join(root,'templates/react/app/src/panda.d.ts'),...[app,mountedApp].map(bundle=>path.join(bundle,'Contents/Resources/resources/template/app/src/panda.d.ts')))
    for(const file of copies){sdkHashes[file]=await fileHash(file);assert.equal(sdkHashes[file],expected,`Platform SDK differs: ${file}`)}
  }
  await json('sdk-hashes.json',sdkHashes)
  report.sdk={apiVersion:1,copies:Object.keys(sdkHashes).length,match:true}
  assert.deepEqual(await inventory(app), appBefore, 'Built app changed during verification')
  assert.equal(await fileHash(dmg), report.dmg.sha256, 'DMG changed during verification')
  report.entriesCompared = Object.keys(appBefore).length
  report.status = 'passed'
} catch (error) {
  report.status = 'failed'
  report.error = { name: error.name, message: error.message, stack: error.stack }
  process.exitCode = 1
} finally {
  if (mounted) {
    try { await command('dmg-detach', '/usr/bin/hdiutil', ['detach', mountPoint]); detached = true }
    catch (error) { report.status = 'failed'; report.detachError = error.message; process.exitCode = 1 }
  }
  report.mount = { path: mountPoint, readonly: true, detached, mounted }
  if (mountPoint && (!mounted || detached)) await rmdir(mountPoint).catch(() => {})
  report.completedAt = new Date().toISOString()
  await json('verification.json', report)
  console.log(JSON.stringify({ status: report.status, report: path.join(output, 'verification.json'), dmg: report.dmg, entriesCompared: report.entriesCompared, sdk: report.sdk, detached, error: report.error?.message, detachError: report.detachError }, null, 2))
}
