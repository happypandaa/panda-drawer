import { spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, copyFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { npmCommand } from './npm-command.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const sdk = join(root, 'packages/sdk')
const metadata = JSON.parse(await readFile(join(sdk, 'package.json'), 'utf8'))
const filename = `local-projects-sdk-${metadata.version}.tgz`
const temporary = await mkdtemp(join(tmpdir(), 'local-projects-sdk-pack-'))

async function installImmutable(source, destination) {
  await mkdir(dirname(destination), { recursive: true })
  try {
    const existing = await readFile(destination)
    if (!existing.equals(await readFile(source))) {
      throw new Error(`Refusing to replace a different SDK with the same version: ${destination}. Publish a new package version and update the template lockfile.`)
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error
    await copyFile(source, destination)
  }
}

try {
  const npm = npmCommand(['pack', '--json', '--pack-destination', temporary])
  const result = spawnSync(npm.command, npm.args, { cwd: sdk, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stderr || 'npm pack failed')
  const packages = JSON.parse(result.stdout)
  if (packages.length !== 1 || packages[0].filename !== filename) throw new Error('Unexpected npm package name')
  const packed = join(temporary, filename)
  await installImmutable(packed, join(sdk, 'releases', filename))
  await installImmutable(packed, join(root, 'templates/react/app/vendor', filename))
  console.log(`${metadata.name}@${metadata.version}: ${packages[0].integrity}`)
} finally {
  await rm(temporary, { recursive: true, force: true })
}
