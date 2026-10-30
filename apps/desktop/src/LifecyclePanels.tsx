import { Download } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { ProjectExportDialog } from './ProjectExportDialog';

/** The remaining export action lives in the application details disclosure. */
export function ProjectExportAction({ project, status, onTask, disabled, active = true, onDraftStateChange, instanceId }: {
  project: ProjectInfo; status?: MaintenanceStatus; onTask: (task: TaskRecord) => void; disabled: boolean;
  active?: boolean; onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void; instanceId?: string;
}) {
  const [exportOpen, setExportOpen] = useState(false);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const blocked = disabled || !active || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired);
  useEffect(() => { draftCallback.current?.({ dirty: false, pending: exportOpen }); }, [exportOpen]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => { setExportOpen(false); }, [identity, project.name, active]);
  return <section className="project-more-row project-export-row" aria-label="导出应用">
    <div className="project-more-description"><h3>导出 ZIP</h3><p>保存一份包含源码、应用文件、数据库和用户文件的归档。</p></div>
    <button type="button" className="refresh-button" disabled={blocked} onClick={() => setExportOpen(true)}><Download size={20} />导出 ZIP</button>
    {exportOpen && active && <ProjectExportDialog key={identity} project={project} status={status} disabled={blocked} onTask={onTask} onClose={() => setExportOpen(false)} />}
  </section>;
}
