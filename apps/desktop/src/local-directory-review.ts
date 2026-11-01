import type { NodeDirectoryReview } from './node-app-types';
import { deletedSourceMatches, onProjectDeleted, projectWasDeleted } from './project-deletion';

export type LocalReviewCategory = 'needs-changes' | 'unsupported' | 'insufficient';
export interface LocalReviewReason { title: string; description: string }
export interface RecentProjectFolder {
  path: string;
  selectedAt: string;
  result: '检查未完成' | '上次未通过' | '可以添加' | '空文件夹' | '已添加' | '已移动或删除';
  issues: string[];
  ownerId?: string;
}

const recentFolders = new Map<string, RecentProjectFolder[]>();
onProjectDeleted(notice => {
  for (const [scope, items] of recentFolders) recentFolders.set(scope, items.filter(item => !deletedSourceMatches(notice, { appId: item.ownerId, sourceRoots: [item.path] })));
});

export function readRecentProjectFolders(scope: string): RecentProjectFolder[] {
  return [...(recentFolders.get(scope) ?? [])].filter(item => !projectWasDeleted(item.ownerId));
}

export function saveRecentProjectFolder(scope: string, item: RecentProjectFolder): RecentProjectFolder[] {
  if (projectWasDeleted(item.ownerId)) return readRecentProjectFolders(scope);
  const items = [item, ...readRecentProjectFolders(scope).filter(previous => normalizedFolderPath(previous.path) !== normalizedFolderPath(item.path))].slice(0, 5);
  recentFolders.set(scope, items);
  return items;
}

export function removeRecentProjectFolder(scope: string, path: string): RecentProjectFolder[] {
  const items = readRecentProjectFolders(scope).filter(previous => normalizedFolderPath(previous.path) !== normalizedFolderPath(path));
  recentFolders.set(scope, items);
  return items;
}
export function resetRecentProjectFoldersForTests() { recentFolders.clear(); }

export function isAbsoluteFolderPath(path: string): boolean { return path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path) || /^\\\\[^\\]+\\/.test(path); }

// This only supports an early UI hint. Filesystem aliases and actual directory
// identity remain the native inspector/registration's responsibility.
export function normalizedFolderPath(path: string): string { return path.replaceAll('\\', '/').replace(/\/$/, '') || '/'; }

export const reviewCategoryLabels: Record<LocalReviewCategory, string> = { 'needs-changes': '需要修改', unsupported: '当前不适合', insufficient: '信息不足' };

// One fallback for both entries. The local inspector supplies `category`
// directly; Git source inspection does not yet, so both use this same mapping
// instead of each screen keeping its own text rules.
export function reviewCategoryFromIssues(issues: string[]): LocalReviewCategory {
  const text = issues.join('\n');
  if (/Python|Java\b|Docker|多服务|原生扩展|本机编译|\.node\b/i.test(text)) return 'unsupported';
  if (!text || /无法安全读取|无法读取|没有读取权限|不可访问|超[出限]|文件太多|多个应用|无法确定/.test(text)) return 'insufficient';
  return 'needs-changes';
}

export function reviewCategoryTitle(category: LocalReviewCategory): string {
  return category === 'unsupported' ? '这个项目目前没法直接使用' : category === 'insufficient' ? '还需要进一步确认这个项目' : '这个项目需要修改后才能添加';
}

export function localReviewCategory(review: NodeDirectoryReview): LocalReviewCategory {
  if (review.category && ['needs-changes', 'unsupported', 'insufficient'].includes(review.category)) return review.category;
  return reviewCategoryFromIssues(review.issues);
}

/** Short, readable form of a long path; the full path stays in `title`. */
export function displayPath(path: string): string {
  const home = path.replace(/^\/(?:Users|home)\/[^/]+(?=\/|$)/, '~');
  const parts = home.split('/');
  if (home.length <= 60 || parts.length <= 5) return home;
  return [...parts.slice(0, 2), '…', ...parts.slice(-3)].join('/');
}

export function folderName(path: string): string {
  const parts = normalizedFolderPath(path).split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function parentPath(path: string): string {
  const normalized = normalizedFolderPath(path);
  const index = normalized.lastIndexOf('/');
  return index > 0 ? normalized.slice(0, index) : '/';
}

export function recentTimeLabel(value: string, now = new Date()): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const time = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((day(now) - day(date)) / 86_400_000);
  if (diff === 0) return `今天 ${time}`;
  if (diff === 1) return `昨天 ${time}`;
  return `${date.getMonth() + 1}月${date.getDate()}日`;
}

export function recentStatus(item: RecentProjectFolder): string {
  return item.result === '上次未通过' && item.issues.length ? `上次：${reviewCategoryLabels[reviewCategoryFromIssues(item.issues)]}` : item.result;
}

export function localReviewReason(issue: string): LocalReviewReason {
  if (/未找到.*panda\.json|缺少.*panda\.json|missing.*panda\.json/i.test(issue)) return { title: '缺少应用说明文件', description: '平台还不知道这个项目叫什么、从哪里启动，需要让 AI 检查并补齐。' };
  if (/没有读取权限|permission|denied|无法安全读取|不可访问/i.test(issue)) return { title: '暂时无法读取项目文件', description: '请重新选择并授权，或让能读取这个文件夹的 AI 检查。' };
  if (/端口|port|initialize|ready|protocol/i.test(issue)) return { title: '启动方式需要调整', description: '程序需要按平台分配的端口和启动方式运行，避免冲突。' };
  if (/shutdown|stopping|退出|关[闭窗]/i.test(issue)) return { title: '关闭应用时还不能正常退出', description: '程序需要先保存内容并结束，再关闭应用。' };
  if (/\.node|原生扩展|本机编译/i.test(issue)) return { title: '需要额外的本机组件', description: '平台目前无法准备这类组件，请让 AI 评估替代办法。' };
  if (/Python|Java|Docker|多服务/i.test(issue)) return { title: '运行环境暂不支持', description: '核心功能可能需要额外环境，请让 AI 判断能否保留功能并调整。' };
  if (/node_modules|依赖|lock|锁文件|构建结果|不存在|not found|missing|not exist/i.test(issue)) return { title: '运行所需的文件还没准备好', description: '请让 AI 检查依赖、版本记录和构建结果，再回来添加。' };
  if (/数据|database|APP_DATA_DIR/i.test(issue)) return { title: '数据保存位置需要调整', description: '文件数据应保存在项目 data/；请让 AI 检查应用是否正确使用 APP_DATA_DIR。' };
  if (/密钥|secret|credential|\.env/i.test(issue)) return { title: '配置方式需要调整', description: '实际设置和密钥应通过平台填写，不能放在网页代码里。' };
  if (/SSR|npm run dev|开发服务器|vite/i.test(issue)) return { title: '网页还需要准备为可用版本', description: '请先生成完整网页和程序，不能直接用开发服务器运行。' };
  if (/太多|超[出限]|预算|限额|budget|too many/i.test(issue)) return { title: '检查还没有完成', description: '可能选到了上层文件夹，请选择应用本身所在的文件夹。' };
  return { title: '项目还需要进一步检查', description: '请把检查详情交给 AI，判断需要怎样修改才能使用。' };
}

export function localReviewComparison(previous: string[] | undefined, current: string[]): string | null {
  if (!previous?.length) return null;
  const currentIssues = new Set(current);
  const solved = new Set(previous.filter(issue => !currentIssues.has(issue))).size;
  if (!solved) return null;
  return `和上次相比：已解决 ${solved} 项，还剩 ${currentIssues.size} 项。`;
}
