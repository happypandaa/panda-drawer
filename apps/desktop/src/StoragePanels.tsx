import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, Download, Eye, File, FileAudio, FileImage, FileVideo, HardDrive, RefreshCw, Search, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import type { ByteCount, MaintenanceStatus, ProjectInfo, ProjectStorage, StorageFiles, StoragePart, StoredFile, TaskRecord } from './types';
import { bytesText, partBytes, timeText, percentOf, exactBytes } from './storage-utils';
import { messageFrom } from './ui-utils';

const storageCategories = [
  { key: 'source', label: '源码与资料', description: '代码、项目说明和开发资料' },
  { key: 'output', label: '应用文件', description: '当前可运行的应用文件' },
  { key: 'attachments', label: '用户文件', description: '你保存或上传的图片、文档等' },
  { key: 'database', label: '数据库', description: '应用内保存的业务记录' },
  { key: 'dependencies', label: '开发依赖', description: '构建应用时使用的开发工具包' },
  { key: 'cache', label: '缓存与日志', description: '可再生成的构建缓存及日志' },
  { key: 'other', label: '其他文件', description: '暂未归入以上类别的文件' },
];
const cleanupCategories = [
  ['dependencies', '开发依赖', '之后构建会重新安装'],
  ['buildCache', '构建缓存', '之后构建可重新生成'],
  ['logs', '可清理的任务日志', '删除后无法恢复'],
];
function issueText(issue: unknown) { if (typeof issue === 'object' && issue) { const data = issue as Record<string, unknown>; return [data.path, data.message ?? data.reason ?? data.code].filter(Boolean).join(' · '); } return String(issue); }
function Issues({ items }: { items?: unknown[] }) { return items?.length ? <details className="storage-issues"><summary>{items.length} 项未能完整统计 / 需要检查</summary>{items.map((issue, index) => <p key={index}>{issueText(issue)}</p>)}</details> : null; }
function knownBytes(value: ByteCount | undefined): bigint | null {
  try { const bytes = value == null ? null : typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return bytes !== null && bytes >= 0n ? bytes : null; } catch { return null; }
}
function hasCompleteRatios(current: ProjectStorage['currentProject'] | undefined) {
  if (!current?.complete || current.stale || current.status === 'not_scanned') return false;
  const total = knownBytes(current.totalBytes);
  const categories = storageCategories.map(({ key }) => current.categories?.[key]);
  const bytes = categories.map((part) => knownBytes(partBytes(part)));
  return total !== null && total > 0n && categories.every((part) => part?.complete !== false) && bytes.every((value) => value !== null) && bytes.reduce<bigint>((sum, value) => sum + (value ?? 0n), 0n) === total;
}

