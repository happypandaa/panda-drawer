import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface ProjectGitStatus {
  repository: {
    state: string;
    root: string | null;
    markers: { path: string; kind: string; scope: string }[];
    head: { oid: string | null; branch: string | null } | null;
    indexState: string;
    git: { path: string; version: string } | null;
  };
  scope: {
    policy: string;
    includedRoots: string[];
    includedPaths: string[];
    excluded: { path: string; reason: string }[];
    unclassifiedPaths: string[];
    protectedTrackedPaths: string[];
    complete: boolean;
  };
  changes: { path: string; kind: string; staged: boolean }[];
  snapshotDigest: string;
  canInitialize: boolean;
  canSave: boolean;
  database: { state: string; reason: string };
  issues: unknown[];
}

export interface ProjectGitPanelProps {
  projectId: string;
  instanceId?: string;
  disabled?: boolean;
  task?: TaskRecord | null;
  onTask?: (task: TaskRecord) => void;
  onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}

const repositoryLabels: Record<string, string> = {
  none: '尚未初始化本项目仓库', 'project-root': '仓库位于项目根目录',
  'source-only': '源码目录内已有仓库', ancestor: '父目录已有仓库',
  ambiguous: '检测到多个仓库位置', unsupported: '当前仓库形式尚不支持', unsafe: '仓库路径需要检查',
};
const changeLabels: Record<string, string> = { add: '新增', modify: '修改', delete: '删除' };
const recheckingNotice = '任务已结束，正在重新核对当前源码与版本。';

// Project drafts stay separate. A platform-instance change retains the draft
// while replacing the request lifetime, so stale responses cannot publish it.
export function ProjectGitPanel(props: ProjectGitPanelProps) {
  return <ProjectGitProject key={props.projectId} {...props} />;
}

type Draft = { message: string; authorName: string; authorEmail: string };
function ProjectGitProject(props: ProjectGitPanelProps) {
  const [draft, setDraft] = useState<Draft>({ message: '', authorName: '', authorEmail: '' });
  return <ProjectGitPanelInstance key={props.instanceId ?? ''} {...props} draftValues={draft} changeDraft={setDraft} />;
}

function relevantTask(value: TaskRecord | null | undefined, projectId: string): value is TaskRecord {
  return Boolean(value && value.projectId === projectId && (value.type === 'gitInitialize' || value.type === 'gitSave'));
}

