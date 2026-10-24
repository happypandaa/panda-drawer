import { invoke } from '@tauri-apps/api/core';
import { BookOpen, ChevronRight, Copy, FolderOpen, HardDrive, Save } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { ProjectOperationRecovery } from './ProjectOperationRecovery';
import { ProjectLifecyclePanel } from './LifecyclePanels';
import { ProjectStoragePanel } from './StoragePanels';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { ProjectGitPanel } from './ProjectGitPanel';
import type { MaintenanceStatus, ProjectInfo, ProjectMetadata, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText, timeText } from './storage-utils';
export type OverviewTab = 'overview' | 'storage' | 'manage';
export function ProjectOverview({ project, instanceId, initialTab, status, task, disabled, onTask, onChanged, onClose, onDismissGuardChange, onDevelopmentStateChange, technicalDetails }: {
  project: ProjectInfo; instanceId?: string; initialTab: OverviewTab; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean;
  onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; onClose: () => void;
  technicalDetails?: ReactNode;
  onDismissGuardChange?: (guard: (() => boolean) | null) => void;
  onDevelopmentStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [tab, setTab] = useState(initialTab);
  const [visited, setVisited] = useState<OverviewTab[]>([initialTab]);
  const [editing, setEditing] = useState(false);
  const [metadata, setMetadata] = useState<ProjectMetadata | null>(null);
  const [draft, setDraft] = useState({ name: '', description: '' });
  const [latest, setLatest] = useState<ProjectMetadata | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [storageDraft, setStorageDraft] = useState({ dirty: false, pending: false });
  const [recoveryPending, setRecoveryPending] = useState(false);
  const [lifecycleDraft, setLifecycleDraft] = useState({ dirty: false, pending: false });
  const dataRecoveryRequired = Boolean(project.buildInfo?.dataRecoveryRequired);
  const [versionsOpen, setVersionsOpen] = useState(dataRecoveryRequired);
  const [versionDraft, setVersionDraft] = useState({ dirty: false, pending: false });
  const completedVersionTasks = useRef(new Set<string>());
  const developmentCallback = useRef(onDevelopmentStateChange); developmentCallback.current = onDevelopmentStateChange;
  const [dismissConfirm, setDismissConfirm] = useState(false);
  const [dismissMessage, setDismissMessage] = useState('');
  const pending = useRef(false);
  const request = useRef(0);
  const alive = useRef(true);
  const discardApproved = useRef(false);
  const dismissNotice = useRef<HTMLDivElement>(null);
  const tabPrefix = useId();
  const editingBlocked = disabled || dataRecoveryRequired || versionDraft.pending || !status || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final) || (status.mode === 'maintenance' && !status.readyForChanges);
  const maintenanceInput = status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {};
  const dirty = Boolean(metadata && (draft.name !== metadata.name || draft.description !== metadata.description));
  const operationPending = busy || storageDraft.pending || recoveryPending || lifecycleDraft.pending || versionDraft.pending;
  const guardState = useRef({ dirty: false, pending: false });
  guardState.current = { dirty: dirty || storageDraft.dirty || lifecycleDraft.dirty || versionDraft.dirty, pending: operationPending };
  useEffect(() => { developmentCallback.current?.(versionDraft); }, [versionDraft]);
  useEffect(() => () => { developmentCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => {
    setTab(initialTab); setVisited((previous) => previous.includes(initialTab) ? previous : [...previous, initialTab]);
  }, [initialTab]);
  useEffect(() => {
    if (dataRecoveryRequired) { setTab('overview'); setVisited(previous => previous.includes('overview') ? previous : [...previous, 'overview']); setVersionsOpen(true); }
  }, [dataRecoveryRequired]);
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
  function selectTab(next: OverviewTab) {
    if (pending.current || guardState.current.pending) { setDismissMessage('请等待当前操作完成，再切换页面。'); return; }
    setTab(next); setVisited((previous) => previous.includes(next) ? previous : [...previous, next]);
    setDismissMessage('');
    document.getElementById(`${tabPrefix}-${next}`)?.focus();
  }
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
  async function copy(text: string) { try { await navigator.clipboard.writeText(text); if (alive.current) setNotice('路径已复制。'); } catch { if (alive.current) setNotice('请选中下方完整路径后复制。'); } }
  const manifest = metadata?.manifest ?? project.manifest;
  const requirements = manifest?.runtimeRequirements;
  const networks = Object.entries(requirements?.network ?? {}).filter(([, values]) => Array.isArray(values)).map(([kind, values]) => [kind, values.filter((item) => item && typeof item.origin === 'string')] as const);
  const devices = Object.entries(requirements?.devices ?? {}).filter(([, value]) => value && typeof value === 'object');
  const tabs = [['overview', '概览'], ['storage', '空间与文件'], ['manage', '更多操作']] as const;
  const stateText = ({ ready: '可正常打开', unbuilt: '等待构建', error: '需要检查', missing: '文件缺失', maintenance: '操作暂停', recoveryRequired: '有操作待处理' })[project.state];
  return <section className="project-overview project-overview-embedded detail-overview" aria-label={`${project.name}项目详情`}>
    {(dismissConfirm || dismissMessage) && <div ref={dismissNotice} tabIndex={-1} className="operation-confirmation overview-dismiss-confirmation" role="group" aria-label="关闭详情确认">
      {dismissConfirm ? <><strong>有尚未保存或提交的内容</strong><p>关闭将放弃基本信息或尚未提交的操作选择。已保存的应用数据不受影响。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="danger-button" disabled={operationPending} onClick={() => { discardApproved.current = true; onClose(); }}>放弃未提交内容并关闭</button></div></> : <><p role="status">{dismissMessage}</p><button className="text-button" onClick={() => setDismissMessage('')}>知道了</button></>}
    </div>}
    <ProjectOperationRecovery key={`${instanceId ?? ''}:${project.id}:${project.projectRoot}`} projectId={project.id} instanceId={instanceId} status={status} task={task} disabled={disabled} onTask={onTask} onChanged={onChanged} onPendingChange={setRecoveryPending} />
    <div className="overview-tabs" role="tablist" aria-label="项目详情">{tabs.map(([key, label], index) => <button key={key} id={`${tabPrefix}-${key}`} role="tab" tabIndex={tab === key ? 0 : -1} aria-selected={tab === key} aria-controls={`${tabPrefix}-${key}-panel`} disabled={operationPending} onKeyDown={(event) => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault(); if (operationPending || pending.current) return;
      const targetIndex = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
      selectTab(tabs[targetIndex][0]);
    }} onClick={() => selectTab(key)}>{label}</button>)}</div>
    <div className="overview-body detail-overview-body" role="tabpanel" id={`${tabPrefix}-overview-panel`} aria-labelledby={`${tabPrefix}-overview`} hidden={tab !== 'overview'}>
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
      <dl className="detail-summary-facts"><div><dt>运行状态</dt><dd className={project.state === 'ready' ? 'detail-ready-text' : ''}>{stateText}</dd></div><div><dt>最近构建</dt><dd>{timeText(project.buildInfo?.provenance?.completedAt)}</dd></div><div><dt>当前应用占用</dt><dd>{bytesText(project.storage?.currentProject?.totalBytes)}{project.storage?.currentProject?.stale ? ' · 待更新' : ''}</dd></div></dl>
      {project.buildInfo?.sourceState === 'modified' && <p className="maintenance-warning">内容已修改，当前应用仍使用上次的构建。完成修改后点击上方“构建更新”。</p>}
      {project.issues.length > 0 && <div className="detail-project-issues">{project.issues.map(issue => <p className="environment-issue" key={issue}>{issue}</p>)}</div>}
      <section className="detail-usage-section"><h3>使用与数据</h3>
        <button className="detail-document-row" disabled={disabled || !manifest?.docs?.readme} onClick={() => void reveal('readme')}><BookOpen size={24} /><span><strong>查看使用说明</strong><small>了解{metadata?.name || project.name}的功能与使用方法。</small></span><ChevronRight size={18} /></button>
        <div className="detail-data-row"><HardDrive size={24} /><span><strong>数据保存在本机</strong><small>业务记录和用户文件独立保存，构建更新不会替换当前业务数据。</small></span></div>
      </section>
      <section className="detail-location-section"><h3>应用位置</h3><div className="detail-location-row"><FolderOpen size={22} /><span><strong>本机项目目录</strong><code>{project.projectRoot}</code></span><button className="icon-button" aria-label="复制项目目录" onClick={() => void copy(project.projectRoot)}><Copy size={20} /></button></div></section>
      {dataRecoveryRequired && <p className="maintenance-warning" role="status">数据取回尚未处理完成。请在下方核对恢复状态，明确继续或撤回后再使用应用。</p>}
      <details className="detail-technical-information" open={versionsOpen || dataRecoveryRequired || versionDraft.dirty || versionDraft.pending} onToggle={event => { if ((dataRecoveryRequired || versionDraft.dirty || versionDraft.pending) && !event.currentTarget.open) event.currentTarget.open = true; else setVersionsOpen(event.currentTarget.open); }}><summary>源码与数据版本</summary>
        {versionsOpen && <ProjectGitPanel projectId={project.id} instanceId={instanceId} task={task} disabled={disabled || busy || storageDraft.pending || recoveryPending || lifecycleDraft.pending} onTask={value => { onTask(value); if (value.final && !completedVersionTasks.current.has(value.taskId)) { completedVersionTasks.current.add(value.taskId); void onChanged(); } }} onDraftStateChange={setVersionDraft} />}
      </details>
      <details className="detail-requirements"><summary>联网与设备要求<span>{!devices.length && !networks.some(([, items]) => items.length) ? '未声明额外需求' : '查看用途声明'}<ChevronRight size={16} /></span></summary><p>这些是应用声明的用途；使用摄像头或麦克风时仍需系统授权。</p>{networks.map(([kind, origins]) => <div key={kind}><h4>{({ connect: '网络连接', images: '外部图片', media: '外部音视频' } as Record<string, string>)[kind] ?? kind}</h4>{origins.map((item, index) => <p key={`${item.origin}:${index}`}><code>{item.origin}</code> · {item.purpose || '未说明用途'}{item.dataSent ? ` · 发送数据：${item.dataSent}` : ''}</p>)}</div>)}{devices.map(([device, value]) => <p key={device}>{device === 'camera' ? '摄像头' : device === 'microphone' ? '麦克风' : device} · {value.purpose || '未说明用途'}</p>)}{!devices.length && !networks.some(([, items]) => items.length) && <p>未声明外部连接或设备用途。</p>}</details>
      <details className="detail-technical-information"><summary>详细运行信息</summary><code className="overview-project-id">{project.id}</code><ObjectLayoutPanel layout={project.objectLayout} />{technicalDetails}</details>
    </div>
    {visited.includes('storage') && <div role="tabpanel" id={`${tabPrefix}-storage-panel`} aria-labelledby={`${tabPrefix}-storage`} hidden={tab !== 'storage'}><ProjectStoragePanel project={project} status={status} task={task} disabled={disabled || versionDraft.dirty || versionDraft.pending} onTask={onTask} onChanged={onChanged} showTask={false} active={tab === 'storage'} onDraftStateChange={setStorageDraft} /></div>}
    {visited.includes('manage') && <div role="tabpanel" id={`${tabPrefix}-manage-panel`} aria-labelledby={`${tabPrefix}-manage`} hidden={tab !== 'manage'}>{dirty && <p className="overview-unsaved-note">概览中有未保存的基本信息。删除应用不会保存这些编辑。</p>}<ProjectLifecyclePanel project={project} instanceId={instanceId} status={status} task={task} disabled={disabled || versionDraft.dirty || versionDraft.pending} onTask={onTask} active={tab === 'manage'} onDraftStateChange={setLifecycleDraft} /></div>}
  </section>;
}
