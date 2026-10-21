import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type Inspection = { kind: string; name?: string; issues: unknown[]; executionState: string };
type Candidate = { sourceId: string; taskId: string; name: string; status: string; url: string; commit?: string; sourcePath?: string; evidencePath: string; inspection?: Inspection; error?: unknown; contentDigest?: string };
const statusText: Record<string, string> = { acquiring: '正在获取', ready: '已获取，待接入', failed: '获取失败', cancelled: '已取消', interrupted: '已中断', unavailable: '记录需要检查' };
const kindText: Record<string, string> = { 'project-v1-source': '平台网页项目源码', 'node-service-source': '带本地服务的应用源码', 'needs-adaptation': '需要适配的源码' };

export function SourceAcquisitionPanel({ disabled, onClose, onTask }: { disabled: boolean; onClose: () => void; onTask: (task: TaskRecord) => void }) {
  const [url, setUrl] = useState('');
  const [name, setName] = useState('');
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [inspection, setInspection] = useState<{ sourcePath: string; inspection: Inspection } | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const readPending = useRef(false);
  const callbacks = useRef({ onTask }); callbacks.current = { onTask };
  const read = useCallback(async () => {
    if (readPending.current) return;
    readPending.current = true; setReading(true);
    try {
      const list = await invoke<Candidate[]>('source_list');
      if (!alive.current) return;
      setCandidates(list);
      const active = list.find(item => item.status === 'acquiring');
      if (active) {
        const value = await invoke<TaskRecord>('task_get', { taskId: active.taskId });
        if (alive.current) { setTask(value); callbacks.current.onTask(value); }
      }
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { readPending.current = false; if (alive.current) setReading(false); }
  }, []);
  useEffect(() => { alive.current = true; void read(); return () => { alive.current = false; }; }, [read]);
  useEffect(() => {
    if (!task || task.final) return;
    let disposed = false; let polling = false;
    const timer = window.setInterval(() => {
      if (polling) return; polling = true;
      void invoke<TaskRecord>('task_get', { taskId: task.taskId }).then(value => {
        if (disposed || !alive.current) return;
        setTask(value); callbacks.current.onTask(value);
        if (value.final) void read();
      }).catch(cause => { if (!disposed && alive.current) setError(`任务状态暂时无法读取：${messageFrom(cause)}。请刷新，勿重复提交。`); }).finally(() => { polling = false; });
    }, 1000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [task, read]);
  const acquiring = candidates.some(item => item.status === 'acquiring') || Boolean(task && !task.final);
  async function acquire() {
    if (pending.current || disabled || acquiring || !url.trim()) return;
    pending.current = true; setBusy(true); setError(''); setNotice('');
    try {
      const value = await invoke<TaskRecord>('source_acquire', { input: { url: url.trim(), ...(name.trim() ? { name: name.trim() } : {}) } });
      if (!alive.current) return;
      setTask(value); callbacks.current.onTask(value);
      setNotice('获取任务已接受，尚未安装或运行。可以关闭此页，稍后回来查看结果。');
      await read();
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  }
  async function cancel() {
    if (pending.current || !task || task.final) return;
    pending.current = true; setBusy(true); setError('');
    try {
      const value = await invoke<TaskRecord>('source_cancel', { taskId: task.taskId });
      if (!alive.current) return;
      setTask(value); callbacks.current.onTask(value);
      setNotice(value.final ? '任务已经结束，请核对最终结果。' : '已请求取消，正在等待任务停止；部分下载现场会保留。');
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  }
  async function inspect() {
    if (pending.current || disabled) return;
    pending.current = true; setBusy(true); setError('');
    try {
      const sourcePath = await invoke<string | null>('choose_path', { kind: 'source' });
      if (!sourcePath || !alive.current) return;
      const value = await invoke<{ sourcePath: string; inspection: Inspection }>('source_inspect', { sourcePath });
      if (alive.current) setInspection(value);
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  }
  return <Modal title="获取项目源码" description="从公开仓库获取，或先检查本机源码。" wide closeDisabled={busy} onClose={onClose}>
    {error && <p role="alert" className="task-error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    <form className="create-project-form" onSubmit={event => { event.preventDefault(); void acquire(); }}>
      <label htmlFor="source-url">公开仓库链接</label><input id="source-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/作者/项目.git" readOnly={busy} />
      <label htmlFor="source-name">备注名称（选填）</label><input id="source-name" value={name} onChange={event => setName(event.target.value)} readOnly={busy} />
      <p>支持 GitHub、Gitee 的公开 HTTPS 仓库。仅下载当前版本并检查，不运行项目代码。私有仓库及 SSH 尚未接入。</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || busy || acquiring || !url.trim()}>{busy ? '正在提交…' : '获取源码'}</button><button type="button" className="refresh-button" disabled={disabled || busy} onClick={() => void inspect()}>检查本机源码目录</button></div>
    </form>
    {inspection && <section><h3>本机检查结果</h3><code className="task-result-path">{inspection.sourcePath}</code><InspectionView value={inspection.inspection} /></section>}
    {task && <><ProjectTask task={task} />{!task.final && <button className="refresh-button" disabled={busy} onClick={() => void cancel()}>取消获取</button>}</>}
    <section><div className="storage-heading"><h3>已获取与保留的现场</h3><button className="refresh-button" disabled={reading || busy} onClick={() => void read()}>刷新结果</button></div>
      <p>获取完成不等于可以运行；需先核对声明并完成适配或安装。失败、中断的目录保留，重试使用新目录。</p>
      {!candidates.length && <p>暂无获取记录。</p>}
      {candidates.map(item => <article className="operation-confirmation" key={item.sourceId}><h4>{item.name} · {statusText[item.status] ?? item.status}</h4><code className="task-result-path">{item.url}</code>{item.commit && <p>源码版本：<code>{item.commit}</code></p>}<code className="task-result-path">{item.sourcePath ?? item.evidencePath}</code>{item.inspection && <InspectionView value={item.inspection} />}{Boolean(item.error) && <p role="alert">{messageFrom(item.error)}</p>}</article>)}
    </section>
  </Modal>;
}
function InspectionView({ value }: { value: Inspection }) {
  return <><p>{kindText[value.kind] ?? value.kind} · 未执行代码</p>{value.issues.map((issue, index) => <p key={index}>{messageFrom(issue)}</p>)}</>;
}