function ProjectGitPanelInstance({ projectId, disabled = false, task: externalTask, onTask, onDraftStateChange, draftValues, changeDraft }: ProjectGitPanelProps & { draftValues: Draft; changeDraft: (value: Draft) => void }) {
  const id = useId();
  const [status, setStatus] = useState<ProjectGitStatus | null>(null);
  const [fresh, setFresh] = useState(false);
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const { message, authorName, authorEmail } = draftValues;
  const [acceptedDraft, setAcceptedDraft] = useState('');
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [pollError, setPollError] = useState('');
  const [notice, setNotice] = useState('');
  const [task, setTask] = useState<TaskRecord | null>(() => relevantTask(externalTask, projectId) ? externalTask : null);
  const lifetime = useRef(0);
  const mounted = useRef(false);
  const readSequence = useRef(0);
  const pending = useRef(false);
  const polling = useRef(false);
  const currentTask = useRef<TaskRecord | null>(task);
  const seenTasks = useRef(new Set(task ? [task.taskId] : []));
  const callback = useRef(onTask); callback.current = onTask;
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const current = useCallback((generation: number) => mounted.current && lifetime.current === generation, []);

  const read = useCallback(async () => {
    const generation = lifetime.current;
    const request = ++readSequence.current;
    setReading(true); setFresh(false);
    try {
      const value = await invoke<ProjectGitStatus>('project_git_status', { id: projectId });
      if (!current(generation) || request !== readSequence.current) return;
      setStatus(value); setFresh(true); setReadError('');
    } catch (cause) {
      if (current(generation) && request === readSequence.current) setReadError(messageFrom(cause));
    } finally {
      if (current(generation) && request === readSequence.current) {
        setReading(false);
        setNotice(value => value === recheckingNotice ? '' : value);
      }
    }
  }, [projectId, current]);

  const acceptTask = useCallback((value: TaskRecord, notify = true) => {
    // A cancellation acknowledgement may arrive after the final poll.
    if (currentTask.current?.taskId === value.taskId && currentTask.current.final && !value.final) return false;
    if (currentTask.current?.taskId !== value.taskId && seenTasks.current.has(value.taskId)) return false;
    const becameFinal = value.final && !(currentTask.current?.taskId === value.taskId && currentTask.current.final);
    seenTasks.current.add(value.taskId);
    currentTask.current = value; setTask(value); if (notify) callback.current?.(value);
    if (value.type === 'gitSave' && value.final && value.status !== 'succeeded') setAcceptedDraft('');
    return becameFinal;
  }, []);

  const poll = useCallback(async () => {
    const selected = currentTask.current;
    if (!selected || selected.final || polling.current) return;
    polling.current = true;
    const generation = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('task_get', { taskId: selected.taskId });
      if (!current(generation) || currentTask.current?.taskId !== selected.taskId) return;
      if (value.taskId !== selected.taskId || value.projectId !== projectId) throw new Error('任务身份不符，请重新核对状态。');
      setPollError('');
      if (acceptTask(value)) { setNotice(recheckingNotice); await read(); }
    } catch (cause) {
      if (current(generation)) setPollError(`任务状态暂时无法读取：${messageFrom(cause)}。请刷新状态，勿重复提交。`);
    } finally { polling.current = false; }
  }, [projectId, current, acceptTask, read]);

  useEffect(() => {
    mounted.current = true; lifetime.current += 1;
    if (currentTask.current && !currentTask.current.final) void poll(); else void read();
    return () => { mounted.current = false; lifetime.current += 1; };
  }, [read, poll]);
  useEffect(() => {
    if (!relevantTask(externalTask, projectId) || externalTask === currentTask.current) return;
    const previous = currentTask.current;
    if (previous?.taskId !== externalTask.taskId && previous && !previous.final) return;
    if (previous?.taskId === externalTask.taskId && previous.final) return;
    if (previous?.taskId !== externalTask.taskId && seenTasks.current.has(externalTask.taskId)) return;
    readSequence.current += 1; setFresh(false); setReading(false);
    if (acceptTask(externalTask, false)) void read();
  }, [externalTask, projectId, acceptTask, read, task?.taskId, task?.final]);
  useEffect(() => {
    if (!task || task.final) return;
    const timer = window.setInterval(() => { void poll(); }, 1000);
    return () => window.clearInterval(timer);
  }, [task?.taskId, task?.final, poll]);

  const draft = JSON.stringify([message, authorName, authorEmail]);
  const dirty = Boolean(message || authorName || authorEmail) && draft !== acceptedDraft;
  useEffect(() => { draftCallback.current?.({ dirty, pending: busy }); }, [dirty, busy]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);

  const active = Boolean(task && !task.final);
  const writable = !disabled && !busy && !active && !reading && fresh;
  const hasChanges = Boolean(status?.changes.length);
  const validAuthor = authorName.trim().length > 0 && authorName.trim().length <= 200
    && !/[\x00-\x1f\x7f<>]/.test(authorName)
    && authorEmail.trim().length <= 254 && /^[^\s<>@]+@[^\s<>@]+$/.test(authorEmail.trim());
  const validMessage = Boolean(message.trim()) && message.length <= 4000 && !/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(message);

  async function submit(operation: 'initialize' | 'save') {
    if (pending.current || currentTask.current && !currentTask.current.final || !writable || !status) return;
    if (operation === 'initialize' ? !status.canInitialize : !status.canSave || !hasChanges || !validAuthor || !validMessage) return;
    pending.current = true; setBusy(true); setError(''); setNotice('');
    const generation = lifetime.current;
    readSequence.current += 1; setFresh(false);
    try {
      const input = operation === 'initialize'
        ? { expectedSnapshot: status.snapshotDigest, initialBranch: 'main' }
        : { expectedHead: status.repository.head?.oid ?? null, expectedSnapshot: status.snapshotDigest, message: message.trim(), author: { name: authorName.trim(), email: authorEmail.trim() } };
      const value = await invoke<TaskRecord>(operation === 'initialize' ? 'project_git_initialize' : 'project_git_save', { id: projectId, input });
      if (!current(generation)) return;
      if (value.projectId !== projectId) throw new Error('任务所属项目不符，请刷新状态后检查。');
      setPollError('');
      if (operation === 'save' && (!value.final || value.status === 'succeeded')) setAcceptedDraft(draft);
      const finished = acceptTask(value);
      setNotice(value.final ? '任务已结束，请核对结果。' : '任务已接受，最终结果确认前请勿重复提交。');
      if (finished) await read();
    } catch (cause) {
      if (current(generation)) { setError(messageFrom(cause)); setNotice('请先刷新当前状态，再决定是否重试。填写内容已保留。'); }
    } finally { pending.current = false; if (current(generation)) setBusy(false); }
  }

  async function cancel() {
    const selected = currentTask.current;
    if (pending.current || !selected || selected.final) return;
    pending.current = true; setBusy(true); setError('');
    const generation = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('project_git_cancel', { id: projectId, taskId: selected.taskId });
      if (!current(generation) || currentTask.current?.taskId !== selected.taskId) return;
      if (value.taskId !== selected.taskId || value.projectId !== projectId) throw new Error('任务身份不符，请刷新状态后检查。');
      const finished = acceptTask(value);
      setNotice(currentTask.current?.final ? '任务已经结束，请核对最终结果。' : '已请求停止，正在等待最终结果。已提交的版本不会因取消而撤销。');
      if (finished) await read();
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { pending.current = false; if (current(generation)) setBusy(false); }
  }

  const head = status?.repository.head;
  return <section className="project-git-panel" aria-labelledby={`${id}-title`}>
    <div className="storage-heading"><h3 id={`${id}-title`}>源码版本</h3><button type="button" className="refresh-button" disabled={reading || busy || disabled} onClick={() => { if (currentTask.current && !currentTask.current.final) void poll(); else void read(); }}>{reading ? '核对中…' : '刷新状态'}</button></div>
    <p>在本项目的标准 Git 仓库中保存下面列出的源码和开发文档。本次不保存数据库、附件或数据快照，也不提供分支切换和版本取回。</p>
    <p>仅操作本机；不会推送远端。保存前请核对文件范围，路径排除不能代替对源码中敏感内容的检查。</p>
    {error && <p className="task-error" role="alert">{error}</p>}
    {readError && <p className="task-error" role="alert">{readError}</p>}
    {pollError && <p className="task-error" role="alert">{pollError}</p>}
    {notice && <p role="status">{notice}</p>}
    {reading && <p role="status">正在核对本机仓库和源码范围…</p>}
    {status && <>
      {!fresh && !reading && <p className="maintenance-warning">下面是上次预览，当前状态需要重新核对。</p>}
      <dl className="path-list">
        <div className="path-row"><dt>仓库位置</dt><dd>{repositoryLabels[status.repository.state] ?? '仓库状态需要检查'}{status.repository.root && <code className="task-result-path">{status.repository.root}</code>}</dd></div>
        <div className="path-row"><dt>已核验的当前版本</dt><dd>{head ? head.oid ? <code>{head.oid}</code> : '已核验：尚无提交' : '尚未核验 HEAD'}{head?.branch && <p>分支：<code>{head.branch.replace(/^refs\/heads\//, '')}</code></p>}</dd></div>
        <div className="path-row"><dt>暂存区</dt><dd>{status.repository.indexState === 'matched' ? '与当前版本一致' : status.repository.indexState === 'staged-changes' ? '有已有暂存或冲突内容，需要先处理' : '尚未核验'}</dd></div>
      </dl>
      {status.repository.markers.length > 0 && <details><summary>检测到的 Git 位置</summary><ul>{status.repository.markers.map((marker, index) => <li key={`${marker.path}:${index}`}><code>{marker.path}</code> · {marker.scope === 'project' ? '项目根目录' : marker.scope === 'ancestor' ? '父目录' : '源码目录'}</li>)}</ul></details>}
      {status.issues.map((issue, index) => <p key={index} className="task-error" role="alert">{messageFrom(issue)}</p>)}
      <section aria-label="本次源码范围">
        <h4>本次范围 · {status.scope.complete ? '已完整核对' : '仍需检查'}</h4>
        <PathList title="允许纳入的目录与声明" items={status.scope.includedRoots.map(path => ({ path }))} />
        <PathList key={`included:${status.snapshotDigest}`} title="本次纳入的文件" items={status.scope.includedPaths.map(path => ({ path }))} />
        <PathList key={`excluded:${status.snapshotDigest}`} title="受保护或排除的路径" items={status.scope.excluded} />
        <PathList key={`unknown:${status.snapshotDigest}`} title="尚未分类的路径" items={status.scope.unclassifiedPaths.map(path => ({ path }))} />
        <PathList key={`tracked:${status.snapshotDigest}`} title="已经被 Git 跟踪的受保护路径" items={status.scope.protectedTrackedPaths.map(path => ({ path }))} />
        {status.scope.protectedTrackedPaths.length > 0 && <p>这些受保护路径已经在已有版本或暂存区中；本步不会自行移除跟踪或改写历史。</p>}
      </section>
      <PathList key={`changes:${status.snapshotDigest}`} title="待保存的源码变更" items={status.changes.map(change => ({ path: change.path, reason: `${changeLabels[change.kind] ?? change.kind}${change.staged ? ' · 已有暂存变更' : ''}` }))} />
      {!status.changes.length && status.scope.complete && !status.issues.length && <p>本次范围内没有未保存的源码变更。</p>}
      {status.canInitialize && <div className="operation-confirmation"><p>初始化会在项目根目录建立 main 分支仓库，不创建版本。随后填写作者和说明，再保存第一个源码版本。</p><button type="button" className="primary-button" disabled={!writable} onClick={() => void submit('initialize')}>初始化本项目 Git</button></div>}
      <form className="create-project-form" onSubmit={event => { event.preventDefault(); void submit('save'); }}>
        <label htmlFor={`${id}-message`}>版本说明</label><textarea id={`${id}-message`} value={message} onChange={event => changeDraft({ ...draftValues, message: event.target.value })} maxLength={4000} readOnly={busy} rows={3} />
        <label htmlFor={`${id}-name`}>作者姓名</label><input id={`${id}-name`} value={authorName} onChange={event => changeDraft({ ...draftValues, authorName: event.target.value })} maxLength={200} readOnly={busy} autoComplete="off" />
        <label htmlFor={`${id}-email`}>作者邮箱</label><input id={`${id}-email`} type="email" value={authorEmail} onChange={event => changeDraft({ ...draftValues, authorEmail: event.target.value })} maxLength={254} readOnly={busy} autoComplete="off" />
        <p>作者仅用于本次提交，由你填写，不读取或修改全局 Git 作者配置。</p>
        <button type="submit" className="primary-button" disabled={!writable || !status.canSave || !hasChanges || !validAuthor || !validMessage}>保存源码版本</button>
      </form>
    </>}
    {task && <><ProjectTask key={task.taskId} task={task} />{!task.final && <><p role="status">关闭此面板不会停止任务。</p><button type="button" className="refresh-button" disabled={busy} onClick={() => void cancel()}>取消当前操作</button></>}</>}
  </section>;
}

function PathList({ title, items }: { title: string; items: { path: string; reason?: string }[] }) {
  const [visible, setVisible] = useState(200);
  return <details><summary>{title}（{items.length}）</summary>{items.length ? <><ul>{items.slice(0, visible).map((item, index) => <li key={`${item.path}:${index}`}><code>{item.path}</code>{item.reason && <> · {item.reason}</>}</li>)}</ul>{items.length > visible && <button type="button" className="text-button" onClick={() => setVisible(value => value + 200)}>继续显示（剩余 {items.length - visible} 项）</button>}</> : <p>无。</p>}</details>;
}
