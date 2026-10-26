import { Download, FileArchive, Info, Trash2 } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { ProjectExportDialog } from './ProjectExportDialog';
import { ProjectDeleteDialog } from './ProjectDeleteDialog';

export { ImportPanel } from './ArchiveImportPanel';
type TaskHandler = (task: TaskRecord) => void;

export function ProjectLifecyclePanel({ project, status, task, onTask, disabled, active = true, deleteRequested = 0, onDraftStateChange, instanceId }: {
  project: ProjectInfo; status?: MaintenanceStatus; task?: TaskRecord; onTask: TaskHandler; onClose?: () => void; disabled: boolean;
  active?: boolean; deleteRequested?: number; onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void; instanceId?: string;
}) {
  const [exportOpen, setExportOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const handledDeleteRequest = useRef(0);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const headingId = useId();
  const deleteTitleId = useId();
  const blocked = disabled || !active || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired);
  const deleteBlocked = blocked || exportOpen || !project.documentEpoch;
  useEffect(() => { draftCallback.current?.({ dirty: false, pending: deleteOpen || exportOpen }); }, [deleteOpen, exportOpen]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => { setExportOpen(false); setDeleteOpen(false); }, [identity]);
  useEffect(() => { setExportOpen(false); }, [project.name]);
  useEffect(() => {
    if (!active) { setExportOpen(false); setDeleteOpen(false); }
    else if (deleteRequested > handledDeleteRequest.current) { handledDeleteRequest.current = deleteRequested; setDeleteOpen(true); }
  }, [active, deleteRequested]);
  return <section className="lifecycle-card lifecycle-panel-embedded project-more-panel" aria-labelledby={headingId}>
    <h2 id={headingId}>更多操作</h2>
    <p className="project-more-intro">导出归档，或管理这台电脑上的应用。</p>
    <section className="project-more-row project-export-row" aria-label="导出应用">
      <span className="project-more-icon" aria-hidden="true"><FileArchive size={20} /></span>
      <div className="project-more-description"><h3>导出 ZIP</h3><p>保存一份包含源码、应用文件、数据库和用户文件的归档。</p></div>
      <button type="button" className="refresh-button" disabled={blocked} onClick={() => setExportOpen(true)}><Download size={20} />导出 ZIP</button>
    </section>
    <section className="project-delete-section" aria-labelledby={deleteTitleId}>
      <div className="project-delete-heading project-more-row">
        <span className="project-more-icon" aria-hidden="true"><Trash2 size={20} /></span>
        <div className="project-more-description"><h3 id={deleteTitleId}>删除应用</h3><p>永久删除当前应用及其本机数据，无法撤销。</p></div>
        <button type="button" className="danger-button" disabled={deleteBlocked} onClick={() => setDeleteOpen(true)}><Trash2 size={20} />删除应用</button>
      </div>
      {status?.recoveryRequired && <p className="task-error">项目有待处理的恢复现场，请先核对并处理，保留相关证据。</p>}
      {!status && <p className="project-help">维护状态尚未读取，请先刷新状态再删除。</p>}
      {!project.documentEpoch && <p className="project-help" role="status">项目身份尚未读取，不能确认删除。请关闭详情并刷新项目列表，重新核对后再删除。</p>}
      {(disabled || Boolean(status?.activeTask)) && <p className="project-help">当前操作进行中，请等待完成后再删除。</p>}
    </section>
    <p className="project-more-footnote"><Info size={20} aria-hidden="true" />删除前会再次确认应用和范围，不进入平台或系统回收站。</p>
    {exportOpen && active && <ProjectExportDialog key={identity} project={project} status={status} disabled={blocked} onTask={onTask} onClose={() => setExportOpen(false)} />}
    {deleteOpen && active && <ProjectDeleteDialog key={identity} project={project} status={status} task={task} instanceId={instanceId} disabled={blocked} onTask={onTask} onClose={() => setDeleteOpen(false)} />}
  </section>;
}
