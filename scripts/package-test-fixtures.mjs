// Filesystem fixtures use the host's actual link types. A skipped file-symlink
// subtest provides no link-rejection proof; junction and hardlink tests still run.
import { symlink } from 'node:fs/promises';

export const posixPermissions = {
  skip: process.platform === 'win32' && 'POSIX mode-bit fixture; Windows payload checks use the native backend',
};

export const macRuntimePublication = {
  skip: process.platform === 'win32' && 'macOS rename/chmod publication fixture; Windows publication uses the native backend',
};

export function directoryLink(target, destination) {
  return symlink(target, destination, process.platform === 'win32' ? 'junction' : undefined);
}

export async function fileSymlinkFixture(t, target, destination) {
  try { await symlink(target, destination); return true; } catch (error) {
    if (process.platform !== 'win32' || !['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) throw error;
    t.diagnostic(`Windows file symlink fixture unavailable (${error.code}); no file-symlink rejection proof.`);
    t.skip('Windows host does not permit this disposable file symlink fixture');
    return false;
  }
}
