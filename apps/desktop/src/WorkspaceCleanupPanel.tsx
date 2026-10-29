import { invoke } from '@tauri-apps/api/core';
import { Copy, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import { markHistoryScanStale, publishHistoryScan } from './history-scan-store';
import { bytesText, exactBytes, timeText } from './storage-utils';
import type { ByteCount, RetainedWorkspaceRoot, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type CleanupKind = 'retainedWorkspace' | 'restoreOriginal';
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
  revision: number;
  enabled: boolean;
  active: boolean;
  migrating: boolean;
  // Scan once when the panel first becomes active without a usable result.
  autoScan?: boolean;
  absentRecords?: RetainedWorkspaceRoot[];
  cleanupTask?: TaskRecord | null;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onPendingChange?: (pending: boolean) => void;
}
const kinds: Record<CleanupKind, string> = { retainedWorkspace: '迁移前的旧工作区', restoreOriginal: '取回前的原件' };
const groups: { kind: CleanupKind; description: string }[] = [
  { kind: 'retainedWorkspace', description: '迁移工作区时保留的原目录，软件已不再使用。清理会删除该目录中的全部文件，当前工作区不受影响。' },
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
          setPlan(value); setSelected([]); setConsumed(false); setNow(Date.now()); setScannedAt(Date.now()); clearConfirmation();
          publishHistoryScan(value.workspaceRoot, value.items);
        } catch (cause) { setError(messageFrom(cause)); }
      }
    } else {
      clearConfirmation(); markHistoryScanStale();
      if (runningCleanupTasks.current.delete(task.taskId)) setResultFocus({ taskId: task.taskId });
      void callbacks.current.onRefresh().catch((cause) => { if (alive.current) setError(`清理任务已结束，但重新读取空间失败：${messageFrom(cause)}`); });
    }
  }, [clearConfirmation]);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (props.cleanupTask) track(props.cleanupTask, false); }, [props.cleanupTask, track]);
  useEffect(() => { clearConfirmation(); }, [props.active, props.migrating, props.revision, props.workspaceRoot, clearConfirmation]);
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
      try { const task = await invoke<TaskRecord>('task_get', { taskId }); if (cancelled) return; track(task); if (!task.final) schedule(taskId, 1000); }
      catch (cause) { if (!cancelled) { setPollError(`任务状态暂未读取，操作仍可能继续：${messageFrom(cause)}`); schedule(taskId, 2000); } }
    }
    for (const task of tasks) schedule(task.taskId, 500);
    return () => { cancelled = true; for (const timer of timers) clearTimeout(timer); };
  }, [previewTask?.taskId, previewTask?.final, cleanupTask?.taskId, cleanupTask?.final, track]);
  const cleanupRunning = Boolean(cleanupTask && !cleanupTask.final) || Boolean(props.cleanupTask && !props.cleanupTask.final);
  const previewRunning = Boolean(previewTask && !previewTask.final);
  const blocked = !props.enabled || !props.active || props.migrating || Boolean(pending) || cleanupRunning || previewRunning;
  const expired = Boolean(plan && now >= plan.expiresAt);
  const wrongWorkspace = Boolean(plan && plan.workspaceRoot !== props.workspaceRoot);
  const planBlocked = blocked || !plan || expired || wrongWorkspace || consumed;
  const selectedItems = plan?.items.filter((item) => selected.includes(item.id)) ?? [];
  const fingerprint = JSON.stringify([plan, [...selected].sort(), props.workspaceRoot, props.revision]);
  const confirmationCurrent = reviewed === fingerprint && selectedItems.length > 0 && selectedItems.every(cleanable);
  useEffect(() => {
    if (!resultFocus || handledResultFocus.current === resultFocus) return;
    // Consume hidden-tab requests too, so returning to a tab never steals focus.
    handledResultFocus.current = resultFocus;
    if (props.active && resultFocus.taskId === cleanupTask?.taskId) focusAndReveal(resultHeading.current);
  }, [resultFocus, props.active, cleanupTask?.taskId]);
  const preview = useCallback(async () => {
    if (blocked || operation.current) return;
    operation.current = true; setPending('preview'); setError(''); setPollError(''); setNotice(''); setPlan(null); setSelected([]); clearConfirmation();
    try { track(await invoke<TaskRecord>('workspace_cleanup_preview')); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }, [blocked, clearConfirmation, track]);
  useEffect(() => {
    if (!props.autoScan || autoScanned.current || blocked || plan) return;
    autoScanned.current = true;
    void preview();
  }, [props.autoScan, blocked, plan, preview]);
  async function submit() {
    if (planBlocked || operation.current || !plan || Date.now() >= plan.expiresAt || !confirmationCurrent || !acknowledged) return;
    operation.current = true; setPending('cleanup'); setConsumed(true); setError(''); setPollError(''); setNotice(''); callbacks.current.onPendingChange?.(true);
    // The service requires its fixed phrase; the explicit checkbox is the user's confirmation.
    const input = { planId: plan.planId, itemIds: selectedItems.map((item) => item.id), confirmation: plan.confirmationText };
    clearConfirmation();
    try {
      const task = await invoke<TaskRecord>('workspace_cleanup', { input });
      track(task);
      if (alive.current && task.type === 'workspaceCleanup') setResultFocus({ taskId: task.taskId });
    }
    catch (cause) { if (alive.current) setError(`${messageFrom(cause)}。写入结果可能需要核对；请重新扫描，不会自动重复提交。`); }
    finally { operation.current = false; callbacks.current.onPendingChange?.(false); if (alive.current) setPending(null); }
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
  const absentRecords = (props.absentRecords ?? []).filter((record) => record.absent === true);
  const scanning = pending === 'preview' || previewRunning;
  return <section className="lifecycle-card workspace-cleanup-card" aria-labelledby={`${id}-title`}>
    <div className="history-heading">
      <div><h2 id={`${id}-title`}>历史资料</h2><p>这些资料不会随删除项目自动清理。勾选不再需要的内容，确认后永久删除。当前项目和未完成的恢复不在这里，不会被删除。</p></div>
      <div className="history-scan-status">{scanning ? <span role="status">正在扫描…</span> : scannedAt ? <span>{props.autoScan ? '已自动扫描' : '已扫描'} · {timeText(scannedAt)}</span> : null}<button className="refresh-button" disabled={blocked} onClick={() => void preview()}><RefreshCw size={14} />{scanning ? '正在扫描…' : plan ? '重新扫描' : '扫描可清理范围'}</button></div>
    </div>
    {previewTask && (!previewTask.final || previewTask.status !== 'succeeded') && <ProjectTask task={previewTask} />}
    {!plan && !scanning && !previewTask && <div className="workspace-cleanup-intro"><ShieldCheck size={19} /><p>扫描只读取目录，不会删除文件。没有默认勾选；永久清理无法撤销。</p></div>}
    {plan && <>
      <div className="workspace-cleanup-plan"><p>以下大小为普通文件的逻辑字节，不等于实际可释放的物理磁盘空间。范围在 {timeText(plan.expiresAt)} 前有效，过期后需重新扫描。</p></div>
      {(expired || wrongWorkspace || consumed) && <p className="workspace-operation-status" role="status">{wrongWorkspace ? '工作区已改变，请重新扫描。' : consumed ? '本次范围已经提交过，请等待最终结果后重新扫描核对，不能直接重复提交。' : '本次范围已过期，请重新扫描后再选择和确认。'}</p>}
      {plan.items.length || absentRecords.length ? <div className="workspace-cleanup-items" aria-label="历史清理候选范围">{groups.map(({ kind, description }) => {
        const items = plan.items.filter((item) => item.kind === kind);
        const records = kind === 'retainedWorkspace' ? absentRecords : [];
        if (!items.length && !records.length) return null;
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
          {records.map((record, index) => <article className="workspace-cleanup-item is-absent" key={record.taskId ?? `absent-${index}`}><span aria-hidden="true">—</span><div className="workspace-cleanup-item-main"><strong>{record.path ?? record.sourcePath ?? '原目录路径未记录'}</strong><div className="workspace-cleanup-item-meta"><span>目录当前不存在</span></div></div><span className="workspace-cleanup-size">—</span></article>)}
        </section>;
      })}</div> : <div className="workspace-empty-state"><Trash2 size={26} /><p>没有发现历史资料</p><span>无需为了让空间数字归零而删除平台程序或当前项目。</span></div>}
      <details className="workspace-disclosure workspace-cleanup-preserved"><summary>始终保留的内容</summary><div className="workspace-disclosure-body">{plan.preserved.length ? <ul>{plan.preserved.map((value, index) => <li key={index}>{value}</li>)}</ul> : <p>范围之外的文件保持原样。</p>}</div></details>
      <div className="history-selection-bar" role="region" aria-label="已选历史资料">
        <strong>已选 {selectedItems.length} 项 · 共 {totalText(selectedItems)}</strong><span>清理后无法恢复</span>
        <button className="refresh-button" disabled={!selectedItems.length || pending === 'cleanup'} onClick={() => { setSelected([]); clearConfirmation(); }}>取消选择</button>
        <button ref={reviewButton} className="danger-button" disabled={planBlocked || !selectedItems.length || !selectedItems.every(cleanable)} onClick={() => { setAcknowledged(false); setReviewed(fingerprint); }}>清理所选…</button>
      </div>
    </>}
    {confirmationCurrent && plan && <Modal title={`永久清理 ${selectedItems.length} 项历史资料？`} className="history-cleanup-dialog" closeDisabled={pending === 'cleanup'} onClose={clearConfirmation}>
      <ul className="history-confirm-list">{selectedItems.map((item) => <li key={item.id}><span><strong>{item.name || item.label}</strong><small>{kinds[item.kind]}{item.projectId ? ` · 项目 ${item.projectId}` : ''}</small></span><span>{bytesText(item.totalBytes)}</span></li>)}</ul>
      <p className="history-confirm-total">合计 {totalText(selectedItems)}</p>
      <p>所选目录会直接永久删除，其中全部文件都无法恢复。不会删除当前项目和未勾选的历史资料。请先停止外部 AI、编辑器或工具访问这些目录。</p>
      <label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={planBlocked} onChange={(event) => setAcknowledged(event.target.checked)} />我理解这些内容将永久删除，无法恢复</label>
      <details className="workspace-disclosure"><summary>技术详情</summary><div className="workspace-disclosure-body"><p>当前工作区：<code>{plan.workspaceRoot}</code></p><ul>{selectedItems.map((item) => <li key={item.id}><code>{item.path}</code></li>)}</ul><p>计划 {plan.planId} · 有效至 {timeText(plan.expiresAt)}。提交时会再次核对每一项，范围变化时不会删除。</p>{plan.preserved.length > 0 && <><p>始终保留：</p><ul>{plan.preserved.map((value, index) => <li key={index}>{value}</li>)}</ul></>}</div></details>
      <div className="confirmation-actions"><button className="refresh-button" disabled={pending === 'cleanup'} onClick={clearConfirmation}>取消，保留</button><button className="danger-button" disabled={planBlocked || !acknowledged} onClick={() => void submit()}>{pending === 'cleanup' ? '正在提交…' : `永久清理 ${totalText(selectedItems)}`}</button></div>
    </Modal>}
    {cleanupTask && <div className="workspace-cleanup-result"><h3 ref={resultHeading} tabIndex={-1}>清理结果</h3><ProjectTask task={cleanupTask} /><CleanupResult task={cleanupTask} /></div>}
    {(pending === 'cleanup' || cleanupRunning) && <p className="workspace-operation-status" role="status">清理任务会持续执行，切换页面不会取消。完成前，删除、迁移和其他清理操作暂时不可用。</p>}
    {(error || pollError) && <p className="task-error" role="alert">{error || pollError}</p>}{notice && <p className="workspace-inline-notice" role="status">{notice}</p>}
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
