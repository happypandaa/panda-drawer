import type { GitImportSnapshot } from './source-import';
import type { TaskRecord } from './types';

/** Presentation of the existing import session; it never starts or polls work. */
export interface GitImportViewState {
  sessionId: string;
  url: string;
  name: string;
  task: TaskRecord | null;
  snapshot: GitImportSnapshot | null;
  view: 'running' | 'failed' | 'unknown' | 'success';
  busy: boolean;
  error: string;
  warning: string;
}

export function gitImportNeedsAttention(state: GitImportViewState | null) {
  return Boolean(state && (state.view === 'failed' || state.view === 'unknown' || state.snapshot?.cleanupPending));
}

/** Dismissing a failed card must not discard uncertain writes or cleanup. */
export function canDismissGitImport(state: GitImportViewState) {
  if (state.busy || state.view !== 'failed') return false;
  if (!state.task && !state.snapshot) return true; // Definite pre-admission rejection.
  return Boolean(state.task?.final && state.snapshot?.commitState === 'not_committed'
    && !state.snapshot.cleanupPending && state.snapshot.remainingPaths.length === 0);
}

export function gitImportLabel(state: GitImportViewState) {
  if (state.view === 'unknown') return '添加结果待核对';
  if (state.snapshot?.cleanupPending && !state.busy) return state.snapshot.commitState === 'committed' ? '已添加 · 清理待处理' : '清理待处理';
  if (state.view === 'failed') return '添加失败';
  if (state.view === 'success') return '已添加';
  const labels: Record<string, string> = {
    queued: '等待下载', downloading: '下载中', checking: '检查中', publishing: '准备项目文件夹',
    dependencies: '准备依赖中', registering: '添加中', cleanup: '清理临时文件中', completed: '正在核对结果',
  };
  return labels[state.task?.phase ?? 'queued'] ?? '添加中';
}
