import { t } from './i18n';
import type { NodeAppDetail } from './node-app-types';
import type { ProjectInfo, TaskRecord } from './types';
import { deletedSourceMatches, taskWasDeleted } from './project-deletion';
import type { ProjectDeletedNotice } from './project-deletion';
import { messageFrom } from './ui-utils';
import { gitRepositoryAddress } from './git-repository';

export interface GitImportSnapshot {
  task: TaskRecord;
  appId: string;
  sourcePath: string | null;
  commitState: 'not_committed' | 'committed' | 'unknown';
  cleanupPending: boolean;
  remainingPaths: string[];
  app: NodeAppDetail | ProjectInfo | null;
  runtime?: 'web' | 'node' | null;
  webProjectId?: string | null;
  buildConfirmation?: { revision: string; sourcePath: string; sourceUrl: string; sourceDir: string; script: string } | null;
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
  private sourceUrl: string | null = null;
  constructor(private invoke: Invoke, private callbacks: Callbacks) {}
  private check() {
    if (this.revoked || !this.callbacks.isCurrent()) throw new SourceImportError(t("当前添加流程已失效。"), true);
  }
  revoke(notice: ProjectDeletedNotice): boolean {
    if (!deletedSourceMatches(notice, { appId: this.snapshot?.appId, taskIds: [this.task?.taskId], sourceRoots: [this.snapshot?.sourcePath] })) return false;
    this.revoked = true; this.task = null; this.snapshot = null; this.warning = '';
    this.callbacks.onRevoked?.(); return true;
  }
  private acceptTask(task: TaskRecord) {
    this.check();
    if (taskWasDeleted(task)) { this.revoked = true; this.callbacks.onRevoked?.(); throw new SourceImportError(t("这个项目已经删除。"), true); }
    if (!task || task.type !== 'nodeGitImport' || typeof task.taskId !== 'string' || typeof task.final !== 'boolean' || this.task && this.task.taskId !== task.taskId) throw new SourceImportError(t("添加任务身份无法核对，请勿重复添加。"), true);
    if (this.task?.final && !task.final) return;
    this.task = task; this.callbacks.onTask(task); this.callbacks.onChange();
  }
  private accept(value: GitImportSnapshot) {
    this.check();
    if (!value || !value.task || typeof value.appId !== 'string' || !['not_committed', 'committed', 'unknown'].includes(value.commitState)
      || typeof value.cleanupPending !== 'boolean' || !Array.isArray(value.remainingPaths)
      || value.remainingPaths.some(path => typeof path !== 'string') || value.sourcePath !== null && typeof value.sourcePath !== 'string'
      || value.runtime != null && !['web', 'node'].includes(value.runtime)
      || value.webProjectId != null && !/^prj_[a-f0-9]{32}$/.test(value.webProjectId)
      || value.app && (value.app.id !== value.appId || !['web', 'node'].includes(value.app.runtime) || typeof value.app.name !== 'string')
      || value.app && value.runtime != null && value.app.runtime !== value.runtime) throw new SourceImportError(t("添加结果无法核对，请勿重复添加。"), true);
    if (this.snapshot?.webProjectId && value.webProjectId !== this.snapshot.webProjectId) throw new SourceImportError(t("网页实例身份发生变化，请重新核对原任务。"), true);
    if (this.snapshot && this.snapshot.appId !== value.appId) {
      const converted = value.runtime === 'web' && value.webProjectId === value.appId && !this.snapshot.appId.startsWith('prj_')
        && (value.task.projectId === value.appId || value.task.final && value.commitState === 'not_committed' && value.task.projectId === null);
      if (!converted) throw new SourceImportError(t("添加的实例身份发生变化，请重新核对原任务。"), true);
    }
    const confirmation = value.buildConfirmation;
    if (confirmation != null && (value.runtime !== 'web' || value.task.status !== 'waitingConfirmation' || value.task.final
      || value.commitState !== 'not_committed' || value.webProjectId !== value.appId || value.task.projectId !== value.appId
      || typeof confirmation.revision !== 'string' || !confirmation.revision
      || typeof confirmation.sourcePath !== 'string' || confirmation.sourcePath !== value.sourcePath
      || typeof confirmation.sourceUrl !== 'string' || this.sourceUrl !== null && confirmation.sourceUrl !== this.sourceUrl
      || typeof confirmation.sourceDir !== 'string' || !confirmation.sourceDir || typeof confirmation.script !== 'string' || !confirmation.script)) {
      throw new SourceImportError(t("构建确认内容无法核对，请重新核对原任务。"), true);
    }
    if (value.task.status === 'waitingConfirmation' && !confirmation) throw new SourceImportError(t("缺少可核对的构建确认内容。"), true);
    if (this.snapshot?.task.final && !value.task.final) return this.snapshot;
    this.acceptTask(value.task); this.snapshot = value; this.callbacks.onChange(); return value;
  }
  async reconcile(): Promise<GitImportSnapshot> {
    this.check();
    const value = await this.invoke<GitImportSnapshot>('node_git_import_status', { importId: this.importId });
    this.check(); return this.accept(value);
  }
  async run(input: { url: string; branch: string | null; name?: string }): Promise<GitImportSnapshot> {
    this.check();
    if (this.started) throw new SourceImportError(t("这次添加已经受理，请核对原任务。"), true);
    this.started = true;
    this.sourceUrl = gitRepositoryAddress(input.url)?.url ?? input.url;
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
      catch { throw new SourceImportError(t("添加结果待核对：{p0}", { p0: messageFrom(cause) }), true); }
    }
    return this.wait();
  }
  async wait(continueAfterConfirmation = false): Promise<GitImportSnapshot> {
    this.check();
    for (;;) {
      let value: GitImportSnapshot;
      try { value = await this.reconcile(); this.warning = ''; }
      catch (cause) {
        this.check();
        if (cause instanceof SourceImportError) throw cause;
        this.warning = t("暂时读不到进度，仍在核对原任务：{p0}", { p0: messageFrom(cause) });
        this.callbacks.onChange();
        await this.pause(); continue;
      }
      if (value.task.final) {
        if (value.commitState === 'unknown') throw new SourceImportError(t("添加结果待核对，请勿重复添加。"), true);
        if (value.commitState !== 'committed') throw new SourceImportError(messageFrom(value.task.error ?? t("添加未完成。")));
        return value;
      }
      if (!continueAfterConfirmation && value.task.status === 'waitingConfirmation') return value;
      await this.pause();
    }
  }
  private async pause() {
    await (this.callbacks.pause?.() ?? new Promise<void>(resolve => window.setTimeout(resolve, 1000)));
    this.check();
  }
  async confirmBuild(expectedRevision: string): Promise<GitImportSnapshot> {
    this.check();
    if (!expectedRevision || this.snapshot?.buildConfirmation?.revision !== expectedRevision) throw new SourceImportError(t("构建确认已变化，请先重新核对。"), true);
    const current = await this.reconcile();
    if (current.task.status !== 'waitingConfirmation') return current.task.final ? current : this.wait();
    if (current.buildConfirmation?.revision !== expectedRevision) {
      this.warning = t("项目文件或构建说明已变化，请核对新的确认内容后再决定。"); this.callbacks.onChange(); return current;
    }
    try {
      this.acceptTask(await this.invoke<TaskRecord>('node_git_import_confirm_build', { importId: this.importId, expectedRevision }));
    } catch (cause) {
      this.check();
      try {
        const value = await this.reconcile();
        if (value.task.status === 'waitingConfirmation') {
          this.warning = t("构建尚待确认：{p0}", { p0: messageFrom(cause) }); this.callbacks.onChange(); return value;
        }
        return value.task.final ? value : this.wait();
      } catch { throw new SourceImportError(t("构建确认结果待核对：{p0}", { p0: messageFrom(cause) }), true); }
    }
    return this.wait();
  }
  async cancel(waitUntilClosed = true): Promise<GitImportSnapshot | null> {
    this.check();
    if (!this.task || this.task.final) return this.snapshot;
    const value = await this.invoke<TaskRecord>('node_git_import_cancel', { taskId: this.task.taskId });
    this.check(); this.acceptTask(value);
    return waitUntilClosed ? this.wait(true) : this.snapshot;
  }
  async retryCleanup() {
    this.check();
    if (!this.task) return this.reconcile();
    const value = await this.invoke<GitImportSnapshot>('node_git_import_retry_cleanup', { taskId: this.task.taskId });
    this.check(); return this.accept(value);
  }
}
