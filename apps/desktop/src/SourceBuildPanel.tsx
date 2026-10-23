import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface SourceBuildCandidate {
  sourceId: string;
  name: string;
  status: string;
  contentDigest?: string;
  inspection?: { kind: string; name?: string; nextStep?: string };
}
function eligible(source: SourceBuildCandidate) {
  return source.status === 'ready' && source.inspection?.kind === 'project-v1-source'
    && typeof source.sourceId === 'string' && Boolean(source.sourceId)
    && typeof source.contentDigest === 'string' && /^[a-f0-9]{64}$/i.test(source.contentDigest);
}
function checkedTask(value: unknown, expectedId?: string): TaskRecord {
  const task = value as Partial<TaskRecord> | null;
  if (!task || task.type !== 'sourceBuild' || typeof task.taskId !== 'string' || !task.taskId
    || (expectedId && task.taskId !== expectedId) || typeof task.final !== 'boolean'
    || !['queued', 'running', 'succeeded', 'failed', 'interrupted'].includes(task.status ?? '')
    || task.final !== ['succeeded', 'failed', 'interrupted'].includes(task.status ?? '')
    || typeof task.phase !== 'string' || (task.result !== null && (typeof task.result !== 'object' || Array.isArray(task.result)))
    || (task.error !== null && (!task.error || typeof task.error.code !== 'string' || typeof task.error.message !== 'string')))
    throw new Error('候选构建任务身份或状态不完整，请刷新任务后核对，不要重复提交。');
  return task as TaskRecord;
}

