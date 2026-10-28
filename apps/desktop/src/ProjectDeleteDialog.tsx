import { invoke } from '@tauri-apps/api/core';
import { TriangleAlert, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { DeleteProgress } from './DeleteProgress';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export function ProjectDeleteDialog({ project, status, task, instanceId, disabled, onTask, onClose, onPendingChange, feedback }: {
  project: ProjectInfo;
  status?: MaintenanceStatus;
  task?: TaskRecord;
  instanceId?: string;
  disabled: boolean;
  onTask: (task: TaskRecord) => void;
  onClose: () => void;
  onPendingChange?: (pending: boolean) => void;
  feedback?: ReactNode;
}) {
  const [acknowledgedIdentity, setAcknowledgedIdentity] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [accepted, setAccepted] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const confirmationIdentity = JSON.stringify([identity, project.name, status?.mode, status?.maintenanceId]);
  const context = useRef(identity); context.current = identity;
  const acceptedIdentity = useRef<string | null>(null);
  const callbacks = useRef({ onTask, onClose, onPendingChange }); callbacks.current = { onTask, onClose, onPendingChange };
  const acknowledged = acknowledgedIdentity === confirmationIdentity;
  const observed = acceptedIdentity.current !== identity ? null : accepted && task?.taskId === accepted.taskId && task.projectId === project.id && task.type === 'delete' && (!accepted.final || task.final) ? task : accepted;
  const busy = submitting || Boolean(observed && !observed.final);
  const unavailable = disabled || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired) || !project.documentEpoch;
  const blocked = unavailable || busy;
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; callbacks.current.onPendingChange?.(false); };
  }, []);
  useEffect(() => {
    sequence.current++; setAcknowledgedIdentity(null); acceptedIdentity.current = null; setAccepted(null); setError('');
  }, [identity]);
  useEffect(() => { setAcknowledgedIdentity(null); }, [confirmationIdentity]);
  useEffect(() => { if (unavailable) setAcknowledgedIdentity(null); }, [unavailable]);
  useEffect(() => {
    if (acceptedIdentity.current === identity && observed?.final && accepted && !accepted.final) setAccepted(observed);
  }, [identity, observed, accepted]);
  useEffect(() => { callbacks.current.onPendingChange?.(busy); }, [busy]);

  async function remove() {
    if (operation.current || blocked || !acknowledged) return;
    operation.current = true; setSubmitting(true); setAcknowledgedIdentity(null); setError('');
    const request = ++sequence.current; const selected = identity;
    const current = () => alive.current && request === sequence.current && context.current === selected;
    try {
      const value = await invoke<TaskRecord>('project_delete', { projectId: project.id, input: { expectedDocumentEpoch: project.documentEpoch, ...(status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {}) } });
      if (!current()) return;
      if (!value?.taskId || value.projectId !== project.id || value.type !== 'delete' || typeof value.final !== 'boolean') throw new Error('删除任务的项目身份或结果不符。请核对任务记录，不要重复提交。');
      acceptedIdentity.current = selected; setAccepted(value);
      if (value.status === 'failed' || value.status === 'interrupted') setError(value.error ? messageFrom(value.error) : '删除未完成，请核对任务结果后再决定。');
      callbacks.current.onTask(value);
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setSubmitting(false); }
  }
  function close() {
    if (!operation.current && !busy) callbacks.current.onClose();
  }
  const progressDetails: Record<string, string> = {
    queued: '删除任务已受理，正在等待执行',
    'delete-freeze': '正在停止项目写入并关闭数据库',
    'delete-prepared': '正在核对项目身份和删除范围',
    'delete-intent': '正在永久删除项目文件',
  };
  return <Modal title="删除项目" titleIcon={<Trash2 size={22} />} description={busy ? undefined : '核对删除范围，确认后无法撤销。'} className="project-delete-modal" closeDisabled={busy} beforeClose={() => !operation.current && !busy} onClose={close}>
    <section className="project-delete-confirmation" aria-label="删除项目确认">
      <div className="project-delete-review" hidden={busy} aria-busy={busy}>
      <h3>永久删除「{project.name}」？此操作无法撤销。</h3>
      <p className="project-delete-warning"><TriangleAlert size={18} />本次删除不进入平台或系统回收站，无法从回收站恢复。</p>
      <p>项目 ID：<code>{project.id}</code></p>
      <div className="project-delete-scope"><h4>本次删除</h4><p>将永久删除此项目目录内的源码、数据库、附件、图标、当前网页和说明，并从项目列表移除。</p><code className="workspace-scope-path">{project.projectRoot}</code></div>
      <div className="project-delete-scope"><h4>继续保留</h4><p>迁移前的旧工作区、操作与恢复记录、其他项目和外部导出的 ZIP 不会自动删除。</p></div>
      <p>已有项目窗口将无法继续保存。请先处理未保存内容；详情页的草稿不会因删除而自动保存。</p>
      {!status && <p className="project-help">维护状态尚未读取，请先刷新状态再删除。</p>}
      {!project.documentEpoch && <p className="project-help" role="status">项目身份尚未读取，不能确认删除。请关闭弹窗并刷新项目列表，重新核对后再删除。</p>}
      {status?.recoveryRequired && <p className="task-error">项目有待处理的恢复现场，请先核对并处理，保留相关证据。</p>}
      {!busy && (disabled || Boolean(status?.activeTask)) && <p className="project-help">当前操作进行中或目标已变化，请等待完成并重新核对后再删除。</p>}
      <label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked} onChange={event => setAcknowledgedIdentity(event.target.checked ? confirmationIdentity : null)} />我已核对项目和删除目录，处理未保存内容，并理解永久删除无法撤销</label>
      </div>
      {busy && <DeleteProgress name={project.name} detail={submitting ? '正在提交删除请求' : progressDetails[observed?.phase ?? ''] ?? '正在等待删除任务的最终结果'} />}
      {error && <p className="task-error" role="alert">{error}</p>}
      {observed && (observed.final ? <ProjectTask task={observed} /> : <details className="delete-task-details"><summary>查看删除任务详情</summary><ProjectTask task={observed} /></details>)}
      {feedback}
      <div className="confirmation-actions" hidden={busy}><button type="button" className="refresh-button" disabled={busy} onClick={close}>取消，保留项目</button><button type="button" className="danger-button" disabled={blocked || !acknowledged} onClick={() => void remove()}>{busy ? '正在永久删除…' : '确认永久删除项目'}</button></div>
    </section>
  </Modal>;
}
