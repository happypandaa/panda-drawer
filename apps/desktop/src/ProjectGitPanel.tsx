import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import { ProjectGitVersions } from './ProjectGitVersions';
import type { ProjectGitVersionsResult } from './ProjectGitVersions';
import { ProjectGitSourceRestore, sourceRestoreTaskTypes } from './ProjectGitSourceRestore';
import type { SourceRestoreOperation, SourceRestorePreview, SourceRestoreStatus } from './ProjectGitSourceRestore';
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
  canListVersions: boolean;
  canCreateBranch: boolean;
  branchCreationIssue: unknown | null;
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

type Draft = { message: string; authorName: string; authorEmail: string; branchName: string; targetCommit: string };
function ProjectGitProject(props: ProjectGitPanelProps) {
  const [draft, setDraft] = useState<Draft>({ message: '', authorName: '', authorEmail: '', branchName: '', targetCommit: '' });
  return <ProjectGitPanelInstance key={props.instanceId ?? ''} {...props} draftValues={draft} changeDraft={setDraft} />;
}

function relevantTask(value: TaskRecord | null | undefined, projectId: string): value is TaskRecord {
  return Boolean(value && value.projectId === projectId && ['gitInitialize', 'gitSave', 'gitBranchCreate', ...sourceRestoreTaskTypes].includes(value.type));
}

function validBranchName(name: string) {
  return name.length >= 1 && name.length <= 128 && name !== 'HEAD' && /^[A-Za-z0-9][A-Za-z0-9._/-]*$/.test(name)
    && !name.includes('..') && !name.endsWith('.') && name.split('/').every(part => part && !part.startsWith('.') && !part.endsWith('.lock'));
}

