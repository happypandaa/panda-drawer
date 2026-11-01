import { invoke } from '@tauri-apps/api/core';
import { TriangleAlert, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { DeleteProgress } from './DeleteProgress';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform } from './frontend-platform';

interface DeletePreview {
  projectId: string; instanceId: string; projectRoot: string | null;
  expectedDocumentEpoch: string | null; mode: string; maintenanceId: string | null;
  deletePaths: string[]; alreadyAbsent: boolean; cleanupOnly: boolean;
  ownership: 'user' | 'platform' | null; sourcePreserved: boolean;
}
function validPreview(value: DeletePreview, project: ProjectInfo, instanceId: string | undefined, isAbsolutePath: (path: string) => boolean) {
  const absent = value?.alreadyAbsent === true || value?.cleanupOnly === true;
  return Boolean(value && value.projectId === project.id && typeof value.instanceId === 'string' && value.instanceId.length > 0 && (!instanceId || value.instanceId === instanceId)
    && typeof value.alreadyAbsent === 'boolean' && typeof value.cleanupOnly === 'boolean' && !(value.alreadyAbsent && value.cleanupOnly)
    && (absent ? value.projectRoot === null && value.expectedDocumentEpoch === null : value.projectRoot === project.projectRoot && typeof value.expectedDocumentEpoch === 'string' && value.expectedDocumentEpoch.length > 0)
    && (absent || value.ownership === project.ownership && value.sourcePreserved === (project.ownership === 'user'))
    && ['ready', 'maintenance', 'unknown'].includes(value.mode) && (value.maintenanceId === null || typeof value.maintenanceId === 'string')
    && (value.mode !== 'maintenance' || Boolean(value.maintenanceId))
    && Array.isArray(value.deletePaths) && value.deletePaths.every(path => typeof path === 'string' && isAbsolutePath(path)) && (!value.alreadyAbsent || value.deletePaths.length === 0));
}

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
  const platform = useFrontendPlatform();
  const [acknowledgedIdentity, setAcknowledgedIdentity] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [accepted, setAccepted] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const [preview, setPreview] = useState<DeletePreview | null>(null);
  const [reading, setReading] = useState(true);
  const [readError, setReadError] = useState('');
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const confirmationIdentity = JSON.stringify([identity, project.name, preview?.expectedDocumentEpoch, preview?.mode, preview?.maintenanceId, preview?.deletePaths, project.ownership, preview?.ownership, preview?.sourcePreserved]);
  const context = useRef(identity); context.current = identity;
  const acceptedIdentity = useRef<string | null>(null);
  const callbacks = useRef({ onTask, onClose, onPendingChange }); callbacks.current = { onTask, onClose, onPendingChange };
  const acknowledged = acknowledgedIdentity === confirmationIdentity;
  const observed = acceptedIdentity.current !== identity ? null : accepted && task?.taskId === accepted.taskId && task.projectId === project.id && task.type === 'delete' && (!accepted.final || task.final) ? task : accepted;
  const busy = submitting || Boolean(observed && !observed.final);
  const unavailable = disabled || reading || !preview || Boolean(status?.activeTask);
  const blocked = unavailable || busy;
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; callbacks.current.onPendingChange?.(false); };
  }, []);
  useEffect(() => {
    sequence.current++; setAcknowledgedIdentity(null); acceptedIdentity.current = null; setAccepted(null); setError(''); setPreview(null);
  }, [identity]);
  useEffect(() => { setAcknowledgedIdentity(null); }, [confirmationIdentity]);
  useEffect(() => { if (unavailable) setAcknowledgedIdentity(null); }, [unavailable]);
  useEffect(() => {
    if (acceptedIdentity.current === identity && observed?.final && accepted && !accepted.final) setAccepted(observed);
  }, [identity, observed, accepted]);
  useEffect(() => { callbacks.current.onPendingChange?.(busy); }, [busy]);

  async function readPreview() {
    if (operation.current || observed && !observed.final) return;
    const request = ++sequence.current; const selected = identity;
    setReading(true); setPreview(null); setReadError(''); setAcknowledgedIdentity(null);
    const current = () => alive.current && request === sequence.current && context.current === selected;
    try {
      const value = await invoke<DeletePreview>('project_delete_preview', { projectId: project.id });
      if (!current()) return;
      if (!validPreview(value, project, instanceId, platform.isAbsolutePath)) throw new Error('删除范围或项目身份不符，请重新核对。');
      setPreview(value);
    } catch (cause) { if (current()) setReadError(messageFrom(cause)); }
    finally { if (current()) setReading(false); }
  }
  useEffect(() => { void readPreview(); }, [identity, status?.mode, status?.maintenanceId]);

  async function remove() {
    if (operation.current || blocked || !acknowledged || !preview) return;
    operation.current = true; setSubmitting(true); setAcknowledgedIdentity(null); setError('');
    const request = ++sequence.current; const selected = identity;
    const current = () => alive.current && request === sequence.current && context.current === selected;
    try {
      const value = await invoke<TaskRecord>('project_delete', { projectId: project.id, input: { ...(preview.expectedDocumentEpoch ? { expectedDocumentEpoch: preview.expectedDocumentEpoch } : {}), ...(preview.mode === 'maintenance' ? { maintenanceId: preview.maintenanceId } : {}) } });
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
    finalizing: '正在清除项目登记与关联材料',
    'delete-finalizing': '正在清除项目登记与关联材料',
  };
  return <Modal title="删除项目" titleIcon={<Trash2 size={22} />} description={busy ? undefined : '核对删除范围，确认后无法撤销。'} className="project-delete-modal" closeDisabled={busy} beforeClose={() => !operation.current && !busy} onClose={close}>
    <section className="project-delete-confirmation" aria-label="删除项目确认">
      <div className="project-delete-review" hidden={busy} aria-busy={busy}>
      <h3>永久删除「{project.name}」？此操作无法撤销。</h3>
      <p className="project-delete-warning"><TriangleAlert size={18} />本次删除不进入平台或系统回收站，无法从回收站恢复。</p>
      <p>项目 ID：<code>{project.id}</code></p>
      <div className="project-delete-scope"><h4>本次删除</h4><p>{project.ownership === 'user' ? '只清除这个应用的平台登记、配置、日志、任务和缓存。' : '整个项目文件夹（包括源码、Git、未提交修改、data/ 下的数据库及附件）一起删除，同时清除平台登记、配置、日志、任务和缓存。'}</p><p>浏览器数据：localStorage、IndexedDB、Cookie 等一律清除，重新添加文件夹也无法恢复。</p>{preview ? preview.deletePaths.map(path => <code className="workspace-scope-path" key={path}>{path}</code>) : <code className="workspace-scope-path">{project.projectRoot}</code>}</div>
      <div className="project-delete-scope"><h4>继续保留</h4><p>{project.ownership === 'user' ? '项目文件夹、源码、Git 和 data/ 下的文件数据保留。' : '其他项目、原仓库，以及已完成保存到外部文件夹的交付文件保留。'}平台全局运行环境和设置保留。</p></div>
      <p>已有项目窗口将无法继续保存。请先处理未保存内容；详情页的草稿不会因删除而自动保存。</p>
      {reading && <p className="project-help" role="status">正在核对删除范围…</p>}
      {readError && <p className="task-error" role="alert">无法核对删除范围：{readError}</p>}
      {!reading && !busy && <button type="button" className="text-button" onClick={() => void readPreview()}>重新核对删除范围</button>}
      {preview?.alreadyAbsent && <p className="project-help">项目已不存在，确认后将核对删除完成状态。</p>}
      {preview?.cleanupOnly && <p className="project-help">项目目录已删除，本次只清除已核实归属的剩余材料。</p>}
      {preview?.mode === 'unknown' && <p className="project-help">项目状态异常，仍可按已核实范围永久删除。</p>}
      {status?.recoveryRequired && <p className="task-error">当前未完成操作会先停止，再按已核实的范围删除平台材料。</p>}
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
