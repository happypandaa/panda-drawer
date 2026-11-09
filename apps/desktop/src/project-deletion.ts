import type { TaskRecord } from './types';

export interface ProjectDeletedNotice {
  version: 1;
  kind: 'app';
  id: string;
  taskIds: string[];
  sourceIds: string[];
  sourceRoots: string[];
  instanceId: string;
}
const listeners = new Set<(notice: ProjectDeletedNotice) => void>();
const projects = new Set<string>();
const tasks = new Set<string>();
const sources = new Set<string>();
let instance: string | null = null;
let pending: ProjectDeletedNotice[] = [];
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string' && Boolean(item));
export function checkedProjectDeletion(value: unknown): ProjectDeletedNotice | null {
  if (!value || typeof value !== 'object') return null;
  const notice = value as ProjectDeletedNotice;
  return notice.version === 1 && notice.kind === 'app'
    && typeof notice.id === 'string' && Boolean(notice.id) && typeof notice.instanceId === 'string' && Boolean(notice.instanceId)
    && strings(notice.taskIds) && strings(notice.sourceIds) && strings(notice.sourceRoots) ? notice : null;
}
export function observeDeletionRuntime(value: string) {
  if (instance !== value) { instance = value; projects.clear(); tasks.clear(); sources.clear(); }
  const queued = pending; pending = [];
  queued.filter(notice => notice.instanceId === value).forEach(notice => acceptProjectDeletion(notice));
}
export function acceptProjectDeletion(value: unknown): boolean {
  const notice = checkedProjectDeletion(value);
  if (!notice) return false;
  if (!instance || notice.instanceId !== instance) { pending.push(notice); return false; }
  projects.add(notice.id); notice.taskIds.forEach(id => tasks.add(id)); notice.sourceIds.forEach(id => sources.add(id));
  // Paths and task payloads are delivered synchronously and never retained in
  // this suppression index. Outstanding readers keep only their revocation IDs.
  for (const listener of listeners) listener(notice);
  return true;
}
export function onProjectDeleted(listener: (notice: ProjectDeletedNotice) => void): () => void {
  listeners.add(listener); return () => { listeners.delete(listener); };
}
export const projectWasDeleted = (id: string | null | undefined) => Boolean(id && projects.has(id));
export const sourceWasDeleted = (id: string | null | undefined) => Boolean(id && sources.has(id));
export function taskWasDeleted(task: TaskRecord | string | null | undefined): boolean {
  if (!task) return false;
  if (typeof task === 'string') return tasks.has(task);
  return tasks.has(task.taskId) || projectWasDeleted(taskProjectOwner(task)) || sourceWasDeleted(task.result?.sourceId as string | undefined);
}
export function taskProjectOwner(task: TaskRecord): string | null {
  const candidate = task.result?.candidate;
  const value = task.projectId ?? task.result?.ownerAppId ?? task.result?.appId
    ?? (candidate && typeof candidate === 'object' && 'appId' in candidate ? candidate.appId : null);
  return typeof value === 'string' ? value : null;
}
export function deletedSourceMatches(notice: ProjectDeletedNotice, value: { appId?: string | null; taskIds?: (string | null | undefined)[]; sourceIds?: (string | null | undefined)[]; sourceRoots?: (string | null | undefined)[] }): boolean {
  // Immutable identities take precedence over a reused filesystem path.
  if (value.appId) return value.appId === notice.id;
  if (value.taskIds?.some(id => id && notice.taskIds.includes(id))) return true;
  if (value.sourceIds?.some(Boolean)) return value.sourceIds.some(id => id && notice.sourceIds.includes(id));
  return Boolean(value.sourceRoots?.some(path => path && notice.sourceRoots.some(root => normalized(path) === normalized(root) || normalized(path).startsWith(`${normalized(root)}/`))));
}
function normalized(path: string) { return path.replaceAll('\\', '/').replace(/\/$/, ''); }
export function resetProjectDeletionsForTests() { instance = null; pending = []; projects.clear(); tasks.clear(); sources.clear(); }