function ProjectGitPanelInstance({ projectId, disabled = false, task: externalTask, onTask, onDraftStateChange, draftValues, changeDraft }: ProjectGitPanelProps & { draftValues: Draft; changeDraft: (value: Draft) => void }) {
  const id = useId();
  const [status, setStatus] = useState<ProjectGitStatus | null>(null);
  const [fresh, setFresh] = useState(false);
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const { message, authorName, authorEmail, branchName, targetCommit } = draftValues;
  const [acceptedDraft, setAcceptedDraft] = useState('');
  const [acceptedBranchName, setAcceptedBranchName] = useState('');
  const [acceptedTarget, setAcceptedTarget] = useState('');
  const [restorePreview, setRestorePreview] = useState<SourceRestorePreview | null>(null);
  const [recovery, setRecovery] = useState<SourceRestoreStatus | null>(null);
  const [recoveryFresh, setRecoveryFresh] = useState(false);
  const [recoveryReading, setRecoveryReading] = useState(false);
  const [recoveryError, setRecoveryError] = useState('');
  const [externalEditingStopped, setExternalEditingStopped] = useState(false);
  const [restoreConfirmed, setRestoreConfirmed] = useState(false);
  const [versions, setVersions] = useState<ProjectGitVersionsResult | null>(null);
  const [versionsFresh, setVersionsFresh] = useState(false);
  const [listing, setListing] = useState(false);
  const [versionsError, setVersionsError] = useState('');
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [pollError, setPollError] = useState('');
  const [notice, setNotice] = useState('');
  const [task, setTask] = useState<TaskRecord | null>(() => relevantTask(externalTask, projectId) ? externalTask : null);
  const lifetime = useRef(0);
  const mounted = useRef(false);
  const readSequence = useRef(0);
  const statusPending = useRef(false);
  const versionsSequence = useRef(0);
  const versionsPending = useRef(false);
  const versionsRequested = useRef(false);
  const pending = useRef(false);
  const polling = useRef(false);
  const currentTask = useRef<TaskRecord | null>(task);
  const seenTasks = useRef(new Set(task ? [task.taskId] : []));
  const callback = useRef(onTask); callback.current = onTask;
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const current = useCallback((generation: number) => mounted.current && lifetime.current === generation, []);

  const readVersions = useCallback(async (preview: ProjectGitStatus, fromStatus = false) => {
    if (!preview.canListVersions || pending.current && !fromStatus || currentTask.current && !currentTask.current.final || versionsPending.current || statusPending.current && !fromStatus) return;
    const generation = lifetime.current; const statusRequest = readSequence.current;
    const request = ++versionsSequence.current;
    versionsPending.current = true; setListing(true); setVersionsFresh(false); setVersionsError('');
    setRestorePreview(null); setRestoreConfirmed(false);
    const valid = () => current(generation) && statusRequest === readSequence.current && request === versionsSequence.current;
    try {
      const value = await invoke<ProjectGitVersionsResult>('project_git_versions', { id: projectId, limit: 50 });
      if (!valid()) return;
      setVersions(value); setVersionsFresh(true);
      if (value.head.oid !== preview.repository.head?.oid || value.head.branch !== preview.repository.head?.branch) {
        setFresh(false); setVersionsError('读取版本时当前分支或版本已变化，请刷新状态后再操作。');
      }
    } catch (cause) { if (valid()) setVersionsError(messageFrom(cause)); }
    finally { if (current(generation) && request === versionsSequence.current) { versionsPending.current = false; setListing(false); } }
  }, [projectId, current]);

  const read = useCallback(async () => {
    if (versionsPending.current) return;
    const generation = lifetime.current;
    const request = ++readSequence.current;
    statusPending.current = true; setReading(true); setFresh(false); setVersionsFresh(false);
    setRestorePreview(null); setRestoreConfirmed(false); setRecoveryFresh(false); setRecoveryReading(true);
    // Recovery is independently readable even when ordinary Git inspection fails.
    // Keep these reads serial: each backend read holds the project operation lease.
    try {
      const value = await invoke<SourceRestoreStatus>('project_git_source_restore_status', { id: projectId });
      if (!current(generation) || request !== readSequence.current) return;
      setRecovery(value); setRecoveryFresh(true); setRecoveryError('');
    } catch (cause) {
      if (current(generation) && request === readSequence.current) setRecoveryError(messageFrom(cause));
    } finally {
      if (current(generation) && request === readSequence.current) setRecoveryReading(false);
    }
    if (!current(generation) || request !== readSequence.current) return;
    try {
      const value = await invoke<ProjectGitStatus>('project_git_status', { id: projectId });
      if (!current(generation) || request !== readSequence.current) return;
      setStatus(value); setFresh(true); setReadError('');
      if (versionsRequested.current && value.canListVersions) await readVersions(value, true);
    } catch (cause) {
      if (current(generation) && request === readSequence.current) setReadError(messageFrom(cause));
    } finally {
      if (current(generation) && request === readSequence.current) {
        setReading(false);
        statusPending.current = false;
        setNotice(value => value === recheckingNotice ? '' : value);
      }
    }
  }, [projectId, current, readVersions]);

  const acceptTask = useCallback((value: TaskRecord, notify = true) => {
    // A cancellation acknowledgement may arrive after the final poll.
    if (currentTask.current?.taskId === value.taskId && currentTask.current.final && !value.final) return false;
    if (currentTask.current?.taskId !== value.taskId && seenTasks.current.has(value.taskId)) return false;
    const becameFinal = value.final && !(currentTask.current?.taskId === value.taskId && currentTask.current.final);
    seenTasks.current.add(value.taskId);
    currentTask.current = value; setTask(value); if (notify) callback.current?.(value);
    if (value.type === 'gitSave' && value.final && value.status !== 'succeeded') setAcceptedDraft('');
    if (value.type === 'gitBranchCreate' && value.final && value.status !== 'succeeded') setAcceptedBranchName('');
    if (value.type === 'gitSourceRestoreApply' && value.final && value.status !== 'succeeded') setAcceptedTarget('');
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
    readSequence.current += 1; versionsSequence.current += 1;
    statusPending.current = false; versionsPending.current = false;
    setFresh(false); setReading(false); setVersionsFresh(false); setListing(false);
    setRestorePreview(null); setRestoreConfirmed(false); setRecoveryFresh(false); setRecoveryReading(false);
    if (acceptTask(externalTask, false)) void read();
  }, [externalTask, projectId, acceptTask, read, task?.taskId, task?.final]);
  useEffect(() => {
    if (!task || task.final) return;
    const timer = window.setInterval(() => { void poll(); }, 1000);
    return () => window.clearInterval(timer);
  }, [task?.taskId, task?.final, poll]);

  const draft = JSON.stringify([message, authorName, authorEmail]);
  const dirty = Boolean(message || authorName || authorEmail) && draft !== acceptedDraft || Boolean(branchName) && branchName !== acceptedBranchName || Boolean(targetCommit) && targetCommit !== acceptedTarget;
  const restoreActive = Boolean(task && !task.final && sourceRestoreTaskTypes.includes(task.type));
  useEffect(() => { draftCallback.current?.({ dirty, pending: busy || restoreActive || recoveryReading }); }, [dirty, busy, restoreActive, recoveryReading]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);

  const active = Boolean(task && !task.final);
  const idle = !busy && !active && !reading && !listing;
  const recoveryClear = recoveryFresh && recovery?.state === 'clear';
  const writable = !disabled && idle && fresh && recoveryClear;
  const hasChanges = Boolean(status?.changes.length);
  const branchReady = Boolean(status?.canCreateBranch && !hasChanges && status.repository.indexState === 'matched' && status.repository.head?.oid && status.repository.head.branch);
  const validAuthor = authorName.trim().length > 0 && authorName.trim().length <= 200
    && !/[\x00-\x1f\x7f<>]/.test(authorName)
    && authorEmail.trim().length <= 254 && /^[^\s<>@]+@[^\s<>@]+$/.test(authorEmail.trim());
  const validMessage = Boolean(message.trim()) && message.length <= 4000 && !/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(message);

  async function submit(operation: 'initialize' | 'save' | 'create-branch') {
    if (pending.current || statusPending.current || versionsPending.current || currentTask.current && !currentTask.current.final || !writable || !status) return;
    if (operation === 'initialize' ? !status.canInitialize : operation === 'save' ? !status.canSave || !hasChanges || !validAuthor || !validMessage : !branchReady || !validBranchName(branchName)) return;
    pending.current = true; setBusy(true); setError(''); setNotice('');
    const generation = lifetime.current;
    readSequence.current += 1; versionsSequence.current += 1; setFresh(false); setVersionsFresh(false);
    setRestorePreview(null); setRestoreConfirmed(false); setRecoveryFresh(false);
    try {
      const input = operation === 'initialize'
        ? { expectedSnapshot: status.snapshotDigest, initialBranch: 'main' }
        : operation === 'save' ? { expectedHead: status.repository.head?.oid ?? null, expectedSnapshot: status.snapshotDigest, message: message.trim(), author: { name: authorName.trim(), email: authorEmail.trim() } }
          : { name: branchName, expectedHead: status.repository.head!.oid!, expectedBranch: status.repository.head!.branch!, expectedSnapshot: status.snapshotDigest };
      const value = await invoke<TaskRecord>(operation === 'initialize' ? 'project_git_initialize' : operation === 'save' ? 'project_git_save' : 'project_git_branch_create', { id: projectId, input });
      if (!current(generation)) return;
      if (value.projectId !== projectId) throw new Error('任务所属项目不符，请刷新状态后检查。');
      setPollError('');
      if (operation === 'save' && (!value.final || value.status === 'succeeded')) setAcceptedDraft(draft);
      if (operation === 'create-branch' && (!value.final || value.status === 'succeeded')) setAcceptedBranchName(branchName);
      const finished = acceptTask(value);
      setNotice(value.final ? '任务已结束，请核对结果。' : '任务已接受，最终结果确认前请勿重复提交。');
      if (finished) await read();
    } catch (cause) {
      if (current(generation)) { setError(messageFrom(cause)); setNotice('请先刷新当前状态，再决定是否重试。填写内容已保留。'); }
    } finally { pending.current = false; if (current(generation)) setBusy(false); }
  }

  function selectRestoreTarget(value: string) {
    if (pending.current || currentTask.current && !currentTask.current.final) return;
    changeDraft({ ...draftValues, targetCommit: value });
    setRestorePreview(null); setRestoreConfirmed(false);
  }

  async function previewRestore() {
    if (pending.current || statusPending.current || versionsPending.current || !idle || disabled || !recoveryClear || !externalEditingStopped || !/^[0-9a-f]{40}$/.test(targetCommit)) return;
    pending.current = true; setBusy(true); setError(''); setNotice(''); setRestorePreview(null); setRestoreConfirmed(false);
    const generation = lifetime.current;
    try {
      const value = await invoke<SourceRestorePreview>('project_git_source_restore_preview', { id: projectId, input: { targetCommit } });
      if (!current(generation)) return;
      if (value.targetCommit !== targetCommit) throw new Error('预览的目标版本不符，请重新核对。');
      setRestorePreview(value);
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { pending.current = false; if (current(generation)) setBusy(false); }
  }

  async function submitRestore(operation?: SourceRestoreOperation, action?: 'continue' | 'undo') {
    if (pending.current || statusPending.current || versionsPending.current || !idle || disabled || !externalEditingStopped) return;
    if (operation ? !recoveryFresh || !recovery?.operations.includes(operation) || (action === 'continue' ? !operation.canContinue : action !== 'undo' || !operation.canUndo)
      : !recoveryClear || !restorePreview?.canApply || !restoreConfirmed || restorePreview.targetCommit !== targetCommit) return;
    const input = operation
      ? { restoreTaskId: operation.restoreTaskId, expectedPlanDigest: operation.planDigest, expectedJournalDigest: operation.journalDigest, action }
      : { targetCommit: restorePreview!.targetCommit, expectedHead: restorePreview!.head.oid, expectedBranch: restorePreview!.head.branch, expectedSnapshot: restorePreview!.snapshotDigest, expectedPlanDigest: restorePreview!.planDigest };
    pending.current = true; setBusy(true); setError(''); setNotice('');
    const generation = lifetime.current;
    readSequence.current += 1; versionsSequence.current += 1;
    setFresh(false); setVersionsFresh(false); setRecoveryFresh(false); setRestorePreview(null); setRestoreConfirmed(false);
    try {
      const value = await invoke<TaskRecord>(operation ? 'project_git_source_restore_recover' : 'project_git_source_restore_apply', { id: projectId, input });
      if (!current(generation)) return;
      const expectedType = operation ? action === 'continue' ? 'gitSourceRestoreContinue' : 'gitSourceRestoreUndo' : 'gitSourceRestoreApply';
      if (value.projectId !== projectId || value.type !== expectedType) throw new Error('取回任务身份不符，请重新核对恢复状态，勿重复提交。');
      setPollError('');
      if (!operation && (!value.final || value.status === 'succeeded')) setAcceptedTarget(targetCommit);
      const finished = acceptTask(value);
      setNotice(value.final ? '源码操作已结束，请核对结果和恢复状态。' : '源码操作已接受，请等待最终结果。');
      if (finished) await read();
    } catch (cause) {
      if (current(generation)) { setError(messageFrom(cause)); setNotice('请重新核对恢复状态；本次未自动重试或撤回。所选版本与填写内容已保留。'); }
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
      setNotice(currentTask.current?.final ? '任务已经结束，请核对最终结果。' : sourceRestoreTaskTypes.includes(selected.type) ? '已请求停止，正在等待最终结果。可能仍需处理源码恢复状态；不会自动撤回已经替换的文件。' : '已请求停止，正在等待最终结果。已提交的版本不会因取消而撤销。');
      if (finished) await read();
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { pending.current = false; if (current(generation)) setBusy(false); }
  }

  const head = status?.repository.head;
  return <section className="project-git-panel" aria-labelledby={`${id}-title`}>
    <div className="storage-heading"><h3 id={`${id}-title`}>源码版本</h3><button type="button" className="refresh-button" disabled={reading || listing || busy} onClick={() => { if (currentTask.current && !currentTask.current.final) void poll(); else void read(); }}>{reading ? '核对中…' : '刷新状态'}</button></div>
    <p>在本项目的标准 Git 仓库中保存下面列出的源码和开发文档。本次不保存数据库、附件或数据快照；可创建新分支，或预览后将已保存源码取回为待保存修改。暂不切换到已有分支。</p>
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
      <ProjectGitVersions value={versions} fresh={versionsFresh} reading={listing} error={versionsError} canRead={idle && fresh && status.canListVersions} onRead={() => { if (!idle || !fresh || !status.canListVersions) return; versionsRequested.current = true; void readVersions(status); }} canSelect={idle && !disabled && recoveryClear && versionsFresh} onSelect={selectRestoreTarget} />
      <form className="create-project-form" aria-label="创建定制分支" onSubmit={event => { event.preventDefault(); void submit('create-branch'); }}>
        <h4>从当前版本创建定制分支</h4>
        <p>仅在源码与暂存区干净时，从当前版本创建并切换到新分支。当前提交、源码、暂存区、业务数据和运行产物保持不变；不会自动构建或更新应用。</p>
        {status.repository.head?.oid && <p>分支起点：<code>{status.repository.head.oid}</code></p>}
        <label htmlFor={`${id}-branch`}>新分支名称</label><input id={`${id}-branch`} value={branchName} onChange={event => changeDraft({ ...draftValues, branchName: event.target.value })} maxLength={128} readOnly={busy} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} placeholder="custom/my-changes" />
        <p>以英文字母或数字开头，可使用英文字母、数字、短横线、下划线、点和斜线；最长 128 个字符。名称不得与已有分支重复。</p>
        {branchName && !validBranchName(branchName) && <p role="alert">分支名称无效：请勿使用 HEAD、连续点、空路径段、隐藏路径段、.lock 后缀或末尾点。</p>}
        {Boolean(status.branchCreationIssue) && <p className="task-error" role="alert">{messageFrom(status.branchCreationIssue)}</p>}
        {!branchReady && !status.branchCreationIssue && <p>需先保存首个版本，并确认当前源码无变更、暂存区和项目范围可用，再创建分支。</p>}
        <button type="submit" className="primary-button" disabled={!writable || !branchReady || !validBranchName(branchName)}>创建并切换到新分支</button>
      </form>
    </>}
    <ProjectGitSourceRestore targetCommit={targetCommit} onTargetChange={selectRestoreTarget} preview={restorePreview} recovery={recovery} recoveryFresh={recoveryFresh} recoveryError={recoveryError}
      busy={busy || active} canPreview={idle && !disabled && recoveryClear} canApply={idle && !disabled && recoveryClear} canRecover={idle && !disabled && recoveryFresh} canRefresh={idle}
      stopped={externalEditingStopped} confirmed={restoreConfirmed} onStoppedChange={value => { setExternalEditingStopped(value); setRestoreConfirmed(false); }} onConfirmedChange={setRestoreConfirmed}
      onPreview={() => void previewRestore()} onApply={() => void submitRestore()} onRefresh={() => void read()} onRecover={(operation, action) => void submitRestore(operation, action)} />
    {task && <><ProjectTask key={task.taskId} task={task} />{!task.final && <><p role="status">{restoreActive ? '源码操作结束前请保留此面板。停止后仍需核对恢复状态。' : '关闭此面板不会停止任务。'}</p><button type="button" className="refresh-button" disabled={busy} onClick={() => void cancel()}>取消当前操作</button></>}</>}
  </section>;
}

function PathList({ title, items }: { title: string; items: { path: string; reason?: string }[] }) {
  const [visible, setVisible] = useState(200);
  return <details><summary>{title}（{items.length}）</summary>{items.length ? <><ul>{items.slice(0, visible).map((item, index) => <li key={`${item.path}:${index}`}><code>{item.path}</code>{item.reason && <> · {item.reason}</>}</li>)}</ul>{items.length > visible && <button type="button" className="text-button" onClick={() => setVisible(value => value + 200)}>继续显示（剩余 {items.length - visible} 项）</button>}</> : <p>无。</p>}</details>;
}
