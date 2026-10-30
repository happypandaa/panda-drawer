import { invoke } from '@tauri-apps/api/core';
import { Copy, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import { markHistoryScanStale, publishHistoryScan } from './history-scan-store';
import { bytesText, exactBytes, timeText } from './storage-utils';
import type { ByteCount, ProjectInfo, TaskRecord } from './types';
import { bigBytes, bigText, partBytes } from './storage-utils';
import { projectRegenerable, regenerableCategories } from './workspace-storage-data';
import { messageFrom } from './ui-utils';

type CleanupKind = 'restoreOriginal';
interface CleanupItem {
  id: string;
  kind: CleanupKind;
  label: string;
  path: string;
  projectId?: string | null;
  name?: string | null;
  totalBytes: ByteCount;
  fileCount: ByteCount;
  canClean: boolean;
  blockedReasons: string[];
}
interface CleanupPlan {
  planId: string;
  expiresAt: number;
  workspaceRoot: string;
  items: CleanupItem[];
  preserved: string[];
  confirmationText: string;
}
interface CleanupPanelProps {
  workspaceRoot: string | null;
  instanceId?: string;
  revision: number;
  enabled: boolean;
  active: boolean;
  migrating: boolean;
  compact?: boolean;
  projects?: ProjectInfo[];
  isProjectBusy?: (project: ProjectInfo) => boolean;
  onProjectTask?: (task: TaskRecord, project: ProjectInfo) => void;
  // Scan once when the panel first becomes active without a usable result.
  autoScan?: boolean;
  cleanupTask?: TaskRecord | null;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onPendingChange?: (pending: boolean) => void;
}
const kinds: Record<CleanupKind, string> = { restoreOriginal: '取回前的原件' };
const groups: { kind: CleanupKind; description: string }[] = [
  { kind: 'restoreOriginal', description: '从版本记录取回数据或源码时，为防万一保留的原文件。取回仍未完成时会保留。' },
];
// Protected items carry a user-facing reason after a stable code prefix.
const reasonText = (reason: string) => reason.startsWith('CLEANUP_ITEM_PROTECTED: ') ? reason.slice('CLEANUP_ITEM_PROTECTED: '.length) : reason;
const confirmationPhrase = '永久清理所选内容';
function readPlan(result: Record<string, unknown> | null): CleanupPlan {
  if (!result || typeof result.planId !== 'string' || !result.planId || typeof result.expiresAt !== 'number' || !Number.isFinite(result.expiresAt) || typeof result.workspaceRoot !== 'string' || !Array.isArray(result.items) || !Array.isArray(result.preserved) || result.confirmationText !== confirmationPhrase) throw new Error('清理范围返回不完整，请重新扫描；未提交任何清理操作。');
  const ids = new Set<string>();
  for (const value of result.items) {
    if (!value || typeof value !== 'object') throw new Error('清理范围中的条目无效，请重新扫描。');
    const item = value as Record<string, unknown>;
    if (typeof item.id !== 'string' || !item.id || ids.has(item.id) || !Object.prototype.hasOwnProperty.call(kinds, String(item.kind)) || typeof item.label !== 'string' || typeof item.path !== 'string' || !item.path.startsWith('/') || typeof item.canClean !== 'boolean' || !Array.isArray(item.blockedReasons) || !item.blockedReasons.every((reason) => typeof reason === 'string')) throw new Error('清理范围中的条目不完整，请重新扫描。');
    ids.add(item.id);
  }
  if (!result.preserved.every((value) => typeof value === 'string')) throw new Error('清理范围缺少明确的保留内容，请重新扫描。');
  return result as unknown as CleanupPlan;
}
function checkedTask(value: TaskRecord, type: string, taskId?: string, projectId?: string): TaskRecord {
  if (!value || typeof value.taskId !== 'string' || !value.taskId || value.type !== type || typeof value.final !== 'boolean' || !['queued', 'running', 'succeeded', 'failed', 'interrupted'].includes(value.status) || value.final !== ['succeeded', 'failed', 'interrupted'].includes(value.status) || taskId && value.taskId !== taskId || projectId && value.projectId !== projectId || !projectId && value.projectId != null) throw new Error('清理任务回执身份或格式不符，实际提交结果需要核对；不会重复提交。');
  return value;
}
function countText(value: ByteCount | undefined) {
  if (value == null) return '文件数尚未统计';
  try { const count = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return count < 0n ? '文件数未知' : `${count} 个文件`; } catch { return '文件数未知'; }
}
function exact(value: ByteCount | undefined): bigint | null {
  try { if (value == null) return null; const bytes = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return bytes >= 0n ? bytes : null; } catch { return null; }
}
// A total with any unknown member is shown as a lower bound.
function totalText(items: CleanupItem[]) {
  let total = 0n; let unknown = false;
  for (const item of items) { const bytes = exact(item.totalBytes); if (bytes === null) unknown = true; else total += bytes; }
  const text = bytesText({ $int64: total.toString() });
  return unknown ? `至少 ${text}` : text;
}
function itemDescription(item: CleanupItem) {
  return [kinds[item.kind], item.name || item.label, item.projectId ? `项目 ${item.projectId}` : null, item.path].filter(Boolean).join('，');
}
const cleanable = (item: CleanupItem) => item.canClean && !item.blockedReasons.length;
function focusAndReveal(element: HTMLElement | null) {
  element?.focus({ preventScroll: true });
  element?.scrollIntoView({ block: 'start' });
}

export function WorkspaceCleanupPanel(props: CleanupPanelProps) {
  const [reviewOpen, setReviewOpen] = useState(false);
  const [selectedProjects, setSelectedProjects] = useState<string[]>([]);
  const [projectResults, setProjectResults] = useState<TaskRecord[]>([]);
  const observedTasks = useRef(new Map<string, TaskRecord>());
  const [planInstance, setPlanInstance] = useState(props.instanceId);
  const [plan, setPlan] = useState<CleanupPlan | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [previewTask, setPreviewTask] = useState<TaskRecord | null>(null);
  const [cleanupTask, setCleanupTask] = useState<TaskRecord | null>(props.cleanupTask ?? null);
  const [pending, setPending] = useState<'preview' | 'cleanup' | null>(null);
  const [consumed, setConsumed] = useState(false);
  const [reviewed, setReviewed] = useState<string | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [scannedAt, setScannedAt] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  const [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [notice, setNotice] = useState('');
  // A new object per request, so the final transition can ask again for the same task.
  const [resultFocus, setResultFocus] = useState<{ taskId: string } | null>(null);
  const alive = useRef(true);
  const operation = useRef(false);
  const autoScanned = useRef(false);
  const finalTasks = useRef(new Set<string>());
  const runningCleanupTasks = useRef(new Set<string>());
  const reviewButton = useRef<HTMLButtonElement>(null);
  const resultHeading = useRef<HTMLHeadingElement>(null);
  const handledResultFocus = useRef(resultFocus);
  const callbacks = useRef(props); callbacks.current = props;
  const id = useId();
  const clearConfirmation = useCallback(() => { setReviewed(null); setAcknowledged(false); }, []);
  const track = useCallback((task: TaskRecord, report = true) => {
    if (observedTasks.current.get(task.taskId)?.final && !task.final) return;
    observedTasks.current.set(task.taskId, task);
    if (report) callbacks.current.onTask(task);
    if (!alive.current) return;
    if (task.type === 'workspaceCleanupPreview') setPreviewTask(task);
    else if (task.type === 'workspaceCleanup') {
      setCleanupTask(task);
      if (!task.final) runningCleanupTasks.current.add(task.taskId);
    }
    else return;
    setPollError('');
    if (!task.final || finalTasks.current.has(task.taskId)) return;
    finalTasks.current.add(task.taskId);
    if (task.type === 'workspaceCleanupPreview') {
      if (task.status === 'succeeded') {
        try {
          const value = readPlan(task.result);
          setPlan(value); setSelected([]); setSelectedProjects([]); setConsumed(false); setNow(Date.now()); setScannedAt(Date.now()); clearConfirmation();
          publishHistoryScan(value.workspaceRoot, value.items);
        } catch (cause) { setError(messageFrom(cause)); }
      }
    } else {
      clearConfirmation(); markHistoryScanStale();
      if (runningCleanupTasks.current.delete(task.taskId)) setResultFocus({ taskId: task.taskId });
      if (!operation.current) void callbacks.current.onRefresh().catch((cause) => { if (alive.current) setError(`清理任务已结束，但重新读取空间失败：${messageFrom(cause)}`); });
    }
  }, [clearConfirmation]);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (props.cleanupTask) track(props.cleanupTask, false); }, [props.cleanupTask, track]);
  useEffect(() => { clearConfirmation(); }, [props.active, props.migrating, props.revision, props.workspaceRoot, props.instanceId, clearConfirmation]);
  useEffect(() => {
    if (!plan) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [plan]);
  useEffect(() => {
    const tasks = [previewTask, cleanupTask].filter((task): task is TaskRecord => Boolean(task && !task.final));
    if (!tasks.length) return;
    let cancelled = false;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    function schedule(taskId: string, delay: number) { const timer = setTimeout(() => { timers.delete(timer); void poll(taskId); }, delay); timers.add(timer); }
    async function poll(taskId: string) {
      try { const expected = tasks.find(task => task.taskId === taskId)!; const task = checkedTask(await invoke<TaskRecord>('task_get', { taskId }), expected.type, taskId); if (cancelled) return; track(task); if (!task.final) schedule(taskId, 1000); }
      catch (cause) { if (!cancelled) { setPollError(`任务状态暂未读取，操作仍可能继续：${messageFrom(cause)}`); schedule(taskId, 2000); } }
    }
    for (const task of tasks) schedule(task.taskId, 500);
    return () => { cancelled = true; for (const timer of timers) clearTimeout(timer); };
  }, [previewTask?.taskId, previewTask?.final, cleanupTask?.taskId, cleanupTask?.final, track]);
  const cleanupRunning = Boolean(cleanupTask && !cleanupTask.final) || Boolean(props.cleanupTask && !props.cleanupTask.final);
  const previewRunning = Boolean(previewTask && !previewTask.final);
  const blocked = !props.enabled || !props.active || props.migrating || Boolean(pending) || cleanupRunning || previewRunning;
  const expired = Boolean(plan && now >= plan.expiresAt);
  const wrongWorkspace = Boolean(plan && plan.workspaceRoot !== props.workspaceRoot || planInstance !== props.instanceId);
  const planBlocked = blocked || !plan || expired || wrongWorkspace || consumed;
  const selectedItems = plan?.items.filter((item) => selected.includes(item.id)) ?? [];
  const temporaryProjects = (props.projects ?? []).filter(project => (projectRegenerable(project) ?? 0n) > 0n);
  const selectedTargets = temporaryProjects.filter(project => selectedProjects.includes(project.id));
  const fingerprint = JSON.stringify([plan, [...selected].sort(), selectedTargets.map(project => [project.id, project.projectRoot, project.documentEpoch, project.storage, props.isProjectBusy?.(project)]), props.workspaceRoot, props.instanceId, props.revision]);
  const selectedCount = selectedItems.length + selectedTargets.length;
  const confirmationCurrent = reviewed === fingerprint && selectedCount > 0 && selectedItems.every(cleanable) && selectedTargets.every(project => !props.isProjectBusy?.(project));
  const selectedTotal = selectedItems.reduce((sum, item) => sum + (exact(item.totalBytes) ?? 0n), 0n) + selectedTargets.reduce((sum, project) => sum + (projectRegenerable(project) ?? 0n), 0n);
  const selectedTotalText = `${selectedTargets.length ? '约 ' : selectedItems.some(item => exact(item.totalBytes) === null) ? '至少 ' : ''}${bigText(selectedTotal)}`;
  useEffect(() => {
    if (!resultFocus || handledResultFocus.current === resultFocus) return;
    // Consume hidden-tab requests too, so returning to a tab never steals focus.
    handledResultFocus.current = resultFocus;
    if (props.active && resultFocus.taskId === cleanupTask?.taskId) focusAndReveal(resultHeading.current);
  }, [resultFocus, props.active, cleanupTask?.taskId]);
  const preview = useCallback(async () => {
    if (blocked || operation.current) return;
    operation.current = true; setPlanInstance(props.instanceId); setPending('preview'); setError(''); setPollError(''); setNotice(''); setPlan(null); setSelected([]); setSelectedProjects([]); clearConfirmation();
    try { track(checkedTask(await invoke<TaskRecord>('workspace_cleanup_preview'), 'workspaceCleanupPreview')); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }, [blocked, clearConfirmation, track, props.instanceId]);
  useEffect(() => {
    if (!props.autoScan || autoScanned.current || blocked || plan) return;
    autoScanned.current = true;
    void preview();
  }, [props.autoScan, blocked, plan, preview]);
  async function waitForFinal(initial: TaskRecord, project?: ProjectInfo) {
    let task = initial;
    while (!task.final) {
      await new Promise(resolve => setTimeout(resolve, 500));
      if (!alive.current) throw new Error('清理面板已关闭，后续清理未提交。请核对已提交的任务后重新扫描。');
      const observed = observedTasks.current.get(task.taskId);
      if (observed?.final) { task = checkedTask(observed, initial.type, initial.taskId, project?.id); break; }
      try {
        task = checkedTask(await invoke<TaskRecord>('task_get', { taskId: task.taskId }), initial.type, initial.taskId, project?.id);
        if (project) {
          callbacks.current.onProjectTask?.(task, project);
          setProjectResults(current => [...current.filter(value => value.taskId !== task.taskId), task]);
        } else track(task);
        setPollError('');
      } catch (cause) {
        throw new Error(`任务状态暂未读取，操作仍可能继续：${messageFrom(cause)}。后续项目未提交；请核对同一任务后重新扫描，不会重复提交。`);
      }
    }
    return task;
  }
  async function submit() {
    if (planBlocked || operation.current || !plan || Date.now() >= plan.expiresAt || !confirmationCurrent || !acknowledged) return;
    operation.current = true; setPending('cleanup'); setConsumed(true); setError(''); setPollError(''); setNotice(''); callbacks.current.onPendingChange?.(true);
    const input = { planId: plan.planId, itemIds: selectedItems.map(item => item.id), confirmation: plan.confirmationText };
    // Capture the reviewed targets. Refreshing a completed task must not alter this batch.
    const targets = [...selectedTargets];
    const batchInstance = props.instanceId;
    clearConfirmation();
    try {
      // History plans expire and require no running project operation. Complete history first.
      if (selectedItems.length) {
        const task = checkedTask(await invoke<TaskRecord>('workspace_cleanup', { input }), 'workspaceCleanup');
        track(task);
        setResultFocus({ taskId: task.taskId });
        const final = await waitForFinal(task);
        if (final.status !== 'succeeded' || final.result?.completed !== true) throw new Error('旧资料清理未完整成功，后续临时文件清理未提交；请核对结果后重新扫描。');
      }
      for (const project of targets) {
        const currentProject = callbacks.current.projects?.find(value => value.id === project.id);
        if (!alive.current || callbacks.current.instanceId !== batchInstance || callbacks.current.workspaceRoot !== plan.workspaceRoot || !currentProject || currentProject.documentEpoch !== project.documentEpoch || currentProject.projectRoot !== project.projectRoot || callbacks.current.migrating || callbacks.current.isProjectBusy?.(currentProject)) throw new Error(`「${project.name}」当前不能清理，后续项目未提交；请重新核对。`);
        const task = checkedTask(await invoke<TaskRecord>('storage_cleanup', { projectId: project.id, input: { categories: [...regenerableCategories] } }), 'storageCleanup', undefined, project.id);
        callbacks.current.onProjectTask?.(task, project);
        setProjectResults(current => [...current, task]);
        const final = await waitForFinal(task, project);
        if (final.status !== 'succeeded' || final.result?.completed === false) throw new Error(`「${project.name}」清理未完整成功，后续项目未提交；请核对结果后重新扫描。`);
      }
      markHistoryScanStale();
      setNotice('所选清理任务已结束，请核对实际删除与保留结果。');
    }
    catch (cause) { if (alive.current) setError(`${messageFrom(cause)}。写入结果可能需要核对；请重新扫描，不会自动重复提交。`); }
    finally {
      markHistoryScanStale();
      operation.current = false; callbacks.current.onPendingChange?.(false);
      if (alive.current) {
        setPending(null);
        try { await callbacks.current.onRefresh(); } catch (cause) { setError(`清理任务已结束，但重新读取空间失败：${messageFrom(cause)}`); }
      }
    }
  }
  function toggle(item: CleanupItem, checked: boolean) {
    setSelected((current) => checked ? [...current.filter((value) => value !== item.id), item.id] : current.filter((value) => value !== item.id));
    clearConfirmation();
  }
  function toggleGroup(items: CleanupItem[], select: boolean) {
    const ids = items.map((item) => item.id);
    setSelected((current) => select ? [...current.filter((value) => !ids.includes(value)), ...ids] : current.filter((value) => !ids.includes(value)));
    clearConfirmation();
  }
  async function copyPath(value: string) { try { await navigator.clipboard.writeText(value); if (alive.current) setNotice('目录路径已复制。'); } catch { if (alive.current) setNotice('无法自动复制，请选中显示的完整目录路径手动复制。'); } }
  const scanning = pending === 'preview' || previewRunning;
  const body = <>
    <div className="history-heading">
      <div><h2 id={`${id}-title`}>{props.compact ? '临时文件和旧资料' : '历史资料'}</h2><p>这里列出网页项目取回数据或源码前保留的原件。勾选不再需要的内容，确认后永久删除。当前项目和未完成的恢复不在此删除范围。</p></div>
      <div className="history-scan-status">{scanning ? <span role="status">正在扫描…</span> : scannedAt ? <span>{props.autoScan ? '已自动扫描' : '已扫描'} · {timeText(scannedAt)}</span> : null}<button className="refresh-button" disabled={blocked} onClick={() => void preview()}><RefreshCw size={14} />{scanning ? '正在扫描…' : plan ? '重新扫描' : '扫描可清理范围'}</button></div>
    </div>
    {previewTask && (!previewTask.final || previewTask.status !== 'succeeded') && <ProjectTask task={previewTask} />}
    {!plan && !scanning && !previewTask && <div className="workspace-cleanup-intro"><ShieldCheck size={19} /><p>扫描只读取目录，不会删除文件。没有默认勾选；永久清理无法撤销。</p></div>}
    {temporaryProjects.length > 0 && <section className="history-group" aria-label="可重新生成文件">
      <div className="history-group-heading"><h3>临时文件</h3><button type="button" className="text-button" disabled={planBlocked} onClick={() => { setSelectedProjects(temporaryProjects.filter(project => !props.isProjectBusy?.(project)).map(project => project.id)); clearConfirmation(); }}>全选临时文件</button></div>
      {temporaryProjects.map(project => <article className="workspace-cleanup-item" key={project.id}>
        <input type="checkbox" aria-label={`选择「${project.name}」的临时文件`} checked={selectedProjects.includes(project.id)} disabled={planBlocked || Boolean(props.isProjectBusy?.(project))} onChange={event => { setSelectedProjects(current => event.target.checked ? [...current.filter(id => id !== project.id), project.id] : current.filter(id => id !== project.id)); clearConfirmation(); }} />
        <div className="workspace-cleanup-item-main"><strong>{project.name}</strong><p>来源：开发依赖、构建缓存与可清理的任务日志</p><code className="workspace-cleanup-path">{project.projectRoot}</code><p>开发依赖 {bigText(bigBytes(partBytes(project.storage?.currentProject.categories?.dependencies)))} · 构建缓存与日志 {bigText(bigBytes(partBytes(project.storage?.currentProject.categories?.cache)))}</p>{props.isProjectBusy?.(project) && <p>当前有任务或恢复状态需要处理，暂不可清理。</p>}</div>
        <strong className="workspace-cleanup-size">约 {bigText(projectRegenerable(project))}</strong>
      </article>)}
      <p>以上为最近统计的估计值；受保护的内容会保留。源码、数据库、用户文件与当前应用不在临时文件清理范围。</p>
    </section>}
    {plan && <>
      <div className="workspace-cleanup-plan"><p>以下大小为普通文件的逻辑字节，不等于实际可释放的物理磁盘空间。范围在 {timeText(plan.expiresAt)} 前有效，过期后需重新扫描。</p></div>
      {(expired || wrongWorkspace || consumed) && <p className="workspace-operation-status" role="status">{wrongWorkspace ? '工作区已改变，请重新扫描。' : consumed ? '本次范围已经提交过，请等待最终结果后重新扫描核对，不能直接重复提交。' : '本次范围已过期，请重新扫描后再选择和确认。'}</p>}
      {plan.items.length ? <div className="workspace-cleanup-items" aria-label="历史清理候选范围">{groups.map(({ kind, description }) => {
        const items = plan.items.filter((item) => item.kind === kind);
        if (!items.length) return null;
        const open = items.filter(cleanable);
        const allSelected = open.length > 0 && open.every((item) => selected.includes(item.id));
        return <section className="history-group" key={kind} aria-labelledby={`${id}-${kind}`}>
          <div className="history-group-heading"><div><h3 id={`${id}-${kind}`}>{kinds[kind]}</h3><span>{items.length} 项 · {totalText(items)}</span><p>{description}</p></div>{open.length > 0 && <button className="text-button" disabled={planBlocked} onClick={() => toggleGroup(open, !allSelected)}>{allSelected ? '取消本组' : '全选本组'}</button>}</div>
          {items.map((item) => <article className={`workspace-cleanup-item${cleanable(item) ? '' : ' is-blocked'}`} key={item.id}>
            <input type="checkbox" aria-label={`选择${itemDescription(item)}`} checked={selected.includes(item.id)} disabled={planBlocked || !cleanable(item)} onChange={(event) => toggle(item, event.target.checked)} />
            <div className="workspace-cleanup-item-main"><strong>{item.name || item.label || kinds[item.kind]}</strong>{!cleanable(item) && <span className="history-chip">暂不可清理</span>}
              <div className="workspace-cleanup-item-meta">{item.projectId && <span>项目 ID：<code>{item.projectId}</code></span>}<span>{countText(item.fileCount)}</span></div>
              {(item.name || item.label) !== item.path && <code className="workspace-cleanup-path" title={item.path}>{item.path}</code>}
              {!cleanable(item) && <div className="workspace-cleanup-blocked">{item.blockedReasons.length ? item.blockedReasons.map((reason, index) => <p key={index}>{reasonText(reason)}</p>) : <p>此条目未获得清理许可，请检查关联任务与恢复状态。</p>}</div>}
            </div>
            <strong className="workspace-cleanup-size" title={exactBytes(item.totalBytes)}>{bytesText(item.totalBytes)}</strong>
            <button className="text-button" aria-label={`复制${itemDescription(item)}的目录`} onClick={() => void copyPath(item.path)}><Copy size={13} />复制路径</button>
          </article>)}
        </section>;
      })}</div> : <div className="workspace-empty-state"><Trash2 size={26} /><p>没有发现历史资料</p><span>无需为了让空间数字归零而删除平台程序或当前项目。</span></div>}
      <details className="workspace-disclosure workspace-cleanup-preserved"><summary>始终保留的内容</summary><div className="workspace-disclosure-body">{plan.preserved.length ? <ul>{plan.preserved.map((value, index) => <li key={index}>{value}</li>)}</ul> : <p>范围之外的文件保持原样。</p>}</div></details>
      <div className="history-selection-bar" role="region" aria-label="已选历史资料">
        <strong>已选 {selectedCount} 项 · 共 {selectedTotalText}</strong><span>清理后无法恢复</span>
        <button className="refresh-button" disabled={!selectedCount || pending === 'cleanup'} onClick={() => { setSelected([]); setSelectedProjects([]); clearConfirmation(); }}>取消选择</button>
        <button ref={reviewButton} className="danger-button" disabled={planBlocked || !selectedCount || !selectedItems.every(cleanable) || selectedTargets.some(project => props.isProjectBusy?.(project))} onClick={() => { setAcknowledged(false); setReviewed(fingerprint); }}>清理所选…</button>
      </div>
    </>}
    </>;
  const confirmation = confirmationCurrent && plan && <>
      <ul className="history-confirm-list">{selectedItems.map((item) => <li key={item.id}><span><strong>{item.name || item.label}</strong><small>来源：{kinds[item.kind]}{item.projectId ? ` · 项目 ${item.projectId}` : ''}</small><code>{item.path}</code></span><span>{bytesText(item.totalBytes)}</span></li>)}</ul>
      {selectedTargets.map(project => <div className="workspace-cleanup-item" key={project.id}><div><strong>{project.name}</strong><p>来源：开发依赖、构建缓存与可清理的任务日志</p><code>{project.projectRoot}</code></div><strong>约 {bigText(projectRegenerable(project))}</strong></div>)}
      <p className="history-confirm-total">合计 {selectedTotalText}</p>
      <p>所选目录会直接永久删除，其中全部文件都无法恢复。不会删除当前项目和未勾选的历史资料。请先停止外部 AI、编辑器或工具访问这些目录。</p>
      <label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={planBlocked} onChange={(event) => setAcknowledged(event.target.checked)} />我理解这些内容将永久删除，无法恢复</label>
      <details className="workspace-disclosure"><summary>技术详情</summary><div className="workspace-disclosure-body"><p>当前工作区：<code>{plan.workspaceRoot}</code></p><ul>{selectedItems.map((item) => <li key={item.id}><code>{item.path}</code></li>)}</ul><p>计划 {plan.planId} · 有效至 {timeText(plan.expiresAt)}。提交时会再次核对每一项，范围变化时不会删除。</p>{plan.preserved.length > 0 && <><p>始终保留：</p><ul>{plan.preserved.map((value, index) => <li key={index}>{value}</li>)}</ul></>}</div></details>
      <div className="confirmation-actions"><button className="refresh-button" disabled={pending === 'cleanup'} onClick={clearConfirmation}>{props.compact ? '返回选择' : '取消，保留'}</button><button className="danger-button" disabled={planBlocked || !acknowledged} onClick={() => void submit()}>{pending === 'cleanup' ? '正在提交…' : `永久清理 ${selectedTotalText}`}</button></div>
    </>;
  const results = <>
    {cleanupTask && <div className="workspace-cleanup-result"><h3 ref={resultHeading} tabIndex={-1}>清理结果</h3><ProjectTask task={cleanupTask} /><CleanupResult task={cleanupTask} /></div>}
    {projectResults.map(task => <div className="workspace-cleanup-result" key={task.taskId}><ProjectTask task={task} /></div>)}
    {(pending === 'cleanup' || cleanupRunning) && <p className="workspace-operation-status" role="status">清理任务会持续执行，切换页面不会取消。完成前，删除、迁移和其他清理操作暂时不可用。</p>}
    {(error || pollError) && <p className="task-error" role="alert">{error || pollError}</p>}{notice && <p className="workspace-inline-notice" role="status">{notice}</p>}
  </>;
  if (!props.compact) return <section className="lifecycle-card workspace-cleanup-card" aria-labelledby={`${id}-title`}>{body}{confirmation && <Modal title={`永久清理 ${selectedCount} 项历史资料？`} className="history-cleanup-dialog" closeDisabled={pending === 'cleanup'} onClose={clearConfirmation}>{confirmation}</Modal>}{results}</section>;
  return <section className="lifecycle-card workspace-cleanup-card" aria-labelledby={`${id}-card-title`}>
    <div className="section-heading"><div><h2 id={`${id}-card-title`}>临时文件和旧资料</h2><p>清理可重新生成的文件和已结束取回操作保留的原件。先核对明细，再选择并确认。</p></div><button type="button" className="refresh-button" disabled={blocked} onClick={() => { setReviewOpen(true); if (!plan || expired || consumed || wrongWorkspace) void preview(); }}>清理</button></div>
    {reviewOpen && <Modal title={confirmation ? `永久清理 ${selectedCount} 项内容？` : '核对临时文件和旧资料'} className="history-cleanup-dialog" closeDisabled={pending === 'cleanup'} onClose={() => { setReviewOpen(false); clearConfirmation(); }}>
      {confirmation || body}{results}
    </Modal>}
    {!reviewOpen && results}
  </section>;
}

function CleanupResult({ task }: { task: TaskRecord }) {
  const result = task.result ?? (task.error?.details?.partialResult as Record<string, unknown> | undefined);
  if (!result) return null;
  const itemResults = Array.isArray(result.items) ? result.items : [];
  const groups: [string, unknown][] = [['实际已删除', result.deleted], ['未完成 / 失败', result.failures], ['仍保留 / 未处理', result.remaining], ['已跳过', result.skipped]];
  return <div className="workspace-cleanup-result-details">
    {itemResults.length > 0 && <div className="workspace-cleanup-item-results" aria-label="逐项清理结果">{itemResults.map((entry, index) => <CleanupItemResult key={objectValue(entry)?.id as string ?? index} value={entry} />)}</div>}
    {task.final && itemResults.length === 0 && result.completed !== true && <p>{objectValue(result.error)?.code === 'CLEANUP_PLAN_CHANGED' ? '执行前的范围校验未通过，尚未开始删除。请重新扫描后核对。' : '本任务缺少完整的逐项删除汇总，实际删除数量尚未确认。请查看持久回执与日志核对，不能把未知数量视为零。'}</p>}
    {groups.map(([label, value]) => Array.isArray(value) && value.length > 0 ? <section key={label}><h4>{label}</h4><ul>{value.map((entry, index) => { const item = objectValue(entry); return <li key={index}>{item ? <><code>{String(item.path ?? item.label ?? item.id ?? '记录')}</code>{(item.logicalBytes ?? item.totalBytes) != null && <span>{bytesText((item.logicalBytes ?? item.totalBytes) as ByteCount)}</span>}<p>{messageFrom(item.message ?? item.reason ?? item.error ?? item.status ?? '')}</p></> : <span>{String(entry)}</span>}</li>; })}</ul></section> : null)}
    <details className="workspace-disclosure"><summary>查看完整结构化结果</summary><pre tabIndex={0}>{JSON.stringify(result, null, 2)}</pre></details>
    {task.final && (task.status !== 'succeeded' || result.completed === false) && <p>任务未完整成功。记录中已删除的内容无法恢复；修复失败原因后，重新扫描并核对剩余范围，不自动重试。</p>}
  </div>;
}
function objectValue(value: unknown): Record<string, unknown> | null { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null; }
function integerText(value: unknown) {
  try { if (value == null) return null; const count = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(String(objectValue(value)?.$int64)); return count >= 0n ? count.toString() : null; } catch { return null; }
}
function CleanupItemResult({ value }: { value: unknown }) {
  const item = objectValue(value);
  if (!item) return <p>{String(value)}</p>;
  const failure = objectValue(item.error);
  const removed = objectValue(item.removed) ?? objectValue(objectValue(failure?.details)?.cleanupResult);
  const completed = item.completed === true && item.status === 'removed';
  const skipped = item.status === 'skipped';
  const status = completed ? '已清理' : skipped ? '未处理' : '清理未完成';
  const fileCount = integerText(removed?.removedFileCount);
  const entryCount = integerText(removed?.removedEntryCount);
  const lowerBounds = removed?.countsAreLowerBounds === true || removed?.accountingComplete === false;
  const remainingPath = typeof removed?.remainingPath === 'string' ? removed.remainingPath : !completed && typeof item.path === 'string' ? item.path : null;
  return <article className={`workspace-cleanup-item-result${completed ? ' is-complete' : ' needs-attention'}`}>
    <div className="workspace-cleanup-item-result-heading"><strong>{status}</strong><span>{kinds[String(item.kind) as CleanupKind] ?? '历史副本'}</span></div>
    <code className="workspace-cleanup-path">{String(item.path ?? item.id ?? '目录未记录')}</code>
    {removed && <div className="workspace-cleanup-removed-counts"><span>{lowerBounds ? '已确认至少删除' : '实际删除'}：<strong title={exactBytes(removed.removedLogicalBytes as ByteCount | undefined)}>{bytesText(removed.removedLogicalBytes as ByteCount | undefined)}</strong></span>{fileCount !== null && <span>文件或链接：{fileCount} 个</span>}{entryCount !== null && <span>文件、链接及目录条目：{entryCount} 项</span>}</div>}
    {lowerBounds && <p>统计尚未完整，显示的数量是已确认删除的下限，实际删除量可能更多。请结合回执核对残留内容。</p>}
    {!completed && !removed && <p>{skipped ? '此项没有继续处理，请先解决前一项的问题。' : '此项没有完整的删除数量记录，请查看错误与回执，不能视为全部保留或全部清理。'}</p>}
    {failure && <p className="task-error">{messageFrom(failure)}</p>}
    {typeof item.reason === 'string' && <p>{item.reason}</p>}
    {remainingPath && <div className="workspace-cleanup-remaining"><span>{skipped ? '未处理目录' : '待核对的残留目录'}</span><code>{remainingPath}</code></div>}
    {typeof item.progressPath === 'string' && <details className="workspace-disclosure"><summary>查看逐文件清理记录位置</summary><code className="workspace-cleanup-path">{item.progressPath}</code></details>}
  </article>;
}
