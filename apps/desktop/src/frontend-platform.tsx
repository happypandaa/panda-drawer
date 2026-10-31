import { createContext, useContext } from 'react';

// Presentation and lexical input hints only. Native commands remain responsible
// for opening, validating and authorizing any selected filesystem object.
function windowsAbsolutePath(value: string): boolean {
  if (value.includes('\0')) return false;
  const path = value.replaceAll('/', '\\');
  if (/^[a-z]:\\/i.test(path)) return true;
  if (path.startsWith('\\\\?\\')) {
    const extended = path.slice(4);
    if (/^[a-z]:/i.test(extended)) return /^[a-z]:\\/i.test(extended);
    if (/^UNC\\/i.test(extended)) return /^[^\\]+\\[^\\]+(?:\\|$)/.test(extended.slice(4));
    return /^[^\\]+(?:\\|$)/.test(extended);
  }
  return /^\\\\[^\\]+\\[^\\]+(?:\\|$)/.test(path);
}

const macAbsolutePath = (value: string) => value.startsWith('/');
const common = {
  openFolderLabel: '在文件管理器中打开',
  nodePathPlaceholder: '选择本机 Node 可执行文件的绝对路径',
  gitPathPlaceholder: '选择本机 Git 可执行文件的绝对路径',
  workspacePathPlaceholder: '选择新工作区的绝对路径',
  workspaceMigrationDescription: '把工作区移到新位置。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。',
  workspaceLocationRequirements: '空文件夹、可写入；支持的位置以本机核对结果为准。',
  exitExplanation: '当前是内部开发版，只使用专用数据目录。退出软件时会关闭应用窗口并停止受管服务。',
  isAbsolutePath: (value: string) => macAbsolutePath(value) || windowsAbsolutePath(value),
};
const mac = {
  ...common,
  openFolderLabel: '在访达中打开',
  nodePathPlaceholder: '例如 /opt/homebrew/bin/node',
  gitPathPlaceholder: '例如 /usr/bin/git',
  workspacePathPlaceholder: '/Volumes/磁盘/新工作区',
  workspaceMigrationDescription: '把工作区移到新位置，比如外置磁盘。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。',
  workspaceLocationRequirements: '空文件夹、可写入、APFS 格式，在本机或直连磁盘上。',
  exitExplanation: '当前是内部开发版，只使用专用数据目录。macOS 关闭主窗口后软件仍保留在 Dock，可点击 Dock 图标重新打开；从菜单退出或按 Cmd+Q 才会关闭应用窗口并停止受管服务。其他平台关闭主窗口仍会退出。',
  isAbsolutePath: macAbsolutePath,
};
const windows = {
  ...common,
  openFolderLabel: '在资源管理器中打开',
  nodePathPlaceholder: '例如 C:\\Program Files\\nodejs\\node.exe',
  gitPathPlaceholder: '例如 C:\\Program Files\\Git\\cmd\\git.exe',
  workspacePathPlaceholder: '例如 D:\\项目\\新工作区',
  workspaceMigrationDescription: '把工作区移到另一处本机内置 NTFS 磁盘位置。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。',
  workspaceLocationRequirements: '空文件夹、可写入、NTFS 格式，位于本机内置磁盘。',
  exitExplanation: '当前是内部开发版，只使用专用数据目录。关闭主窗口或从菜单退出时，会关闭应用窗口并停止受管服务。',
  isAbsolutePath: windowsAbsolutePath,
};

export type FrontendPlatform = typeof common;

export function frontendPlatformFromBootstrap(platform: Record<string, unknown> | null | undefined): FrontendPlatform {
  const build = platform?.build;
  const target = build && typeof build === 'object' && 'target' in build ? build.target : undefined;
  // This is the actual Rust compilation target supplied by bootstrap_info,
  // rather than a user agent, path guess or default-to-macOS assumption.
  if (target === 'x86_64-pc-windows-msvc') return windows;
  if (target === 'aarch64-apple-darwin' || target === 'x86_64-apple-darwin') return mac;
  return common;
}

const FrontendPlatformContext = createContext<FrontendPlatform>(common);
export const FrontendPlatformProvider = FrontendPlatformContext.Provider;
export const useFrontendPlatform = () => useContext(FrontendPlatformContext);
