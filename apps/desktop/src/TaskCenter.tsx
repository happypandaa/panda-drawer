import { t } from './i18n';
import { ListChecks } from 'lucide-react';
import { useEffect, useId, useRef } from 'react';
import { ProjectTask, taskLabel } from './ProjectTask';
import type { TaskRecord } from './types';

const statusText: Record<TaskRecord['status'], string> = { queued: '排队中', running: '进行中', waitingConfirmation: '需要确认', succeeded: '完成', failed: '未完成', interrupted: '已中断' };
export function taskNeedsAttention(task: TaskRecord) {
  if (task.status === 'waitingConfirmation') return true;
  if (!task.final) return false;
  if (task.status === 'failed' || task.status === 'interrupted') return true;
  const result = task.result ?? task.error?.details;
  const nestedPending = (value: unknown) => Boolean(value && typeof value === 'object' && 'cleanupPending' in value && value.cleanupPending === true);
  return result?.cleanupPending === true || nestedPending(result?.sourceCleanup)
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
  const active = values.filter(task => !task.final && task.status !== 'waitingConfirmation');
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
      container.current?.querySelector<HTMLButtonElement>('[data-task-close]')?.focus();
    }
    wasVisible.current = visible;
  }, [visible, open]);
  if (!visible && !open) return null;
  const downloading = active.length > 0 && active.every(task => task.type === 'nodeGitImport' && ['queued', 'downloading'].includes(task.phase));
  const sameType = active.length > 0 && active.every(task => task.type === active[0].type);
  const activity = downloading ? t("正在下载 {p0} 个应用", { p0: String(activeCount) })
    : sameType && active[0].type === 'nodeGitImport' ? t("正在添加 {p0} 个应用", { p0: String(activeCount) })
      : sameType ? t("正在{p0}{p1}", { p0: String(taskLabel(active[0].type)), p1: String(activeCount > 1 ? t(" {p0} 项", { p0: String(activeCount) }) : '') })
        : importFlow?.active && !active.length ? t("正在添加 1 个应用") : t("{p0} 项任务进行中", { p0: String(activeCount) });
  const label = attentionCount > 0 ? t("{p0} 项需要处理{p1}", { p0: String(attentionCount), p1: String(activeCount > 0 ? t(" · {p0} 项进行中", { p0: String(activeCount) }) : '') }) : activity;
  const title = (task: TaskRecord) => { const name = projectName(task.projectId); return name ? `${taskLabel(task.type)} · ${name}` : taskLabel(task.type); };
  const entry = (task: TaskRecord) => <li key={task.taskId} className={`task-center-entry is-${task.status}`}>
    <details><summary><span className="history-chip">{t(statusText[task.status])}</span><strong>{title(task)}</strong></summary><ProjectTask task={task} />{canViewTask?.(task) && <button className="refresh-button" onClick={() => onViewTask?.(task)}>{t("查看添加流程")}</button>}</details>
  </li>;
  return <div className="task-center" ref={container}>
    {visible && <button ref={button} type="button" className="radio-task-button" aria-expanded={open} aria-controls={`${id}-panel`} aria-label={label} disabled={disabled} onClick={() => onOpenChange(!open)}>
      <ListChecks size={16} aria-hidden="true" />{label}
    </button>}
    {open && <section className="task-center-panel" id={`${id}-panel`} aria-label={t("任务")}>
      <div className="task-center-heading"><h2>{t("任务")}</h2><span>{t("关闭弹出层不会中断任务")}</span><button type="button" className="text-button" aria-label={t("关闭任务")} data-task-close onClick={close}>{t("关闭")}</button></div>
      {importFlow && (importFlow.active || importFlow.needsAttention) && <div className="task-center-import"><strong>{importFlow.label ? t(importFlow.label) : (importFlow.needsAttention ? t("添加应用需要处理") : t("正在添加应用"))}</strong><button className="refresh-button" onClick={importFlow.onOpen}>{t("查看添加流程")}</button></div>}
      {recoveryFlow?.needsAttention && <div className="task-center-import"><strong>{t("上次添加需要核对")}</strong><button className="refresh-button" onClick={recoveryFlow.onOpen}>{t("查看上次添加")}</button></div>}
      {active.length > 0 && <><h3>{t("进行中")}</h3><ul aria-label={t("进行中的任务")}>{active.map(entry)}</ul></>}
      {attention.length > 0 && <><h3>{t("需要处理")}</h3><ul aria-label={t("需要处理的任务")}>{attention.map(entry)}</ul></>}
      {done.length > 0 && <><h3>{t("最近完成")}</h3><ul aria-label={t("最近完成的任务")}>{done.map(entry)}</ul></>}
      {!tasks.length && !importFlow?.active && !importFlow?.needsAttention && !recoveryFlow?.needsAttention && <p className="radio-tasks-empty">{t("暂无任务。")}</p>}
    </section>}
  </div>;
}
