import { invoke } from '@tauri-apps/api/core';
import { Archive, ArrowRight, ChevronRight, Download, Eye, PanelsTopLeft, RefreshCw, Search, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import type { MaintenanceStatus, ProjectInfo, ProjectStorage, StorageFiles, StoragePart, StoredFile, TaskRecord } from './types';
import { bytesText, partBytes, timeText, percentOf, exactBytes } from './storage-utils';
import { messageFrom } from './ui-utils';
import { spaceOverview } from './workspace-space-model';
import type { WorkspaceStorageSnapshot } from './workspace-space-model';
const categoryNames: Record<string, string> = { source: '源码与项目资料', output: '网页产物', attachments: '用户素材', database: '数据库', dependencies: '开发依赖', cache: '缓存与日志', other: '其他文件' };
function issueText(issue: unknown) { if (typeof issue === 'object' && issue) { const data = issue as Record<string, unknown>; return [data.path, data.message ?? data.reason ?? data.code].filter(Boolean).join(' · '); } return String(issue); }
function Issues({ items }: { items?: unknown[] }) { return items?.length ? <details className="storage-issues"><summary>{items.length} 项未能完整统计 / 需要检查</summary>{items.map((issue, index) => <p key={index}>{issueText(issue)}</p>)}</details> : null; }
export function ProjectStoragePanel({ project, status, task, disabled, onTask, onChanged, showTask = true }: { project: ProjectInfo; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean; onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; showTask?: boolean }) {
  const [info, setInfo] = useState<ProjectStorage | null>(null); const [error, setError] = useState(''); const [busy, setBusy] = useState(false);
  const [historyTask, setHistoryTask] = useState<TaskRecord | null>(null);
  const [clean, setClean] = useState<string[]>([]); const [confirm, setConfirm] = useState(false); const [ack, setAck] = useState(false); const pending = useRef(false);
  const onTaskRef = useRef(onTask); onTaskRef.current = onTask; const taskRef = useRef(task); taskRef.current = task;
  const read = useCallback(async () => { try { const value = await invoke<ProjectStorage>('storage_info', { projectId: project.id }); setInfo(value); if (value.activeTask && typeof value.activeTask === 'object' && 'status' in value.activeTask && (!taskRef.current || taskRef.current.final || taskRef.current.type === 'storageScan')) onTaskRef.current(value.activeTask as TaskRecord); } catch (cause) { setError(messageFrom(cause)); } }, [project.id]);
  useEffect(() => { void read(); }, [read, task?.status]);
  const operationBlocked = disabled || busy || Boolean(task && !task.final) || Boolean(status?.activeTask) || !status || status.recoveryRequired;
  const cleanupBlocked = operationBlocked || status?.mode !== 'maintenance' || !status.readyForChanges;
  async function run(command: 'storage_scan' | 'storage_cleanup') {
    if (pending.current || operationBlocked || command === 'storage_cleanup' && cleanupBlocked) return;
    pending.current = true; setBusy(true); setError('');
    try { const accepted = await invoke<TaskRecord>(command, { projectId: project.id, input: command === 'storage_cleanup' ? { categories: clean, maintenanceId: status?.maintenanceId } : {} }); onTask(accepted); setConfirm(false); setAck(false); await read(); await onChanged(); }
    catch (cause) { setError(messageFrom(cause)); } finally { pending.current = false; setBusy(false); }
  }
  const current = info?.currentProject;
  async function showHistoryTask(taskId: string) { try { setHistoryTask(await invoke<TaskRecord>('task_get', { taskId })); } catch (cause) { setError(messageFrom(cause)); } }
  const history = info?.relatedHistory as { entries?: StoragePart[]; issues?: unknown[] } | undefined;
  return <div className="overview-body">
    <div className="storage-heading"><div><h3>当前项目空间</h3><p>按普通文件的逻辑字节统计，使用十进制 KB / MB / GB；悬停数字可查看精确字节。扫描期间文件可能变化，结果不是实时物理占用。</p></div><button className="refresh-button" onClick={() => void run('storage_scan')} disabled={operationBlocked}><RefreshCw size={14} />{busy ? '提交中…' : '重新扫描'}</button></div>
    <div className="storage-total"><strong title={exactBytes(current?.totalBytes)}>{bytesText(current?.totalBytes)}</strong><span>{!current || current.status === 'not_scanned' ? '尚未扫描' : `${current.complete ? '扫描完整' : '部分统计'}${current.stale ? ' · 结果已过期，请重新扫描' : ''}`}</span></div>
    {current?.completedAt && <p className="storage-timestamp">扫描 {timeText(current.startedAt)} 至 {timeText(current.completedAt)}</p>}
    <div className="storage-category-grid">{Object.entries(categoryNames).map(([key, label]) => { const category = current?.categories?.[key]; const count = category?.fileCount; const countLabel = typeof count === 'number' ? count : count && typeof count === 'object' && '$int64' in count ? String(count.$int64) : null; const percent = current?.complete && !current.stale ? percentOf(partBytes(category), current.totalBytes) : null; return <div className="storage-category" key={key}><span>{label}</span><strong title={exactBytes(partBytes(category))}>{bytesText(partBytes(category))}</strong>{countLabel !== null && <small>{countLabel} 个文件</small>}{percent !== null && <><small>{percent.toFixed(1)}%</small><span className="storage-ratio" aria-label={`${label}占比${percent.toFixed(1)}%`}><span style={{ width: `${percent}%` }} /></span></>}{key === 'source' && <small>固定素材 {bytesText(partBytes(category?.fixedAssets as StoragePart | undefined))} · 已包含在源码中</small>}{key === 'database' && <small>主库 {bytesText(partBytes(category?.main as StoragePart | undefined))} · 伴随文件 {bytesText(partBytes(category?.companions as StoragePart | undefined))}</small>}</div>; })}</div><Issues items={current?.issues} />
    <section className="overview-section"><h3>关联历史占用</h3><p>备份与操作保留现场单独展示，不计入当前项目大小，也不会通过下方空间清理删除。</p>{history?.entries?.length ? history.entries.map((entry, index) => <div className="history-storage-row" key={index}><span>{String((entry.scope as Record<string, unknown> | undefined)?.kind ?? '历史文件')}</span><strong title={exactBytes(partBytes(entry))}>{bytesText(partBytes(entry))}</strong><code>{String(entry.root ?? entry.path ?? '')}</code><small>{typeof entry.taskId === 'string' && <button className="text-button" disabled={operationBlocked} onClick={() => void showHistoryTask(entry.taskId as string)}>查看关联任务 {entry.taskId}</button>}</small><small>{String(entry.protectionReason ?? '受保护的历史文件')}</small><Issues items={entry.issues} /></div>) : <p>{info ? '未发现已关联的历史占用记录。' : '尚未读取。'}</p>}<Issues items={history?.issues} />{historyTask && <ProjectTask task={historyTask} />}</section>
    <AttachmentBrowser project={project} blocked={disabled || !status || status.mode === 'maintenance' || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final)} />
    <section className="overview-section"><h3>清理可再生成文件</h3><p>需要先在“备份与维护”进入维护并准备好基线。只清理明确选择的依赖、构建缓存或日志；保留业务数据库、素材、源码和当前网页。</p>
      <div className="cleanup-options">{[['dependencies', '项目依赖（之后构建会重新安装）'], ['buildCache', '构建缓存'], ['logs', '项目构建日志']].map(([key, label]) => <label key={key}><input type="checkbox" checked={clean.includes(key)} disabled={busy} onChange={(event) => { setClean((value) => event.target.checked ? [...value, key] : value.filter((item) => item !== key)); setConfirm(false); }} />{label}</label>)}</div>
      <button className="refresh-button" onClick={() => { setConfirm(true); setAck(false); }} disabled={cleanupBlocked || !clean.length}>检查清理范围</button>
      {confirm && <div className="operation-confirmation"><strong>清理「{project.name}」的所选文件</strong><p>项目 ID：{project.id}</p><p>范围：{clean.map((key) => ({ dependencies: '项目依赖', buildCache: '构建缓存', logs: '项目构建日志' }[key])).join('、')}。依赖和缓存之后可重新生成，日志删除后无法恢复。最终结果显示实际删除字节，文件系统可用空间变化可能不同。</p><label className="checkbox-label"><input type="checkbox" checked={ack} onChange={(event) => setAck(event.target.checked)} />我已核对项目和清理范围</label><div className="confirmation-actions"><button className="danger-button" disabled={!ack || cleanupBlocked} onClick={() => void run('storage_cleanup')}>确认清理</button><button className="text-button" onClick={() => setConfirm(false)}>取消</button></div></div>}
      {showTask && task && ['storageScan', 'storageCleanup'].includes(task.type) && <ProjectTask task={task} />}
    </section>{error && <p role="alert" className="environment-issue">{error}</p>}
  </div>;
}
function AttachmentBrowser({ project, blocked }: { project: ProjectInfo; blocked: boolean }) {
  const [filters, setFilters] = useState({ type: 'all', prefix: '', query: '', sort: 'name', order: 'asc' });
  const [applied, setApplied] = useState(filters); const [result, setResult] = useState<StorageFiles | null>(null); const [loading, setLoading] = useState(false); const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  const [preview, setPreview] = useState<{ path: string; dataUrl: string; mimeType: string } | null>(null); const request = useRef(0); const previewRequest = useRef(0); const pending = useRef(false);
  const list = useCallback(async (cursor?: string) => { const sequence = ++request.current; setLoading(true); setError(''); try { const value = await invoke<StorageFiles>('storage_files', { projectId: project.id, input: { ...applied, type: applied.type === 'all' ? undefined : applied.type, limit: 40, ...(cursor ? { cursor } : {}) } }); if (sequence === request.current) setResult((previous) => ({ ...value, files: cursor ? [...(previous?.files ?? []), ...value.files] : value.files })); } catch (cause) { if (sequence === request.current) setError(messageFrom(cause)); } finally { if (sequence === request.current) setLoading(false); } }, [project.id, applied]);
  useEffect(() => { setResult(null); setPreview(null); previewRequest.current++; void list(); return () => { request.current++; previewRequest.current++; }; }, [list]);
  useEffect(() => { if (blocked) { previewRequest.current++; setPreview(null); } }, [blocked, project.documentEpoch]);
  async function access(file: StoredFile, kind: 'preview' | 'download') {
    if (blocked || pending.current) return; pending.current = true; setError(''); setNotice(''); const sequence = ++previewRequest.current;
    try { if (kind === 'preview') { const value = await invoke<{ dataUrl: string; mimeType: string }>('storage_preview', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch }); if (sequence !== previewRequest.current) return; if (!/^(image\/(png|jpeg|gif|webp)|audio\/(mpeg|wav|mp4|ogg|webm)|video\/(mp4|webm))$/.test(value.mimeType) || !value.dataUrl.startsWith(`data:${value.mimeType};base64,`)) throw new Error('此格式不支持安全预览，请下载文件。'); setPreview({ path: file.path, ...value }); } else { const value = await invoke<{ savedPath: string } | null>('storage_download', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch }); if (value) setNotice(`已保存到 ${value.savedPath}`); } } catch (cause) { if (sequence === previewRequest.current) setError(messageFrom(cause)); } finally { pending.current = false; }
  }
  return <section className="overview-section"><h3>用户素材</h3><p>只浏览项目 attachmentsDir 范围。预览内部上限 16 MB；其他文档和较大的文件可下载。素材不在此处删除。</p><form className="attachment-filters" onSubmit={(event) => { event.preventDefault(); setApplied({ ...filters }); }}>
    <label>类型<select value={filters.type} onChange={(e) => setFilters({ ...filters, type: e.target.value })}>{[['all', '全部'], ['image', '图片'], ['audio', '音频'], ['video', '视频'], ['document', '文档'], ['other', '其他']].map(([v, label]) => <option key={v} value={v}>{label}</option>)}</select></label>
    <label>目录前缀<input placeholder="例如 lessons/" value={filters.prefix} onChange={(e) => setFilters({ ...filters, prefix: e.target.value })} /></label><label>名称搜索<input value={filters.query} onChange={(e) => setFilters({ ...filters, query: e.target.value })} /></label>
    <label>排序<select value={filters.sort} onChange={(e) => setFilters({ ...filters, sort: e.target.value })}><option value="name">名称</option><option value="size">大小</option><option value="modifiedAt">修改时间</option></select></label><label>顺序<select value={filters.order} onChange={(e) => setFilters({ ...filters, order: e.target.value })}><option value="asc">升序</option><option value="desc">降序</option></select></label><button className="refresh-button" disabled={loading}><Search size={13} />查询</button>
  </form>{blocked && <p className="environment-issue">项目维护、任务或工作区状态暂不允许读取素材内容；预览已关闭。</p>}
    <p className="storage-timestamp">已载入 {result?.files.length ?? 0} 个文件{result?.nextCursor ? ' · 还有下一页' : ''}。列表为当前筛选条件下的分页结果。</p><div className="attachment-list">{result?.files.map((file) => <div className="attachment-row" key={file.path}><div><code>{file.path}</code><span title={exactBytes(file.sizeBytes ?? file.size)}>{file.displayType ?? file.mediaType ?? file.mimeType ?? file.contentType ?? '文件'} · {bytesText(file.sizeBytes ?? file.size)} · {timeText(file.modifiedAt)}</span></div><div><button className="text-button" disabled={blocked} onClick={() => void access(file, 'preview')}><Eye size={13} />预览</button><button className="text-button" disabled={blocked} onClick={() => void access(file, 'download')}><Download size={13} />下载</button></div></div>)}</div>
    {!loading && result && !result.files.length && <p>当前条件下没有素材文件。</p>}{loading && <p role="status">正在读取素材列表…</p>}{result?.nextCursor && <button className="refresh-button" disabled={loading} onClick={() => void list(result.nextCursor!)}>继续读取下一页</button>}{result && <p className="storage-timestamp">{result.complete ? '本次枚举完整' : '部分结果'} · {timeText(result.completedAt)}</p>}<Issues items={result?.issues} />
    {preview && <div className="attachment-preview"><div><strong>{preview.path}</strong><button className="icon-button" onClick={() => { previewRequest.current++; setPreview(null); }} aria-label="关闭素材预览"><X size={16} /></button></div>{preview.mimeType.startsWith('image/') ? <img src={preview.dataUrl} alt={preview.path} /> : preview.mimeType.startsWith('audio/') ? <audio controls src={preview.dataUrl} /> : <video controls src={preview.dataUrl} />}</div>}{error && <p role="alert" className="environment-issue">{error}</p>}{notice && <p role="status">{notice}</p>}
  </section>;
}
export function WorkspaceStorageSummary({ revision, enabled, projects, onSelect, onCleanup }: { revision: number; enabled: boolean; projects?: ProjectInfo[]; onSelect?: (projectId: string) => void; onCleanup?: () => void }) {
  const [data, setData] = useState<WorkspaceStorageSnapshot | null>(null); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const request = useRef(0);
  const read = useCallback(async () => { if (!enabled) return; const sequence = ++request.current; setBusy(true); try { const value = await invoke<WorkspaceStorageSnapshot>('workspace_storage'); if (sequence === request.current) { setData(value); setError(''); } } catch (cause) { if (sequence === request.current) setError(messageFrom(cause)); } finally { if (sequence === request.current) setBusy(false); } }, [enabled]);
  useEffect(() => { void read(); return () => { request.current++; }; }, [read, revision]);
  const overview = spaceOverview(data);
  const scannedProjects = Array.isArray(data?.workspace.projects) ? data.workspace.projects as StoragePart[] : [];
  const rankedProjects = projects?.map((project) => {
    const scanned = scannedProjects.find((part) => (part.scope as { projectId?: string } | undefined)?.projectId === project.id);
    return scanned ? { ...project, storage: { ...project.storage, currentProject: scanned } } : project;
  });
  return <>
    <section className="lifecycle-card workspace-summary-card space-summary" aria-labelledby="workspace-summary-title" aria-busy={busy}>
      <div className="space-summary-top">
        <div><h2 id="workspace-summary-title">小抽屉占用</h2><div className="space-total"><strong className="space-total-value" title={exactBytes(overview.total)}>{data ? bytesText(overview.total) : busy ? '统计中…' : '尚未统计'}</strong>{overview.total !== null && !overview.complete && <span className="space-status">已统计</span>}{overview.stale && <span className="space-status">需要更新</span>}</div></div>
        <div className="space-summary-actions"><button className="refresh-button" disabled={!enabled || busy} onClick={() => void read()}><RefreshCw size={14} />{busy ? '统计中…' : '刷新'}</button>{onCleanup && <button className="refresh-button" onClick={onCleanup}><Archive size={14} />检查历史文件<ArrowRight size={14} /></button>}</div>
      </div>
      {!error && overview.barReady && <div className="space-usage-bar" role="img" aria-label="各类文件占用比例">{overview.groups.map(({ id, label, part }) => <span key={id} className={`space-color-${id}`} style={{ flexGrow: percentOf(partBytes(part), overview.total) ?? 0 }} title={`${label} ${bytesText(partBytes(part))}`} />)}</div>}
      {!!overview.groups.length && <dl className="space-legend">{overview.groups.map(({ id, label, part }) => <div key={id}><dt><i className={`space-dot space-color-${id}`} aria-hidden="true" />{label}</dt><dd title={exactBytes(partBytes(part))}>{bytesText(partBytes(part))}{(part.complete === false || part.stale === true) && <small>{part.stale ? '需要更新' : '部分统计'}</small>}</dd></div>)}</dl>}
      {error && <p role="alert" className="environment-issue">{data ? '更新失败，显示上次统计。' : '暂时无法统计。'}<span className="space-error-detail">{error}</span></p>}
      {data && !overview.complete && <p className="space-summary-footer">部分位置尚未统计完整</p>}
      <details className="workspace-disclosure space-details"><summary><span>明细与说明</span>{overview.updatedAt && <small>最近统计 {timeText(overview.updatedAt)}</small>}</summary><div className="workspace-disclosure-body">
        <p>网页项目包含源码、依赖和数据；已安装应用与设置包含 Node 应用。</p>
        <p>系统废纸篓不计入这里。文件大小不等于可释放的磁盘空间。</p>
        {data ? [data.platformProgram, data.workspace, ...data.otherLocalLocations].map((part, index) => <div className="workspace-location-summary" key={String(part.root ?? index)}><div className="space-scope-heading"><h3>{scopeName(String((part.scope as Record<string, unknown> | undefined)?.kind ?? (index === 0 ? 'platformProgram' : index === 1 ? 'workspace' : '')))}</h3><strong title={exactBytes(partBytes(part))}>{bytesText(partBytes(part))}</strong></div><small>{scopeStatus(part)}</small><ScopeDetails part={part} /></div>) : <p>尚未统计。</p>}
        {data?.disks?.map((disk, index) => <div className="workspace-disk-row" key={index}><code>{String(disk.exampleRoot ?? disk.path ?? disk.root ?? disk.mountPoint ?? '所在磁盘')}</code><span>磁盘可用 {bytesText((disk.availableBytes ?? null) as import('./types').ByteCount)}</span></div>)}
      </div></details>
    </section>
    {rankedProjects && onSelect && <ProjectSpaceRanking projects={rankedProjects} onSelect={onSelect} />}
  </>;
}
function scopeStatus(part?: StoragePart) { return part ? `${part.complete ? '统计完整' : '部分统计 / 未知'}${part.stale ? ' · 需要更新' : ''}` : '尚未统计'; }
function scopeName(name: string) { return ({ applicationData: '已安装应用与设置', retainedWorkspace: '迁移保留目录', platformProgram: '软件本体', workspace: '当前工作区' } as Record<string, string>)[name] ?? '其他本地文件'; }
function ScopeDetails({ part }: { part?: StoragePart }) {
  const categories = part?.categories as Record<string, StoragePart> | undefined;
  return <>
    <code className="workspace-scope-path">{String(part?.root ?? part?.path ?? '')}</code>
    {Boolean(part?.completedAt) && <small className="storage-timestamp">扫描时间：{timeText(part!.completedAt as string | number)}</small>}
    <Issues items={part?.issues} />
    {categories && <>
      <dl className="workspace-storage-breakdown">{Object.entries(categories).map(([name, item]) => <div key={name}><dt>{({ projects: '网页项目文件', backups: '历史备份', trash: '历史删除资料与记录', operations: '操作现场', sharedCache: '共享缓存', control: '平台控制记录', metadata: '平台元数据', logs: '日志', other: '其他', applicationData: '应用数据', platformProgram: '程序文件', retainedWorkspace: '保留文件' } as Record<string, string>)[name] ?? name}</dt><dd title={exactBytes(partBytes(item))}>{bytesText(partBytes(item))}</dd></div>)}</dl>
    </>}
  </>;
}
export function ProjectSpaceRanking({ projects, onSelect }: { projects: ProjectInfo[]; onSelect: (projectId: string) => void }) {
  const [expanded, setExpanded] = useState(false);
  const numeric = (project: ProjectInfo) => { const value = project.storage?.currentProject?.totalBytes; try { return value == null ? null : typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); } catch { return null; } };
  const ordered = [...projects].sort((a, b) => { const left = numeric(a); const right = numeric(b); return left === right ? a.name.localeCompare(b.name) : left === null ? 1 : right === null ? -1 : left > right ? -1 : 1; });
  const visible = expanded ? ordered : ordered.slice(0, 5);
  return <section className="lifecycle-card workspace-project-ranking"><div className="space-ranking-heading"><h2>网页项目占用</h2><span>{projects.length} 个项目</span></div><div className="project-size-list">{visible.map((project) => <button className="project-size-row" key={project.id} onClick={() => onSelect(project.id)}><PanelsTopLeft size={18} aria-hidden="true" /><span className="space-project-name"><strong>{project.name}</strong></span><span className="space-project-size" title={exactBytes(project.storage?.currentProject?.totalBytes)}><strong>{bytesText(project.storage?.currentProject?.totalBytes)}</strong>{project.storage?.currentProject?.stale ? <small>需要更新</small> : project.storage?.currentProject?.complete === false ? <small>部分统计</small> : null}</span><ChevronRight size={16} aria-hidden="true" /></button>)}</div>{ordered.length > 5 && <button className="text-button workspace-ranking-toggle" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? '收起' : `查看全部 ${ordered.length} 个项目`}</button>}{!projects.length && <p className="space-empty">还没有网页项目</p>}</section>;
}
