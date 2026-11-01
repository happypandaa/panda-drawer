import { invoke } from '@tauri-apps/api/core';
import { BookOpen, ChevronRight, FolderOpen, Save, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { ApplicationDetails, ApplicationDetailRow } from './ApplicationDetails';
import { ProjectAttachmentBrowser } from './StoragePanels';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { projectRowFor } from './workspace-storage-data';
import type { WorkspaceStorage } from './workspace-storage-data';
import type { StoragePart } from './types';
import type { MaintenanceStatus, ProjectInfo, ProjectMetadata, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText, partBytes } from './storage-utils';
export function ProjectOverview({ project, instanceId, status, task, disabled, onTask: _onTask, onChanged, onClose, onDismissGuardChange, onDevelopmentStateChange, technicalDetails, actions, handoff, notices, onDelete }: {
  project: ProjectInfo; instanceId?: string; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean;
  onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; onClose: () => void;
  technicalDetails?: ReactNode; actions?: ReactNode; handoff?: ReactNode; notices?: ReactNode; onDelete?: () => void; running?: boolean;
  onDismissGuardChange?: (guard: (() => boolean) | null) => void;
  onDevelopmentStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [storage, setStorage] = useState<StoragePart | null>(null);
  useEffect(() => {
    let cancelled = false; setStorage(null);
    void invoke<WorkspaceStorage>('workspace_storage').then(value => { if (!cancelled) setStorage(projectRowFor(value, project.id) ?? null); }).catch(() => {});
    return () => { cancelled = true; };
  }, [project.id, project.projectRoot, instanceId, project.storage?.currentProject?.completedAt, task?.final ? task.taskId : null]);
  const [editing, setEditing] = useState(false);
  const [metadata, setMetadata] = useState<ProjectMetadata | null>(null);
  const [draft, setDraft] = useState({ name: '', description: '' });
  const [latest, setLatest] = useState<ProjectMetadata | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [folderError, setFolderError] = useState('');
  const [notice, setNotice] = useState('');
  const [fileBusy, setFileBusy] = useState(false);
  const [moreVisited, setMoreVisited] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const developmentCallback = useRef(onDevelopmentStateChange); developmentCallback.current = onDevelopmentStateChange;
  const [dismissConfirm, setDismissConfirm] = useState(false);
  const [dismissMessage, setDismissMessage] = useState('');
  const pending = useRef(false);
  const request = useRef(0);
  const alive = useRef(true);
  const discardApproved = useRef(false);
  const dismissNotice = useRef<HTMLDivElement>(null);
  const editingBlocked = disabled || !status || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final) || (status.mode === 'maintenance' && !status.readyForChanges);
  const maintenanceInput = status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {};
  const dirty = Boolean(metadata && (draft.name !== metadata.name || draft.description !== metadata.description));
  const morePending = busy || fileBusy;
  const operationPending = morePending;
  const guardState = useRef({ dirty: false, pending: false });
  guardState.current = { dirty: dirty, pending: operationPending };
  useEffect(() => { developmentCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => () => { developmentCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => {
    onDismissGuardChange?.(() => {
      if (pending.current || guardState.current.pending) { setDismissMessage('操作、恢复核对或文件选择仍在进行，请等待完成后再关闭。'); return false; }
      if (discardApproved.current) { discardApproved.current = false; return true; }
      if (guardState.current.dirty) { setDismissConfirm(true); setDismissMessage(''); return false; }
      return true;
    });
    return () => onDismissGuardChange?.(null);
  }, [onDismissGuardChange]);
  useEffect(() => { if (dismissConfirm || dismissMessage) dismissNotice.current?.focus(); }, [dismissConfirm, dismissMessage]);
  useEffect(() => {
    alive.current = true; pending.current = false; setBusy(false); const sequence = ++request.current;
    void invoke<ProjectMetadata>('project_metadata', { projectId: project.id }).then((value) => {
      if (alive.current && sequence === request.current) { setMetadata(value); setDraft({ name: value.name, description: value.description }); }
    }).catch((cause) => { if (alive.current && sequence === request.current) setError(messageFrom(cause)); });
    return () => { alive.current = false; request.current++; };
  }, [project.id]);
  async function change(input: Record<string, unknown>, importSelectedIcon = false) {
    if (pending.current || editingBlocked || !metadata?.metadataRevision) return;
    pending.current = true; setBusy(true); setError(''); setNotice(''); const sequence = ++request.current;
    try {
      const sourcePath = importSelectedIcon ? await invoke<string | null>('choose_path', { kind: 'icon' }) : undefined;
      if (!alive.current || sequence !== request.current || (importSelectedIcon && !sourcePath)) return;
      const value = await invoke<ProjectMetadata>(importSelectedIcon ? 'project_icon_import' : 'project_metadata_set', { projectId: project.id, ...(sourcePath ? { sourcePath } : {}), input: { expectedRevision: metadata.metadataRevision, ...maintenanceInput, ...input } });
      if (!alive.current || sequence !== request.current) return;
      setMetadata(value); setLatest(null);
      if (!importSelectedIcon && Object.hasOwn(input, 'name')) setDraft({ name: value.name, description: value.description });
      setNotice('项目信息已保存，已打开的项目页面不会重新加载。'); await onChanged();
    } catch (cause) {
      if (alive.current && sequence === request.current) {
        setError(messageFrom(cause));
        if (messageFrom(cause).includes('METADATA_CHANGED')) {
          try { const value = await invoke<ProjectMetadata>('project_metadata', { projectId: project.id }); if (alive.current && sequence === request.current) setLatest(value); }
          catch (latestCause) { if (alive.current && sequence === request.current) setError(messageFrom(latestCause)); }
        }
      }
    } finally { if (alive.current && sequence === request.current) { pending.current = false; setBusy(false); } }
  }
  async function readLatest() {
    if (pending.current) return;
    const sequence = ++request.current;
    try { const value = await invoke<ProjectMetadata>('project_metadata', { projectId: project.id }); if (alive.current && sequence === request.current) setLatest(value); }
    catch (cause) { if (alive.current && sequence === request.current) setError(messageFrom(cause)); }
  }
  async function reveal(target: 'root' | 'readme') { try { await invoke('project_reveal', { projectId: project.id, target }); } catch (cause) { if (alive.current) setError(messageFrom(cause)); } }
  const manifest = metadata?.manifest ?? project.manifest;
  const requirements = manifest?.runtimeRequirements;
  const networks = Object.entries(requirements?.network ?? {}).filter(([, values]) => Array.isArray(values)).map(([kind, values]) => [kind, values.filter((item) => item && typeof item.origin === 'string')] as const);
  const devices = Object.entries(requirements?.devices ?? {}).filter(([, value]) => value && typeof value === 'object');
  return <section className="project-overview project-overview-embedded detail-overview" aria-label={`${project.name}项目详情`}>
    {(dismissConfirm || dismissMessage) && <div ref={dismissNotice} tabIndex={-1} className="operation-confirmation overview-dismiss-confirmation" role="group" aria-label="关闭详情确认">
      {dismissConfirm ? <><strong>有尚未保存或提交的内容</strong><p>关闭将放弃基本信息或尚未提交的操作选择。已保存的应用数据不受影响。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="danger-button" disabled={operationPending} onClick={() => { discardApproved.current = true; onClose(); }}>放弃未提交内容并关闭</button></div></> : <><p role="status">{dismissMessage}</p><button className="text-button" onClick={() => setDismissMessage('')}>知道了</button></>}
    </div>}
    <ApplicationDetails actions={actions} handoff={handoff} notices={<>{folderError && <p className="task-error" role="alert">{folderError}</p>}{notices}</>}
      information={<><ApplicationDetailRow label="项目文件夹" action={project.projectRoot ? <button className="text-button" onClick={() => { setFolderError(''); void invoke('open_project_folder', { path: project.projectRoot }).catch(cause => { if (alive.current) setFolderError(messageFrom(cause)); }); }}>打开</button> : undefined}><code>{project.projectRoot}</code></ApplicationDetailRow><ApplicationDetailRow label="占用空间">{bytesText(storage ? partBytes(storage) : project.storage?.currentProject?.totalBytes)}{storage ? storage.complete !== true ? ' · 部分统计' : '' : project.storage?.currentProject?.stale ? ' · 待更新' : ''}</ApplicationDetailRow></>}
      deleteAction={onDelete ? <button className="danger-button" disabled={disabled || operationPending || !status || Boolean(status.activeTask) || status.recoveryRequired} onClick={onDelete}><Trash2 size={15} />停止并删除…</button> : undefined}
      more={<details className="detail-overview-body application-detail-more" open={moreOpen || guardState.current.dirty || morePending} onToggle={event => { if ((guardState.current.dirty || morePending) && !event.currentTarget.open) event.currentTarget.open = true; else { if (event.currentTarget.open) setMoreVisited(true); setMoreOpen(event.currentTarget.open); } }}><summary>更多</summary>
      <section className="detail-information">
        <div className="detail-section-heading"><h3>应用信息</h3><button className="text-button" aria-expanded={editing} onClick={() => setEditing(value => !value)}>{editing ? '收起编辑' : '编辑信息'}<ChevronRight size={16} /></button></div>
        <p className="detail-description">{metadata?.description || project.description || '尚未填写简介。可以编辑信息，补充这个应用的用途。'}</p>
        <form hidden={!editing} className="detail-metadata-form overview-section" onSubmit={(event) => { event.preventDefault(); void change({ name: draft.name, description: draft.description || null }); }}>
          <label>项目名称<input value={draft.name} onChange={(event) => setDraft((value) => ({ ...value, name: event.target.value }))} disabled={busy || !metadata} maxLength={512} /></label>
          <label>简介<textarea value={draft.description} onChange={(event) => setDraft((value) => ({ ...value, description: event.target.value }))} rows={3} disabled={busy || !metadata} maxLength={16384} /></label>
          <div className="metadata-icon"><span className="project-icon">{metadata?.iconDataUrl ? <img src={metadata.iconDataUrl} alt="当前项目图标" /> : <FolderOpen size={26} />}</span><div><strong>{metadata?.iconStatus.state === 'valid' ? '自定义 PNG 图标' : '默认应用图标'}</strong>{metadata?.iconStatus.message && <p>{metadata.iconStatus.message}</p>}<p>静态 PNG，最大 8 MB。<span title="单边不超过 4096 像素，总计不超过 4 百万像素">查看图标要求</span></p></div></div>
          <div className="metadata-actions"><button type="button" className="refresh-button" onClick={() => void change({}, true)} disabled={busy || editingBlocked || !metadata?.metadataRevision}>导入 PNG 图标</button><button type="button" className="text-button" onClick={() => void change({ icon: null })} disabled={busy || editingBlocked || !metadata?.icon}>使用默认图标</button><button className="primary-button" type="submit" disabled={busy || editingBlocked || !metadata?.metadataRevision || !dirty || !draft.name.trim()}><Save size={16} />{busy ? '正在保存…' : '保存基本信息'}</button></div>
          {editingBlocked && <p className="environment-issue">当前操作状态暂不允许保存，编辑内容仍保留。</p>}
        </form>
        {latest && <div className="operation-confirmation"><strong>信息已在其他位置修改，你的编辑仍保留</strong><p>当前名称：{latest.name}</p><p>当前简介：{latest.description || '未填写'}</p><div className="confirmation-actions"><button className="refresh-button" disabled={!latest.metadataRevision} onClick={() => { setMetadata(latest); setLatest(null); setError(''); setNotice('已采用最新版本作为基础，请核对编辑后再次保存。'); }}>保留我的编辑，使用最新版本核对</button><button className="text-button" onClick={() => { setMetadata(latest); setDraft({ name: latest.name, description: latest.description }); setLatest(null); setError(''); }}>放弃我的编辑，使用最新内容</button></div></div>}
        {error && <p role="alert" className="environment-issue">{error} <button className="text-button" onClick={() => void readLatest()}>读取最新信息以核对</button></p>}{notice && <p role="status">{notice}</p>}{metadata?.metadataIssues.map((issue) => <p key={issue} className="environment-issue">{issue}</p>)}
      </section>
      {project.buildInfo?.sourceState === 'modified' && <p className="maintenance-warning">内容已修改，当前应用仍使用上次的构建。完成修改后点击上方“构建更新”。</p>}
      {project.issues.length > 0 && <div className="detail-project-issues">{project.issues.map(issue => <p className="environment-issue" key={issue}>{issue}</p>)}</div>}
      <section className="detail-usage-section"><h3>使用与数据</h3>
        {manifest?.docs?.readme && <button className="detail-document-row" disabled={disabled} onClick={() => void reveal('readme')}><BookOpen size={24} /><span><strong>查看使用说明</strong><small>了解{metadata?.name || project.name}的功能与使用方法。</small></span><ChevronRight size={18} /></button>}
      </section>

      {moreVisited && <ProjectAttachmentBrowser project={project} blocked={disabled || !status || status.mode === 'maintenance' || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final) || busy} onBusyChange={setFileBusy} />}
      <details className="detail-requirements"><summary>联网与设备要求<span>{!devices.length && !networks.some(([, items]) => items.length) ? '未声明额外需求' : '查看用途声明'}<ChevronRight size={16} /></span></summary><p>这些是应用声明的用途；使用摄像头或麦克风时仍需系统授权。</p>{networks.map(([kind, origins]) => <div key={kind}><h4>{({ connect: '网络连接', images: '外部图片', media: '外部音视频' } as Record<string, string>)[kind] ?? kind}</h4>{origins.map((item, index) => <p key={`${item.origin}:${index}`}><code>{item.origin}</code> · {item.purpose || '未说明用途'}{item.dataSent ? ` · 发送数据：${item.dataSent}` : ''}</p>)}</div>)}{devices.map(([device, value]) => <p key={device}>{device === 'camera' ? '摄像头' : device === 'microphone' ? '麦克风' : device} · {value.purpose || '未说明用途'}</p>)}{!devices.length && !networks.some(([, items]) => items.length) && <p>未声明外部连接或设备用途。</p>}</details>
      <details className="detail-technical-information"><summary>详细运行信息</summary><code className="overview-project-id">{project.id}</code><ObjectLayoutPanel layout={project.objectLayout} />{technicalDetails}</details>
      </details>}
    />
  </section>;
}
