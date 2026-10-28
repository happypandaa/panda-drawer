import type { NodeAppDeleteResult, NodeDeletionTaskSnapshot } from './node-app-types';

export function nodeDeletionActive(task: NodeDeletionTaskSnapshot): boolean {
  if (['deleted', 'abandoned'].includes(task.phase)) return false;
  if (task.active === true) return true;
  if (task.phase === 'failed') return false;
  return true;
}

export function nodeDeletionLabel(task: NodeDeletionTaskSnapshot): string {
  if (task.abandonRequested && !['deleted', 'abandoned'].includes(task.phase)) return nodeDeletionActive(task) ? '正在放弃删除，等待安全收尾…' : '放弃删除未完成';
  return ({ waiting: task.waitingFor.length ? `等待${[...new Set(task.waitingFor)].join('、')}完成` : '等待已有操作完成', stopping: '正在关闭窗口并停止应用…', staging: '正在准备删除文件…', removing: '正在删除文件…', failed: '删除未完成', deleted: '删除已完成', abandoned: '已放弃删除' })[task.phase];
}

export function validNodeDeleteResult(value: NodeAppDeleteResult | null, task: Pick<NodeDeletionTaskSnapshot, 'appId' | 'ownership'>): boolean {
  return Boolean(value && value.appId === task.appId && value.operationOutcome === 'deleted' && value.ownership === task.ownership && value.sourcePreserved === true && value.evidenceRetained === true);
}

export function validNodeDeletionTask(value: NodeDeletionTaskSnapshot, appId?: string): boolean {
  const paths = (items: unknown): boolean => Array.isArray(items) && items.every(path => typeof path === 'string' && path.startsWith('/'));
  const targets = (items: unknown): boolean => Array.isArray(items) && items.every(item => item && typeof item.path === 'string' && item.path.startsWith('/') && typeof item.state === 'string'
    && (item.stagedPath == null || typeof item.stagedPath === 'string' && item.stagedPath.startsWith('/')) && (item.reason == null || typeof item.reason === 'string'));
  return Boolean(value && typeof value.taskId === 'string' && value.taskId && typeof value.appId === 'string' && value.appId && (!appId || value.appId === appId)
    && typeof value.name === 'string' && value.name && typeof value.targetRevision === 'string' && value.targetRevision
    && Number.isSafeInteger(value.journalRevision) && value.journalRevision >= 1
    && typeof value.active === 'boolean'
    && ['waiting', 'stopping', 'staging', 'removing', 'failed', 'deleted', 'abandoned'].includes(value.phase)
    && ['managed', 'external'].includes(value.ownership) && typeof value.sourcePath === 'string' && value.sourcePath.startsWith('/')
    && typeof value.renamedAny === 'boolean' && typeof value.canAbandon === 'boolean' && typeof value.canRetry === 'boolean'
    && (!value.renamedAny || !value.canAbandon) && (!['deleted', 'abandoned'].includes(value.phase) || !value.canAbandon && !value.canRetry)
    && (value.abandonRequested == null || typeof value.abandonRequested === 'boolean')
    && (!value.abandonRequested || !value.canAbandon && (!value.canRetry || value.phase === 'failed'))
    && Array.isArray(value.waitingFor) && value.waitingFor.every(name => typeof name === 'string' && name.trim())
    && targets(value.targets) && targets(value.remaining) && paths(value.preservedPaths)
    && (value.error == null || typeof value.error.code === 'string' && typeof value.error.message === 'string')
    && (value.phase !== 'deleted' || value.remaining.length === 0 && validNodeDeleteResult(value.result, value))
    && (value.phase !== 'abandoned' || !value.renamedAny) && (value.phase === 'deleted' || value.result == null));
}
