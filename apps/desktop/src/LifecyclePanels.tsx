import { invoke } from '@tauri-apps/api/core';
import { Download, FileArchive, FolderOpen, Info, Trash2, Upload, X } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { ProjectExportDialog } from './ProjectExportDialog';

type TaskHandler = (task: TaskRecord) => void;
function useReveal() {
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { panel.current?.scrollIntoView({ behavior: 'smooth', block: 'start' }); }, []);
  return panel;
}
function useOperation(onTask: TaskHandler, scope?: string) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lock = useRef(false);
  const generation = useRef(0);
  const alive = useRef(true);
  const callback = useRef(onTask); callback.current = onTask;
  useEffect(() => {
    alive.current = true; generation.current++; lock.current = false; setPending(false); setError(null);
    return () => { alive.current = false; generation.current++; };
  }, [scope]);
  async function submit(command: string, args: Record<string, unknown>, after?: () => void) {
    if (lock.current) return;
    lock.current = true; setPending(true); setError(null);
    const current = generation.current;
    const isCurrent = () => alive.current && current === generation.current;
    try {
      const task = await invoke<TaskRecord>(command, args);
      if (!isCurrent()) return;
      callback.current(task);
      if (task.status === 'failed' || task.status === 'interrupted') setError(task.error ? messageFrom(task.error) : '操作未完成，请查看任务详情。');
      else after?.();
    }
    catch (cause) { if (isCurrent()) setError(messageFrom(cause)); }
    finally { if (isCurrent()) { lock.current = false; setPending(false); } }
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
  const [path, setPath] = useState(''); const [mode, setMode] = useState<'original' | 'copy'>('copy'); const [name, setName] = useState('');
  const { pending, error, submit } = useOperation(onTask);
  return <section ref={panel} className="lifecycle-card" aria-labelledby="import-title">
    <div className="section-heading"><div><h2 id="import-title">导入完整项目</h2><p>从平台导出的 ZIP 导入项目。导入先校验归档，不运行项目脚本。</p></div><button className="icon-button" onClick={onClose} disabled={pending} aria-label="关闭导入"><X size={18} /></button></div>
    <form onSubmit={(event) => { event.preventDefault(); void submit('project_import', { input: { archivePath: path.trim(), mode, ...(mode === 'copy' && name.trim() ? { name: name.trim() } : {}) } }); }}>
      <PathInput label="完整项目 ZIP" kind="import" value={path} onChange={setPath} disabled={pending || disabled} />
      <fieldset disabled={pending || disabled} className="choice-group"><legend>导入方式</legend>
        <label><input type="radio" name="import-mode" value="copy" checked={mode === 'copy'} onChange={() => setMode('copy')} />作为新项目导入（默认）</label>
        <label><input type="radio" name="import-mode" value="original" checked={mode === 'original'} onChange={() => setMode('original')} />保留原 ID（用于恢复）</label>
      </fieldset>
      <p className="project-help">{mode === 'original' ? '用于接回原项目身份。如果该 ID 已在项目列表或历史保留项目中，平台会拒绝导入，不会覆盖或合并。' : '每次导入都生成新 ID，重复导入可以并存；数据库、附件和源码各自独立。'}</p>
      {mode === 'copy' && <label>副本名称（选填）<input value={name} onChange={(event) => setName(event.target.value)} disabled={pending || disabled} /></label>}
      <div className="form-footer"><span>维护中的快照会继续保持维护。</span><button className="primary-button" type="submit" disabled={pending || disabled || !path.trim().startsWith('/')}><Upload size={14} />{pending ? '正在提交…' : '校验并导入'}</button></div>
    </form>
    {error && <p className="task-error" role="alert">{error}</p>}
  </section>;
}

