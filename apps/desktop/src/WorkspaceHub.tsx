import { invoke } from '@tauri-apps/api/core';
import { Archive, ArrowRight, Copy, FolderOpen, HardDrive, RefreshCw, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { ProjectSpaceRanking, WorkspaceStorageSummary } from './StoragePanels';
import { WorkspaceCleanupPanel } from './WorkspaceCleanupPanel';
import { bytesText, exactBytes, timeText } from './storage-utils';
import type { ProjectInfo, RetainedWorkspaceRoot, TaskRecord, TrashEntry, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';

interface WorkspaceHubProps {
  workspace: WorkspaceInfo | null;
  revision: number;
  enabled: boolean;
  migrating?: boolean;
  cleanupTask?: TaskRecord | null;
  projects: ProjectInfo[];
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onSelect: (projectId: string) => void;
}
type HubTab = 'overview' | 'location' | 'trash' | 'cleanup';
const hubTabs = [
  { id: 'overview' as const, label: '空间总览', icon: HardDrive },
  { id: 'location' as const, label: '工作区位置', icon: FolderOpen },
  { id: 'trash' as const, label: '回收站', icon: Trash2 },
  { id: 'cleanup' as const, label: '清理历史', icon: Archive },
];

export function WorkspaceHub(props: WorkspaceHubProps) {
  const [tab, setTab] = useState<HubTab>('overview');
  const [localCleanupTask, setLocalCleanupTask] = useState<TaskRecord | null>(props.cleanupTask ?? props.workspace?.cleanupTask ?? null);
  const [cleanupPending, setCleanupPending] = useState(false);
  const alive = useRef(true);
  const taskCallback = useRef(props.onTask); taskCallback.current = props.onTask;
  const taskFromWorkspace = props.cleanupTask ?? props.workspace?.cleanupTask;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (taskFromWorkspace) setLocalCleanupTask(taskFromWorkspace); }, [taskFromWorkspace]);
  const trackCleanupTask = useCallback((task: TaskRecord) => {
    if (alive.current && task.type === 'workspaceCleanup') setLocalCleanupTask(task);
    taskCallback.current(task);
  }, []);
  const id = useId();
  const migrating = props.migrating || Boolean(props.workspace?.activeTask) || ['running', 'paused', 'migrating'].includes(props.workspace?.migrationState ?? '');
  const root = props.workspace?.workspaceRoot ?? 'disconnected';
  const cleanupBusy = cleanupPending || Boolean(localCleanupTask && !localCleanupTask.final) || Boolean(taskFromWorkspace && !taskFromWorkspace.final);
  function activateTab(value: HubTab) {
    setTab(value);
    document.getElementById(`${id}-${value}-tab`)?.focus();
  }
  function navigateTabs(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const next = event.key === 'ArrowRight' ? (index + 1) % hubTabs.length
      : event.key === 'ArrowLeft' ? (index + hubTabs.length - 1) % hubTabs.length
        : event.key === 'Home' ? 0 : event.key === 'End' ? hubTabs.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    activateTab(hubTabs[next].id);
  }
  return <div className="workspace-hub">
    <div className="workspace-hub-tabs" role="tablist" aria-label="空间与工作区">
      {hubTabs.map(({ id: value, label, icon: Icon }, index) => <button key={value} id={`${id}-${value}-tab`} type="button" role="tab" aria-selected={tab === value} aria-controls={`${id}-${value}-panel`} tabIndex={tab === value ? 0 : -1} className={tab === value ? 'selected' : ''} onClick={() => setTab(value)} onKeyDown={(event) => navigateTabs(event, index)}><Icon size={16} />{label}</button>)}
    </div>
    {migrating && <p className="workspace-operation-status" role="status">工作区正在迁移，新操作暂时停用。请等待任务最终结果；已有窗口的草稿会保留。</p>}
    {cleanupBusy && <p className="workspace-operation-status" role="status">历史清理正在进行，管理写操作暂时停用。可在“清理历史”查看任务与实际结果。</p>}
    <div className="workspace-hub-panel" id={`${id}-overview-panel`} role="tabpanel" aria-labelledby={`${id}-overview-tab`} hidden={tab !== 'overview'}>
      <WorkspaceStorageSummary key={root} revision={props.revision} enabled={props.enabled && !migrating && tab === 'overview'} compact />
      <ProjectSpaceRanking projects={props.projects} onSelect={props.onSelect} />
      <div className="workspace-quick-actions"><button className="text-button" onClick={() => activateTab('location')}><FolderOpen size={14} />查看工作区位置<ArrowRight size={14} /></button><button className="text-button" onClick={() => activateTab('trash')}><Trash2 size={14} />管理回收站<ArrowRight size={14} /></button><button className="text-button" onClick={() => activateTab('cleanup')}><Archive size={14} />查看历史清理范围<ArrowRight size={14} /></button></div>
    </div>
    <div className="workspace-hub-panel" id={`${id}-location-panel`} role="tabpanel" aria-labelledby={`${id}-location-tab`} hidden={tab !== 'location'}>
      <WorkspaceLocation key={root} workspace={props.workspace} enabled={props.enabled && !cleanupBusy} active={tab === 'location'} migrating={migrating} onTask={props.onTask} onRefresh={props.onRefresh} />
    </div>
    <div className="workspace-hub-panel" id={`${id}-trash-panel`} role="tabpanel" aria-labelledby={`${id}-trash-tab`} hidden={tab !== 'trash'}>
      <WorkspaceTrash key={root} revision={props.revision} enabled={props.enabled && !cleanupBusy} active={tab === 'trash'} migrating={migrating} onTask={props.onTask} onRefresh={props.onRefresh} />
    </div>
    <div className="workspace-hub-panel" id={`${id}-cleanup-panel`} role="tabpanel" aria-labelledby={`${id}-cleanup-tab`} hidden={tab !== 'cleanup'}>
      <WorkspaceCleanupPanel key={root} workspaceRoot={props.workspace?.workspaceRoot ?? null} revision={props.revision} enabled={props.enabled} active={tab === 'cleanup'} migrating={migrating} cleanupTask={localCleanupTask} onTask={trackCleanupTask} onRefresh={props.onRefresh} onPendingChange={setCleanupPending} />
    </div>
  </div>;
}

