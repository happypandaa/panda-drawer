import type { ByteCount, StoragePart } from './types';

export interface WorkspaceStorageSnapshot {
  platformProgram: StoragePart;
  workspace: StoragePart;
  otherLocalLocations: StoragePart[];
  knownLocalTotal?: StoragePart;
  disks?: Record<string, unknown>[];
}

interface SpaceGroup {
  id: string;
  label: string;
  part: StoragePart;
}

interface SpaceOverview {
  total: ByteCount | null;
  complete: boolean;
  stale: boolean;
  groups: SpaceGroup[];
  barReady: boolean;
  updatedAt?: string | number;
}

const groupNames: Record<string, string> = {
  projects: '网页项目',
  apps: '已安装应用与设置',
  program: '软件本体',
  history: '备份与历史资料',
  cache: '缓存与平台记录',
  other: '其他文件',
  workspace: '工作区文件',
};

function numeric(value: unknown): bigint | null {
  if (typeof value === 'number') return Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : null;
  if (!value || typeof value !== 'object' || !('$int64' in value) || typeof value.$int64 !== 'string' || !/^\d+$/.test(value.$int64)) return null;
  try { return BigInt(value.$int64); } catch { return null; }
}

function measured(part: StoragePart): bigint | null {
  // An explicit null total is unknown, even if another legacy field is present.
  return numeric(part.totalBytes !== undefined ? part.totalBytes : part.bytes !== undefined ? part.bytes : part.sizeBytes);
}

function encoded(value: bigint | null): ByteCount {
  return value === null ? null : value <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : { $int64: value.toString() };
}

function sumKnown(parts: StoragePart[]): bigint | null {
  let total = 0n;
  let found = false;
  for (const part of parts) {
    const value = measured(part);
    if (value !== null) { total += value; found = true; }
  }
  return found ? total : null;
}

function combined(parts: StoragePart[]): StoragePart {
  return {
    totalBytes: encoded(sumKnown(parts)),
    complete: parts.every((part) => part.complete === true && measured(part) !== null),
    stale: parts.some((part) => part.stale === true),
  };
}

function categoryGroup(name: string): string {
  if (name === 'projects') return 'projects';
  if (['backups', 'operations', 'trash'].includes(name)) return 'history';
  if (['sharedCache', 'control', 'metadata', 'logs'].includes(name)) return 'cache';
  return 'other';
}

function categoriesOf(part: StoragePart): [string, StoragePart][] | null {
  if (!part.categories || typeof part.categories !== 'object' || Array.isArray(part.categories)) return null;
  const categories = Object.entries(part.categories);
  if (categories.some(([, value]) => !value || typeof value !== 'object' || Array.isArray(value))) return null;
  const entries = categories as [string, StoragePart][];
  const total = measured(part);
  if (total === null || entries.some(([, item]) => measured(item) === null)) return null;
  if (!entries.length && (part.complete !== true || part.stale === true)) return null;
  const categoryTotal = entries.reduce((sum, [, item]) => sum + measured(item)!, 0n);
  // Keep the independently measured root when a category breakdown is absent,
  // unknown or inconsistent. A missing category is never invented as zero.
  return categoryTotal === total ? entries : null;
}

export function spaceOverview(data: WorkspaceStorageSnapshot | null): SpaceOverview {
  if (!data) return { total: null, complete: false, stale: false, groups: [], barReady: false };

  // The scanner has already excluded nested roots. Project rows are references
  // into workspace and must never be added to these disjoint measured scopes.
  const scopes = [data.workspace, data.platformProgram, ...data.otherLocalLocations];
  const knownTotal = sumKnown(scopes);
  const reportedTotal = data.knownLocalTotal ? measured(data.knownLocalTotal) : null;
  // The backend starts its accumulator at zero even if every root is unknown.
  // A reported number needs at least one independently measured scope.
  const total = knownTotal === null ? null : reportedTotal ?? knownTotal;
  const scopeStale = scopes.some((part) => part.stale === true) || data.knownLocalTotal?.stale === true;
  const complete = scopes.every((part) => part.complete === true && measured(part) !== null)
    && (!data.knownLocalTotal || data.knownLocalTotal.complete === true && reportedTotal !== null && reportedTotal === knownTotal);
  const buckets = new Map<string, StoragePart[]>();
  function add(id: string, part: StoragePart) {
    const items = buckets.get(id) ?? [];
    items.push(part);
    buckets.set(id, items);
  }

  const categories = categoriesOf(data.workspace);
  if (categories) {
    for (const [name, part] of categories) {
      add(categoryGroup(name), {
        ...part,
        complete: data.workspace.complete === true && part.complete !== false,
        stale: data.workspace.stale === true || part.stale === true,
      });
    }
  } else add('workspace', data.workspace);
  add('program', data.platformProgram);
  for (const part of data.otherLocalLocations) {
    const kind = part.scope && typeof part.scope === 'object' && 'kind' in part.scope ? part.scope.kind : undefined;
    add(kind === 'applicationData' ? 'apps' : kind === 'retainedWorkspace' ? 'history' : 'other', part);
  }

  const groups: SpaceGroup[] = [];
  for (const id of ['projects', 'workspace', 'apps', 'program', 'history', 'cache', 'other']) {
    const parts = buckets.get(id);
    if (!parts) continue;
    const part = combined(parts);
    if (measured(part) === 0n && part.complete === true && part.stale !== true) continue;
    groups.push({ id, label: groupNames[id], part });
  }
  const groupTotal = sumKnown(groups.map((group) => group.part));
  const stale = scopeStale || groups.some((group) => group.part.stale === true);
  const barReady = complete && !stale && total !== null && total > 0n
    && groups.every((group) => group.part.complete === true && measured(group.part) !== null)
    && groupTotal === total;
  let updatedAt: string | number | undefined;
  let newest = -Infinity;
  for (const part of scopes) {
    if (typeof part.completedAt !== 'string' && typeof part.completedAt !== 'number') continue;
    const timestamp = new Date(part.completedAt).getTime();
    if (Number.isFinite(timestamp) && timestamp > newest) { newest = timestamp; updatedAt = part.completedAt; }
  }
  return { total: encoded(total), complete, stale, groups, barReady, ...(updatedAt === undefined ? {} : { updatedAt }) };
}