export function ProjectStoragePanel({ project, status, task, disabled, onTask, onChanged, showTask = true, active = true, onDraftStateChange }: { project: ProjectInfo; status?: MaintenanceStatus; task?: TaskRecord; disabled: boolean; onTask: (task: TaskRecord) => void; onChanged: () => Promise<void>; showTask?: boolean; active?: boolean; onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void }) {
  const [info, setInfo] = useState<ProjectStorage | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  const [clean, setClean] = useState<string[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [ack, setAck] = useState(false);
  const pending = useRef(false);
  const request = useRef(0);
  const operation = useRef(0);
  const mounted = useRef(false);
  const scope = `${project.id}:${project.documentEpoch}`;
  const scopeRef = useRef(scope); scopeRef.current = scope;
  const onTaskRef = useRef(onTask); onTaskRef.current = onTask;
  const taskRef = useRef(task); taskRef.current = task;
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  useEffect(() => { draftCallback.current?.({ dirty: expanded && clean.length > 0, pending: busy || fileBusy }); }, [expanded, clean, busy, fileBusy]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current++; operation.current++; }; }, []);
  const read = useCallback(async () => {
    const sequence = ++request.current;
    try {
      const value = await invoke<ProjectStorage>('storage_info', { projectId: project.id });
      if (!mounted.current || sequence !== request.current || scope !== scopeRef.current) return;
      setInfo(value); setError('');
      if (value.activeTask && typeof value.activeTask === 'object' && 'status' in value.activeTask && (!taskRef.current || taskRef.current.final || taskRef.current.type === 'storageScan')) onTaskRef.current(value.activeTask as TaskRecord);
    } catch (cause) { if (mounted.current && sequence === request.current && scope === scopeRef.current) setError(messageFrom(cause)); }
  }, [project.id, scope]);
  useEffect(() => {
    setInfo(null); setError(''); setClean([]); setExpanded(false); setConfirm(false); setAck(false); setBusy(false); setFileBusy(false); pending.current = false; operation.current++;
    return () => { request.current++; };
  }, [scope]);
  useEffect(() => { void read(); }, [read, task?.taskId, task?.status, task?.final]);
  useEffect(() => { if (!active) { setConfirm(false); setAck(false); } }, [active]);
  const operationBlocked = !active || disabled || busy || fileBusy || Boolean(task && !task.final) || Boolean(status?.activeTask) || Boolean(info?.activeTask) || !status || status.recoveryRequired;
  const cleanupBlocked = operationBlocked || Boolean(status && status.mode !== 'ready' && (status.mode !== 'maintenance' || !status.readyForChanges));
  async function run(command: 'storage_scan' | 'storage_cleanup') {
    if (pending.current || operationBlocked || command === 'storage_cleanup' && (cleanupBlocked || !clean.length || !confirm || !ack)) return;
    pending.current = true; const sequence = ++operation.current; setBusy(true); setError('');
    try {
      const accepted = await invoke<TaskRecord>(command, { projectId: project.id, input: command === 'storage_cleanup' ? { categories: clean, ...(status?.mode === 'maintenance' && status.maintenanceId ? { maintenanceId: status.maintenanceId } : {}) } : {} });
      if (!mounted.current || sequence !== operation.current || scope !== scopeRef.current) return;
      onTaskRef.current(accepted); setConfirm(false); setAck(false); setExpanded(false); setClean([]);
      await read();
      if (mounted.current && sequence === operation.current && scope === scopeRef.current) await onChanged();
    } catch (cause) { if (mounted.current && sequence === operation.current && scope === scopeRef.current) setError(messageFrom(cause)); }
    finally { if (mounted.current && sequence === operation.current && scope === scopeRef.current) { pending.current = false; setBusy(false); } }
  }
  const current = info?.currentProject;
  const ratiosAvailable = hasCompleteRatios(current);
  const activeStorageTask = info?.activeTask && typeof info.activeTask === 'object' && 'type' in info.activeTask ? info.activeTask as TaskRecord : null;
  const scanning = Boolean(task?.type === 'storageScan' && !task.final || activeStorageTask?.type === 'storageScan' && !activeStorageTask.final);
  const scanStatus = `${scanning ? '正在扫描 · ' : ''}${!current || current.status === 'not_scanned' ? '尚无扫描结果' : `${current.complete ? '统计完整' : '部分统计'}${current.stale ? ' · 结果已过期，请重新扫描' : ''}`}`;
  return <div className="overview-body project-storage-panel">
    <div className="project-storage-heading">
      <div><h3>当前应用占用</h3><div className="project-storage-total"><strong title={exactBytes(current?.totalBytes)}>{bytesText(current?.totalBytes)}</strong><span>{storageCategories.length} 类文件</span></div><p className="project-storage-scan-time">{current?.completedAt ? `上次扫描 ${timeText(current.completedAt)} · ` : ''}{scanStatus}</p></div>
      <button className="refresh-button" onClick={() => void run('storage_scan')} disabled={operationBlocked}><RefreshCw size={20} />{busy ? '提交中…' : scanning ? '正在扫描…' : '重新扫描'}</button>
    </div>
    <div className={`project-storage-bar${ratiosAvailable ? '' : ' is-incomplete'}`} role="img" aria-label={ratiosAvailable ? '当前应用文件占用分类' : '分类比例暂不可用，请完成一次完整扫描'}>{ratiosAvailable && storageCategories.map(({ key, label }) => <span key={key} className={`storage-color-${key}`} title={`${label} ${percentOf(partBytes(current?.categories?.[key]), current?.totalBytes)?.toFixed(1)}%`} style={{ width: `${percentOf(partBytes(current?.categories?.[key]), current?.totalBytes) ?? 0}%` }} />)}</div>
    <dl className="project-storage-categories">{storageCategories.map(({ key, label, description }) => {
      const category = current?.categories?.[key];
      const percent = ratiosAvailable ? percentOf(partBytes(category), current?.totalBytes) : null;
      return <div key={key}><dt><i aria-hidden="true" className={`storage-color-${key}`} />{label}</dt><dd className="project-storage-purpose">{description}</dd><dd className="project-storage-category-size" title={exactBytes(partBytes(category))}>{bytesText(partBytes(category))}</dd><dd className="project-storage-category-ratio" aria-label={`${label}占比`}>{percent === null ? '—' : `${percent.toFixed(1)}%`}</dd></div>;
    })}</dl>
    <p className="project-storage-measurement">按文件大小统计，可能与实际磁盘占用不同。{!ratiosAvailable && current?.status !== 'not_scanned' && '完整且未过期的分类统计可显示占比。'}</p>
    <details className="project-storage-measurement-details"><summary>统计说明</summary><p>使用十进制 KB / MB / GB；悬停数字可查看精确字节。扫描期间文件可能变化，结果不是实时物理占用。固定素材已计入源码，数据库包含主库和伴随文件。</p>{current?.startedAt && <p>扫描开始：{timeText(current.startedAt)}{current.completedAt ? `，完成：${timeText(current.completedAt)}` : ''}</p>}</details>
    <Issues items={current?.issues} />
    <AttachmentBrowser project={project} onBusyChange={setFileBusy} blocked={!active || disabled || !status || status.mode === 'maintenance' || status.recoveryRequired || Boolean(status.activeTask) || Boolean(task && !task.final) || busy || Boolean(info?.activeTask)} />
    <section className="project-storage-cleanup" aria-labelledby="project-storage-cleanup-title">
      <div className="project-storage-cleanup-entry"><HardDrive size={22} aria-hidden="true" /><div><h4 id="project-storage-cleanup-title">清理可重新生成文件</h4><p>仅清理开发依赖、构建缓存和日志，保留当前应用与用户数据。</p></div><button className="refresh-button" aria-expanded={expanded} disabled={cleanupBlocked} onClick={() => { setExpanded((value) => !value); setConfirm(false); setAck(false); }}>{expanded ? '收起清理范围' : '查看可清理内容'}</button></div>
      {expanded && <div className="project-storage-cleanup-details"><p>保留源码、业务数据库、用户文件和当前可运行的应用文件。只删除下方明确选择的内容。</p><div className="cleanup-options">{cleanupCategories.map(([key, label, description]) => <label key={key}><input type="checkbox" checked={clean.includes(key)} disabled={cleanupBlocked} onChange={(event) => { setClean((value) => event.target.checked ? [...value, key] : value.filter((item) => item !== key)); setConfirm(false); setAck(false); }} /><span>{label}<small>{description}</small></span></label>)}</div><button className="refresh-button" onClick={() => { setConfirm(true); setAck(false); }} disabled={cleanupBlocked || !clean.length}>检查清理范围</button>
      {confirm && <div className="operation-confirmation"><strong>清理「{project.name}」的所选文件</strong><p>应用 ID：{project.id}</p><p>范围：{clean.map((key) => cleanupCategories.find(([value]) => key === value)?.[1]).join('、')}。依赖和缓存之后可重新生成，日志删除后无法恢复。最终结果显示实际删除字节，文件系统可用空间变化可能不同。</p><label className="checkbox-label"><input type="checkbox" checked={ack} disabled={cleanupBlocked} onChange={(event) => setAck(event.target.checked)} />我已核对应用和清理范围</label><div className="confirmation-actions"><button className="danger-button" disabled={!ack || cleanupBlocked} onClick={() => void run('storage_cleanup')}>{busy ? '提交中…' : '确认清理'}</button><button className="text-button" disabled={busy} onClick={() => { setConfirm(false); setAck(false); }}>取消</button></div></div>}
      {cleanupBlocked && <p role="status">当前有任务或恢复状态需要处理，暂不能清理文件。</p>}</div>}
      {showTask && task && ['storageScan', 'storageCleanup'].includes(task.type) && <ProjectTask task={task} />}
    </section>{error && <p role="alert" className="environment-issue">{error}</p>}
  </div>;
}

