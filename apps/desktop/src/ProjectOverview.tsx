import { invoke } from '@tauri-apps/api/core';
import { BookOpen, ChevronRight, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { NetworkRequirement } from './generated/panda-manifest';
import { ApplicationDetails, ApplicationDetailRow } from './ApplicationDetails';
import { ProjectAttachmentBrowser } from './StoragePanels';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { AiProjectHelp } from './AiProjectHelp';
import { projectRowFor } from './workspace-storage-data';
import type { WorkspaceStorage } from './workspace-storage-data';
import type { MaintenanceStatus, ProjectInfo, StoragePart, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText, partBytes } from './storage-utils';

export function ProjectOverview({ project, instanceId, status, task, disabled, onDismissGuardChange, onDevelopmentStateChange, technicalDetails, actions, handoff, notices, onDelete }: {
  project: ProjectInfo; instanceId?: string; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean;
  onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; onClose: () => void;
  technicalDetails?: ReactNode; actions?: ReactNode; handoff?: ReactNode; notices?: ReactNode; onDelete?: () => void; running?: boolean;
  onDismissGuardChange?: (guard: (() => boolean) | null) => void;
  onDevelopmentStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [storage, setStorage] = useState<StoragePart | null>(null);
  const [error, setError] = useState('');
  const [fileBusy, setFileBusy] = useState(false);
  const [moreVisited, setMoreVisited] = useState(false);
  const alive = useRef(true);
  const pending = useRef(false); pending.current = fileBusy;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    let cancelled = false; setStorage(null);
    void invoke<WorkspaceStorage>('workspace_storage').then(value => { if (!cancelled) setStorage(projectRowFor(value, project.id) ?? null); }).catch(() => {});
    return () => { cancelled = true; };
  }, [project.id, project.projectRoot, instanceId, project.storage?.currentProject?.completedAt, task?.final ? task.taskId : null]);
  useEffect(() => {
    onDismissGuardChange?.(() => {
      if (!pending.current) return true;
      setError('文件操作仍在进行，请等待完成后再关闭。'); return false;
    });
    return () => onDismissGuardChange?.(null);
  }, [onDismissGuardChange]);
  useEffect(() => { onDevelopmentStateChange?.({ dirty: false, pending: fileBusy }); }, [fileBusy, onDevelopmentStateChange]);
  useEffect(() => () => onDevelopmentStateChange?.({ dirty: false, pending: false }), [onDevelopmentStateChange]);
  const manifest = project.manifest;
  const requirements = manifest?.runtimeRequirements;
  const networks: [string, NetworkRequirement[]][] = Object.entries(requirements?.network ?? {}).filter(([, values]) => Array.isArray(values)).map(([kind, values]) => [kind, (values as NetworkRequirement[]).filter(item => item && typeof item.origin === 'string')]);
  const devices = Object.entries(requirements?.devices ?? {}).filter(([, value]) => value && typeof value === 'object');
  async function revealReadme() {
    try { await invoke('project_reveal', { projectId: project.id, target: 'readme' }); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
  }
  return <section className="project-overview project-overview-embedded detail-overview" aria-label={`${project.name}项目详情`}>
    {['error', 'entryMissing'].includes(project.state) && <AiProjectHelp context={{ mode: 'start-failed', runtime: 'web', sourceKind: project.ownership === 'user' ? 'local' : 'managed', sourcePath: project.projectRoot, appName: project.name, issues: project.issues }} />}
    <ApplicationDetails actions={actions} handoff={handoff} notices={<>{error && <p className="task-error" role="alert">{error}</p>}{notices}</>}
      information={<>
        <ApplicationDetailRow label="项目文件夹" action={project.projectRoot ? <button className="text-button" onClick={() => { setError(''); void invoke('open_project_folder', { path: project.projectRoot }).catch(cause => { if (alive.current) setError(messageFrom(cause)); }); }}>打开</button> : undefined}><code>{project.projectRoot}</code></ApplicationDetailRow>
        <ApplicationDetailRow label="占用空间">{bytesText(storage ? partBytes(storage) : project.storage?.currentProject?.totalBytes)}{storage ? storage.complete !== true ? ' · 部分统计' : '' : project.storage?.currentProject?.stale ? ' · 待更新' : ''}</ApplicationDetailRow>
      </>}
      deleteAction={onDelete ? <button className="danger-button" disabled={disabled || fileBusy || !status || Boolean(status.activeTask) || status.recoveryRequired} onClick={onDelete}><Trash2 size={15} />停止并删除…</button> : undefined}
      more={<details className="detail-overview-body application-detail-more" onToggle={event => { if (event.currentTarget.open) setMoreVisited(true); }}><summary>更多</summary>
        <section className="detail-information"><h3>应用信息</h3><p className="detail-description">{project.description || '尚未填写简介。'}</p>{manifest?.app.version && <p>版本：{manifest.app.version}</p>}</section>
        {project.issues.length > 0 && <div className="detail-project-issues">{project.issues.map(issue => <p className="environment-issue" key={issue}>{issue}</p>)}</div>}
        {manifest?.documentation?.readme && <button className="detail-document-row" disabled={disabled} onClick={() => void revealReadme()}><BookOpen size={24} /><span><strong>查看使用说明</strong><small>了解{project.name}的功能与使用方法。</small></span><ChevronRight size={18} /></button>}
        {manifest?.platform && moreVisited && <ProjectAttachmentBrowser project={project} blocked={disabled || !status || status.mode === 'maintenance' || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final)} onBusyChange={setFileBusy} />}
        <details className="detail-requirements"><summary>联网与设备要求<span>{!devices.length && !networks.some(([, items]) => items.length) ? '未声明额外需求' : '查看用途声明'}<ChevronRight size={16} /></span></summary><p>这些是应用声明的用途；使用摄像头或麦克风时仍需系统授权。</p>{networks.map(([kind, origins]) => <div key={kind}><h4>{({ connect: '网络连接', images: '外部图片', media: '外部音视频' } as Record<string, string>)[kind] ?? kind}</h4>{origins.map((item, index) => <p key={`${item.origin}:${index}`}><code>{item.origin}</code> · {item.purpose || '未说明用途'}{item.dataSent ? ` · 发送数据：${item.dataSent}` : ''}</p>)}</div>)}{devices.map(([device, value]) => <p key={device}>{device === 'camera' ? '摄像头' : device === 'microphone' ? '麦克风' : device} · {value.purpose || '未说明用途'}</p>)}{!devices.length && !networks.some(([, items]) => items.length) && <p>未声明外部连接或设备用途。</p>}</details>
        <details className="detail-technical-information"><summary>详细运行信息</summary><code className="overview-project-id">{project.id}</code><ObjectLayoutPanel layout={project.objectLayout} />{technicalDetails}</details>
      </details>}
    />
  </section>;
}
