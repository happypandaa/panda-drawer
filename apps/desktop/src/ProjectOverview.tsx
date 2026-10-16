import { invoke } from '@tauri-apps/api/core';
import { Archive, Copy, FolderOpen, Save, Settings2, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { BackupMaintenancePanel } from './BackupMaintenancePanel';
import { ProjectLifecyclePanel } from './LifecyclePanels';
import { ProjectTask } from './ProjectTask';
import { ProjectStoragePanel } from './StoragePanels';
import type { MaintenanceStatus, ProjectInfo, ProjectMetadata, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText, timeText } from './storage-utils';
export type OverviewTab = 'overview' | 'storage' | 'backup' | 'manage';
export function ProjectOverview({ project, initialTab, status, task, disabled, onTask, onChanged, onHandoff, onClose }: {
  project: ProjectInfo; initialTab: OverviewTab; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean;
  onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; onHandoff: () => void; onClose: () => void;
}) {
  const [tab, setTab] = useState(initialTab);
  useEffect(() => { setTab(initialTab); }, [initialTab]);
  const [metadata, setMetadata] = useState<ProjectMetadata | null>(null);
  const [draft, setDraft] = useState({ name: '', description: '' });
  const [latest, setLatest] = useState<ProjectMetadata | null>(null);
  const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  const panel = useRef<HTMLElement | null>(null);
  useEffect(() => { panel.current?.scrollIntoView({ block: 'start', behavior: 'smooth' }); }, []);
  const pending = useRef(false); const request = useRef(0);
  const editingBlocked = disabled || !status || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final) || (status.mode === 'maintenance' && !status.readyForChanges);
  const maintenanceInput = status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {};
  const dirty = metadata && (draft.name !== metadata.name || draft.description !== metadata.description);
  useEffect(() => { let live = true; void invoke<ProjectMetadata>('project_metadata', { projectId: project.id }).then((value) => { if (live) { setMetadata(value); setDraft({ name: value.name, description: value.description }); } }).catch((cause) => { if (live) setError(messageFrom(cause)); }); return () => { live = false; request.current++; }; }, [project.id]);
  async function change(input: Record<string, unknown>, sourcePath?: string) {
    if (pending.current || editingBlocked || !metadata?.metadataRevision) return;
    pending.current = true; setBusy(true); setError(''); setNotice(''); const sequence = ++request.current;
    try {
      const value = await invoke<ProjectMetadata>(sourcePath ? 'project_icon_import' : 'project_metadata_set', { projectId: project.id, ...(sourcePath ? { sourcePath } : {}), input: { expectedRevision: metadata.metadataRevision, ...maintenanceInput, ...input } });
      if (sequence !== request.current) return;
      setMetadata(value); setLatest(null); if (!sourcePath && Object.hasOwn(input, 'name')) setDraft({ name: value.name, description: value.description });
      setNotice('项目信息已保存。正在使用的项目页面保留当前内容。'); await onChanged();
    } catch (cause) { if (sequence === request.current) { setError(messageFrom(cause)); if (messageFrom(cause).includes('METADATA_CHANGED')) await readLatest(); } }
    finally { pending.current = false; if (sequence === request.current) setBusy(false); }
  }
  async function readLatest() { try { setLatest(await invoke<ProjectMetadata>('project_metadata', { projectId: project.id })); } catch (cause) { setError(messageFrom(cause)); } }
  async function selectIcon() { try { const sourcePath = await invoke<string | null>('choose_path', { kind: 'icon' }); if (sourcePath) await change({}, sourcePath); } catch (cause) { setError(messageFrom(cause)); } }
  async function reveal(target: 'root' | 'readme') { try { await invoke('project_reveal', { projectId: project.id, target }); } catch (cause) { setError(messageFrom(cause)); } }
  async function copy(text: string) { try { await navigator.clipboard.writeText(text); setNotice('路径已复制。'); } catch { setNotice('请选中下方完整路径后复制。'); } }
  const manifest = metadata?.manifest ?? project.manifest;
  const requirements = manifest?.runtimeRequirements;
  const networks = Object.entries(requirements?.network ?? {}).filter(([, values]) => Array.isArray(values)).map(([kind, values]) => [kind, values.filter((item) => item && typeof item.origin === 'string')] as const);
  const devices = Object.entries(requirements?.devices ?? {}).filter(([, value]) => value && typeof value === 'object');
  return <section ref={panel} className="project-overview" aria-label={`${project.name}项目管理`}>
    <div className="overview-heading"><div className="overview-identity"><span className="project-icon">{metadata?.iconDataUrl ? <img src={metadata.iconDataUrl} alt="" /> : <FolderOpen size={26} />}</span><div><h2>{metadata?.name ?? project.name}</h2><code>{project.id}</code></div></div><button className="icon-button" aria-label="关闭项目管理" onClick={onClose}><X size={18} /></button></div>
    <div className="overview-tabs" role="tablist" aria-label="项目设置">{([['overview', '概览'], ['storage', '空间与素材'], ['backup', '备份与维护'], ['manage', '导出与管理']] as const).map(([key, label]) => <button key={key} role="tab" aria-selected={tab === key} onClick={() => setTab(key)}>{label}</button>)}</div>
    {tab === 'overview' && <div className="overview-body">
      <div className="overview-status-grid"><div><span>运行状态</span><strong>{({ ready: '可打开', unbuilt: '等待构建', error: '需要检查', missing: '文件缺失', maintenance: '维护中', recoveryRequired: '恢复待处理' })[project.state]}</strong></div><div><span>维护状态</span><strong>{!status ? '尚未读取' : status.recoveryRequired ? '恢复待处理' : status.mode === 'maintenance' ? status.readyForChanges ? '维护中 · 可修改' : '维护中 · 请等待基线' : '正常运行'}</strong></div><div><span>当前任务</span><strong>{task && !task.final ? `${task.type} · ${task.phase}` : '无运行中任务'}</strong></div></div>
      <section className="overview-section"><h3>基本信息</h3><p>名称和简介帮助你区分项目；同名项目仍有独立 ID 和数据。</p>
        <form onSubmit={(event) => { event.preventDefault(); void change({ name: draft.name, description: draft.description || null }); }}>
          <label>项目名称<input value={draft.name} onChange={(event) => setDraft((value) => ({ ...value, name: event.target.value }))} disabled={busy || !metadata} maxLength={512} /></label>
          <label>简介<textarea value={draft.description} onChange={(event) => setDraft((value) => ({ ...value, description: event.target.value }))} rows={3} disabled={busy || !metadata} maxLength={16384} /></label>
          <div className="metadata-icon"><span className="project-icon">{metadata?.iconDataUrl ? <img src={metadata.iconDataUrl} alt="当前项目图标" /> : <FolderOpen size={26} />}</span><div><strong>{metadata?.iconStatus.state === 'valid' ? '自定义 PNG 图标' : '默认项目图标'}</strong><p>{metadata?.iconStatus.message ?? '选择本机静态 PNG。图标保存在项目 meta/ 目录。'}</p><p>内部测试限制：8 MB、单边 4096 像素、总计 4 百万像素；不支持动画。</p></div></div>
          <div className="metadata-actions"><button type="button" className="refresh-button" onClick={() => void selectIcon()} disabled={busy || editingBlocked || !metadata?.metadataRevision}>导入 PNG 图标</button><button type="button" className="text-button" onClick={() => void change({ icon: null })} disabled={busy || editingBlocked || !metadata?.icon}>使用默认图标</button><button className="primary-button" type="submit" disabled={busy || editingBlocked || !metadata?.metadataRevision || !dirty || !draft.name.trim()}><Save size={14} />{busy ? '正在保存…' : '保存基本信息'}</button></div>
        </form>
        {editingBlocked && <p className="environment-issue">当前任务、维护准备或工作区状态暂不允许保存。编辑内容仍保留。</p>}
        {latest && <div className="operation-confirmation"><strong>信息已在其他位置修改，你的编辑仍保留</strong><p>当前名称：{latest.name}</p><p>当前简介：{latest.description || '未填写'}</p><div className="confirmation-actions"><button className="refresh-button" disabled={!latest.metadataRevision} onClick={() => { setMetadata(latest); setLatest(null); setError(''); setNotice('已采用最新版本作为基础，请核对编辑后再次保存。'); }}>保留我的编辑，使用最新版本核对</button><button className="text-button" onClick={() => { setMetadata(latest); setDraft({ name: latest.name, description: latest.description }); setLatest(null); setError(''); }}>放弃我的编辑，使用最新内容</button></div></div>}
        {error && <p role="alert" className="environment-issue">{error} <button className="text-button" onClick={() => void readLatest()}>读取最新信息以核对</button></p>}{notice && <p role="status">{notice}</p>}{metadata?.metadataIssues.map((issue) => <p key={issue} className="environment-issue">{issue}</p>)}
      </section>
      <section className="overview-section"><h3>项目位置与 AI 入口</h3>{([['root', '项目目录', project.projectRoot], ['readme', 'README', manifest?.docs?.readme ? `${project.projectRoot}/${manifest.docs.readme}` : 'README 路径不可用']] as const).map(([target, label, path]) => <div className="overview-path" key={target}><span>{label}</span><code>{path}</code><button className="text-button" onClick={() => void copy(path)}><Copy size={13} />复制路径</button><button className="text-button" onClick={() => void reveal(target)}><FolderOpen size={13} />在 Finder 显示</button></div>)}<button className="refresh-button" onClick={onHandoff}><Copy size={14} />复制给 AI 的完整说明</button></section>
      <section className="overview-section"><h3>构建与来源核对</h3><p>{({ unbuilt: '尚未构建', matched: '源码与当前网页一致', modified: '源码已修改，当前网页仍为旧版', unknown: '构建来源未知' })[project.buildInfo?.sourceState ?? 'unknown']}{project.buildInfo?.stale ? ' · 需要重新核对' : ''}</p><p>核对时间：{timeText(project.buildInfo?.checkedAt)} · 最近成功构建：{timeText(project.buildInfo?.provenance?.completedAt)}</p><p>{project.buildInfo?.reason ?? '平台不会自动重新加载已打开的项目页面。'}</p>{project.issues.map((issue) => <p key={issue} className="environment-issue">{issue}</p>)}</section>
      <section className="overview-section"><h3>声明的外部连接与设备用途</h3><p>这些是项目清单中的用途声明。使用摄像头或麦克风时，仍需浏览器或系统授权。</p>{networks.map(([kind, origins]) => <div key={kind}><h4>{({ connect: '网络连接', images: '外部图片', media: '外部音视频' } as Record<string, string>)[kind] ?? kind}</h4>{origins.map((item, index) => <p key={`${item.origin}:${index}`}><code>{item.origin}</code> · {typeof item.purpose === 'string' ? item.purpose : '未说明用途'}{item.dataSent ? ` · 发送数据：${item.dataSent}` : ''}</p>)}</div>)}{devices.map(([device, value]) => <p key={device}>{device === 'camera' ? '摄像头' : device === 'microphone' ? '麦克风' : device} · {typeof value.purpose === 'string' ? value.purpose : '未说明用途'}</p>)}{!devices.length && !networks.some(([, items]) => items.length) && <p>未声明外部连接或设备用途。</p>}</section>
      <section className="overview-section"><h3>空间摘要</h3><p>{bytesText(project.storage?.currentProject.totalBytes)} · {project.storage?.currentProject.complete ? '扫描完整' : '尚未统计或部分统计'}{project.storage?.currentProject.stale ? ' · 需要重新扫描' : ''}</p><p>扫描时间：{timeText(project.storage?.currentProject.completedAt)}</p></section><div className="metadata-actions"><button className="refresh-button" onClick={() => setTab('storage')}><FolderOpen size={14} />查看空间与素材</button><button className="refresh-button" onClick={() => setTab('backup')}><Archive size={14} />备份与维护</button><button className="refresh-button" onClick={() => setTab('manage')}><Settings2 size={14} />导出、复制与回收站</button></div>
      {task && <ProjectTask task={task} />}
    </div>}
    {tab === 'storage' && <ProjectStoragePanel project={project} status={status} task={task} disabled={disabled} onTask={onTask} onChanged={onChanged} />}
    {tab === 'backup' && <BackupMaintenancePanel project={project} status={status} task={task} disabled={disabled} onTask={onTask} onChanged={onChanged} onClose={onClose} />}
    {tab === 'manage' && <ProjectLifecyclePanel project={project} status={status} disabled={disabled} onTask={onTask} onClose={onClose} />}
  </section>;
}