const defaultFileFilters = { type: 'all', prefix: '', query: '', sort: 'name', order: 'asc' };
function fileType(file: StoredFile) {
  const type = file.displayType ?? file.mediaType ?? file.mimeType ?? file.contentType ?? '';
  if (/^image(?:\/|$)|图片/.test(type)) return { label: '图片', Icon: FileImage };
  if (/^audio(?:\/|$)|音频/.test(type)) return { label: '音频', Icon: FileAudio };
  if (/^video(?:\/|$)|视频/.test(type)) return { label: '视频', Icon: FileVideo };
  return { label: /document|pdf|text|文档/.test(type) ? '文档' : '文件', Icon: File };
}
function AttachmentBrowser({ project, blocked, onBusyChange }: { project: ProjectInfo; blocked: boolean; onBusyChange: (value: boolean) => void }) {
  const [expanded, setExpanded] = useState(false);
  const [filters, setFilters] = useState(defaultFileFilters);
  const [applied, setApplied] = useState(defaultFileFilters);
  const [result, setResult] = useState<StorageFiles | null>(null);
  const [loading, setLoading] = useState(false);
  const [accessing, setAccessing] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [preview, setPreview] = useState<{ path: string; dataUrl: string; mimeType: string } | null>(null);
  const request = useRef(0);
  const listPending = useRef(false);
  const mounted = useRef(false);
  const busyCallback = useRef(onBusyChange); busyCallback.current = onBusyChange;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; busyCallback.current(false); }; }, []);
  useEffect(() => { busyCallback.current(accessing); }, [accessing]);
  const previewRequest = useRef(0);
  const pending = useRef(false);
  const identity = `${project.id}:${project.documentEpoch}`;
  const scope = useRef(identity); scope.current = identity;
  const blockedRef = useRef(blocked); blockedRef.current = blocked;
  const list = useCallback(async (cursor?: string) => {
    const sequence = ++request.current; listPending.current = true; setLoading(true); setError('');
    try {
      const value = await invoke<StorageFiles>('storage_files', { projectId: project.id, input: { ...applied, type: applied.type === 'all' ? undefined : applied.type, limit: expanded ? 40 : 2, ...(cursor ? { cursor } : {}) } });
      if (sequence === request.current && identity === scope.current) setResult((previous) => ({ ...value, files: cursor ? [...(previous?.files ?? []), ...value.files] : value.files }));
    } catch (cause) { if (sequence === request.current && identity === scope.current) setError(messageFrom(cause)); }
    finally { if (sequence === request.current && identity === scope.current) { listPending.current = false; setLoading(false); } }
  }, [project.id, identity, applied, expanded]);
  useEffect(() => { setExpanded(false); setFilters(defaultFileFilters); setApplied(defaultFileFilters); setNotice(''); setAccessing(false); pending.current = false; }, [identity]);
  useEffect(() => { setResult(null); setPreview(null); previewRequest.current++; void list(); return () => { request.current++; previewRequest.current++; }; }, [list]);
  useEffect(() => { if (blocked) { previewRequest.current++; setPreview(null); } }, [blocked, project.documentEpoch]);
  async function access(file: StoredFile, kind: 'preview' | 'download') {
    if (blockedRef.current || pending.current) return;
    pending.current = true; setAccessing(true); setError(''); setNotice(''); const sequence = ++previewRequest.current;
    const isCurrent = () => mounted.current && sequence === previewRequest.current && identity === scope.current && !blockedRef.current;
    try {
      if (kind === 'preview') {
        const value = await invoke<{ dataUrl: string; mimeType: string }>('storage_preview', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch });
        if (!isCurrent()) return;
        if (!/^(image\/(png|jpeg|gif|webp)|audio\/(mpeg|wav|mp4|ogg|webm)|video\/(mp4|webm))$/.test(value.mimeType) || !value.dataUrl.startsWith(`data:${value.mimeType};base64,`)) throw new Error('此格式不支持安全预览，请下载文件。');
        setPreview({ path: file.path, ...value });
      } else {
        const value = await invoke<{ savedPath: string } | null>('storage_download', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch });
        if (isCurrent() && value) setNotice(`已保存到 ${value.savedPath}`);
      }
    } catch (cause) { if (isCurrent()) setError(messageFrom(cause)); }
    finally { if (mounted.current && identity === scope.current) { pending.current = false; setAccessing(false); } }
  }
  const files = expanded ? result?.files : result?.files.slice(0, 2);
  return <section className="project-storage-files" aria-labelledby="project-storage-files-title"><div className="project-storage-files-heading"><h3 id="project-storage-files-title">用户文件</h3><button className="text-button" aria-expanded={expanded} onClick={() => { setExpanded((value) => !value); setFilters(defaultFileFilters); setApplied(defaultFileFilters); }}>{expanded ? '收起文件列表' : '浏览全部文件'}<ChevronRight className={expanded ? 'is-expanded' : ''} size={16} /></button></div>
    {expanded && <><p className="project-storage-file-note">浏览应用保存或上传的文件。图片、音频和视频可预览（上限 16 MB）；其他文档和较大的文件可下载。</p><form className="attachment-filters" onSubmit={(event) => { event.preventDefault(); if (!loading) setApplied({ ...filters }); }}>
      <label>类型<select value={filters.type} onChange={(event) => setFilters({ ...filters, type: event.target.value })}>{[['all', '全部'], ['image', '图片'], ['audio', '音频'], ['video', '视频'], ['document', '文档'], ['other', '其他']].map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <label>目录前缀<input placeholder="例如 lessons/" value={filters.prefix} onChange={(event) => setFilters({ ...filters, prefix: event.target.value })} /></label><label>名称搜索<input value={filters.query} onChange={(event) => setFilters({ ...filters, query: event.target.value })} /></label>
      <label>排序<select value={filters.sort} onChange={(event) => setFilters({ ...filters, sort: event.target.value })}><option value="name">名称</option><option value="size">大小</option><option value="modifiedAt">修改时间</option></select></label><label>顺序<select value={filters.order} onChange={(event) => setFilters({ ...filters, order: event.target.value })}><option value="asc">升序</option><option value="desc">降序</option></select></label><button className="refresh-button" disabled={loading}><Search size={16} />查询</button>
    </form></>}
    {blocked && <p className="environment-issue">当前有维护、任务或恢复状态需要处理，暂不能预览或下载文件。</p>}
    <div className="project-storage-file-list">{files?.map((file) => { const { label, Icon } = fileType(file); return <div className="project-storage-file-row" key={file.path}><Icon size={20} aria-hidden="true" /><span className="project-storage-file-name" title={file.path}>{file.path}</span><span className="project-storage-file-meta" title={`${exactBytes(file.sizeBytes ?? file.size)} · ${timeText(file.modifiedAt)}`}>{label} · {bytesText(file.sizeBytes ?? file.size)}</span><div className="project-storage-file-actions">{expanded && <button className="icon-button" aria-label={`预览 ${file.path}`} title="预览" disabled={blocked || accessing} onClick={() => void access(file, 'preview')}><Eye size={18} /></button>}<button className="icon-button" aria-label={`下载 ${file.path}`} title="下载" disabled={blocked || accessing} onClick={() => void access(file, 'download')}><Download size={18} /></button></div></div>; })}</div>
    {!loading && result && !result.files.length && <p>当前条件下没有用户文件。</p>}{loading && <p role="status">正在读取文件列表…</p>}
    {expanded && <><p className="project-storage-file-note">已载入 {result?.files.length ?? 0} 个文件{result?.nextCursor ? ' · 还有下一页' : ''}。列表为当前筛选条件下的分页结果。</p>{result?.nextCursor && <button className="refresh-button" disabled={loading} onClick={() => { if (!listPending.current) void list(result.nextCursor!); }}>继续读取下一页</button>}{result && <p className="project-storage-file-note">{result.complete ? '本次枚举完整' : '部分结果'} · {timeText(result.completedAt)}</p>}</>}
    <Issues items={result?.issues} />
    {preview && <div className="attachment-preview"><div><strong>{preview.path}</strong><button className="icon-button" onClick={() => { previewRequest.current++; setPreview(null); }} aria-label="关闭文件预览"><X size={16} /></button></div>{preview.mimeType.startsWith('image/') ? <img src={preview.dataUrl} alt={preview.path} /> : preview.mimeType.startsWith('audio/') ? <audio controls src={preview.dataUrl} /> : <video controls src={preview.dataUrl} />}</div>}
    {error && <p role="alert" className="environment-issue">{error}</p>}{notice && <p role="status">{notice}</p>}
  </section>;
}

