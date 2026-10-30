// Keep the caller's existing parent-component policy. Windows accepts both
// separators; on macOS a backslash is an ordinary filename character.
export function hasTraversal(value, { platform = process.platform } = {}) {
  return value.split(platform === 'win32' ? /[\\/]/ : '/').includes('..');
}
