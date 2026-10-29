import type { NodeAppDetail } from './node-app-types';
import type { TaskRecord } from './types';
import { deletedSourceMatches, taskWasDeleted } from './project-deletion';
import type { ProjectDeletedNotice } from './project-deletion';
import { messageFrom } from './ui-utils';

export interface GitImportSnapshot {
  task: TaskRecord;
  appId: string;
  sourcePath: string | null;
  commitState: 'not_committed' | 'committed' | 'unknown';
  cleanupPending: boolean;
  remainingPaths: string[];
  app: NodeAppDetail | null;
}
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
interface Callbacks {
  isCurrent: () => boolean;
  onChange: () => void;
  onTask: (task: TaskRecord) => void;
  onRevoked?: () => void;
  pause?: () => Promise<void>;
}
export class SourceImportError extends Error {
  constructor(message: string, public recovery = false) { super(message); }
}

/** One backend transaction owns all writes, including cleanup after this view disappears. */
export class SourceImportSession {
  readonly importId = crypto.randomUUID();
  task: TaskRecord | null = null;
  snapshot: GitImportSnapshot | null = null;
  warning = '';
  private revoked = false;
  private started = false;
  constructor(private invoke: Invoke, private callbacks: Callbacks) {}
  private check() {
    if (this.revoked || !this.callbacks.isCurrent()) throw new SourceImportError('当前添加流程已失效。', true);
  }
  revoke(notice: ProjectDeletedNotice): boolean {
    if (!deletedSourceMatches(notice, { appId: this.snapshot?.appId, taskIds: [this.task?.taskId], sourceRoots: [this.snapshot?.sourcePath] })) return false;
    this.revoked = true; this.task = null; this.snapshot = null; this.warning = '';
    this.callbacks.onRevoked?.(); return true;
  }
  private acceptTask(task: TaskRecord) {
    this.check();
    if (taskWasDeleted(task)) { this.revoked = true; this.callbacks.onRevoked?.(); throw new SourceImportError('这个项目已经删除。', true); }
    if (!task || task.type !== 'nodeGitImport' || typeof task.taskId !== 'string' || typeof task.final !== 'boolean' || this.task && this.task.taskId !== task.taskId) throw new SourceImportError('添加任务身份无法核对，请勿重复添加。', true);
    if (this.task?.final && !task.final) return;
    this.task = task; this.callbacks.onTask(task); this.callbacks.onChange();
  }
  private accept(value: GitImportSnapshot) {
    this.check();
    if (!value || typeof value.appId !== 'string' || !['not_committed', 'committed', 'unknown'].includes(value.commitState)
      || typeof value.cleanupPending !== 'boolean' || !Array.isArray(value.remainingPaths)
      || value.remainingPaths.some(path => typeof path !== 'string') || value.sourcePath !== null && typeof value.sourcePath !== 'string'
      || this.snapshot && this.snapshot.appId !== value.appId || value.app && value.app.id !== value.appId) throw new SourceImportError('添加结果无法核对，请勿重复添加。', true);
    this.acceptTask(value.task); this.snapshot = value; this.callbacks.onChange(); return value;
  }
  async reconcile(): Promise<GitImportSnapshot> {
    this.check();
    const value = await this.invoke<GitImportSnapshot>('node_git_import_status', { importId: this.importId });
    this.check(); return this.accept(value);
  }
  async run(input: { url: string; branch: string | null; name?: string }): Promise<GitImportSnapshot> {
    this.check();
    if (this.started) throw new SourceImportError('这次添加已经受理，请核对原任务。', true);
    this.started = true;
    try {
      this.acceptTask(await this.invoke<TaskRecord>('node_git_import_start', { input: { importId: this.importId, ...input } }));
    } catch (cause) {
      this.check();
      const details = cause && typeof cause === 'object' && 'details' in cause ? cause.details : null;
      if (details && typeof details === 'object' && 'operationOutcome' in details && details.operationOutcome === 'not_started') {
        this.started = false;
        throw new SourceImportError(messageFrom(cause));
      }
      // A lost start response is resolved with the same importId, never a second submit.
      try { await this.reconcile(); }
      catch { throw new SourceImportError(`添加结果待核对：${messageFrom(cause)}`, true); }
    }
    return this.wait();
  }
  async wait(): Promise<GitImportSnapshot> {
    this.check();
    for (;;) {
      let value: GitImportSnapshot;
      try { value = await this.reconcile(); this.warning = ''; }
      catch (cause) {
        this.check();
        if (cause instanceof SourceImportError) throw cause;
        this.warning = `暂时读不到进度，仍在核对原任务：${messageFrom(cause)}`;
        this.callbacks.onChange();
        await this.pause(); continue;
      }
      if (value.task.final) {
        if (value.commitState === 'unknown') throw new SourceImportError('添加结果待核对，请勿重复添加。', true);
        if (value.commitState !== 'committed') throw new SourceImportError(messageFrom(value.task.error ?? '添加未完成。'));
        return value;
      }
      await this.pause();
    }
  }
  private async pause() {
    await (this.callbacks.pause?.() ?? new Promise<void>(resolve => window.setTimeout(resolve, 1000)));
    this.check();
  }
  async cancel() {
    this.check();
    if (!this.task || this.task.final) return;
    const value = await this.invoke<TaskRecord>('node_git_import_cancel', { taskId: this.task.taskId });
    this.check(); this.acceptTask(value);
  }
  async retryCleanup() {
    this.check();
    if (!this.task) return this.reconcile();
    const value = await this.invoke<GitImportSnapshot>('node_git_import_retry_cleanup', { taskId: this.task.taskId });
    this.check(); return this.accept(value);
  }
}
