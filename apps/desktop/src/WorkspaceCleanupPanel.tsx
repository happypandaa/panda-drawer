import { invoke } from '@tauri-apps/api/core';
import { ArrowRight, Copy, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import { bytesText, exactBytes, timeText } from './storage-utils';
import type { ByteCount, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type CleanupKind = 'retainedWorkspace' | 'recoveryOriginal' | 'trash';
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
  cleanupTask?: TaskRecord | null;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onPendingChange?: (pending: boolean) => void;
}
const kinds: Record<CleanupKind, string> = { retainedWorkspace: '迁移保留目录', recoveryOriginal: '已完成恢复的原现场', trash: '旧版保留项目' };
const confirmationPhrase = '永久清理所选内容';
function readPlan(result: Record<string, unknown> | null): CleanupPlan {
  if (!result || typeof result.planId !== 'string' || !result.planId || typeof result.expiresAt !== 'number' || !Number.isFinite(result.expiresAt) || typeof result.workspaceRoot !== 'string' || !Array.isArray(result.items) || !Array.isArray(result.preserved) || result.confirmationText !== confirmationPhrase) throw new Error('清理范围返回不完整，请重新扫描；未提交任何清理操作。');
  const ids = new Set<string>();
  for (const value of result.items) {
    if (!value || typeof value !== 'object') throw new Error('清理范围中的条目无效，请重新扫描。');
    const item = value as Record<string, unknown>;
    if (typeof item.id !== 'string' || !item.id || ids.has(item.id) || !['retainedWorkspace', 'recoveryOriginal', 'trash'].includes(String(item.kind)) || typeof item.label !== 'string' || typeof item.path !== 'string' || !item.path.startsWith('/') || typeof item.canClean !== 'boolean' || !Array.isArray(item.blockedReasons) || !item.blockedReasons.every((reason) => typeof reason === 'string')) throw new Error('清理范围中的条目不完整，请重新扫描。');
    ids.add(item.id);
  }
  if (!result.preserved.every((value) => typeof value === 'string')) throw new Error('清理范围缺少明确的保留内容，请重新扫描。');
  return result as unknown as CleanupPlan;
}
function countText(value: ByteCount | undefined) {
  if (value == null) return '文件数尚未统计';
  try { const count = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return count < 0n ? '文件数未知' : `${count} 个文件`; } catch { return '文件数未知'; }
}
function itemDescription(item: CleanupItem) {
  return [kinds[item.kind], item.name || item.label, item.projectId ? `项目 ${item.projectId}` : null, item.path].filter(Boolean).join('，');
}
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
  const [typedConfirmation, setTypedConfirmation] = useState('');
  const [acknowledged, setAcknowledged] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [notice, setNotice] = useState('');
  const [focusRequest, setFocusRequest] = useState<{ target: 'review' | 'result'; taskId?: string } | null>(null);
  const alive = useRef(true);
  const operation = useRef(false);
  const finalTasks = useRef(new Set<string>());
  const runningCleanupTasks = useRef(new Set<string>());
  const reviewButton = useRef<HTMLButtonElement>(null);
  const reviewHeading = useRef<HTMLHeadingElement>(null);
  const resultHeading = useRef<HTMLHeadingElement>(null);
  const handledFocusRequest = useRef(focusRequest);
  const callbacks = useRef(props); callbacks.current = props;
  const id = useId();
  const clearConfirmation = useCallback(() => { setReviewed(null); setTypedConfirmation(''); setAcknowledged(false); }, []);
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
        try { setPlan(readPlan(task.result)); setSelected([]); setConsumed(false); setNow(Date.now()); clearConfirmation(); }
        catch (cause) { setError(messageFrom(cause)); }
      }
    } else {
      clearConfirmation();
      if (runningCleanupTasks.current.delete(task.taskId)) setFocusRequest({ target: 'result', taskId: task.taskId });
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
  const confirmationCurrent = reviewed === fingerprint && selectedItems.length > 0 && selectedItems.every((item) => item.canClean && !item.blockedReasons.length);
  useEffect(() => {
    if (!focusRequest || handledFocusRequest.current === focusRequest) return;
    // Consume hidden-tab requests too, so returning to a tab never steals focus.
    handledFocusRequest.current = focusRequest;
    if (!props.active) return;
    if (focusRequest.target === 'review' && confirmationCurrent) focusAndReveal(reviewHeading.current);
    else if (focusRequest.target === 'result' && focusRequest.taskId === cleanupTask?.taskId) focusAndReveal(resultHeading.current);
  }, [focusRequest, props.active, confirmationCurrent, cleanupTask?.taskId]);
  function beginReview() { clearConfirmation(); setReviewed(fingerprint); setFocusRequest({ target: 'review' }); }
  function cancelReview() { clearConfirmation(); focusAndReveal(reviewButton.current); }
  async function preview() {
    if (blocked || operation.current) return;
    operation.current = true; setPending('preview'); setError(''); setPollError(''); setNotice(''); setPlan(null); setSelected([]); clearConfirmation();
    try { track(await invoke<TaskRecord>('workspace_cleanup_preview')); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }
  async function submit() {
    if (planBlocked || operation.current || !plan || Date.now() >= plan.expiresAt || !confirmationCurrent || typedConfirmation !== plan.confirmationText || !acknowledged) return;
    operation.current = true; setPending('cleanup'); setConsumed(true); setError(''); setPollError(''); setNotice(''); callbacks.current.onPendingChange?.(true);
    const input = { planId: plan.planId, itemIds: selectedItems.map((item) => item.id), confirmation: typedConfirmation };
    clearConfirmation();
    try {
      const task = await invoke<TaskRecord>('workspace_cleanup', { input });
      track(task);
      if (alive.current && task.type === 'workspaceCleanup') setFocusRequest({ target: 'result', taskId: task.taskId });
    }
    catch (cause) { if (alive.current) setError(`${messageFrom(cause)}。写入结果可能需要核对；请重新扫描，不会自动重复提交。`); }
    finally { operation.current = false; callbacks.current.onPendingChange?.(false); if (alive.current) setPending(null); }
  }
  async function copyPath(value: string) { try { await navigator.clipboard.writeText(value); if (alive.current) setNotice('目录路径已复制。'); } catch { if (alive.current) setNotice('无法自动复制，请选中显示的完整目录路径手动复制。'); } }
  return <section className="lifecycle-card workspace-cleanup-card" aria-labelledby={`${id}-title`}>
    <div className="section-heading"><div><h2 id={`${id}-title`}>清理历史</h2><p>先扫描并查看目录，再选择明确不再需要的历史副本。扫描本身不会删除文件。</p></div><button className="refresh-button" disabled={blocked} onClick={() => void preview()}><RefreshCw size={14} />{pending === 'preview' || previewRunning ? '正在扫描…' : plan ? '重新扫描范围' : '扫描可清理范围'}</button></div>
    <div className="workspace-cleanup-intro"><ShieldCheck size={19} /><p>当前工作区的活跃项目、全部历史备份和未解决的恢复现场会保留。没有默认勾选；永久清理无法撤销。可清理状态以扫描结果和提交时的再次核对为准。</p></div>
    {previewTask && (!previewTask.final || previewTask.status !== 'succeeded') && <ProjectTask task={previewTask} />}
    {plan && <>
      <div className="workspace-cleanup-plan"><span>本次扫描工作区</span><code>{plan.workspaceRoot}</code><p>确认有效至 {timeText(plan.expiresAt)} · 计划 {plan.planId}</p><p>以下大小为普通文件的逻辑字节，不等于实际可释放的物理磁盘空间。</p>{plan.items.some((item) => item.kind === 'retainedWorkspace') && <p>迁移保留目录是迁移时点的完整副本，可能包含后来已删除的项目及当时的备份副本。选择清理会删除该目录中的全部文件；当前工作区的历史备份保留。</p>}</div>
      {(expired || wrongWorkspace || consumed) && <p className="workspace-operation-status" role="status">{wrongWorkspace ? '工作区已改变，请重新扫描。' : consumed ? '本次范围已经提交过，请等待最终结果后重新扫描核对，不能直接重复提交。' : '本次范围已过期，请重新扫描后再选择和确认。'}</p>}
      {plan.items.length ? <div className="workspace-cleanup-items" aria-label="历史清理候选范围">{plan.items.map((item) => <article className={`workspace-cleanup-item${item.canClean && !item.blockedReasons.length ? '' : ' is-blocked'}`} key={item.id}>
        <div className="workspace-cleanup-item-heading"><label className="checkbox-label"><input type="checkbox" aria-label={`选择${itemDescription(item)}`} checked={selected.includes(item.id)} disabled={planBlocked || !item.canClean || Boolean(item.blockedReasons.length)} onChange={(event) => { setSelected((current) => event.target.checked ? [...current.filter((value) => value !== item.id), item.id] : current.filter((value) => value !== item.id)); clearConfirmation(); }} /><span><strong>{item.label || item.name || kinds[item.kind]}</strong><small>{kinds[item.kind]}</small></span></label><strong title={exactBytes(item.totalBytes)}>{bytesText(item.totalBytes)}</strong></div>
        <code className="workspace-cleanup-path">{item.path}</code><div className="workspace-cleanup-item-meta"><span>{countText(item.fileCount)}</span>{item.projectId && <span>项目 ID：<code>{item.projectId}</code></span>}{item.name && <span>项目名称：{item.name}</span>}<button className="text-button" aria-label={`复制${itemDescription(item)}的目录`} onClick={() => void copyPath(item.path)}><Copy size={13} />复制目录</button></div>
        {(!item.canClean || item.blockedReasons.length > 0) && <div className="workspace-cleanup-blocked"><strong>暂不可清理</strong>{item.blockedReasons.length ? item.blockedReasons.map((reason, index) => <p key={index}>{reason}</p>) : <p>此条目未获得清理许可，请检查关联任务与恢复状态。</p>}</div>}
      </article>)}</div> : <div className="workspace-empty-state"><Trash2 size={26} /><p>本次没有发现可列出的历史清理范围</p><span>无需为了让空间数字归零而删除平台程序、备份或当前项目。</span></div>}
      <div className="workspace-cleanup-preserved"><h3>本次始终保留</h3>{plan.preserved.length ? <ul>{plan.preserved.map((value, index) => <li key={index}>{value}</li>)}</ul> : <p>扫描未列出可删除的其他位置。范围之外的文件保持原样。</p>}</div>
      <div className="workspace-cleanup-review-action"><p>已选择 {selectedItems.length} 项。每项均会删除显示目录中的完整历史副本。</p><button ref={reviewButton} className="danger-button" disabled={planBlocked || !selectedItems.length || selectedItems.some((item) => !item.canClean || item.blockedReasons.length)} onClick={beginReview}><ArrowRight size={14} />核对所选清理范围</button></div>
      {confirmationCurrent && <div className="operation-confirmation workspace-cleanup-confirmation" aria-label="历史清理范围确认"><h3 ref={reviewHeading} tabIndex={-1}>永久清理以下 {selectedItems.length} 项？</h3><p>所选目录会直接永久删除。其中全部文件，包括源码、数据库、附件、网页、说明和历史副本，都将无法恢复。</p><ul>{selectedItems.map((item) => <li key={item.id}><strong>{item.label}</strong><code>{item.path}</code><span>{bytesText(item.totalBytes)}{item.projectId ? ` · 项目 ${item.projectId}` : ''}</span></li>)}</ul><p>当前工作区：<code>{plan.workspaceRoot}</code>。上方“本次始终保留”范围不会删除。</p><label htmlFor={`${id}-confirmation`}>输入“{plan.confirmationText}”确认<input id={`${id}-confirmation`} value={typedConfirmation} disabled={planBlocked} autoComplete="off" onChange={(event) => setTypedConfirmation(event.target.value)} /></label><label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={planBlocked} onChange={(event) => setAcknowledged(event.target.checked)} />我已核对全部目录及保留内容，并已停止外部 AI、编辑器和工具使用或写入这些历史目录</label><div className="confirmation-actions"><button className="danger-button" disabled={planBlocked || typedConfirmation !== plan.confirmationText || !acknowledged} onClick={() => void submit()}>{pending === 'cleanup' ? '正在提交…' : '确认永久清理所选内容'}</button><button className="refresh-button" disabled={pending === 'cleanup' || cleanupRunning} onClick={cancelReview}>取消，继续保留</button></div></div>}
    </>}
    {cleanupTask && <div className="workspace-cleanup-result"><h3 ref={resultHeading} tabIndex={-1}>清理任务与实际结果</h3><ProjectTask task={cleanupTask} /><CleanupResult task={cleanupTask} /></div>}
    {(pending === 'cleanup' || cleanupRunning) && <p className="workspace-operation-status" role="status">清理任务会持续执行，切换页签不会取消。请等待最终结果；迁移和历史保留项目操作暂时停用。</p>}
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
    {result.historicalBackupsRetained === true && <p>当前工作区的历史备份已保留。</p>}
    {typeof result.receiptPath === 'string' && <div className="workspace-cleanup-receipt"><strong>本次清理回执</strong><code>{result.receiptPath}</code><p>回执用于核对实际结果，会保留在固定开发数据目录中。</p></div>}
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
