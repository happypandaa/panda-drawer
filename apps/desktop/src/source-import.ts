import { installedResult, preparedResult } from './NodeInstallPanel';
import type { InstalledNodeResult, PreparedNodeInstall } from './NodeInstallPanel';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface SourceCandidate {
  sourceId: string; taskId: string; status: string; name: string; url: string;
  contentDigest?: string; buildSourceDigest?: string | null; commit?: string | null;
  sourcePath?: string;
  inspection?: { kind: string; name?: string; requiresBuild?: boolean; issues: unknown[] };
}
export interface ImportCleanupResult {
  completed: boolean; sourceId: string; sourceRetained: boolean; removedPaths: string[];
  failures: { path: string; error: unknown }[]; receiptPath: string;
}
type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
interface Callbacks {
  onTask: (task: TaskRecord) => void; onChange: () => void; isCurrent: () => boolean;
  pause?: () => Promise<void>;
}
export class SourceImportError extends Error {
  constructor(message: string, public recovery = false) { super(message); }
}
const record = (value: unknown): value is Record<string, unknown> => Boolean(value && typeof value === 'object' && !Array.isArray(value));

// Owns only the operation the user started in this dialog. Historical tasks are
// never resumed or installed by mounting the component or reading source_list.
export class SourceImportSession {
  sourceTaskId: string | null = null;
  source: SourceCandidate | null = null;
  candidate: PreparedNodeInstall | null = null;
  installed: InstalledNodeResult | null = null;
  task: TaskRecord | null = null;
  phase: 'download' | 'check' | 'prepare' | 'install' | 'cleanup' | 'done' = 'download';
  warning = '';
  cancelled = false;
  private cancelRequested = false;
  private cancelling = false;
  private uncertain = false;
  constructor(private invoke: Invoke, private callbacks: Callbacks) {}
  private check() {
    if (!this.callbacks.isCurrent()) throw new SourceImportError('当前连接已改变，请从源码记录核对之前的任务。', true);
  }
  private changed() { this.check(); this.callbacks.onChange(); }
  private async call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    this.check();
    const value = await this.invoke<T>(command, args);
    this.check();
    return value;
  }
  private accept(value: TaskRecord, type: string, id?: string) {
    if (!value || typeof value.taskId !== 'string' || value.type !== type || value.projectId !== null || typeof value.final !== 'boolean' || id && value.taskId !== id) {
      throw new SourceImportError('任务身份无法确认，请核对结果，勿重复导入。', true);
    }
    if (this.task?.taskId === value.taskId && this.task.final && !value.final) return;
    this.task = value; this.callbacks.onTask(value); this.changed();
  }
  private async wait(value: TaskRecord, type: string): Promise<TaskRecord> {
    this.accept(value, type);
    if (type === 'sourceAcquire') this.sourceTaskId = value.taskId;
    if (this.cancelRequested) await this.cancel();
    while (!this.task!.final) {
      await (this.callbacks.pause?.() ?? new Promise<void>(resolve => window.setTimeout(resolve, 1000)));
      this.check();
      if (this.task!.final) break;
      try {
        const next = await this.call<TaskRecord>('task_get', { taskId: value.taskId });
        this.accept(next, type, value.taskId);
        this.warning = ''; this.changed();
      } catch (cause) {
        this.check();
        // A malformed task identity cannot safely drive subsequent mutations.
        if (cause instanceof SourceImportError) throw cause;
        this.warning = `暂时读不到进度，仍在核对任务：${messageFrom(cause)}`; this.changed();
      }
    }
    const final = this.task!;
    if (final.result?.processResourceClosed === false || final.error?.details?.processResourceClosed === false || final.error?.details?.candidateOwnershipConfirmed === false || final.error?.code === 'NODE_INSTALL_CLEANUP_PENDING') {
      throw new SourceImportError('任务进程尚未确认停止，请先在源码记录中核对，暂不能重试或清理。', true);
    }
    return final;
  }
  private stopped(task: TaskRecord) {
    if (this.cancelRequested || ['SOURCE_CANCELLED', 'SOURCE_ACQUISITION_CANCELLED', 'NODE_INSTALL_CANCELLED'].includes(task.error?.code ?? '')) {
      this.cancelled = true;
      throw new SourceImportError('导入已取消，可以清理本次临时文件后关闭。');
    }
    if (task.status !== 'succeeded') throw new SourceImportError(messageFrom(task.error ?? '任务未完成。'));
  }
  async acquire(input: { url: string; branch: string | null; name?: string }): Promise<SourceCandidate> {
    this.phase = 'download'; this.changed();
    let accepted: TaskRecord;
    try { accepted = await this.call<TaskRecord>('source_acquire', { input }); }
    catch (cause) {
      this.check();
      const details = record(cause) && record(cause.details) ? cause.details : null;
      if (typeof details?.taskId === 'string') this.sourceTaskId = details.taskId;
      // A structured rejection is actionable; an IPC/network failure can have
      // lost an accepted task and must not immediately enable a second submit.
      throw new SourceImportError(messageFrom(cause), !record(cause) || typeof cause.code !== 'string' || Boolean(this.sourceTaskId));
    }
    const task = await this.wait(accepted, 'sourceAcquire');
    this.stopped(task);
    this.phase = 'check'; this.changed();
    const value = task.result;
    if (!record(value) || value.taskId !== task.taskId || typeof value.sourceId !== 'string' || !/^src_[a-f0-9]{32}$/.test(value.sourceId) || value.status !== 'ready') {
      throw new SourceImportError('下载结果不完整，请在源码记录中核对本次任务。', true);
    }
    this.source = value as unknown as SourceCandidate;
    if (this.source.inspection?.kind !== 'node-service-source') {
      const reasons = this.source.inspection?.issues.map(messageFrom).join('；');
      throw new SourceImportError(`这个仓库目前不能直接导入为应用。${reasons || '需要先按平台应用格式适配，再重新导入。'}`);
    }
    if (!/^[a-f0-9]{64}$/.test(this.source.contentDigest ?? '')) throw new SourceImportError('源码校验信息不完整，请核对下载结果。', true);
    return this.source;
  }
  async install(buildApproved: boolean): Promise<InstalledNodeResult> {
    this.check();
    if (!this.source || this.source.inspection?.kind !== 'node-service-source' || this.candidate || this.installed || this.uncertain) throw new SourceImportError('导入状态需要核对，不能重复安装。', true);
    if (this.cancelRequested) { this.cancelled = true; throw new SourceImportError('导入已取消。'); }
    const source = this.source;
    const requiresBuild = source.inspection?.requiresBuild === true && !source.buildSourceDigest;
    if (requiresBuild && !buildApproved) throw new SourceImportError('需要先确认构建此应用。');
    this.phase = 'prepare'; this.changed();
    let accepted: TaskRecord;
    try {
      accepted = await this.call<TaskRecord>('node_install_prepare', { input: { sourceId: source.sourceId, expectedSourceDigest: source.contentDigest, targetAppId: null, ...(requiresBuild ? { buildSource: true } : {}) } });
    } catch (cause) { this.check(); throw new SourceImportError(messageFrom(cause), true); }
    const task = await this.wait(accepted, 'nodeInstallPrepare');
    this.stopped(task);
    const prepared = task.result?.state === 'prepared' && task.result.installed === false ? preparedResult(task.result.candidate) : null;
    if (!prepared || prepared.provenance.sourceId !== source.sourceId || prepared.provenance.sourceDigest !== source.contentDigest || prepared.provenance.sourceCommit !== (source.commit ?? null)) {
      throw new SourceImportError('安装材料与本次下载的源码不一致，已停止导入。', true);
    }
    this.candidate = prepared;
    const fresh = await this.call<{ state: string; candidate?: unknown }>('node_install_candidate', { candidateId: prepared.candidateId });
    const checked = fresh.state === 'prepared' ? preparedResult(fresh.candidate) : null;
    if (!checked || JSON.stringify(checked) !== JSON.stringify(prepared)) throw new SourceImportError('安装材料已变化，已停止导入，请核对记录。', true);
    if (this.cancelRequested) { this.cancelled = true; throw new SourceImportError('导入已取消。'); }
    this.phase = 'install'; this.changed();
    this.uncertain = true;
    let applied: TaskRecord;
    try { applied = await this.wait(await this.call<TaskRecord>('node_install_apply', { candidateId: checked.candidateId }), 'nodeInstallApply'); }
    catch (cause) { this.check(); throw new SourceImportError(`正在核对安装结果：${messageFrom(cause)}`, true); }
    const result = installedResult(applied.result);
    if (result) return this.acceptInstalled(result, applied.status !== 'succeeded');
    const outcome = applied.result?.operationOutcome ?? applied.error?.details?.operationOutcome;
    if (outcome === 'not-committed') this.uncertain = false;
    throw new SourceImportError(messageFrom(applied.error ?? '安装结果尚不能确认。'), this.uncertain);
  }
  private acceptInstalled(result: InstalledNodeResult, needsAttention = false) {
    const candidate = this.candidate;
    if (!candidate || result.candidateId !== candidate.candidateId || result.appId !== candidate.appId || result.generation !== candidate.generation) throw new SourceImportError('安装结果身份不符，请核对记录。', true);
    this.installed = result; this.uncertain = false; this.phase = 'cleanup';
    this.warning = needsAttention ? '应用已经导入，安装收尾需要检查。请保留记录，勿重复导入。' : '';
    this.changed(); return result;
  }
  async reconcile(): Promise<InstalledNodeResult> {
    if (!this.candidate) throw new SourceImportError('请从“源码记录与恢复”查看本次任务，确认结果后继续。', true);
    const value = await this.call<{ state: string; result?: unknown }>('node_install_candidate', { candidateId: this.candidate.candidateId });
    const result = value.state === 'committed' ? installedResult(value.result) : null;
    if (!result) throw new SourceImportError('尚未确认安装完成，请从源码记录核对任务，勿重复导入。', true);
    return this.acceptInstalled(result);
  }
  async cancel() {
    this.cancelRequested = true;
    if (this.cancelling || this.phase === 'install' || !this.task || this.task.final) return;
    const task = this.task;
    const command = task.type === 'sourceAcquire' ? 'source_cancel' : task.type === 'nodeInstallPrepare' ? 'node_install_cancel' : null;
    if (!command) return;
    this.cancelling = true;
    try { this.accept(await this.call<TaskRecord>(command, { taskId: task.taskId }), task.type, task.taskId); }
    finally { this.cancelling = false; }
  }
  async cleanup(): Promise<ImportCleanupResult> {
    if (this.uncertain || this.task && !this.task.final) throw new SourceImportError('任务结果尚未确认，暂不能清理。', true);
    if (!this.sourceTaskId) throw new SourceImportError('没有本次下载任务可供清理。');
    if (this.installed) { this.phase = 'cleanup'; this.changed(); }
    const value = await this.call<ImportCleanupResult>('source_import_cleanup', { sourceTaskId: this.sourceTaskId });
    if (!value || typeof value.completed !== 'boolean' || typeof value.sourceId !== 'string' || this.source && value.sourceId !== this.source.sourceId || typeof value.sourceRetained !== 'boolean' || !Array.isArray(value.removedPaths) || !Array.isArray(value.failures) || typeof value.receiptPath !== 'string') {
      throw new SourceImportError('清理结果不完整，请重试清理并核对记录。');
    }
    if (!value.completed) throw new SourceImportError(`部分临时文件未清理：${value.failures.map(item => messageFrom(item.error)).join('；') || '请重试清理。'}`);
    if (this.installed) { this.phase = 'done'; this.changed(); }
    return value;
  }
}
