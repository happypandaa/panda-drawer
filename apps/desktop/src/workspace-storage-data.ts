import type { NodeAppDetail } from './node-app-types';
import { bigBytes, partBytes } from './storage-utils';
import type { ProjectInfo, StoragePart } from './types';

export interface WorkspaceStorage {
  platformProgram: StoragePart;
  workspace: StoragePart & { projects?: StoragePart[] };
  nodeApps?: StoragePart[];
  otherLocalLocations: StoragePart[];
  knownLocalTotal?: StoragePart;
  disks?: { exampleRoot?: string; availableBytes?: import('./types').ByteCount; [key: string]: unknown }[];
}
export const regenerableCategories = ['dependencies', 'buildCache', 'logs'] as const;
export function projectRegenerable(project: ProjectInfo): bigint | null {
  const current = project.storage?.currentProject;
  if (!current || current.status === 'not_scanned') return null;
  const dependencies = bigBytes(partBytes(current.categories?.dependencies));
  const cache = bigBytes(partBytes(current.categories?.cache));
  return dependencies === null || cache === null ? null : dependencies + cache;
}
export const projectTotal = (project: ProjectInfo) => {
  const current = project.storage?.currentProject;
  return !current || current.status === 'not_scanned' ? null : bigBytes(current.totalBytes);
};
export const nodeRowFor = (rows: StoragePart[] | undefined, appId: string) => rows?.find(row => (row.scope as Record<string, unknown> | undefined)?.appId === appId);
export const projectRowFor = (storage: WorkspaceStorage | null, projectId: string) => storage?.workspace.projects?.find(row => (row.scope as Record<string, unknown> | undefined)?.projectId === projectId);
export interface ApplicationStorageRow { key: string; name: string; bytes: bigint | null; complete: boolean; icon?: string | null }
export function applicationStorageRows(storage: WorkspaceStorage | null, projects: ProjectInfo[], apps: NodeAppDetail[]): ApplicationStorageRow[] {
  return [
    ...projects.map(project => {
      const row = projectRowFor(storage, project.id);
      const fallback = project.storage?.currentProject;
      return { key: `web:${project.id}`, name: project.name, bytes: row ? bigBytes(partBytes(row)) : projectTotal(project), complete: row ? row.complete === true : Boolean(fallback?.complete && !fallback.stale), icon: project.iconDataUrl };
    }),
    ...apps.map(app => { const row = nodeRowFor(storage?.nodeApps, app.id); return { key: `node:${app.id}`, name: app.name, bytes: bigBytes(partBytes(row)), complete: row?.complete === true }; }),
  ].sort((a, b) => a.bytes === b.bytes ? a.name.localeCompare(b.name, 'zh-CN') : a.bytes === null ? 1 : b.bytes === null ? -1 : a.bytes > b.bytes ? -1 : 1);
}
