import { useId } from 'react';
import { bytesText } from './storage-utils';
import { messageFrom } from './ui-utils';

type FileState = { sha256: string; size: number; executable: boolean };
export interface SourceRestorePreview {
  repositoryRoot: string;
  head: { oid: string; branch: string };
  targetCommit: string;
  snapshotDigest: string;
  planDigest: string;
  changes: { path: string; kind: 'add' | 'modify' | 'delete'; before: FileState | null; after: FileState | null }[];
  totalFiles: number;
  totalBytes: number;
  canApply: boolean;
  issues: unknown[];
  git: { path: string; version: string };
  database: { state: string; reason?: string };
}
export interface SourceRestoreOperation {
  restoreTaskId: string;
  targetCommit: string;
  phase: string;
  planDigest: string;
  journalDigest: string;
  canContinue: boolean;
  canUndo: boolean;
  changes: { path: string; kind: 'add' | 'modify' | 'delete'; state: 'before' | 'after' | 'unknown' }[];
  issues: unknown[];
}
export interface SourceRestoreStatus {
  state: 'clear' | 'pending' | 'blocked';
  operations: SourceRestoreOperation[];
  issues: unknown[];
  database: { state: string; reason?: string };
}
export const sourceRestoreTaskTypes = ['gitSourceRestoreApply', 'gitSourceRestoreContinue', 'gitSourceRestoreUndo'];
const changes = { add: '新增', modify: '修改', delete: '删除' };
const states = { before: '取回前内容', after: '取回后的内容', unknown: '内容尚不能确认' };

export function ProjectGitSourceRestore({ targetCommit, onTargetChange, preview, recovery, recoveryFresh, recoveryError, busy, canPreview, canApply, canRecover, canRefresh, stopped, confirmed, onStoppedChange, onConfirmedChange, onPreview, onApply, onRefresh, onRecover }: {
  targetCommit: string; onTargetChange: (value: string) => void;
  preview: SourceRestorePreview | null; recovery: SourceRestoreStatus | null; recoveryFresh: boolean; recoveryError: string;
  busy: boolean; canPreview: boolean; canApply: boolean; canRecover: boolean; canRefresh: boolean;
  stopped: boolean; confirmed: boolean; onStoppedChange: (value: boolean) => void; onConfirmedChange: (value: boolean) => void;
  onPreview: () => void; onApply: () => void; onRefresh: () => void;
  onRecover: (operation: SourceRestoreOperation, action: 'continue' | 'undo') => void;
}) {
  const id = useId();
  return <section aria-label="取回已保存源码">
    <h4>取回已保存源码</h4>
    <p>将本机已保存版本中的允许文件取回为待保存修改。当前分支、Git 提交和暂存区保持不变；不会自动保存版本或构建。数据库、附件和当前可用程序保持原状。</p>
    <p>先停止 Codex、其他编辑器及其他 Git 操作对本项目的修改，并保持停止直到本次操作结束。平台不能阻止外部程序继续写入；未保存修改、未知文件、危险路径或不同的项目清单会阻止取回。</p>
    <label><input type="checkbox" checked={stopped} disabled={busy} onChange={event => onStoppedChange(event.target.checked)} />我已停止本项目的外部编辑和 Git 操作</label>
    <div className="storage-heading"><h4>源码恢复状态</h4><button type="button" className="refresh-button" disabled={!canRefresh} onClick={onRefresh}>重新核对恢复状态</button></div>
    {recoveryError && <p className="task-error" role="alert">{recoveryError}</p>}
    {!recoveryFresh && <p className="maintenance-warning">恢复状态尚未核实或已过期，先重新核对再操作。</p>}
    {recovery?.state === 'clear' && recoveryFresh && <p>没有待处理的源码取回。</p>}
    {recovery && recovery.state !== 'clear' && <p className="maintenance-warning">源码取回尚未处理完成，暂停构建、保存源码和新的 AI 接手。当前程序和业务数据仍可使用，本机版本列表仍可查看。</p>}
    {recovery?.issues.map((issue, index) => <p className="task-error" role="alert" key={index}>{messageFrom(issue)}</p>)}
    {recovery?.operations.map(operation => <div className="operation-confirmation" key={operation.restoreTaskId}>
      <p>待处理取回：<code>{operation.restoreTaskId}</code></p><p>目标源码版本：<code>{operation.targetCommit}</code></p>
      <details><summary>核对恢复文件（{operation.changes.length}）</summary><ul>{operation.changes.map(change => <li key={change.path}><code>{change.path}</code> · {changes[change.kind]} · {states[change.state]}</li>)}</ul></details>
      {operation.issues.map((issue, index) => <p className="task-error" role="alert" key={index}>{messageFrom(issue)}</p>)}
      <p>继续会完成这次取回；撤回会恢复这次操作之前的源码。两者都需要核对当前现场，不会自动执行。</p>
      <div className="confirmation-actions">
        {operation.canContinue && <button type="button" className="primary-button" disabled={!canRecover || !stopped} onClick={() => onRecover(operation, 'continue')}>确认继续取回</button>}
        {operation.canUndo && <button type="button" className="refresh-button" disabled={!canRecover || !stopped} onClick={() => onRecover(operation, 'undo')}>确认撤回这次取回</button>}
      </div>
      {!operation.canContinue && !operation.canUndo && <p>目前没有已核验可执行的恢复操作，请保留现场并处理上方问题。</p>}
    </div>)}
    <label htmlFor={`${id}-target`}>要取回的本机提交</label>
    <input id={`${id}-target`} value={targetCommit} onChange={event => onTargetChange(event.target.value)} readOnly={busy} maxLength={64} autoComplete="off" autoCapitalize="none" spellCheck={false} placeholder="从本机版本列表选择，或填写完整提交 ID" />
    <p>本次取回仅支持完整的 40 位小写 SHA-1 提交 ID，不接受分支名、标签或缩写。</p>
    {targetCommit && !/^[0-9a-f]{40}$/.test(targetCommit) && <p role="alert">这个提交 ID 不符合本次取回范围；64 位提交仍可在版本列表中查看。</p>}
    <button type="button" className="refresh-button" disabled={!canPreview || !stopped || !/^[0-9a-f]{40}$/.test(targetCommit)} onClick={onPreview}>预览取回范围</button>
    {preview && <div className="operation-confirmation" aria-label="源码取回预览">
      <p>项目仓库：<code>{preview.repositoryRoot}</code></p><p>取回来源：<code>{preview.targetCommit}</code></p><p>当前分支：<code>{preview.head.branch.replace(/^refs\/heads\//, '')}</code> · 当前提交：<code>{preview.head.oid}</code></p>
      <p>目标源码范围 {preview.totalFiles} 个文件 · {bytesText(preview.totalBytes)}；本次变更 {preview.changes.length} 个文件</p>
      <ul>{preview.changes.map(change => <li key={change.path}><code>{change.path}</code> · {changes[change.kind]}</li>)}</ul>
      {preview.issues.map((issue, index) => <p className="task-error" role="alert" key={index}>{messageFrom(issue)}</p>)}
      {!preview.canApply && <p>本次预览不允许应用，请先处理列出的问题。</p>}
      <label><input type="checkbox" checked={confirmed} disabled={busy || !preview.canApply} onChange={event => onConfirmedChange(event.target.checked)} />我已核对文件范围，确认取回为待保存修改</label>
      <button type="button" className="primary-button" disabled={!canApply || !stopped || !confirmed || !preview.canApply} onClick={onApply}>确认取回源码</button>
    </div>}
  </section>;
}