export interface WorkspaceStorage { platformProgram: StoragePart; workspace: StoragePart; nodeApps?: StoragePart[]; otherLocalLocations: StoragePart[]; knownLocalTotal?: StoragePart; disks?: Record<string, unknown>[] }
// Where the numbers come from, for the overview's 统计说明 disclosure.
export function StorageScopeDetails({ data }: { data: WorkspaceStorage | null }) {
  const other = data ? combinedLocations(data.otherLocalLocations) : undefined;
  return <div className="workspace-summary-card">
    <div className="workspace-storage-grid workspace-scope-grid">
      <ScopeCard label="平台程序" part={data?.platformProgram} />
      <ScopeCard label="当前工作区" part={data?.workspace} />
      <div className="storage-category workspace-scope-card"><span>其他已知位置</span><strong title={exactBytes(partBytes(other))}>{bytesText(partBytes(other))}</strong><small>{scopeStatus(other)}</small><details className="workspace-disclosure"><summary>查看位置{data ? `（${data.otherLocalLocations.length}）` : ''}</summary><div className="workspace-disclosure-body"><p>迁移前的旧工作区，以及工作区以外的 Node 应用、设置、运行记录与统计缓存。</p>{data ? data.otherLocalLocations.length ? data.otherLocalLocations.map((part, index) => <div className="workspace-location-summary" key={String(part.root ?? index)}><h3>{scopeName(String((part.scope as Record<string, unknown> | undefined)?.kind ?? ''))}</h3><strong title={exactBytes(partBytes(part))}>{bytesText(partBytes(part))}</strong><ScopeDetails part={part} /></div>) : <p>没有其他已知位置。</p> : <p>尚未统计。</p>}</div></details></div>
    </div>
    <StorageRetentionExplanation data={data} />
    <div className="workspace-measurement"><p>数字为文件逻辑字节，不等于实际磁盘占块。不同范围各有统计时间；项目已包含在工作区内，不重复相加。外部 ZIP、下载的 DMG 与未登记的个人文件不计入。</p>{data?.knownLocalTotal && <p>已知范围合计：<strong title={exactBytes(partBytes(data.knownLocalTotal))}>{bytesText(partBytes(data.knownLocalTotal))}</strong> · {data.knownLocalTotal.complete ? '统计完整' : '部分统计，仍有未知范围'}</p>}{data?.disks?.map((disk, index) => <div className="workspace-disk-row" key={index}><code>{String(disk.exampleRoot ?? disk.path ?? disk.root ?? disk.mountPoint ?? '所在磁盘')}</code><span>磁盘可用 {bytesText((disk.availableBytes ?? null) as ByteCount)}</span></div>)}</div>
  </div>;
}
function StorageRetentionExplanation({ data }: { data: WorkspaceStorage | null }) {
  const categories = data?.workspace.categories as Record<string, StoragePart> | undefined;
  const rows: { key: string; label: string; part: StoragePart; parent: StoragePart; explanation: string }[] = [];
  const add = (key: string, label: string, part: StoragePart | undefined, parent: StoragePart | undefined, explanation: string) => {
    if (!part || !parent) return;
    const value = partBytes(part);
    try { if (value != null && (typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64)) === 0n && parent.complete && !parent.stale) return; } catch { /* Keep an unknown size visible. */ }
    rows.push({ key, label, part, parent, explanation });
  };
  add('operations', '操作记录 · 当前工作区', categories?.operations, data?.workspace, '其他项目与平台全局的任务和恢复记录；本项目归属记录随项目删除。');
  add('sharedCache', '共享缓存 · 当前工作区', categories?.sharedCache, data?.workspace, '构建工具等共享的缓存，独立于单个项目保存。');
  const retained = data?.otherLocalLocations.filter((part) => (part.scope as Record<string, unknown> | undefined)?.kind === 'retainedWorkspace') ?? [];
  if (retained.length) {
    const part = combinedLocations(retained);
    add('retained', '迁移前的旧工作区 · 其他已知位置', part, part, '迁移前的原目录已停用；本项目归属材料随项目删除，旧工作区本身可在“历史资料”核对并处理。');
  }
  add('program', '平台程序', data?.platformProgram, data?.platformProgram, '软件自身的文件，不随项目数量变化。');
  const applicationData = data?.otherLocalLocations.filter((part) => (part.scope as Record<string, unknown> | undefined)?.kind === 'applicationData') ?? [];
  if (applicationData.length) {
    const part = combinedLocations(applicationData);
    add('applicationData', '应用设置与运行记录 · 其他已知位置', part, part, '设置、运行记录与统计缓存，独立于项目保存。');
  }
  return <section className="overview-section workspace-retention-explanation" aria-labelledby="workspace-retention-title">
    <h3 id="workspace-retention-title">为什么删除项目后仍有占用</h3>
    <p>删除项目会清除平台管理的源码、数据、配置、日志和相关任务，用户自己的原始文件夹保留。其他项目、平台全局记录、迁移旧目录和程序文件仍可能占用空间。</p>
    {rows.length > 0 && <><p>以下为上方范围中仍有占用或大小未知的项目，不再与上方数字相加：</p><dl className="workspace-retention-list">{rows.map(({ key, label, part, parent, explanation }) => <div key={key}><dt>{label}</dt><dd><strong title={exactBytes(partBytes(part))}>{bytesText(partBytes(part))}</strong>{Boolean(!parent.complete || parent.stale) && <small>{parent.stale ? ' · 已过期' : ' · 部分统计 / 未知'}</small>}<p>{explanation}</p></dd></div>)}</dl></>}
    <p>空间大小按文件逻辑字节统计，不等于可以释放的物理磁盘空间。迁移前的旧工作区可在“历史资料”核对范围后显式处理；未解决的恢复现场仍保留，不会自动删除。</p>
  </section>;
}
export function combinedLocations(parts: StoragePart[]): StoragePart {
  let bytes = 0n; let unknown = false;
  for (const part of parts) { const value = partBytes(part); try { if (value == null) unknown = true; else bytes += typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); } catch { unknown = true; } }
  return { totalBytes: unknown ? null : { $int64: bytes.toString() }, complete: !unknown && parts.every((part) => part.complete), stale: parts.some((part) => part.stale) };
}
function scopeStatus(part?: StoragePart) { return part ? `${part.complete ? '统计完整' : '部分统计 / 未知'}${part.stale ? ' · 已过期' : ''}` : '尚未统计'; }
function scopeName(name: string) { return ({ applicationData: '应用设置与运行记录', retainedWorkspace: '迁移前的旧工作区', platformProgram: '平台程序', workspace: '当前工作区' } as Record<string, string>)[name] ?? '已知本地位置'; }
function ScopeDetails({ part }: { part?: StoragePart }) {
  return <><code className="workspace-scope-path">{String(part?.root ?? part?.path ?? '')}</code>{Boolean(part?.completedAt) && <small className="storage-timestamp">扫描时间：{timeText(part!.completedAt as string | number)}</small>}<Issues items={part?.issues} />{Boolean(part?.categories) && <dl className="workspace-storage-breakdown">{Object.entries(part!.categories as Record<string, StoragePart>).map(([name, item]) => <div key={name}><dt>{({ projects: '当前项目', nodeApps: 'Node 应用', operations: '操作记录', sharedCache: '共享缓存', control: '平台控制记录', metadata: '平台元数据', logs: '日志', other: '其他', applicationData: '应用数据', platformProgram: '程序文件', retainedWorkspace: '保留文件', projectFolder: '项目文件夹（含依赖）', data: '数据', configuration: '配置', cache: '缓存', program: '旧程序代次' } as Record<string, string>)[name] ?? name}</dt><dd title={exactBytes(partBytes(item))}>{bytesText(partBytes(item))}</dd></div>)}</dl>}</>;
}
function ScopeCard({ label, part }: { label: string; part?: StoragePart }) {
  return <div className="storage-category workspace-scope-card"><span>{label}</span><strong title={exactBytes(partBytes(part))}>{bytesText(partBytes(part))}</strong><small>{scopeStatus(part)}</small>{Boolean(part?.completedAt) && <small className="storage-timestamp">{timeText(part!.completedAt as string | number)}</small>}<details className="workspace-disclosure"><summary>查看位置与分类</summary><div className="workspace-disclosure-body"><ScopeDetails part={part} /></div></details></div>;
}
