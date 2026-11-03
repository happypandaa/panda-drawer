import { t } from './i18n';
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
export const defaultPlatformAiDescription = 'macOS 上的本地小应用平台';
function macTrimSlash(path: string): string { return path.length > 1 ? path.replace(/\/+$/, '') : path; }
function macParent(path: string): string { const value = macTrimSlash(path); const index = value.lastIndexOf('/'); return index <= 0 ? '/' : value.slice(0, index); }
function macBaseName(path: string): string { const value = macTrimSlash(path); return value.slice(value.lastIndexOf('/') + 1) || value; }
function macCommonDirectory(paths: string[]): string {
  const parts = paths.map(path => macParent(path).split('/').filter(Boolean));
  if (!parts.length) return '/';
  let length = 0;
  while (parts.every(segments => segments.length > length && segments[length] === parts[0][length])) length++;
  return `/${parts[0].slice(0, length).join('/')}`;
}
const macRelativeDirectory = (dir: string, base: string) => dir === base ? '' : dir.startsWith(base === '/' ? '/' : `${base}/`) ? `${dir.slice(base === '/' ? 1 : base.length + 1)}/` : `${dir}/`;
const windowsSpelling = (path: string) => path.replaceAll('/', '\\');
function windowsRoot(path: string): string {
  return path.match(/^(?:\\\\\?\\UNC\\[^\\]+\\[^\\]+\\?|\\\\\?\\[a-z]:\\|\\\\\?\\[^\\]+\\|\\\\[^\\]+\\[^\\]+\\?|[a-z]:\\)/i)?.[0] ?? '';
}
function windowsTrim(path: string): string { const value = windowsSpelling(path); const root = windowsRoot(value); return value.length > root.length ? value.replace(/\\+$/, '') : value; }
function windowsParent(path: string): string { const value = windowsTrim(path); const root = windowsRoot(value); const at = value.lastIndexOf('\\'); return at < root.length ? root : value.slice(0, at); }
function windowsBaseName(path: string): string { const value = windowsTrim(path); return value.slice(value.lastIndexOf('\\') + 1) || value; }
function windowsCommonDirectory(paths: string[]): string {
  const parts = paths.map(path => windowsParent(path).split('\\'));
  if (!parts.length) return '';
  let length = 0;
  while (parts.every(segments => segments.length > length && segments[length].toLowerCase() === parts[0][length].toLowerCase())) length++;
  const common = parts[0].slice(0, length).join('\\');
  if (!common || !windowsAbsolutePath(`${common}\\`)) return '';
  return windowsTrim(`${common}\\`);
}
function windowsRelativeDirectory(dir: string, base: string): string {
  const path = windowsTrim(dir); const root = windowsTrim(base);
  if (path.toLowerCase() === root.toLowerCase()) return '';
  const prefix = root.endsWith('\\') ? root : `${root}\\`;
  return root && path.toLowerCase().startsWith(prefix.toLowerCase()) ? `${path.slice(prefix.length)}\\` : `${path}\\`;
}
const common = {
  aiHandoffAvailable: false,
  migrationAvailable: true,
  aiPlatformDescription: '本地小应用平台',
  openFolderLabel: '在文件管理器中打开',
  nodePathPlaceholder: '选择本机 Node 可执行文件的绝对路径',
  gitPathPlaceholder: '选择本机 Git 可执行文件的绝对路径',
  workspacePathPlaceholder: '选择新工作区的绝对路径',
  workspaceMigrationDescription: '把工作区移到新位置。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。',
  workspaceLocationRequirements: '空文件夹、可写入；支持的位置以本机核对结果为准。',
  projectDirectorySettingsLabel: '更改项目目录…',
  exitExplanation: '当前是内部开发版，只使用专用数据目录。退出软件时会关闭应用窗口并停止受管服务。',
  localDirectoryInitializationHint: '点击后会在这个空文件夹生成起始文件并添加项目，不安装依赖或自动运行。以后删除应用时，这个文件夹和 data/ 会保留，浏览器数据会清除。',
  localDirectoryCompletionHelp: (_ownership?: string) => '没有临时下载文件需要清理。以后删除应用时，你的文件夹和 data/ 会保留，浏览器数据会清除。',
  localDirectoryReviewDeletion: '此入口直接使用你的文件夹。删除应用会清除平台登记、配置、日志、缓存和浏览器数据；你的文件夹及 data/ 会保留。',
  localCleanupExternalSummary: (externalProjectCount: number) => t("外部项目的文件夹不会被删除：{count} 个。平台设置和浏览器数据会删除，文件夹里的 data/ 会保留。", { count: externalProjectCount }),
  localCleanupPathLabel: (label: string) => label,
  localCleanupWarning: (warning: string) => warning,
  isAbsolutePath: (value: string) => macAbsolutePath(value) || windowsAbsolutePath(value),
  parentPath: macParent,
  pathBaseName: macBaseName,
  commonDirectory: macCommonDirectory,
  relativeDirectory: macRelativeDirectory,
  directoryLabel: (path: string) => path === '/' ? '/' : `${path}/`,
};
const mac = {
  ...common,
  aiHandoffAvailable: true,
  aiPlatformDescription: defaultPlatformAiDescription,
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
  migrationAvailable: false,
  aiPlatformDescription: 'Windows x64 上的本地小应用平台',
  openFolderLabel: '在资源管理器中打开',
  nodePathPlaceholder: '例如 C:\\Program Files\\nodejs\\node.exe',
  gitPathPlaceholder: '例如 C:\\Program Files\\Git\\cmd\\git.exe',
  workspacePathPlaceholder: '例如 D:\\项目\\新工作区',
  workspaceMigrationDescription: '把工作区移到另一处本机内置 NTFS 磁盘位置。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。',
  workspaceLocationRequirements: '空文件夹、可写入、NTFS 格式，位于本机内置磁盘。',
  projectDirectorySettingsLabel: '查看项目目录…',
  exitExplanation: '当前是内部开发版，只使用专用数据目录。关闭主窗口或从菜单退出时，会关闭应用窗口并停止受管服务。',
  localDirectoryInitializationHint: '点击后会在这个空文件夹生成起始文件并添加项目，不安装依赖或自动运行。以后删除应用时，这个文件夹和 data/ 会保留，浏览器数据会清除。',
  localDirectoryCompletionHelp: (ownership?: string) => {
    if (ownership === 'managed') return '以后删除应用时，平台下载的项目文件夹和 data/ 会一并删除，浏览器数据会清除。';
    if (ownership === 'external') return '没有临时下载文件需要清理。以后删除应用时，你选择的文件夹和 data/ 会保留，浏览器数据会清除。';
    return '没有临时下载文件需要清理。删除应用时，你选择的文件夹和 data/ 会保留，浏览器数据会清除。';
  },
  localDirectoryReviewDeletion: '此入口直接使用所选文件夹。删除应用时，你选择的文件夹和 data/ 会保留，浏览器数据会清除。',
  localCleanupExternalSummary: (_externalProjectCount: number) => '你选择的文件夹及 data/ 会保留；平台下载的项目文件夹、平台设置和所有应用的浏览器数据会删除。具体范围以下方清单为准。',
  localCleanupPathLabel: (label: string) => label === '已登记的迁移工作区' ? '平台管理的项目内容' : label,
  localCleanupWarning: (warning: string) => warning.replaceAll('WebKit', 'WebView2').replaceAll('废纸篓', '回收站'),
  isAbsolutePath: windowsAbsolutePath,
  parentPath: windowsParent,
  pathBaseName: windowsBaseName,
  commonDirectory: windowsCommonDirectory,
  relativeDirectory: windowsRelativeDirectory,
  directoryLabel: (path: string) => path ? `${windowsTrim(path).replace(/\\+$/, '')}\\` : '多个磁盘位置',
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

/** Absolute path of the installed platform skill, from bootstrap platform info. */
export function platformSkillPathFromBootstrap(platform: Record<string, unknown> | null | undefined): string | undefined {
  const value = platform?.skillPath;
  return typeof value === 'string' && value ? value : undefined;
}
const PlatformSkillPathContext = createContext<string | undefined>(undefined);
export const PlatformSkillPathProvider = PlatformSkillPathContext.Provider;
export const usePlatformSkillPath = () => useContext(PlatformSkillPathContext);
