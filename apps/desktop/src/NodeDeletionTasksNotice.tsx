import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { NodeDeletionTaskSnapshot } from './node-app-types';
import { nodeDeletionLabel, validNodeDeletionTask } from './node-deletion-task-state';
import { messageFrom } from './ui-utils';

/** Journal discovery must also run when no registered Node applications remain. */
export function NodeDeletionTasksNotice({ instanceId, revision, tasks, enabled, onTasks, onOpen }: {
  instanceId?: string;
  revision: number;
  tasks: NodeDeletionTaskSnapshot[];
  enabled: boolean;
  onTasks: (tasks: NodeDeletionTaskSnapshot[]) => void;
  onOpen: (task: NodeDeletionTaskSnapshot) => void;
}) {
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const callbacks = useRef({ onTasks, onOpen }); callbacks.current = { onTasks, onOpen };
  useEffect(() => {
    let cancelled = false; let reading = false;
    setError('');
    const read = async () => {
      if (cancelled || reading) return;
      reading = true;
      try {
        const values = await invoke<NodeDeletionTaskSnapshot[]>('node_app_delete_tasks');
        if (!Array.isArray(values) || values.some(value => !validNodeDeletionTask(value)) || new Set(values.map(value => value.taskId)).size !== values.length) throw new Error(t("删除任务列表不完整，请重新核对。"));
        if (!cancelled) { callbacks.current.onTasks(values); setError(''); }
      } catch (cause) { if (!cancelled) setError(t("删除任务暂时无法读取：{p0}", { p0: String(messageFrom(cause)) })); }
      finally { reading = false; }
    };
    void read();
    const timer = window.setInterval(() => void read(), 2000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [instanceId, revision, retry]);
  const unfinished = tasks.filter(task => !['deleted', 'abandoned'].includes(task.phase));
  if (!unfinished.length && !error) return null;
  return <section className="notice node-deletion-tasks" aria-label={t("未完成的应用删除")}>
    <div><strong>{t("删除未完成")}</strong><p>{t("以下任务会继续保留删除状态，应用不会自动启动。")}</p>
      {unfinished.map(task => <div className="node-deletion-task-entry" key={task.taskId}><span>{task.name} · {nodeDeletionLabel(task)}</span><button type="button" className="text-button" disabled={!enabled} onClick={() => callbacks.current.onOpen(task)}>{task.phase === 'failed' ? t("处理删除") : t("查看删除进度")}</button></div>)}
      {error && <p className="task-error" role="alert">{t(error)}</p>}
      {error && <button type="button" className="refresh-button" onClick={() => setRetry(value => value + 1)}>{t("重新读取删除任务")}</button>}
    </div>
  </section>;
}
