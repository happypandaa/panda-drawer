import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface BuildResultView { task?: TaskRecord; issues: string[] }
const invalid = '保留的构建结果身份或格式不符，请保留任务和日志并刷新核对；未猜测其他任务为最近构建。';
const object = (value: unknown): value is Record<string, unknown> => Boolean(value && typeof value === 'object' && !Array.isArray(value));

export function matchingProjectTask(value: unknown, projectId: string, expectedId?: string): value is TaskRecord {
  if (!object(value) || typeof value.taskId !== 'string' || !value.taskId || value.projectId !== projectId
    || expectedId !== undefined && value.taskId !== expectedId || typeof value.type !== 'string' || !value.type
    || typeof value.status !== 'string' || !['queued', 'running', 'succeeded', 'failed', 'interrupted'].includes(value.status)
    || value.final !== ['succeeded', 'failed', 'interrupted'].includes(value.status) || typeof value.phase !== 'string'
    || value.result !== null && !object(value.result)
    || value.error !== null && (!object(value.error) || typeof value.error.code !== 'string' || typeof value.error.message !== 'string')) return false;
  return true;
}

export function recoverBuildResult(value: unknown, projectId: string): BuildResultView | null {
  if (value === undefined) return null; // Compatibility with older command snapshots.
  if (!object(value) || typeof value.selection !== 'string' || !['recorded-submission', 'legacy-modified-time', 'none', 'unavailable'].includes(value.selection)
    || !Array.isArray(value.issues) || value.issues.some(issue => !object(issue) || typeof issue.code !== 'string' || typeof issue.message !== 'string')) return { issues: [invalid] };
  const issues = value.issues.map(messageFrom);
  if (value.selection === 'none' || value.selection === 'unavailable') {
    if (value.task !== null || value.selection === 'unavailable' && !issues.length) return { issues: [invalid] };
    return { issues };
  }
  if (!matchingProjectTask(value.task, projectId) || value.task.type !== 'build' || !/^task_[a-fA-F0-9]{32}$/.test(value.task.taskId)) return { issues: [...issues, invalid] };
  if (value.selection === 'legacy-modified-time' && !value.issues.some(issue => issue.code === 'BUILD_RESULT_LEGACY_ORDER')) {
    issues.push('按历史构建记录的修改时间恢复显示，不代表最新提交顺序；请结合任务日志核对。');
  }
  return { task: value.task, issues };
}
