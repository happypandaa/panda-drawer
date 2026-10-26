import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import { sourceRestoreTaskTypes } from './ProjectGitSourceRestore';
import type { SourceRestoreOperation, SourceRestoreStatus } from './ProjectGitSourceRestore';
import type { MaintenanceStatus, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

interface Props {
  projectId: string;
  instanceId?: string;
  status?: MaintenanceStatus;
  task?: TaskRecord | null;
  disabled?: boolean;
  onTask: (task: TaskRecord) => void;
  onChanged: () => Promise<void> | void;
  onPendingChange?: (pending: boolean) => void;
}

const recoveryTaskTypes = [...sourceRestoreTaskTypes];
function relevantTask(task: TaskRecord | null | undefined, projectId: string): task is TaskRecord {
  return Boolean(task && task.projectId === projectId && recoveryTaskTypes.includes(task.type));
}
function verifiedStatus(value: SourceRestoreStatus) {
  return value && ['clear', 'pending', 'blocked'].includes(value.state) && Array.isArray(value.operations) && Array.isArray(value.issues)
    && (value.state !== 'clear' || value.operations.length === 0)
    && value.operations.every(operation => typeof operation.restoreTaskId === 'string' && Boolean(operation.restoreTaskId)
      && /^[0-9a-f]{64}$/.test(operation.planDigest) && /^[0-9a-f]{64}$/.test(operation.journalDigest)
      && typeof operation.canContinue === 'boolean' && typeof operation.canUndo === 'boolean'
      && Array.isArray(operation.changes) && Array.isArray(operation.issues));
}

// Remount requests and confirmations together when the project or service changes.
export function ProjectOperationRecovery(props: Props) {
  return <OperationRecoveryInstance key={JSON.stringify([props.projectId, props.instanceId])} {...props} />;
}

function OperationRecoveryInstance({ projectId, status, task: externalTask, disabled = false, onTask, onChanged, onPendingChange }: Props) {
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  const [recovery, setRecovery] = useState<SourceRestoreStatus | null>(null);
  const [fresh, setFresh] = useState(false);
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [readError, setReadError] = useState('');
  const [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [stopped, setStopped] = useState(false);
  const [confirmation, setConfirmation] = useState<{ operation: SourceRestoreOperation; action: 'continue' | 'undo' } | null>(null);
  const [task, setTask] = useState<TaskRecord | null>(() => relevantTask(externalTask, projectId) ? externalTask : null);
  const alive = useRef(false);
  const generation = useRef(0);
  const sequence = useRef(0);
  const readPending = useRef(false);
  const submitting = useRef(false);
  const polling = useRef(false);
  const taskRef = useRef(task);
  const callbacks = useRef({ onTask, onChanged, onPendingChange });
  callbacks.current = { onTask, onChanged, onPendingChange };
  const statusRef = useRef(status); statusRef.current = status;
  const expandedRef = useRef(expanded); expandedRef.current = expanded;
  const current = useCallback((value: number) => alive.current && value === generation.current, []);
  const knownStatus = status && (!status.projectId || status.projectId === projectId) ? status : undefined;
  const maintenance = knownStatus?.mode === 'maintenance';
  const needsRecovery = Boolean(knownStatus?.recoveryRequired);
  const active = Boolean(task && !task.final);
  const waiting = reading || busy || active;
  const blocked = disabled || waiting || Boolean(knownStatus?.activeTask);

  const acceptTask = useCallback((value: TaskRecord, notify = true) => {
    const previous = taskRef.current;
    if (previous?.taskId === value.taskId && previous.final && !value.final) return false;
    if (previous && !previous.final && previous.taskId !== value.taskId) return false;
    const becameFinal = value.final && !(previous?.taskId === value.taskId && previous.final);
    taskRef.current = value; setTask(value);
    if (notify) callbacks.current.onTask(value);
    return becameFinal;
  }, []);

  const read = useCallback(async (withTask = false) => {
    if (readPending.current || taskRef.current && !taskRef.current.final) return;
    const lifetime = generation.current; const request = ++sequence.current;
    readPending.current = true; setReading(true); setFresh(false); setConfirmation(null); setStopped(false);
    const valid = () => current(lifetime) && request === sequence.current;
    try {
      const value = await invoke<SourceRestoreStatus>('project_git_source_restore_status', { id: projectId });
      if (!valid()) return;
      if (!verifiedStatus(value)) throw new Error('返回的操作状态不完整，请保留现场并重新核对。');
      setRecovery(value); setFresh(true); setReadError('');
      const control = statusRef.current;
      const taskId = control?.activeTask ? typeof control.activeTask === 'string' ? control.activeTask : control.activeTask.taskId
        : value.operations[0]?.restoreTaskId ?? control?.lastTaskId;
      if (withTask && taskId && taskRef.current?.taskId !== taskId) {
        try {
          const record = await invoke<TaskRecord>('task_get', { taskId });
          if (!valid()) return;
          if (record.taskId !== taskId || !relevantTask(record, projectId)) throw new Error('操作任务身份不符，请保留现场并重新核对。');
          acceptTask(record, false); setPollError('');
        } catch (cause) { if (valid()) setPollError(`操作日志暂时无法读取：${messageFrom(cause)}`); }
      }
    } catch (cause) { if (valid()) setReadError(messageFrom(cause)); }
    finally { if (valid()) { readPending.current = false; setReading(false); } }
  }, [projectId, current, acceptTask]);

  const refreshAfterTask = useCallback(async () => {
    const lifetime = generation.current;
    setReading(true);
    try { await callbacks.current.onChanged(); }
    catch (cause) { if (current(lifetime)) setError(`应用状态暂时无法刷新：${messageFrom(cause)}`); }
    if (current(lifetime)) await read(expandedRef.current);
  }, [current, read]);

  const poll = useCallback(async () => {
    const selected = taskRef.current;
    if (!selected || selected.final || polling.current) return;
    const lifetime = generation.current;
    polling.current = true;
    try {
      const value = await invoke<TaskRecord>('task_get', { taskId: selected.taskId });
      if (!current(lifetime) || taskRef.current?.taskId !== selected.taskId) return;
      if (value.taskId !== selected.taskId || value.projectId !== projectId || value.type !== selected.type) throw new Error('操作任务身份不符，请重新核对，勿重复提交。');
      setPollError('');
      if (acceptTask(value)) await refreshAfterTask();
    } catch (cause) { if (current(lifetime)) setPollError(`任务状态暂时无法读取：${messageFrom(cause)}。请保留此页，勿重复提交。`); }
    finally { if (current(lifetime)) polling.current = false; }
  }, [projectId, current, acceptTask, refreshAfterTask]);

  useEffect(() => {
    alive.current = true; generation.current += 1; readPending.current = false; polling.current = false;
    if (taskRef.current && !taskRef.current.final) void poll(); else void read();
    return () => { alive.current = false; generation.current += 1; sequence.current += 1; callbacks.current.onPendingChange?.(false); };
  }, [read, poll]);
  useEffect(() => {
    if (!relevantTask(externalTask, projectId) || externalTask === taskRef.current) return;
    const previous = taskRef.current;
    if (previous?.taskId === externalTask.taskId && previous.final || previous && !previous.final && previous.taskId !== externalTask.taskId) return;
    sequence.current += 1; readPending.current = false; setReading(false); setFresh(false); setStopped(false); setConfirmation(null);
    if (acceptTask(externalTask, false)) void refreshAfterTask();
  }, [externalTask, projectId, acceptTask, refreshAfterTask]);
  useEffect(() => {
    if (!task || task.final) return;
    const timer = window.setInterval(() => { void poll(); }, 1000);
    return () => window.clearInterval(timer);
  }, [task?.taskId, task?.final, poll]);
  useEffect(() => { callbacks.current.onPendingChange?.(waiting); }, [waiting]);
  useEffect(() => { setConfirmation(null); }, [knownStatus?.mode, knownStatus?.maintenanceId, knownStatus?.recoveryRequired]);

  async function refresh() {
    if (submitting.current || readPending.current || taskRef.current && !taskRef.current.final) return;
    submitting.current = true; setBusy(true); setFresh(false); setConfirmation(null); setStopped(false); setError('');
    const lifetime = generation.current;
    try { await callbacks.current.onChanged(); if (current(lifetime)) await read(true); }
    catch (cause) { if (current(lifetime)) setError(messageFrom(cause)); }
    finally { if (current(lifetime)) { submitting.current = false; setBusy(false); } }
  }

  async function submit() {
    if (submitting.current || readPending.current || blocked || !confirmation) return;
    const selected = confirmation;
    if (!fresh || !stopped || !recovery?.operations.includes(selected.operation) || !(selected.action === 'continue' ? selected.operation.canContinue : selected.operation.canUndo)) return;
    const lifetime = generation.current;
    const expectedType = selected.action === 'continue' ? 'gitSourceRestoreContinue' : 'gitSourceRestoreUndo';
    const command = 'project_git_source_restore_recover';
    const args = { id: projectId, input: { restoreTaskId: selected.operation.restoreTaskId, expectedPlanDigest: selected.operation.planDigest, expectedJournalDigest: selected.operation.journalDigest, action: selected.action } };
    submitting.current = true; setBusy(true); setFresh(false); setError(''); setConfirmation(null);
    try {
      const value = await invoke<TaskRecord>(command, args);
      if (!current(lifetime)) return;
      if (!value.taskId || value.projectId !== projectId || value.type !== expectedType) throw new Error('返回的处理任务身份不符，请重新核对状态，勿重复提交。');
      setPollError('');
      if (acceptTask(value)) await refreshAfterTask();
    } catch (cause) { if (current(lifetime)) setError(`${messageFrom(cause)}。请重新核对操作状态后再决定；本次不会自动重试。`); }
    finally { if (current(lifetime)) { submitting.current = false; setBusy(false); } }
  }

  const unresolved = recovery?.state !== 'clear' && recovery !== null;
  const unknown = !knownStatus || Boolean(readError);
  if (!maintenance && !needsRecovery && !unresolved && !unknown && !active && !error) return null;
  const lastError = typeof knownStatus?.lastError === 'string' ? knownStatus.lastError : knownStatus?.lastError?.message;
  return <section className="operation-recovery" aria-labelledby={`${id}-title`}>
    <div className="operation-recovery-banner"><strong id={`${id}-title`}>{unknown ? '操作状态暂未核实' : '有一项操作需要处理'}</strong><button type="button" className="text-button" aria-expanded={expanded} aria-controls={`${id}-details`} disabled={busy || reading} onClick={() => { setExpanded(value => !value); if (!expanded) void read(true); }}>{expanded ? '收起处理说明' : '查看并处理'}</button></div>
    {expanded && <div id={`${id}-details`}>
      {reading && <p role="status">正在核对操作状态…</p>}
      {(unknown || !fresh) && <p>操作状态尚未核实或已过期，请重新核对后再处理。</p>}
      {readError && <p role="alert" className="task-error">{readError}</p>}
      <button type="button" className="refresh-button" disabled={busy || reading || active} onClick={() => void refresh()}>重新核对操作状态</button>
      {maintenance && !needsRecovery && <div className="operation-recovery-item"><h4>应用处于已停用的维护状态</h4><p>当前版本不再提供备份与维护流程。请保留现场和操作记录，由 AI 根据记录处理。</p>{lastError && <p className="task-error">{lastError}</p>}</div>}
      {needsRecovery && <p>有尚未核对完成的恢复操作，请保留原现场和操作记录，由 AI 根据记录处理。</p>}
      {recovery?.issues.map((issue, index) => <p role="alert" className="task-error" key={`issue-${index}`}>{messageFrom(issue)}</p>)}
      {unresolved && <><p>源码取回尚未完成。请先停止 Codex、其他编辑器和 Git 对本应用的修改，并保持停止直至处理结束。</p><label><input type="checkbox" checked={stopped} disabled={busy || reading || active} onChange={event => { setStopped(event.target.checked); setConfirmation(null); }} />我已停止本应用的外部编辑和 Git 操作</label>{recovery.operations.map(operation => <div className="operation-recovery-item" key={operation.restoreTaskId}><h4>未完成的源码取回</h4><code>{operation.restoreTaskId}</code><details><summary>查看涉及文件（{operation.changes.length}）</summary><ul>{operation.changes.map(change => <li key={change.path}><code>{change.path}</code> · {change.state === 'unknown' ? '内容尚不能确认' : change.state === 'after' ? '已取回的内容' : '操作前内容'}</li>)}</ul></details>{operation.issues.map((issue, index) => <p role="alert" className="task-error" key={index}>{messageFrom(issue)}</p>)}<div className="confirmation-actions">{operation.canContinue && <button type="button" className="refresh-button" disabled={blocked || !fresh || !stopped} onClick={() => setConfirmation({ operation, action: 'continue' })}>继续这次取回</button>}{operation.canUndo && <button type="button" className="refresh-button" disabled={blocked || !fresh || !stopped} onClick={() => setConfirmation({ operation, action: 'undo' })}>撤回这次取回</button>}</div>{!operation.canContinue && !operation.canUndo && <p>目前没有已核验可执行的操作，请保留现场并处理上方问题。</p>}</div>)}</>}
      {confirmation && <div className="operation-confirmation" role="group" aria-label="处理操作确认"><p>{confirmation.action === 'continue' ? '确认完成这次源码取回。文件会成为待保存修改；不会自动提交版本或构建。' : '确认撤回这次源码取回，恢复操作前的源码。数据库、附件和当前程序保持不变。'}</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={blocked || !fresh || !stopped} onClick={() => void submit()}>{confirmation.action === 'continue' ? '确认继续取回' : '确认撤回这次取回'}</button><button type="button" className="refresh-button" disabled={busy} onClick={() => setConfirmation(null)}>取消</button></div></div>}
      {error && <p role="alert" className="task-error">{error}</p>}{pollError && <p role="alert" className="task-error">{pollError}</p>}
      {active && <p role="status">处理仍在进行，请等待最终结果后再关闭详情。</p>}
      {task && <ProjectTask key={task.taskId} task={task} />}
    </div>}
  </section>;
}