export function SourceBuildPanel({ sources, instanceId, disabled, beforeMutation, onPendingChange, onTask }: {
  sources: SourceBuildCandidate[];
  instanceId?: string;
  disabled: boolean;
  beforeMutation?: () => boolean;
  onPendingChange: (pending: boolean) => void;
  onTask: (task: TaskRecord) => void;
}) {
  const [selectedId, setSelectedId] = useState('');
  const [acknowledged, setAcknowledged] = useState(false);
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [pending, setPending] = useState(false);
  const [cancellingId, setCancellingId] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [needsRecheck, setNeedsRecheck] = useState(false);
  const alive = useRef(false); const generation = useRef(0); const readSequence = useRef(0);
  const operation = useRef(false); const taskMap = useRef(new Map<string, TaskRecord>());
  const confirmation = useRef<string | null>(null); const recheck = useRef(false);
  const context = useRef({ instanceId, disabled }); context.current = { instanceId, disabled };
  const callbacks = useRef({ onPendingChange, onTask }); callbacks.current = { onPendingChange, onTask };
  const id = useId();
  const available = sources.filter(eligible);
  const selected = available.find(source => source.sourceId === selectedId);
  const binding = selected ? `${selected.sourceId}:${selected.contentDigest}` : '';
  const active = tasks.some(task => !task.final);
  const current = useCallback((started: number) => alive.current && generation.current === started, []);
  const accept = useCallback((value: TaskRecord) => {
    const previous = taskMap.current.get(value.taskId);
    if (previous?.final && !value.final) return;
    taskMap.current.set(value.taskId, value);
    setTasks([...taskMap.current.values()]); callbacks.current.onTask(value);
  }, []);
  const read = useCallback(async () => {
    if (!alive.current || context.current.disabled || operation.current) return;
    const started = generation.current; const request = ++readSequence.current;
    setReading(true);
    try {
      const values = await invoke<unknown>('source_build_list');
      if (!current(started) || request !== readSequence.current || context.current.disabled) return;
      if (!Array.isArray(values)) throw new Error('候选构建任务列表不完整，请重新刷新。');
      const checked = values.map(value => checkedTask(value));
      checked.forEach(accept); recheck.current = false; setNeedsRecheck(false); setError('');
    } catch (cause) {
      if (current(started) && request === readSequence.current) { recheck.current = true; setNeedsRecheck(true); setError(messageFrom(cause)); }
    } finally { if (current(started) && request === readSequence.current) setReading(false); }
  }, [accept, current]);
  useEffect(() => {
    alive.current = true; generation.current++; taskMap.current.clear(); setTasks([]);
    operation.current = false; setPending(false); setCancellingId(null); recheck.current = false; setNeedsRecheck(false);
    confirmation.current = null; setAcknowledged(false); setSelectedId(''); setError(''); setNotice('');
    void read();
    return () => { alive.current = false; generation.current++; callbacks.current.onPendingChange(false); };
  }, [instanceId, read]);
  useEffect(() => { confirmation.current = null; setAcknowledged(false); }, [binding]);
  useEffect(() => {
    if (!active || disabled) return;
    const started = generation.current; let disposed = false; let polling = false;
    const timer = window.setInterval(() => {
      if (polling || operation.current || context.current.disabled) return;
      polling = true;
      const activeTasks = [...taskMap.current.values()].filter(task => !task.final);
      void Promise.all(activeTasks.map(async task => {
        const value = checkedTask(await invoke<unknown>('task_get', { taskId: task.taskId }), task.taskId);
        if (!disposed && current(started) && !context.current.disabled) accept(value);
      })).catch(cause => {
        if (!disposed && current(started)) setError(`候选构建状态暂时未能更新：${messageFrom(cause)} 请刷新任务，系统不会重试构建。`);
      }).finally(() => { polling = false; });
    }, 1000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [active, disabled, accept, current]);
  function changeSource(sourceId: string) {
    if (operation.current || active || disabled) return;
    confirmation.current = null; setAcknowledged(false); setSelectedId(sourceId); setError('');
  }
  async function prepare() {
    if (disabled || operation.current || active || reading || recheck.current || !selected || !acknowledged || confirmation.current !== binding || beforeMutation?.() === false) return;
    const source = selected; const started = generation.current;
    operation.current = true; setPending(true); callbacks.current.onPendingChange(true);
    confirmation.current = null; setAcknowledged(false); readSequence.current++; setReading(false); setError(''); setNotice('');
    try {
      const value = checkedTask(await invoke<unknown>('source_build_prepare', { input: { sourceId: source.sourceId, expectedDigest: source.contentDigest, acknowledgeScripts: true } }));
      if (current(started)) accept(value);
    } catch (cause) {
      if (current(started)) { recheck.current = true; setNeedsRecheck(true); setError(`${messageFrom(cause)} 请先刷新候选构建任务，核对是否已接受；不要重复提交。`); }
    } finally {
      if (current(started)) { operation.current = false; setPending(false); callbacks.current.onPendingChange(false); }
    }
  }
  async function cancel(taskId: string) {
    const task = taskMap.current.get(taskId);
    if (disabled || operation.current || reading || recheck.current || !task || task.final || beforeMutation?.() === false) return;
    const started = generation.current;
    operation.current = true; setPending(true); setCancellingId(taskId); callbacks.current.onPendingChange(true);
    readSequence.current++; setReading(false); setError(''); setNotice('');
    try {
      const value = checkedTask(await invoke<unknown>('source_cancel', { taskId }), taskId);
      if (!current(started)) return;
      accept(value);
      setNotice(value.final ? '候选构建已经结束，请核对最终状态和保留现场。' : '已请求取消候选构建，正在等待最终状态；部分候选与日志会保留。');
    } catch (cause) {
      if (current(started)) {
        recheck.current = true; setNeedsRecheck(true);
        setError(`${messageFrom(cause)} 取消回执尚不能确认，请刷新候选构建任务并核对最终状态；不会自动重试取消。`);
      }
    } finally {
      if (current(started)) { operation.current = false; setPending(false); setCancellingId(null); callbacks.current.onPendingChange(false); }
    }
  }
  return <section className="source-build-panel" aria-label="独立源码构建候选">
    <h3>准备独立网页候选</h3><p>只支持已获取且检查为平台网页项目的源码。平台将源码复制到独立目录，候选不会自动安装或发布。依赖与构建脚本以当前用户权限运行，只执行可信源码。</p>
    <label htmlFor={`${id}-source`}>候选源码</label><select id={`${id}-source`} value={selectedId} disabled={disabled || pending || active} onChange={event => changeSource(event.target.value)}><option value="">请选择已核对的网页项目源码</option>{available.map(source => <option key={source.sourceId} value={source.sourceId}>{source.inspection?.name ?? source.name} · {source.sourceId}</option>)}</select>
    {!available.length && <p>暂无可构建的网页项目源码。Node 服务源码和待适配目录不会进入此构建流程。</p>}
    {selected && <div className="operation-confirmation"><p>将执行：{selected.inspection?.name ?? selected.name}</p><code>{selected.sourceId}</code><label className="checkbox-label"><input type="checkbox" checked={acknowledged && confirmation.current === binding} disabled={disabled || pending || active || needsRecheck} onChange={event => { confirmation.current = event.target.checked ? binding : null; setAcknowledged(event.target.checked); }} />我理解此操作会执行所选源码中的依赖与构建脚本，同意准备独立候选</label><button className="primary-button" disabled={disabled || pending || active || reading || needsRecheck || !acknowledged || confirmation.current !== binding} onClick={() => void prepare()}>{pending && !cancellingId ? '正在提交候选构建…' : '准备独立构建候选'}</button></div>}
    {error && <p role="alert" className="task-error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    <div className="storage-heading"><h4>候选构建任务与保留结果</h4><button className="refresh-button" disabled={disabled || reading || pending} onClick={() => void read()}>{reading ? '正在读取候选任务…' : '刷新候选构建任务'}</button></div>
    {needsRecheck && <p role="status">需要重新读取任务后才能提交构建或取消请求。刷新不会重新执行源码。</p>}
    {!tasks.length && <p>暂无候选构建任务。</p>}
    {tasks.map(task => <div key={task.taskId}><ProjectTask task={task} disabled={disabled} />{!task.final && <button className="refresh-button" disabled={disabled || reading || pending || needsRecheck} onClick={() => void cancel(task.taskId)}>{cancellingId === task.taskId ? '正在提交取消…' : '取消候选构建'}</button>}</div>)}
  </section>;
}
