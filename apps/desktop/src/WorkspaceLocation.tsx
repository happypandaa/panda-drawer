import { invoke } from '@tauri-apps/api/core';
import { ArrowRight, Copy, FolderOpen, RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { RetainedWorkspaceRoot, TaskRecord, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';
import { ProjectTask } from './ProjectTask';

// A submitted operation stays pending until its durable task reaches a final state.
function useTrackedTask(onTask: (task: TaskRecord) => void, onFinished: () => Promise<void>, initialTask?: TaskRecord | null) {
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const callbacks = useRef({ onTask, onFinished }); callbacks.current = { onTask, onFinished };
  const completed = useRef(new Set<string>());
  const known = useRef(new Map<string, TaskRecord>());
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const track = useCallback((value: TaskRecord) => {
    if (!mounted.current) return;
    if (known.current.get(value.taskId)?.final && !value.final) return;
    known.current.set(value.taskId, value);
    setTask(value); setError(''); callbacks.current.onTask(value);
    if (value.final && !completed.current.has(value.taskId)) {
      completed.current.add(value.taskId);
      void callbacks.current.onFinished().catch((cause) => { if (mounted.current) setError(`刷新操作结果失败：${messageFrom(cause)}`); });
    }
  }, []);
  useEffect(() => { if (initialTask) track(initialTask); }, [initialTask, track]);
  useEffect(() => {
    if (!task || task.final) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const value = await invoke<TaskRecord>('task_get', { taskId: task!.taskId });
        if (cancelled) return;
        track(value);
        if (!value.final) timer = setTimeout(() => void poll(), 1000);
      } catch (cause) {
        if (cancelled) return;
        setError(`任务状态暂未读取，操作仍可能进行中：${messageFrom(cause)}`);
        timer = setTimeout(() => void poll(), 2000);
      }
    }
    timer = setTimeout(() => void poll(), 500);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [task?.taskId, task?.final, track]);
  return { task, track, error, running: Boolean(task && !task.final) };
}

const retainedRecord = (value: string | RetainedWorkspaceRoot): RetainedWorkspaceRoot => typeof value === 'string' ? { path: value } : value;

export interface WorkspaceLocationProps {
  workspace: WorkspaceInfo | null;
  dataRoot?: string | null;
  enabled: boolean;
  active: boolean;
  migrating: boolean;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onOpenHistory?: () => void;
}

// Settings › 存储位置: where things live, and moving the whole workspace.
export function WorkspaceLocation({ workspace, dataRoot, enabled, active, migrating, onTask, onRefresh }: WorkspaceLocationProps) {
  const [expanded, setExpanded] = useState(false);
  const [destination, setDestination] = useState('');
  const [confirmedPath, setConfirmedPath] = useState<string | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [retryPending, setRetryPending] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const operation = useRef(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const inputId = useId();
  const tracker = useTrackedTask(onTask, onRefresh);
  const cleanupTracker = useTrackedTask(onTask, onRefresh, workspace?.migrationCleanupTask?.type === 'workspaceMigrationCleanup' ? workspace.migrationCleanupTask : null);
  const records = (workspace?.retainedRoots ?? []).map(retainedRecord);
  const pendingRecords = records.filter(record => record.sourceCleanup?.cleanupPending === true);
  const retryBlocked = !enabled || !active || migrating || pending || retryPending || choosing || tracker.running || cleanupTracker.running;
  const blocked = retryBlocked || pendingRecords.length > 0;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current++; }; }, []);
  useEffect(() => { if (!active) { generation.current++; setChoosing(false); setConfirmedPath(null); setAcknowledged(false); } }, [active]);
  function editDestination(value: string) { setDestination(value); setConfirmedPath(null); setAcknowledged(false); }
  async function choose() {
    if (blocked || operation.current) return;
    const sequence = ++generation.current; setChoosing(true); setError('');
    try { const value = await invoke<string | null>('choose_path', { kind: 'workspace' }); if (sequence === generation.current && value) editDestination(value); }
    catch (cause) { if (sequence === generation.current) setError(`路径选择失败，可直接填写绝对路径：${messageFrom(cause)}`); }
    finally { if (sequence === generation.current) setChoosing(false); }
  }
  async function migrate() {
    if (blocked || operation.current || !acknowledged || confirmedPath !== destination.trim() || !confirmedPath?.startsWith('/')) return;
    operation.current = true; setPending(true); setError('');
    try { const task = await invoke<TaskRecord>('workspace_migrate', { input: { destinationPath: confirmedPath } }); if (mounted.current) tracker.track(task); else onTask(task); }
    catch (cause) { if (mounted.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (mounted.current) setPending(false); }
  }
  async function retryCleanup(record: RetainedWorkspaceRoot) {
    if (retryBlocked || operation.current || !record.taskId || record.sourceCleanup?.cleanupPending !== true) return;
    operation.current = true; setRetryPending(true); setError('');
    try {
      const task = await invoke<TaskRecord>('workspace_migration_cleanup', { migrationId: record.taskId });
      if (mounted.current) cleanupTracker.track(task); else onTask(task);
    } catch (cause) {
      if (mounted.current) setError(`原目录删除结果需要核对：${messageFrom(cause)}。请刷新状态，不会自动重复提交。`);
      void onRefresh().catch(() => {});
    } finally { operation.current = false; if (mounted.current) setRetryPending(false); }
  }
  async function refresh() { setRefreshing(true); setError(''); try { await onRefresh(); } catch (cause) { setError(messageFrom(cause)); } finally { setRefreshing(false); } }
  async function copyPath(value: string | null | undefined, label: string) {
    if (!value) return;
    try { await navigator.clipboard.writeText(value); setNotice(`${label}路径已复制。`); } catch { setError('无法复制，请选择显示的路径手动复制。'); }
  }
  const step = tracker.running || pending ? 3 : confirmedPath ? 2 : 1;
  return <section className="lifecycle-card workspace-location-card" aria-labelledby="workspace-location-title">
    <div className="section-heading"><div><h2 id="workspace-location-title">存储位置</h2><p>软件管理的内容保存在下面两个位置。</p></div><button className="refresh-button" disabled={!enabled || refreshing || pending} onClick={() => void refresh()}><RefreshCw size={14} />{refreshing ? '刷新中…' : '刷新状态'}</button></div>
    <div className="settings-location-row"><div><h3>项目工作区</h3><p>网页项目的源码、数据库和文件，以及平台管理的 Node 项目文件夹保存在这里。</p><code>{workspace?.workspaceRoot ?? '尚未读取'}</code>{workspace?.filesystem && <small>文件系统 {workspace.filesystem}</small>}</div><button className="refresh-button" disabled={!workspace} onClick={() => void copyPath(workspace?.workspaceRoot, '工作区')}><Copy size={14} />复制路径</button></div>
    <div className="settings-location-row"><div><h3>软件数据目录</h3><p>存放 Node 的配置、数据、日志，以及软件设置和运行记录。迁移工作区时不会移动。</p><code>{dataRoot ?? '尚未读取'}</code></div><button className="refresh-button" disabled={!dataRoot} onClick={() => void copyPath(dataRoot, '数据目录')}><Copy size={14} />复制路径</button></div>
    <div className="settings-migration">
      <div className="settings-migration-heading"><div><h3>迁移工作区</h3><p>把工作区移到新位置，比如外置磁盘。平台会复制并核验，在新位置成功重开后自动删除原目录；此前中断会保留原目录。</p></div><button className="refresh-button" disabled={blocked} aria-expanded={expanded} aria-controls={`${inputId}-migration`} onClick={() => { setExpanded(!expanded); setConfirmedPath(null); setAcknowledged(false); }}><FolderOpen size={14} />{expanded ? '收起' : '迁移工作区'}</button></div>
      {expanded && <div className="workspace-migration-form" id={`${inputId}-migration`}>
        <ol className="settings-steps" aria-label="迁移步骤">{['选择位置', '核对', '迁移中'].map((label, index) => <li key={label} aria-current={step === index + 1 ? 'step' : undefined}>{index + 1} {label}</li>)}</ol>
        <form onSubmit={(event) => { event.preventDefault(); if (!blocked && destination.trim().startsWith('/')) { setConfirmedPath(destination.trim()); setAcknowledged(false); } }}>
          <div className="path-input"><label htmlFor={inputId}>新的工作区位置</label><div className="path-input-controls"><input id={inputId} value={destination} disabled={blocked} onChange={(event) => editDestination(event.target.value)} placeholder="/Volumes/磁盘/新工作区" autoComplete="off" /><button className="refresh-button" type="button" disabled={blocked} onClick={() => void choose()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择…'}</button></div></div>
          <div className="settings-migration-notes"><div><strong>位置要求</strong><p>空文件夹、可写入、APFS 格式，在本机或直连磁盘上。</p></div><div><strong>开始前请</strong><p>保存各项目窗口的草稿，关闭正在访问项目的 AI 或编辑器。迁移会短暂暂停服务。</p></div></div>
          <button className="refresh-button" type="submit" disabled={blocked || !destination.trim().startsWith('/')}>下一步：核对<ArrowRight size={14} /></button>
        </form>
        {confirmedPath && <div className="operation-confirmation workspace-migration-confirmation"><strong>确认迁移整个工作区</strong><p>来源：<code>{workspace?.workspaceRoot}</code><br />目标：<code>{confirmedPath}</code></p><p>迁移包含网页项目和平台管理的 Node 项目文件夹。新位置成功重开后，原目录会自动永久删除。请处理窗口草稿后重新进入项目，并重新复制 AI 路径。</p><label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked} onChange={(event) => setAcknowledged(event.target.checked)} />我已核对来源和目标，并处理各窗口草稿与外部写入</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !acknowledged} onClick={() => void migrate()}>{pending ? '提交中…' : '确认开始迁移'}</button><button className="refresh-button" disabled={pending || tracker.running} onClick={() => { setConfirmedPath(null); setAcknowledged(false); }}>取消</button></div></div>}
      </div>}
    </div>
    {pendingRecords.length > 0 && <div className="settings-history-notice" role="status"><span>当前工作区可继续使用；有 {pendingRecords.length} 个迁移原目录的删除尚未完成。请核对下面的残留路径并重试。</span></div>}
    {pendingRecords.length > 0 && <section className="retained-roots workspace-source-cleanup" aria-label="迁移原目录收尾">{pendingRecords.map((record, index) => <article className="retained-root-record" key={record.taskId ?? index}><strong>原目录删除未完成</strong><code>{record.sourceCleanup?.sourcePath ?? record.path ?? record.sourcePath ?? '原目录路径未记录'}</code>{record.sourceCleanup?.error && <p className="task-error">{messageFrom(record.sourceCleanup.error)}</p>}<p>当前工作区可继续使用。重试仅针对这次迁移登记的原目录。</p>{record.taskId && <button className="refresh-button" disabled={retryBlocked} onClick={() => void retryCleanup(record)}>{retryPending || cleanupTracker.running ? '正在删除原目录…' : '重试删除'}</button>}</article>)}</section>}
    <details className="workspace-disclosure"><summary>技术信息与迁移记录</summary><div className="workspace-disclosure-body"><dl className="path-list"><div className="path-row"><dt>工作区格式</dt><dd>{workspace?.formatVersion ?? '未知'}</dd></div><div className="path-row"><dt>文件系统</dt><dd>{workspace?.filesystem ?? '尚未读取'}</dd></div></dl>{records.length ? <div className="retained-roots"><h3>迁移原目录记录</h3><p>这里显示迁移前的位置及当前是否可读取。平台使用上方当前工作区，不会自动切回原目录。</p>{records.map((record, index) => {
      const cleanup = record.sourceCleanup;
      const absent = record.absent === true || cleanup?.phase === 'absent';
      const pendingCleanup = cleanup?.cleanupPending === true;
      const completedCleanup = cleanup?.completed === true;
      const path = cleanup?.sourcePath ?? record.path ?? record.sourcePath ?? '原目录路径未记录';
      return <article className={`retained-root-record${absent ? ' is-absent' : ''}`} key={record.taskId ?? index}><strong>{pendingCleanup ? '原目录删除未完成' : completedCleanup ? '原目录收尾已完成' : absent ? '目录当前不存在' : cleanup?.phase === 'awaitingReopen' ? '等待新位置重开' : '旧迁移记录'}</strong><code>{path}</code><p>{pendingCleanup ? '当前工作区可继续使用，可在上方核对残留并重试删除。' : completedCleanup ? absent ? '原磁盘上的登记路径当前不存在，未据此推断是谁删除了目录。' : '这次迁移的原目录已完成收尾。' : absent ? '原磁盘上的此路径当前不存在；平台不保留删除历史，也不据此认定是谁删除了目录。' : cleanup?.phase === 'awaitingReopen' ? '新位置尚未完成重开，原目录继续保留。' : '这份旧记录仅供核对，不会自动取得新的删除许可。'}</p>{record.reason && <details><summary>查看记录说明</summary><p>{record.reason}</p></details>}</article>;
    })}</div> : <p>没有迁移原目录记录。</p>}</div></details>
    {tracker.running && <p className="workspace-operation-status" role="status">迁移任务已提交，正在等待最终结果。请勿重复提交。</p>}
    {cleanupTracker.task && <div className="workspace-source-cleanup-result"><ProjectTask task={cleanupTracker.task} /></div>}
    {cleanupTracker.running && <p className="workspace-operation-status" role="status">原目录删除会继续执行，离开此页不会取消。当前工作区可继续使用，请勿重复提交。</p>}
    {(error || tracker.error || cleanupTracker.error) && <p role="alert" className="task-error">{error || tracker.error || cleanupTracker.error}</p>}{notice && <p role="status" className="workspace-inline-notice">{notice}</p>}
  </section>;
}
