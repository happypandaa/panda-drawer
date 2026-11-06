import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { BookOpen, ChevronRight, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { NetworkRequirement } from './generated/panda-manifest';
import { ApplicationDetails, ApplicationDetailRow } from './ApplicationDetails';
import { ProjectAttachmentBrowser } from './StoragePanels';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { WebAppConfiguration } from './WebAppConfiguration';
import { AiProjectHelp } from './AiProjectHelp';
import { projectRowFor } from './workspace-storage-data';
import type { WorkspaceStorage } from './workspace-storage-data';
import type { MaintenanceStatus, ProjectInfo, StoragePart, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText, partBytes } from './storage-utils';

export function ProjectOverview({ project, instanceId, status, task, disabled, onDismissGuardChange, onDevelopmentStateChange, technicalDetails, actions, handoff, notices, onDelete, onChanged, configureRequest }: {
  project: ProjectInfo; instanceId?: string; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean;
  onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; onClose: () => void;
  technicalDetails?: ReactNode; actions?: ReactNode; handoff?: ReactNode; notices?: ReactNode; onDelete?: () => void; running?: boolean;
  onDismissGuardChange?: (guard: (() => boolean) | null) => void;
  onDevelopmentStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
  configureRequest?: number;
}) {
  const [storage, setStorage] = useState<StoragePart | null>(null);
  const [error, setError] = useState('');
  const [fileBusy, setFileBusy] = useState(false);
  const [configOpen, setConfigOpen] = useState(false);
  const [configState, setConfigState] = useState({ dirty: false, pending: false });
  const [moreVisited, setMoreVisited] = useState(false);
  const alive = useRef(true);
  const pending = useRef(false); pending.current = fileBusy || configState.pending || configState.dirty;
  useEffect(() => { if (configureRequest) setConfigOpen(true); }, [configureRequest]);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    let cancelled = false; setStorage(null);
    void invoke<WorkspaceStorage>('workspace_storage').then(value => { if (!cancelled) setStorage(projectRowFor(value, project.id) ?? null); }).catch(() => {});
    return () => { cancelled = true; };
  }, [project.id, project.projectRoot, instanceId, project.storage?.currentProject?.completedAt, task?.final ? task.taskId : null]);
  useEffect(() => {
    onDismissGuardChange?.(() => {
      if (!pending.current) return true;
      setError(t("操作仍在进行或配置尚未保存，请先保存或放弃配置修改。")); return false;
    });
    return () => onDismissGuardChange?.(null);
  }, [onDismissGuardChange]);
  useEffect(() => { onDevelopmentStateChange?.({ dirty: configState.dirty, pending: fileBusy || configState.pending }); }, [fileBusy, configState, onDevelopmentStateChange]);
  useEffect(() => () => onDevelopmentStateChange?.({ dirty: false, pending: false }), [onDevelopmentStateChange]);
  const manifest = project.manifest;
  const requirements = manifest?.runtimeRequirements;
  const networks: [string, NetworkRequirement[]][] = Object.entries(requirements?.network ?? {}).filter(([, values]) => Array.isArray(values)).map(([kind, values]) => [kind, (values as NetworkRequirement[]).filter(item => item && typeof item.origin === 'string')]);
  const devices = Object.entries(requirements?.devices ?? {}).filter(([, value]) => value && typeof value === 'object');
  async function revealReadme() {
    try { await invoke('project_reveal', { projectId: project.id, target: 'readme' }); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
  }
  return <section className="project-overview project-overview-embedded detail-overview" aria-label={t("{p0}项目详情", { p0: String(project.name) })}>
    {configOpen ? <>{error && <p className="task-error" role="alert">{t(error)}</p>}<WebAppConfiguration key={project.id} projectId={project.id} instanceId={instanceId} disabled={disabled} onState={setConfigState} onSaved={onChanged} onBack={() => { setError(''); setConfigOpen(false); }} /></> : <>
    {['error', 'entryMissing'].includes(project.state) && <AiProjectHelp context={{ mode: 'start-failed', runtime: 'web', sourceKind: project.ownership === 'user' ? 'local' : 'managed', sourcePath: project.projectRoot, appName: project.name, issues: project.issues }} />}
    <ApplicationDetails actions={actions} handoff={handoff} notices={<>{error && <p className="task-error" role="alert">{t(error)}</p>}{notices}</>}
      information={<>
        {Boolean(manifest?.configuration?.fields.length) && <ApplicationDetailRow label={t("应用配置")} action={<button className="text-button" disabled={disabled || fileBusy} onClick={() => { setError(''); setConfigOpen(true); }}>{t("修改")}</button>}>{project.state === 'needsConfiguration' ? t("请补齐必要配置") : t("已设置或使用默认值")}</ApplicationDetailRow>}
        <ApplicationDetailRow label={t("项目文件夹")} action={project.projectRoot ? <button className="text-button" onClick={() => { setError(''); void invoke('open_project_folder', { path: project.projectRoot }).catch(cause => { if (alive.current) setError(messageFrom(cause)); }); }}>{t("打开")}</button> : undefined}><code>{project.projectRoot}</code></ApplicationDetailRow>
        <ApplicationDetailRow label={t("占用空间")}>{bytesText(storage ? partBytes(storage) : project.storage?.currentProject?.totalBytes)}{storage ? storage.complete !== true ? t(" · 部分统计") : '' : project.storage?.currentProject?.stale ? t(" · 待更新") : ''}</ApplicationDetailRow>
      </>}
      deleteAction={onDelete ? <button className="danger-button" disabled={disabled || fileBusy} onClick={onDelete}><Trash2 size={15} />{t("停止并删除…")}</button> : undefined}
      more={<details className="detail-overview-body application-detail-more" onToggle={event => { if (event.currentTarget.open) setMoreVisited(true); }}><summary>{t("更多")}</summary>
        <section className="detail-information"><h3>{t("应用信息")}</h3>{project.isAgent && <span className="agent-app-badge">{t("agent 应用")}</span>}<p className="detail-description">{project.description || t("尚未填写简介。")}</p>{manifest?.app.version && <p>{t("版本：")}{manifest.app.version}</p>}</section>
        {project.issues.length > 0 && <div className="detail-project-issues">{project.issues.map(issue => <p className="environment-issue" key={issue}>{t(issue)}</p>)}</div>}
        {manifest?.documentation?.readme && <button className="detail-document-row" disabled={disabled} onClick={() => void revealReadme()}><BookOpen size={24} /><span><strong>{t("查看使用说明")}</strong><small>{t("了解{app}的功能与使用方法。", { app: project.name })}</small></span><ChevronRight size={18} /></button>}
        {manifest?.platform && moreVisited && <ProjectAttachmentBrowser project={project} blocked={disabled || !status || status.mode === 'maintenance' || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final)} onBusyChange={setFileBusy} />}
        <details className="detail-requirements"><summary>{t("联网与设备要求")}<span>{!devices.length && !networks.some(([, items]) => items.length) ? t("未声明额外需求") : t("查看用途声明")}<ChevronRight size={16} /></span></summary><p>{t("这些是应用声明的用途；使用摄像头或麦克风时仍需系统授权。")}</p>{networks.map(([kind, origins]) => <div key={kind}><h4>{({ connect: t("网络连接"), images: t("外部图片"), media: t("外部音视频") } as Record<string, string>)[kind] ?? kind}</h4>{origins.map((item, index) => <p key={`${item.origin}:${index}`}><code>{item.origin}</code> · {item.purpose || t("未说明用途")}{item.dataSent ? t(" · 发送数据：{p0}", { p0: String(item.dataSent) }) : ''}</p>)}</div>)}{devices.map(([device, value]) => <p key={device}>{device === 'camera' ? t("摄像头") : device === 'microphone' ? t("麦克风") : device} · {value.purpose || t("未说明用途")}</p>)}{!devices.length && !networks.some(([, items]) => items.length) && <p>{t("未声明外部连接或设备用途。")}</p>}</details>
        <details className="detail-technical-information"><summary>{t("详细运行信息")}</summary><code className="overview-project-id">{project.id}</code><ObjectLayoutPanel layout={project.objectLayout} />{technicalDetails}</details>
      </details>}
    />
    </>}
  </section>;
}