// A submitted operation stays pending until its durable task reaches a final state.
function useTrackedTask(onTask: (task: TaskRecord) => void, onFinished: () => Promise<void>) {
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const callbacks = useRef({ onTask, onFinished }); callbacks.current = { onTask, onFinished };
  const completed = useRef(new Set<string>());
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const track = useCallback((value: TaskRecord) => {
    if (!mounted.current) return;
    setTask(value); setError(''); callbacks.current.onTask(value);
    if (value.final && !completed.current.has(value.taskId)) {
      completed.current.add(value.taskId);
      void callbacks.current.onFinished().catch((cause) => { if (mounted.current) setError(`刷新操作结果失败：${messageFrom(cause)}`); });
    }
  }, []);
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

function WorkspaceLocation({ workspace, enabled, active, migrating, onTask, onRefresh }: {
  workspace: WorkspaceInfo | null; enabled: boolean; active: boolean; migrating: boolean;
  onTask: (task: TaskRecord) => void; onRefresh: () => Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const [destination, setDestination] = useState('');
  const [confirmedPath, setConfirmedPath] = useState<string | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const operation = useRef(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const inputId = useId();
  const tracker = useTrackedTask(onTask, onRefresh);
  const blocked = !enabled || !active || migrating || pending || choosing || tracker.running;
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
  async function refresh() { setRefreshing(true); setError(''); try { await onRefresh(); } catch (cause) { setError(messageFrom(cause)); } finally { setRefreshing(false); } }
  async function copyPath() { try { await navigator.clipboard.writeText(workspace!.workspaceRoot); setNotice('工作区路径已复制。'); } catch { setError('无法复制，请选择下方路径手动复制。'); } }
  return <section className="lifecycle-card workspace-location-card" aria-labelledby="workspace-location-title">
    <div className="section-heading"><div><h2 id="workspace-location-title">工作区位置</h2><p>项目、备份和回收站集中保存在当前工作区。</p></div><button className="refresh-button" disabled={!enabled || refreshing || pending} onClick={() => void refresh()}><RefreshCw size={14} />{refreshing ? '刷新中…' : '刷新状态'}</button></div>
    <div className="workspace-current-location"><span>当前工作区</span><code>{workspace?.workspaceRoot ?? '尚未读取'}</code><div className="workspace-location-actions"><button className="refresh-button" disabled={!workspace} onClick={() => void copyPath()}><Copy size={14} />复制路径</button><button className="refresh-button" disabled={blocked} aria-expanded={expanded} aria-controls={`${inputId}-migration`} onClick={() => { setExpanded(!expanded); setConfirmedPath(null); setAcknowledged(false); }}><FolderOpen size={14} />{expanded ? '收起迁移设置' : '迁移工作区'}</button></div></div>
    <details className="workspace-disclosure"><summary>工作区格式与原目录记录</summary><div className="workspace-disclosure-body"><dl className="path-list"><div className="path-row"><dt>工作区格式</dt><dd>{workspace?.formatVersion ?? '未知'}</dd></div><div className="path-row"><dt>文件系统</dt><dd>{workspace?.filesystem ?? '尚未读取'}</dd></div></dl>{workspace?.retainedRoots?.length ? <div className="retained-roots"><h3>迁移原目录记录</h3><p>这里记录迁移前的位置及后续清理状态。平台使用上方当前工作区，不会自动切回原目录。</p>{workspace.retainedRoots.map((value, index) => {
      const record: RetainedWorkspaceRoot = typeof value === 'string' ? { path: value } : value;
      const cleaned = record.cleaned === true;
      return <article className={`retained-root-record${cleaned ? ' is-cleaned' : ''}`} key={record.taskId ?? index}><strong>{cleaned ? '已清理' : '保留未清理'}</strong><code>{record.path ?? record.sourcePath ?? '原目录路径未记录'}</code><p>{cleaned ? '这里只保留原路径和清理回执，不表示该目录仍存在或仍占用空间。' : record.available === false ? '当前无法读取此原目录，请在“清理历史”检查目录状态；这条记录本身不能证明目录仍然存在或占用空间。' : '此历史目录尚未登记为已清理。可在“清理历史”扫描并核对可处理范围。'}</p>{Boolean(cleaned && record.cleanup?.completedAt) && <small>清理完成：{timeText(record.cleanup?.completedAt)}</small>}{record.cleanup?.receiptPath && <div className="retained-root-receipt"><span>清理回执</span><code>{record.cleanup.receiptPath}</code></div>}{record.reason && <details><summary>查看记录说明</summary><p>{record.reason}</p></details>}</article>;
    })}</div> : <p>没有迁移原目录记录。</p>}</div></details>
    {expanded && <div className="workspace-migration-form" id={`${inputId}-migration`}>
      <form onSubmit={(event) => { event.preventDefault(); if (!blocked && destination.trim().startsWith('/')) { setConfirmedPath(destination.trim()); setAcknowledged(false); } }}>
        <h3>迁移整个工作区</h3><div className="path-input"><label htmlFor={inputId}>新的工作区目录</label><div className="path-input-controls"><input id={inputId} value={destination} disabled={blocked} onChange={(event) => editDestination(event.target.value)} placeholder="/Volumes/磁盘/新工作区" autoComplete="off" /><button className="refresh-button" type="button" disabled={blocked} onClick={() => void choose()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择路径'}</button></div></div>
        <p className="project-help">选择本机或直连外置磁盘上的独立、可写、为空的 APFS 目录。迁移会短暂暂停服务，并保留原目录。请先保存或复制各窗口的草稿，并结束外部 AI / 编辑器对工作区的写入。</p>
        <button className="refresh-button" type="submit" disabled={blocked || !destination.trim().startsWith('/')}>核对迁移范围<ArrowRight size={14} /></button>
      </form>
      {confirmedPath && <div className="operation-confirmation workspace-migration-confirmation"><strong>确认迁移整个工作区</strong><p>来源：<code>{workspace?.workspaceRoot}</code><br />目标：<code>{confirmedPath}</code></p><p>迁移包含所有项目、备份和回收站；原目录会保留。成功后需要从平台重新进入项目，并重新复制 AI 路径。</p><label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked} onChange={(event) => setAcknowledged(event.target.checked)} />我已核对来源和目标，并处理各窗口草稿与外部写入</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !acknowledged} onClick={() => void migrate()}>{pending ? '提交中…' : '确认开始迁移'}</button><button className="refresh-button" disabled={pending || tracker.running} onClick={() => { setConfirmedPath(null); setAcknowledged(false); }}>取消</button></div></div>}
    </div>}
    {tracker.running && <p className="workspace-operation-status" role="status">迁移任务已提交，正在等待最终结果。请勿重复提交。</p>}
    {(error || tracker.error) && <p role="alert" className="task-error">{error || tracker.error}</p>}{notice && <p role="status" className="workspace-inline-notice">{notice}</p>}
  </section>;
}

type TrashDetails = TrashEntry & { restoreAvailable?: boolean };
function trashState(entry: TrashDetails) { return entry.state === 'purgeIncomplete' ? '永久删除未完成' : entry.state === 'needsAttention' ? '需要检查' : entry.mode === 'maintenance' ? '维护状态保留' : '可以恢复'; }
function trashFingerprint(entry: TrashDetails) { return JSON.stringify([entry.trashId, entry.projectId, entry.name, entry.state, entry.mode, entry.path, entry.restoreAvailable, entry.activeTask, entry.lastTaskId, entry.issues]); }
function taskIdFor(entry: TrashDetails) { return (typeof entry.activeTask === 'string' ? entry.activeTask : entry.activeTask?.taskId) ?? entry.lastTaskId; }

function WorkspaceTrash({ revision, enabled, active, migrating, onTask, onRefresh }: {
  revision: number; enabled: boolean; active: boolean; migrating: boolean;
  onTask: (task: TaskRecord) => void; onRefresh: () => Promise<void>;
}) {
  const [entries, setEntries] = useState<TrashDetails[]>([]);
  const [selected, setSelected] = useState<TrashDetails | null>(null);
  const [reading, setReading] = useState(false);
  const [inspecting, setInspecting] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const [purgeTarget, setPurgeTarget] = useState<string | null>(null);
  const [confirmName, setConfirmName] = useState('');
  const [confirmScope, setConfirmScope] = useState(false);
  const selection = useRef<TrashDetails | null>(null);
  const listSequence = useRef(0);
  const detailSequence = useRef(0);
  const alive = useRef(true);
  const operation = useRef(false);
  const afterTask = useRef<() => Promise<void>>(async () => {});
  const taskCallback = useRef(onTask); taskCallback.current = onTask;
  const tracker = useTrackedTask(onTask, () => afterTask.current());
  const blocked = !enabled || !active || migrating || pending || tracker.running;
  const clearConfirmation = useCallback(() => { setPurgeTarget(null); setConfirmName(''); setConfirmScope(false); }, []);
  const select = useCallback((entry: TrashDetails | null) => { selection.current = entry; setSelected(entry); clearConfirmation(); }, [clearConfirmation]);
  const refresh = useCallback(async () => {
    if (!enabled || !active || migrating) return;
    const sequence = ++listSequence.current;
    detailSequence.current++; setInspecting(false); setReading(true); setError(''); clearConfirmation();
    try {
      const list = await invoke<TrashDetails[]>('trash_list');
      if (sequence !== listSequence.current || !alive.current) return;
      setEntries(list);
      const selectedId = selection.current?.trashId;
      const entry = selectedId ? list.find((value) => value.trashId === selectedId) ?? null : null;
      select(entry);
      const taskIds = new Set(list.map(taskIdFor));
      if (entry) taskIds.add(taskIdFor(entry));
      await Promise.all([...taskIds].filter((value): value is string => Boolean(value)).map(async (taskId) => {
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId });
          if (sequence !== listSequence.current || !alive.current) return;
          if (entry && selection.current?.trashId === entry.trashId && taskIdFor(entry) === taskId) tracker.track(task);
          else if (!task.final) taskCallback.current(task);
        } catch (cause) { if (sequence === listSequence.current && alive.current) setError(`读取关联任务失败：${messageFrom(cause)}`); }
      }));
    } catch (cause) { if (sequence === listSequence.current && alive.current) setError(messageFrom(cause)); }
    finally { if (sequence === listSequence.current && alive.current) setReading(false); }
  }, [enabled, active, migrating, select, clearConfirmation, tracker.track]);
  afterTask.current = async () => { clearConfirmation(); await refresh(); await onRefresh(); };
  useEffect(() => { alive.current = true; return () => { alive.current = false; listSequence.current++; detailSequence.current++; }; }, []);
  useEffect(() => { void refresh(); return () => { listSequence.current++; detailSequence.current++; }; }, [refresh, revision]);
  useEffect(() => { if (!active || migrating) { clearConfirmation(); setReading(false); setInspecting(false); } }, [active, migrating, clearConfirmation]);
  async function inspect(entry: TrashDetails) {
    if (blocked || operation.current) return;
    listSequence.current++; setReading(false);
    const sequence = ++detailSequence.current;
    select(entry); setInspecting(true); setError('');
    try {
      const detail = await invoke<TrashDetails>('trash_info', { trashId: entry.trashId });
      if (!alive.current || sequence !== detailSequence.current || selection.current?.trashId !== entry.trashId) return;
      select(detail);
      const taskId = taskIdFor(detail);
      if (taskId) {
        const task = await invoke<TaskRecord>('task_get', { taskId });
        if (alive.current && sequence === detailSequence.current && selection.current?.trashId === entry.trashId) tracker.track(task);
      }
    } catch (cause) { if (alive.current && sequence === detailSequence.current) { setError(messageFrom(cause)); select(null); } }
    finally { if (alive.current && sequence === detailSequence.current) setInspecting(false); }
  }
  const restoreAllowed = selected?.state === 'trashed' && selected.restoreAvailable !== false;
  const actionBlocked = blocked || inspecting || reading || Boolean(selected?.activeTask);
  async function submit(kind: 'trash_restore' | 'trash_purge') {
    const entry = selection.current;
    if (!entry || actionBlocked || operation.current) return;
    if (kind === 'trash_restore' && (entry.state !== 'trashed' || entry.restoreAvailable === false)) return;
    if (kind === 'trash_purge' && (purgeTarget !== trashFingerprint(entry) || confirmName !== entry.name || !confirmScope)) return;
    operation.current = true; setPending(true); setError('');
    listSequence.current++; detailSequence.current++;
    try {
      const task = await invoke<TaskRecord>(kind, { trashId: entry.trashId });
      if (alive.current && selection.current?.trashId === entry.trashId) tracker.track(task);
      else taskCallback.current(task);
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(false); }
  }
  return <section className="lifecycle-card workspace-trash-card" aria-labelledby="workspace-trash-title">
    <div className="section-heading"><div><h2 id="workspace-trash-title">回收站</h2><p>删除的完整项目会一直保留，直到你明确永久删除。</p></div><button className="refresh-button" disabled={!enabled || migrating || reading || pending || tracker.running} onClick={() => void refresh()}><RefreshCw size={14} />{reading ? '读取中…' : '刷新回收站'}</button></div>
    <div className="workspace-trash-layout">
      <div className="workspace-trash-list" aria-label="回收站项目">{entries.length ? entries.map((entry) => <button className={`trash-row${selected?.trashId === entry.trashId ? ' selected' : ''}`} key={entry.trashId} disabled={blocked} aria-pressed={selected?.trashId === entry.trashId} onClick={() => void inspect(entry)}><Archive size={19} /><span><strong>{entry.name || '无法读取的回收站记录'}</strong><small>{timeText(entry.deletedAt)}</small></span><span title={exactBytes(entry.sizeBytes)}>{bytesText(entry.sizeBytes)}<small>{trashState(entry)}</small></span></button>) : <div className="workspace-empty-state"><Trash2 size={26} /><p>{reading ? '正在读取回收站…' : '回收站为空'}</p><span>删除的项目会出现在这里。</span></div>}</div>
      <div className="workspace-trash-details">{selected ? <><div className="workspace-trash-detail-heading"><h3>{selected.name || '无法读取的回收站记录'}</h3><span>{trashState(selected)}</span></div>{inspecting && <p role="status">正在核对回收站记录…</p>}<dl><div><dt>项目 ID</dt><dd><code>{selected.projectId || '未知'}</code></dd></div><div><dt>回收站记录</dt><dd><code>{selected.trashId}</code></dd></div><div><dt>删除时间</dt><dd>{timeText(selected.deletedAt)}</dd></div></dl>{selected.path && <details className="workspace-disclosure"><summary>查看保留目录</summary><code className="workspace-scope-path">{selected.path}</code></details>}{selected.issues?.map((issue, index) => <p className="task-error" key={index}>{messageFrom(issue)}</p>)}<p>恢复会保留原项目 ID 和维护状态。已有窗口需要处理草稿后，再重新加载。</p>{!restoreAllowed && <p className="task-error">{selected.state === 'purgeIncomplete' ? '已有文件被永久删除，此记录不能再恢复。可以查看任务结果后重试永久删除剩余文件。' : '此记录目前不可恢复，请先检查记录状态与关联任务。'}</p>}<div className="confirmation-actions"><button className="primary-button" disabled={actionBlocked || !restoreAllowed} onClick={() => void submit('trash_restore')}>恢复项目</button><button className="danger-button" disabled={actionBlocked} onClick={() => { clearConfirmation(); setPurgeTarget(trashFingerprint(selected)); }}>永久删除…</button></div>
        {purgeTarget === trashFingerprint(selected) && <div className="operation-confirmation purge-confirmation"><strong>永久删除「{selected.name}」？</strong><p>此操作无法撤销。将删除此记录中的源码、数据库、附件、网页产物与说明，范围仅为 <code>{selected.trashId}</code>。历史备份与外部导出的 ZIP 会保留，其他回收站项目不受影响。</p><label>输入项目名称「{selected.name}」确认<input value={confirmName} onChange={(event) => setConfirmName(event.target.value)} disabled={actionBlocked} autoComplete="off" /></label><label className="checkbox-label"><input type="checkbox" checked={confirmScope} onChange={(event) => setConfirmScope(event.target.checked)} disabled={actionBlocked} />我已核对项目和范围，理解完整项目文件将永久删除</label><div className="confirmation-actions"><button className="danger-button" disabled={actionBlocked || confirmName !== selected.name || !confirmScope} onClick={() => void submit('trash_purge')}>{pending ? '提交中…' : '确认永久删除完整项目'}</button><button className="refresh-button" disabled={pending || tracker.running} onClick={clearConfirmation}>取消，继续保留</button></div></div>}
      </> : <div className="workspace-empty-state"><Archive size={26} /><p>选择一个项目查看详情</p><span>恢复和永久删除操作都在详情中进行。</span></div>}</div>
    </div>
    {(pending || tracker.running) && <p className="workspace-operation-status" role="status">{pending ? '正在提交操作…' : '任务正在执行，等待最终结果后更新回收站。'}</p>}
    {(error || tracker.error) && <p className="task-error" role="alert">{error || tracker.error}</p>}
  </section>;
}
