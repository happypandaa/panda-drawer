import { ListChecks } from 'lucide-react';
import { useEffect, useId, useRef } from 'react';
import { ProjectTask, taskLabel } from './ProjectTask';
import type { TaskRecord } from './types';

const statusText: Record<TaskRecord['status'], string> = { queued: '排队中', running: '进行中', succeeded: '完成', failed: '未完成', interrupted: '已中断' };
const needsAttention = (task: TaskRecord) => task.final && (task.status === 'failed' || task.status === 'interrupted');

export interface TaskCenterProps {
  tasks: TaskRecord[];
  projectName: (projectId: string | null) => string | null;
  open: boolean;
  disabled?: boolean;
  onOpenChange: (open: boolean) => void;
}

// Header task button and its popover; switching pages never cancels a task.
export function TaskCenter({ tasks, projectName, open, disabled, onOpenChange }: TaskCenterProps) {
  const id = useId();
  const container = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const active = tasks.filter((task) => !task.final);
  const attention = tasks.filter(needsAttention).slice(0, 10);
  const done = tasks.filter((task) => task.final && !needsAttention(task)).slice(0, 10);
  const count = active.length + attention.length;
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); onOpenChange(false); button.current?.focus(); } };
    const onPointer = (event: MouseEvent) => { if (container.current && !container.current.contains(event.target as Node)) onOpenChange(false); };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onPointer);
    return () => { document.removeEventListener('keydown', onKey); document.removeEventListener('mousedown', onPointer); };
  }, [open, onOpenChange]);
  const title = (task: TaskRecord) => { const name = projectName(task.projectId); return name ? `${taskLabel(task.type)} · ${name}` : taskLabel(task.type); };
  const entry = (task: TaskRecord) => <li key={task.taskId} className={`task-center-entry is-${task.status}`}>
    <details><summary><span className="history-chip">{statusText[task.status]}</span><strong>{title(task)}</strong></summary><ProjectTask task={task} /></details>
  </li>;
  return <div className="task-center" ref={container}>
    <button ref={button} type="button" className="radio-task-button" aria-expanded={open} aria-controls={`${id}-panel`} aria-label={`任务${active.length ? `，${active.length} 项进行中` : ''}${attention.length ? `，${attention.length} 项需要查看` : ''}`} disabled={disabled} onClick={() => onOpenChange(!open)}>
      <ListChecks size={16} aria-hidden="true" />任务{count > 0 && <span className="task-center-count" aria-hidden="true">{count}</span>}
    </button>
    {open && <section className="task-center-panel" id={`${id}-panel`} aria-label="任务">
      <div className="task-center-heading"><h2>任务</h2><span>切换页面不会中断任务</span></div>
      {active.length > 0 && <><h3>进行中</h3><ul aria-label="进行中的任务">{active.map(entry)}</ul></>}
      {attention.length > 0 && <><h3>需要查看</h3><ul aria-label="需要查看的任务">{attention.map(entry)}</ul></>}
      {done.length > 0 && <><h3>最近完成</h3><ul aria-label="最近完成的任务">{done.map(entry)}</ul></>}
      {!tasks.length && <p className="radio-tasks-empty">暂无任务。</p>}
    </section>}
  </div>;
}
