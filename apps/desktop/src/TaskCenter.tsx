import { ListChecks } from 'lucide-react';
import { useEffect, useId, useRef } from 'react';
import { ProjectTask, taskLabel } from './ProjectTask';
import type { TaskRecord } from './types';

const statusText: Record<TaskRecord['status'], string> = { queued: '排队中', running: '进行中', succeeded: '完成', failed: '未完成', interrupted: '已中断' };
export function taskNeedsAttention(task: TaskRecord) {
  if (!task.final) return false;
  if (task.status === 'failed' || task.status === 'interrupted') return true;
  const result = task.result ?? task.error?.details;
  const nestedPending = (value: unknown) => Boolean(value && typeof value === 'object' && 'cleanupPending' in value && value.cleanupPending === true);
  return result?.cleanupPending === true || nestedPending(result?.artifactCleanup) || nestedPending(result?.sourceCleanup)
    || ['unknown', 'needs_attention', 'committed_needs_attention'].includes(String(result?.operationOutcome));
}

export interface TaskCenterProps {
  tasks: TaskRecord[];
  projectName: (projectId: string | null) => string | null;
  open: boolean;
  disabled?: boolean;
  onOpenChange: (open: boolean) => void;
  importFlow?: { taskId?: string; active?: boolean; needsAttention: boolean; label?: string; onOpen: () => void };
  recoveryFlow?: { needsAttention: boolean; onOpen: () => void };
  onViewTask?: (task: TaskRecord) => void;
  canViewTask?: (task: TaskRecord) => boolean;
}

// Completed history stays available in an already-open popover, without
// leaving a permanent trigger in the header once work has finished.
export function TaskCenter({ tasks, projectName, open, disabled, onOpenChange, importFlow, recoveryFlow, onViewTask, canViewTask }: TaskCenterProps) {
  const id = useId();
  const container = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const unique = new Map<string, TaskRecord>();
  for (const task of tasks) if (!unique.get(task.taskId)?.final || task.final) unique.set(task.taskId, task);
  const values = [...unique.values()];
  const active = values.filter(task => !task.final);
  const attention = values.filter(task => taskNeedsAttention(task) || task.final && importFlow?.taskId === task.taskId && importFlow.needsAttention);
  const attentionIds = new Set(attention.map(task => task.taskId));
  const done = values.filter(task => task.final && !attentionIds.has(task.taskId)).slice(0, 10);
  const separateImport = Boolean(importFlow && (!importFlow.taskId || !unique.has(importFlow.taskId)));
  const activeCount = active.length + (separateImport && importFlow?.active ? 1 : 0);
  const attentionCount = attention.length + (separateImport && importFlow?.needsAttention && !importFlow.active ? 1 : 0) + (recoveryFlow?.needsAttention ? 1 : 0);
  const visible = activeCount + attentionCount > 0;
  const restoreFocus = () => {
    const target = button.current?.isConnected ? button.current : document.querySelector<HTMLButtonElement>('.radio-settings-button');
    target?.focus();
  };
  const close = () => { onOpenChange(false); restoreFocus(); };
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); close(); } };
    const onPointer = (event: MouseEvent) => { if (container.current && !container.current.contains(event.target as Node)) onOpenChange(false); };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onPointer);
    return () => { document.removeEventListener('keydown', onKey); document.removeEventListener('mousedown', onPointer); };
  }, [open, onOpenChange]);
  const wasVisible = useRef(visible);
  useEffect(() => {
    if (wasVisible.current && !visible && open && (container.current?.contains(document.activeElement) || document.activeElement === document.body)) {
      container.current?.querySelector<HTMLButtonElement>('[aria-label="关闭任务"]')?.focus();
    }
    wasVisible.current = visible;
  }, [visible, open]);
  if (!visible && !open) return null;
  const downloading = active.length > 0 && active.every(task => task.type === 'nodeGitImport' && ['queued', 'downloading'].includes(task.phase));
  const sameType = active.length > 0 && active.every(task => task.type === active[0].type);
  const activity = downloading ? `正在下载 ${activeCount} 个应用`
    : sameType && active[0].type === 'nodeGitImport' ? `正在添加 ${activeCount} 个应用`
      : sameType ? `正在${taskLabel(active[0].type)}${activeCount > 1 ? ` ${activeCount} 项` : ''}`
        : importFlow?.active && !active.length ? '正在添加 1 个应用' : `${activeCount} 项任务进行中`;
  const label = attentionCount > 0 ? `${attentionCount} 项需要处理${activeCount > 0 ? ` · ${activeCount} 项进行中` : ''}` : activity;
  const title = (task: TaskRecord) => { const name = projectName(task.projectId); return name ? `${taskLabel(task.type)} · ${name}` : taskLabel(task.type); };
  const entry = (task: TaskRecord) => <li key={task.taskId} className={`task-center-entry is-${task.status}`}>
    <details><summary><span className="history-chip">{statusText[task.status]}</span><strong>{title(task)}</strong></summary><ProjectTask task={task} />{canViewTask?.(task) && <button className="refresh-button" onClick={() => onViewTask?.(task)}>查看添加流程</button>}</details>
  </li>;
  return <div className="task-center" ref={container}>
    {visible && <button ref={button} type="button" className="radio-task-button" aria-expanded={open} aria-controls={`${id}-panel`} aria-label={label} disabled={disabled} onClick={() => onOpenChange(!open)}>
      <ListChecks size={16} aria-hidden="true" />{label}
    </button>}
    {open && <section className="task-center-panel" id={`${id}-panel`} aria-label="任务">
      <div className="task-center-heading"><h2>任务</h2><span>关闭弹出层不会中断任务</span><button type="button" className="text-button" aria-label="关闭任务" onClick={close}>关闭</button></div>
      {importFlow && (importFlow.active || importFlow.needsAttention) && <div className="task-center-import"><strong>{importFlow.label ?? (importFlow.needsAttention ? '添加应用需要处理' : '正在添加应用')}</strong><button className="refresh-button" onClick={importFlow.onOpen}>查看添加流程</button></div>}
      {recoveryFlow?.needsAttention && <div className="task-center-import"><strong>上次添加需要核对</strong><button className="refresh-button" onClick={recoveryFlow.onOpen}>查看上次添加</button></div>}
      {active.length > 0 && <><h3>进行中</h3><ul aria-label="进行中的任务">{active.map(entry)}</ul></>}
      {attention.length > 0 && <><h3>需要处理</h3><ul aria-label="需要处理的任务">{attention.map(entry)}</ul></>}
      {done.length > 0 && <><h3>最近完成</h3><ul aria-label="最近完成的任务">{done.map(entry)}</ul></>}
      {!tasks.length && !importFlow?.active && !importFlow?.needsAttention && !recoveryFlow?.needsAttention && <p className="radio-tasks-empty">暂无任务。</p>}
    </section>}
  </div>;
}