export function ProjectLifecyclePanel({ project, status, onTask, disabled, active = true, deleteRequested = 0, onDraftStateChange, instanceId }: {
  project: ProjectInfo; status?: MaintenanceStatus; onTask: TaskHandler; onClose?: () => void; disabled: boolean;
  active?: boolean; deleteRequested?: number; onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void; instanceId?: string;
}) {
  const [exportOpen, setExportOpen] = useState(false);
  const [deleteConfirm, setDeleteConfirm] = useState(false);
  const [deleteAcknowledged, setDeleteAcknowledged] = useState(false);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const { pending, error, submit } = useOperation(onTask, identity);
  const deleteSection = useRef<HTMLElement>(null);
  const handledDeleteRequest = useRef(0);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const headingId = useId();
  const deleteTitleId = useId();
  const maintenanceInput = status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {};
  const blocked = pending || disabled || !active || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired);
  const deleteBlocked = blocked || exportOpen || !project.documentEpoch;
  useEffect(() => { draftCallback.current?.({ dirty: false, pending: pending || exportOpen }); }, [pending, exportOpen]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => { setExportOpen(false); setDeleteConfirm(false); setDeleteAcknowledged(false); }, [identity, project.name]);
  useEffect(() => {
    if (!active) { setExportOpen(false); setDeleteConfirm(false); setDeleteAcknowledged(false); }
    else if (deleteRequested > handledDeleteRequest.current) { handledDeleteRequest.current = deleteRequested; setDeleteConfirm(true); setDeleteAcknowledged(false); deleteSection.current?.focus({ preventScroll: true }); }
  }, [active, deleteRequested]);
  return <section className="lifecycle-card lifecycle-panel-embedded project-more-panel" aria-labelledby={headingId}>
    <h2 id={headingId}>更多操作</h2>
    <p className="project-more-intro">导出归档，或管理这台电脑上的应用。</p>
    <section className="project-more-row project-export-row" aria-label="导出应用">
      <span className="project-more-icon" aria-hidden="true"><FileArchive size={20} /></span>
      <div className="project-more-description"><h3>导出 ZIP</h3><p>保存一份包含源码、应用文件、数据库和用户文件的归档。</p></div>
      <button type="button" className="refresh-button" disabled={blocked} onClick={() => setExportOpen(true)}><Download size={20} />导出 ZIP</button>
    </section>
    <section ref={deleteSection} className="project-delete-section" tabIndex={-1} aria-labelledby={deleteTitleId}>
      <div className="project-delete-heading project-more-row">
        <span className="project-more-icon" aria-hidden="true"><Trash2 size={20} /></span>
        <div className="project-more-description"><h3 id={deleteTitleId}>删除应用</h3><p>永久删除当前应用及其本机数据，无法撤销。</p></div>
        {!deleteConfirm && <button type="button" className="danger-button" disabled={deleteBlocked} onClick={() => { setDeleteConfirm(true); setDeleteAcknowledged(false); }}><Trash2 size={20} />删除应用</button>}
      </div>
      {deleteConfirm && <div className="operation-confirmation project-delete-confirmation" role="group" aria-label="删除项目确认">
        <strong>永久删除「{project.name}」？此操作无法撤销。</strong>
        <p>项目 ID：<code>{project.id}</code></p>
        <p>将永久删除此项目目录内的源码、数据库、附件、图标、当前网页和说明，并从项目列表移除。</p><code className="workspace-scope-path">{project.projectRoot}</code><p>历史备份、旧版保留项目、迁移旧目录、操作与恢复证据和外部导出的 ZIP 不会自动删除。</p>
        <p>已有项目窗口将无法继续保存。请先处理窗口内的未保存内容和本页尚未保存的编辑。</p>
        <label className="checkbox-label"><input type="checkbox" checked={deleteAcknowledged} disabled={deleteBlocked} onChange={(event) => setDeleteAcknowledged(event.target.checked)} />我已核对项目和删除目录，处理未保存内容，并理解永久删除无法撤销</label>
        <div className="confirmation-actions"><button type="button" className="danger-button" disabled={deleteBlocked || !deleteAcknowledged} onClick={() => { if (!deleteBlocked && deleteAcknowledged) void submit('project_delete', { projectId: project.id, input: { ...maintenanceInput, expectedDocumentEpoch: project.documentEpoch } }, () => { setDeleteConfirm(false); setDeleteAcknowledged(false); }); }}><Trash2 size={14} />{pending ? '正在提交…' : '确认永久删除项目'}</button><button type="button" className="refresh-button" disabled={pending} onClick={() => { setDeleteConfirm(false); setDeleteAcknowledged(false); }}>取消，保留项目</button></div>
      </div>}
      {status?.recoveryRequired && <p className="task-error">项目有待处理的恢复现场，请先核对并处理，保留相关证据。</p>}
      {!status && <p className="project-help">维护状态尚未读取，请先刷新状态再删除。</p>}
      {!project.documentEpoch && <p className="project-help" role="status">项目身份尚未读取，不能确认删除。请关闭详情并刷新项目列表，重新核对后再删除。</p>}
      {(disabled || Boolean(status?.activeTask)) && <p className="project-help">当前操作进行中，请等待完成后再删除。</p>}
    </section>
    <p className="project-more-footnote"><Info size={20} aria-hidden="true" />删除前会再次确认应用和范围，不进入平台或系统回收站。</p>
    {error && <p className="task-error" role="alert">{error}</p>}
    {exportOpen && active && <ProjectExportDialog key={identity} project={project} status={status} disabled={blocked} onTask={onTask} onClose={() => setExportOpen(false)} />}
  </section>;
}
