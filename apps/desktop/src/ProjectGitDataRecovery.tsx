import { useState } from 'react';
import { messageFrom } from './ui-utils';

export const dataRestoreTaskTypes = ['gitDataRestoreApply', 'gitDataRestoreContinue', 'gitDataRestoreUndo'];

export interface DataRestoreOperation {
  restoreTaskId: string;
  targetCommit: string;
  planDigest: string;
  journalDigest: string;
  phase: string;
  dataState: 'before' | 'applied' | 'undone' | 'unknown';
  documentEpoch: string | null;
  operationOutcome: 'not_started' | 'not_committed' | 'committed' | 'unknown';
  canContinue: boolean;
  canUndo: boolean;
  slots: { kind: 'database' | 'wal' | 'shm' | 'journal' | 'attachments'; path: string; state: 'before' | 'displaced' | 'applied' | 'absent' | 'unknown' }[];
  completionPublished: boolean;
  originalRetained: boolean;
  retainedPath: string;
  issues: unknown[];
}

export interface DataRestoreRecovery {
  state: 'clear' | 'pending' | 'blocked';
  businessBlocked: boolean;
  operations: DataRestoreOperation[];
  issues: unknown[];
}

export function ProjectGitDataRecovery({ status, fresh, disabled, error, onRecover }: {
  status: DataRestoreRecovery | null;
  fresh: boolean;
  disabled: boolean;
  error: string;
  onRecover: (operation: DataRestoreOperation, action: 'continue' | 'undo') => void;
}) {
  return <>
    {error && <p role="alert" className="task-error">{error}</p>}
    {status && (status.state !== 'clear' || status.businessBlocked) && <section aria-label="业务数据取回恢复" className="operation-confirmation">
      <h4>数据取回需要处理</h4>
      <p>当前业务访问暂时停止。请核对保留的现场，再明确继续或撤回；刷新状态不会自动替换数据或删除现场。</p>
      {!fresh && <p role="status">需要刷新并核对最新恢复状态，才能继续操作。</p>}
      {status.issues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}
      {status.operations.map(operation => <DataRestoreOperationRow
        key={`${operation.restoreTaskId}:${operation.planDigest}:${operation.journalDigest}`}
        operation={operation} disabled={disabled || !fresh} onRecover={onRecover} />)}
    </section>}
  </>;
}

function DataRestoreOperationRow({ operation, disabled, onRecover }: {
  operation: DataRestoreOperation;
  disabled: boolean;
  onRecover: (operation: DataRestoreOperation, action: 'continue' | 'undo') => void;
}) {
  const [confirmed, setConfirmed] = useState(false);
  return <div>
    <p>数据取回任务：<code>{operation.restoreTaskId}</code></p>
    <p>所选版本：<code>{operation.targetCommit}</code></p>
    {operation.operationOutcome === 'committed' && <p>数据切换已提交，现在只可按已提交方向完成收尾。请勿再次提交同一取回。</p>}
    <p>{operation.originalRetained ? '原数据的恢复现场仍保留。' : '原数据的保留情况需要继续核对，请查看处理信息。'}</p>
    {operation.issues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}
    <details><summary>查看恢复现场</summary>
      <p><code>{operation.retainedPath}</code></p>
      <p>阶段：{operation.phase}</p>
      <ul>{operation.slots.map((entry, index) => <li key={`${entry.kind}:${entry.path}:${index}`}><code>{entry.path}</code> · {{ before: '仍在原位置', displaced: '已移到保留现场', applied: '已放入所选版本的内容', absent: '不存在', unknown: '需要核对' }[entry.state]}</li>)}</ul>
    </details>
    {(operation.canContinue || operation.canUndo) && <>
      <label><input type="checkbox" checked={confirmed} disabled={disabled} onChange={event => setConfirmed(event.target.checked)} />已核对本次恢复范围，并停止外部编辑和数据写入</label>
      <div className="confirmation-actions">
        <button type="button" className="primary-button" disabled={disabled || !confirmed || !operation.canContinue} onClick={() => { if (!disabled && confirmed && operation.canContinue) onRecover(operation, 'continue'); }}>继续并核对数据取回</button>
        <button type="button" className="refresh-button" disabled={disabled || !confirmed || !operation.canUndo} onClick={() => { if (!disabled && confirmed && operation.canUndo) onRecover(operation, 'undo'); }}>撤回本次数据取回</button>
      </div>
    </>}
  </div>;
}
