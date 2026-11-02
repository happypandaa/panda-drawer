import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import { bigText } from './storage-utils';
import { projectRegenerable, regenerableCategories } from './workspace-storage-data';
import { messageFrom } from './ui-utils';
import type { ProjectInfo, TaskRecord } from './types';

interface Props {
  projects: ProjectInfo[];
  instanceId?: string;
  workspaceRoot: string | null;
  enabled: boolean;
  isProjectBusy?: (project: ProjectInfo) => boolean;
  onProjectTask?: (task: TaskRecord, project: ProjectInfo) => void;
  onRefresh: () => Promise<void>;
  onPendingChange?: (pending: boolean) => void;
}

function checkedTask(value: TaskRecord, projectId: string, taskId?: string) {
  if (!value || value.type !== 'storageCleanup' || value.projectId !== projectId
    || typeof value.taskId !== 'string' || !value.taskId || (taskId && value.taskId !== taskId)
    || !['queued', 'running', 'succeeded', 'failed', 'interrupted'].includes(value.status)
    || value.final !== ['succeeded', 'failed', 'interrupted'].includes(value.status)) {
    throw new Error(t("清理任务回执身份或格式不符，请核对任务；不会重复提交。"));
  }
  return value;
}

export function RegenerableCleanupPanel(props: Props) {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [uncertain, setUncertain] = useState(false);
  const pending = useRef(false);
  const generation = useRef(0);
  const current = useRef(props); current.current = props;
  const scope = `${props.instanceId ?? ''}:${props.workspaceRoot ?? ''}`;
  const scopeRef = useRef(scope); scopeRef.current = scope;
  useEffect(() => {
    generation.current++; setOpen(false); setSelected([]); setTask(null); setError(''); setNotice(''); setUncertain(false); setBusy(false);
    return () => { generation.current++; };
  }, [scope]);
  const available = props.projects.filter(project => (projectRegenerable(project) ?? 0n) > 0n);
  const blocked = !props.enabled || busy || uncertain || Boolean(task && !task.final);

  async function waitForFinal(initial: TaskRecord, project: ProjectInfo, sequence: number, acceptedScope: string) {
    let value = initial;
    while (!value.final) {
      await new Promise(resolve => window.setTimeout(resolve, 300));
      if (sequence !== generation.current || scopeRef.current !== acceptedScope) return null;
      value = checkedTask(await invoke<TaskRecord>('task_get', { taskId: initial.taskId }), project.id, initial.taskId);
      setTask(value); current.current.onProjectTask?.(value, project);
    }
    return value;
  }
  async function submit() {
    if (blocked || pending.current || !selected.length) return;
    const targets = available.filter(project => selected.includes(project.id));
    if (!targets.length) return;
    const sequence = generation.current; const acceptedScope = scopeRef.current;
    pending.current = true; setBusy(true); setOpen(false); setError(''); setNotice('');
    current.current.onPendingChange?.(true);
    try {
      for (const target of targets) {
        const project = current.current.projects.find(value => value.id === target.id);
        if (sequence !== generation.current || scopeRef.current !== acceptedScope) return;
        if (!current.current.enabled || !project || project.projectRoot !== target.projectRoot
          || project.documentEpoch !== target.documentEpoch || current.current.isProjectBusy?.(project)) {
          throw new Error(t("「{value0}」当前不能清理，后续项目尚未提交。", { value0: target.name }));
        }
        setUncertain(true);
        const accepted = checkedTask(await invoke<TaskRecord>('storage_cleanup', {
          projectId: project.id, input: { categories: [...regenerableCategories] },
        }), project.id);
        if (sequence !== generation.current || scopeRef.current !== acceptedScope) return;
        setUncertain(false); setTask(accepted); current.current.onProjectTask?.(accepted, project);
        const result = await waitForFinal(accepted, project, sequence, acceptedScope);
        if (!result) return;
        if (result.status !== 'succeeded' || result.result?.completed === false) {
          throw new Error(t("「{value0}」清理未完整成功，后续项目尚未提交，请查看任务结果。", { value0: project.name }));
        }
      }
      setNotice(t("所选应用的可重新生成文件已完成清理，请核对任务结果。"));
    } catch (cause) {
      if (sequence === generation.current && scopeRef.current === acceptedScope) setError(messageFrom(cause));
    } finally {
      pending.current = false;
      if (sequence === generation.current && scopeRef.current === acceptedScope) {
        setBusy(false); current.current.onPendingChange?.(false);
        try { await current.current.onRefresh(); } catch (cause) { setError(t("任务已提交，空间统计刷新失败：{value0}", { value0: messageFrom(cause) })); }
      }
    }
  }
  async function reconcile() {
    if (!task || pending.current) return;
    const project = props.projects.find(value => value.id === task.projectId);
    if (!project) return;
    pending.current = true; setBusy(true); setError(''); props.onPendingChange?.(true);
    const sequence = generation.current; const acceptedScope = scopeRef.current;
    try {
      const value = checkedTask(await invoke<TaskRecord>('task_get', { taskId: task.taskId }), project.id, task.taskId);
      if (sequence !== generation.current || scopeRef.current !== acceptedScope) return;
      setTask(value); props.onProjectTask?.(value, project);
      await waitForFinal(value, project, sequence, acceptedScope);
    } catch (cause) { if (sequence === generation.current) setError(messageFrom(cause)); }
    finally {
      pending.current = false;
      if (sequence === generation.current && scopeRef.current === acceptedScope) {
        setBusy(false); props.onPendingChange?.(false);
        try { await props.onRefresh(); } catch (cause) { setError(t("空间统计刷新失败：{value0}", { value0: messageFrom(cause) })); }
      }
    }
  }
  return <section className="lifecycle-card workspace-regenerable-cleanup" aria-label={t("清理可重新生成文件")}>
    <h3>{t("清理可重新生成文件")}</h3>
    <p>{t("清理所选应用的依赖、构建缓存和日志。应用源码、当前网页产物、数据库及附件保留。")}</p>
    <button className="refresh-button" disabled={blocked || !available.length} onClick={() => { setSelected([]); setOpen(true); }}>{t("选择要清理的应用")}</button>
    {notice && <p role="status">{t(notice)}</p>}{error && <p className="task-error" role="alert">{t(error)}</p>}
    {task && <ProjectTask task={task} />}
    {task && !task.final && !busy && <button className="refresh-button" onClick={() => void reconcile()}>{t("重新核对任务")}</button>}
    {open && <Modal title={t("清理可重新生成文件")} closeDisabled={busy} onClose={() => setOpen(false)}>
      <p>{t("选择应用后清理依赖、构建缓存和日志；下次构建可能需要重新准备依赖。")}</p>
      {available.map(project => <label key={project.id}><input type="checkbox" checked={selected.includes(project.id)} disabled={props.isProjectBusy?.(project)} onChange={event => setSelected(values => event.target.checked ? [...values, project.id] : values.filter(id => id !== project.id))} />{project.name} · {bigText(projectRegenerable(project))}</label>)}
      <div className="confirmation-actions"><button className="primary-button" disabled={!selected.length || blocked} onClick={() => void submit()}>{t("清理所选应用")}</button><button className="text-button" onClick={() => setOpen(false)}>{t("取消")}</button></div>
    </Modal>}
  </section>;
}
