import { invoke } from '@tauri-apps/api/core';
import { Archive, Copy, Download, FolderOpen, RefreshCw, Trash2, Upload, X } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { MaintenanceStatus, ProjectInfo, TaskRecord, TrashEntry, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';

type TaskHandler = (task: TaskRecord) => void;
function useReveal() {
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { panel.current?.scrollIntoView({ behavior: 'smooth', block: 'start' }); }, []);
  return panel;
}
function useOperation(onTask: TaskHandler) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lock = useRef(false);
  async function submit(command: string, args: Record<string, unknown>, after?: () => void) {
    if (lock.current) return;
    lock.current = true; setPending(true); setError(null);
    try { onTask(await invoke<TaskRecord>(command, args)); after?.(); }
    catch (cause) { setError(messageFrom(cause)); }
    finally { lock.current = false; setPending(false); }
  }
  return { pending, error, submit };
}
function PathInput({ label, value, onChange, kind, disabled, suggestedName }: { label: string; value: string; onChange: (value: string) => void; kind: 'import' | 'export' | 'workspace'; disabled: boolean; suggestedName?: string }) {
  const id = useId();
  const [choosing, setChoosing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function choose() {
    setChoosing(true); setError(null);
    try { const selected = await invoke<string | null>('choose_path', { kind, suggestedName }); if (selected) onChange(selected); }
    catch (cause) { setError(`路径选择失败，可直接填写绝对路径：${messageFrom(cause)}`); }
    finally { setChoosing(false); }
  }
  return <div className="path-input"><label htmlFor={id}>{label}</label><span className="path-input-controls"><input id={id} value={value} onChange={(event) => onChange(event.target.value)} disabled={disabled || choosing} placeholder={kind === 'workspace' ? '/Volumes/磁盘/新工作区' : '/Users/你的名字/Documents/项目.zip'} autoComplete="off" /><button type="button" className="refresh-button" disabled={disabled || choosing} onClick={() => void choose()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择路径'}</button></span>{error && <span role="alert" className="task-error">{error}</span>}</div>;
}

export function ImportPanel({ onTask, onClose, disabled }: { onTask: TaskHandler; onClose: () => void; disabled: boolean }) {
  const panel = useReveal();
  const [path, setPath] = useState(''); const [mode, setMode] = useState<'original' | 'copy'>('original'); const [name, setName] = useState('');
  const { pending, error, submit } = useOperation(onTask);
  return <section ref={panel} className="lifecycle-card" aria-labelledby="import-title"><div className="section-heading"><div><h2 id="import-title">导入完整项目</h2><p>从平台导出的 ZIP 恢复项目。导入先校验归档，不运行项目脚本。</p></div><button className="icon-button" onClick={onClose} disabled={pending} aria-label="关闭导入"><X size={18} /></button></div><form onSubmit={(event) => { event.preventDefault(); void submit('project_import', { input: { archivePath: path.trim(), mode, ...(mode === 'copy' && name.trim() ? { name: name.trim() } : {}) } }); }}><PathInput label="完整项目 ZIP" kind="import" value={path} onChange={setPath} disabled={pending || disabled} /><fieldset disabled={pending || disabled} className="choice-group"><legend>导入方式</legend><label><input type="radio" name="import-mode" checked={mode === 'original'} onChange={() => setMode('original')} />恢复原项目 ID</label><label><input type="radio" name="import-mode" checked={mode === 'copy'} onChange={() => setMode('copy')} />作为新副本导入</label></fieldset><p className="project-help">{mode === 'original' ? '保留原项目身份。如果该 ID 已在项目列表或回收站中，平台会拒绝导入，不会覆盖。' : '生成新的项目 ID，数据库、附件和源码各自独立。'}</p>{mode === 'copy' && <label>副本名称（选填）<input value={name} onChange={(event) => setName(event.target.value)} disabled={pending || disabled} /></label>}<div className="form-footer"><span>维护中的快照会继续保持维护。</span><button className="primary-button" type="submit" disabled={pending || disabled || !path.trim().startsWith('/')}><Upload size={14} />{pending ? '正在提交…' : '校验并导入'}</button></div></form>{error && <p className="task-error" role="alert">{error}</p>}</section>;
}

export function ProjectLifecyclePanel({ project, status, onTask, disabled, active = true, deleteRequested = 0, onDraftStateChange }: {
  project: ProjectInfo; status?: MaintenanceStatus; onTask: TaskHandler; onClose?: () => void; disabled: boolean;
  active?: boolean; deleteRequested?: number; onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [path, setPath] = useState('');
  const [overwrite, setOverwrite] = useState(false);
  const [name, setName] = useState('');
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const [deleteAcknowledged, setDeleteAcknowledged] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [chooseError, setChooseError] = useState('');
  const { pending, error, submit } = useOperation(onTask);
  const deleteSection = useRef<HTMLElement>(null);
  const chooserGeneration = useRef(0);
  const handledDeleteRequest = useRef(0);
  const chooserLock = useRef(false);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const pathId = useId();
  const maintenanceInput = status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {};
  const deleteBlocked = pending || choosing || disabled || !active || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired);
  const blocked = deleteBlocked || !status;
  useEffect(() => { draftCallback.current?.({ dirty: Boolean(path || name || overwrite), pending: pending || choosing }); }, [path, name, overwrite, pending, choosing]);
  useEffect(() => () => { chooserGeneration.current++; draftCallback.current?.({ dirty: false, pending: false }); }, [project.id]);
  useEffect(() => {
    if (!active) { setDeleteConfirm(false); setDeleteAcknowledged(false); }
    else if (deleteRequested > handledDeleteRequest.current) { handledDeleteRequest.current = deleteRequested; setDeleteConfirm(true); setDeleteAcknowledged(false); deleteSection.current?.focus({ preventScroll: true }); }
  }, [active, deleteRequested]);
  async function chooseExportPath() {
    if (blocked || chooserLock.current) return;
    chooserLock.current = true; setChoosing(true); setChooseError(''); const generation = ++chooserGeneration.current;
    try {
      const selected = await invoke<string | null>('choose_path', { kind: 'export', suggestedName: `${project.id}.zip` });
      if (generation === chooserGeneration.current && selected) setPath(selected);
    } catch (cause) { if (generation === chooserGeneration.current) setChooseError(messageFrom(cause)); }
    finally { if (generation === chooserGeneration.current) { chooserLock.current = false; setChoosing(false); } }
  }
  return <section className="lifecycle-card lifecycle-panel-embedded" aria-label="项目更多操作">
    <section ref={deleteSection} className="project-delete-section" tabIndex={-1} aria-labelledby="project-delete-title">
      <div className="project-delete-heading"><div><h3 id="project-delete-title"><Trash2 size={17} />删除项目</h3><p>删除后移入回收站，可以恢复。项目不会被永久清除。</p></div>{!deleteConfirm && <button className="danger-button" disabled={deleteBlocked} onClick={() => { setDeleteConfirm(true); setDeleteAcknowledged(false); }}><Trash2 size={14} />删除项目</button>}</div>
      {deleteConfirm && <div className="operation-confirmation project-delete-confirmation" role="group" aria-label="删除项目确认">
        <strong>删除「{project.name}」并移入回收站？</strong>
        <p>项目 ID：<code>{project.id}</code></p>
        <p>源码、数据库、附件与当前网页将一起移入回收站，并从项目列表移除。之后可以在“空间与工作区 → 回收站”恢复；历史备份和外部导出的 ZIP 会保留。</p>
        <p>已有项目窗口将无法继续保存。请先处理窗口内的未保存内容和本页尚未保存的编辑。</p>
        <label className="checkbox-label"><input type="checkbox" checked={deleteAcknowledged} disabled={deleteBlocked} onChange={(event) => setDeleteAcknowledged(event.target.checked)} />我已核对项目，并处理未保存内容</label>
        <div className="confirmation-actions"><button className="danger-button" disabled={deleteBlocked || !deleteAcknowledged} onClick={() => { if (!deleteBlocked && deleteAcknowledged) void submit('project_delete', { projectId: project.id, input: maintenanceInput }, () => { setDeleteConfirm(false); setDeleteAcknowledged(false); }); }}><Trash2 size={14} />{pending ? '正在提交…' : '确认删除项目'}</button><button className="refresh-button" disabled={pending} onClick={() => { setDeleteConfirm(false); setDeleteAcknowledged(false); }}>取消，保留项目</button></div>
      </div>}
      {status?.recoveryRequired && <p className="task-error">项目有待处理的恢复状态，请先在备份维护中查看。</p>}
      {!status && <p className="project-help">维护状态尚未读取，请先刷新状态再删除。</p>}
      {(disabled || Boolean(status?.activeTask)) && <p className="project-help">当前操作进行中，请等待完成后再删除。</p>}
    </section>
    <div className="lifecycle-columns">
      <form onSubmit={(event) => { event.preventDefault(); if (!blocked && path.trim().startsWith('/')) void submit('project_export', { projectId: project.id, input: { destinationPath: path.trim(), overwrite, ...maintenanceInput } }, () => { setPath(''); setOverwrite(false); }); }}>
        <h3><Download size={16} />导出完整 ZIP</h3><p>将源码、数据、附件、网页和项目说明保存为归档，可在另一工作区导入。</p>
        <div className="path-input"><label htmlFor={pathId}>导出位置</label><span className="path-input-controls"><input id={pathId} value={path} onChange={(event) => setPath(event.target.value)} disabled={blocked} placeholder="/Users/你的名字/Documents/项目.zip" autoComplete="off" /><button type="button" className="refresh-button" disabled={blocked} onClick={() => void chooseExportPath()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择路径'}</button></span>{chooseError && <p role="alert" className="task-error">{chooseError}</p>}</div>
        <label className="checkbox-label"><input type="checkbox" checked={overwrite} onChange={(event) => setOverwrite(event.target.checked)} disabled={blocked} />允许替换此路径已有的 ZIP（新归档校验成功后才替换）</label>
        <button className="refresh-button" type="submit" disabled={blocked || !path.trim().startsWith('/')}><Download size={14} />导出项目</button>
      </form>
      <form onSubmit={(event) => { event.preventDefault(); if (!blocked) void submit('project_copy', { projectId: project.id, input: { ...(name.trim() ? { name: name.trim() } : {}), ...maintenanceInput } }, () => setName('')); }}>
        <h3><Copy size={16} />创建独立副本</h3><p>副本拥有新的项目 ID、独立数据与附件，后续修改不会影响原项目。</p>
        <label>副本名称（选填）<input value={name} onChange={(event) => setName(event.target.value)} placeholder={`${project.name} · 副本`} disabled={blocked} /></label><button className="refresh-button" type="submit" disabled={blocked}><Copy size={14} />创建副本</button>
      </form>
    </div>
    {error && <p className="task-error" role="alert">{error}</p>}
  </section>;
}

function sizeLabel(bytes: number | null | undefined) { return bytes == null ? '体积尚未统计' : bytes < 1000 * 1000 ? `${(bytes / 1000).toFixed(1)} KB` : `${(bytes / 1000 / 1000).toFixed(1)} MB`; }
export function StoragePage({ workspace, revision, enabled, onTask, onRefresh }: { workspace: WorkspaceInfo | null; revision: number; enabled: boolean; onTask: TaskHandler; onRefresh: () => Promise<void> }) {
  const [entries, setEntries] = useState<TrashEntry[]>([]); const [selected, setSelected] = useState<TrashEntry | null>(null); const [listError, setListError] = useState<string | null>(null); const [reading, setReading] = useState(false);
  const [purgeStep, setPurgeStep] = useState(false); const [confirmName, setConfirmName] = useState(''); const [confirmScope, setConfirmScope] = useState(false);
  const [destination, setDestination] = useState(''); const [migrateConfirm, setMigrateConfirm] = useState(false);
  const { pending, error, submit } = useOperation(onTask);
  const migrating = Boolean(workspace?.activeTask) || ['running', 'paused', 'migrating'].includes(workspace?.migrationState ?? '');
  const blocked = !enabled || pending || migrating;
  const refreshTrash = useCallback(async () => {
    if (!enabled) return;
    setReading(true); setListError(null);
    try {
      const list = await invoke<TrashEntry[]>('trash_list'); setEntries(list);
      setSelected((current) => current ? list.find((entry) => entry.trashId === current.trashId) ?? null : null);
      for (const entry of list) { const id = (typeof entry.activeTask === 'string' ? entry.activeTask : entry.activeTask?.taskId) ?? entry.lastTaskId; if (id) { const task = await invoke<TaskRecord>('task_get', { taskId: id }); if (!task.final) onTask(task); } }
    } catch (cause) { setListError(messageFrom(cause)); }
    finally { setReading(false); }
  }, [enabled, onTask]);
  useEffect(() => { void refreshTrash(); }, [refreshTrash, revision]);
  async function inspect(entry: TrashEntry) {
    setPurgeStep(false); setConfirmName(''); setConfirmScope(false); setListError(null);
    try { const detail = await invoke<TrashEntry>('trash_info', { trashId: entry.trashId }); setSelected(detail); if (detail.lastTaskId) onTask(await invoke<TaskRecord>('task_get', { taskId: detail.lastTaskId })); }
    catch (cause) { setListError(messageFrom(cause)); }
  }
  return <><section className="lifecycle-card" aria-labelledby="trash-title"><div className="section-heading"><div><h2 id="trash-title"><Trash2 size={18} />回收站</h2><p>完整项目长期保留，只有明确执行永久删除才清除文件。</p></div><button className="refresh-button" onClick={() => void refreshTrash()} disabled={!enabled || reading}><RefreshCw size={14} />刷新回收站</button></div>{listError && <p role="alert" className="task-error">{listError}</p>}{!entries.length ? <p className="empty-note">{reading ? '正在读取回收站…' : '回收站为空。'}</p> : <div className="trash-list">{entries.map((entry) => <button className={`trash-row ${selected?.trashId === entry.trashId ? 'selected' : ''}`} key={entry.trashId} onClick={() => void inspect(entry)}><Archive size={19} /><span><strong>{entry.name || '无法读取的回收站记录'}</strong><code>{entry.projectId || entry.trashId}</code></span><span>{entry.deletedAt ? new Date(entry.deletedAt).toLocaleString('zh-CN', { hour12: false }) : '删除时间未知'}<small>{sizeLabel(entry.sizeBytes)} · {entry.state === 'purgeIncomplete' ? '永久删除未完成' : entry.state === 'needsAttention' ? '需要检查' : entry.mode === 'maintenance' ? '维护中' : '已移入回收站'}</small></span></button>)}</div>}{selected && <div className="trash-details"><h3>{selected.name}</h3><dl><div><dt>项目 ID</dt><dd><code>{selected.projectId}</code></dd></div><div><dt>回收站记录</dt><dd><code>{selected.trashId}</code></dd></div>{selected.path && <div><dt>保留目录</dt><dd><code>{selected.path}</code></dd></div>}</dl>{selected.issues?.map((issue) => <p className="task-error" key={issue}>{issue}</p>)}<p>恢复会保留项目原 ID 和维护状态，并为旧窗口更新页面身份。旧窗口草稿会保留，确认处理后再重新加载。</p><div className="confirmation-actions"><button className="primary-button" disabled={blocked || selected.state === 'purgeIncomplete' || Boolean(selected.activeTask)} onClick={() => void submit('trash_restore', { trashId: selected.trashId })}>恢复项目</button><button className="danger-button" disabled={blocked || Boolean(selected.activeTask)} onClick={() => { setPurgeStep(true); setConfirmName(''); setConfirmScope(false); }}>永久删除…</button></div>{purgeStep && <div className="operation-confirmation purge-confirmation"><strong>永久删除「{selected.name}」？此操作无法撤销。</strong><p>将删除此回收站记录中的完整项目：源码、数据库、附件、网页产物和说明。范围仅为 <code>{selected.trashId}</code>。历史备份及外部导出的 ZIP 会保留，其他回收站项目不受影响。</p><label>输入项目名称「{selected.name}」确认<input value={confirmName} onChange={(event) => setConfirmName(event.target.value)} disabled={pending} autoComplete="off" /></label><label className="checkbox-label"><input type="checkbox" checked={confirmScope} onChange={(event) => setConfirmScope(event.target.checked)} disabled={pending} />我已核对项目与删除范围，理解完整项目文件将永久删除</label><div className="confirmation-actions"><button className="danger-button" disabled={blocked || confirmName !== selected.name || !confirmScope} onClick={() => void submit('trash_purge', { trashId: selected.trashId }, () => setPurgeStep(false))}>确认永久删除完整项目</button><button className="refresh-button" disabled={pending} onClick={() => setPurgeStep(false)}>取消，继续保留</button></div></div>}</div>}</section><section className="lifecycle-card" aria-labelledby="workspace-title"><div className="section-heading"><div><h2 id="workspace-title">工作区位置</h2><p>一次迁移所有项目、回收站、历史备份及受保护的现场。</p></div><button className="refresh-button" onClick={() => void onRefresh()} disabled={!enabled}>刷新状态</button></div><dl className="path-list"><div className="path-row"><dt>当前工作区</dt><dd><code>{workspace?.workspaceRoot ?? '尚未读取'}</code></dd></div><div className="path-row"><dt>格式与空间</dt><dd>格式 {workspace?.formatVersion ?? '未知'} · {workspace?.filesystem ?? '文件系统未读取'} · {sizeLabel(workspace?.sizeBytes)}</dd></div></dl>{migrating && <p role="status" className="operation-confirmation">工作区正在迁移。新操作暂停，已有窗口的草稿会保留。请等待任务最终结果，不要重复提交迁移。</p>}<form onSubmit={(event) => { event.preventDefault(); setMigrateConfirm(true); }}><PathInput label="新的工作区目录" kind="workspace" value={destination} onChange={(value) => { setDestination(value); setMigrateConfirm(false); }} disabled={blocked} /><p className="project-help">选择本机或直连外置磁盘上的独立、可写、为空的 APFS 目录。迁移会短暂暂停服务；完成后仍保留原目录，所有旧项目页面需要从平台重新进入。请先保存或复制各窗口的草稿，并结束外部 AI / 编辑器对工作区的写入。</p><button className="refresh-button" type="submit" disabled={blocked || !destination.trim().startsWith('/')}>检查并准备迁移…</button></form>{migrateConfirm && <div className="operation-confirmation"><strong>确认迁移整个工作区</strong><p>来源：<code>{workspace?.workspaceRoot}</code><br />目标：<code>{destination.trim()}</code></p><p>迁移包含项目、备份和回收站；原目录会保留。迁移成功后，请从新的项目详情重新复制 AI 路径。</p><div className="confirmation-actions"><button className="primary-button" disabled={blocked} onClick={() => void submit('workspace_migrate', { input: { destinationPath: destination.trim() } }, () => setMigrateConfirm(false))}>确认开始迁移</button><button className="refresh-button" disabled={pending} onClick={() => setMigrateConfirm(false)}>取消</button></div></div>}{Boolean(workspace?.retainedRoots?.length) && <div className="retained-roots"><h3>迁移后保留的原目录</h3><p>这些目录已停止使用，平台不会自动删除，也不会自动切回。</p>{workspace?.retainedRoots.map((root, index) => <code key={index}>{typeof root === 'string' ? root : root.path ?? root.sourcePath ?? JSON.stringify(root)}</code>)}</div>}</section>{error && <div className="notice notice-error" role="alert"><p>{error}</p></div>}</>;
}
