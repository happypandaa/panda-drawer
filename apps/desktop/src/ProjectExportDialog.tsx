import { invoke } from '@tauri-apps/api/core';
import { Check, FileArchive, FolderOpen, Info } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type Props = {
  project: ProjectInfo;
  status?: MaintenanceStatus;
  disabled: boolean;
  onTask: (task: TaskRecord) => void;
  onClose: () => void;
};

function isExistingDestination(cause: unknown) {
  return typeof cause === 'object' && cause !== null && 'code' in cause && cause.code === 'EXPORT_DESTINATION_EXISTS';
}

export function ProjectExportDialog({ project, status, disabled, onTask, onClose }: Props) {
  const [pending, setPending] = useState<'choosing' | 'exporting' | null>(null);
  const [error, setError] = useState('');
  const [existingPath, setExistingPath] = useState('');
  const generation = useRef(0);
  const alive = useRef(true);
  const lock = useRef(false);
  const callbacks = useRef({ onTask, onClose }); callbacks.current = { onTask, onClose };
  const blocked = disabled || !status || Boolean(status.activeTask) || Boolean(status.recoveryRequired);
  const busy = pending !== null;
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; generation.current++; };
  }, []);

  // Always try without overwrite first. The native save chooser may already
  // confirm a filename, but only an explicit collision review authorizes us to
  // pass overwrite to project_export.
  async function exportTo(destinationPath: string, overwrite: boolean, current: number) {
    const isCurrent = () => alive.current && current === generation.current;
    if (!isCurrent()) return;
    setPending('exporting');
    try {
      const task = await invoke<TaskRecord>('project_export', { projectId: project.id, input: {
        destinationPath, overwrite,
        ...(status?.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {}),
      } });
      if (!isCurrent()) return;
      if (!task.taskId || task.projectId !== project.id || task.type !== 'export') throw new Error('返回的导出任务身份不符，请核对任务记录，勿重复提交。');
      callbacks.current.onTask(task);
      if (task.status === 'failed' || task.status === 'interrupted') {
        if (!overwrite && isExistingDestination(task.error)) setExistingPath(destinationPath);
        else setError(task.error ? messageFrom(task.error) : '导出未完成，请查看任务详情。');
      } else callbacks.current.onClose();
    } catch (cause) {
      if (!isCurrent()) return;
      if (!overwrite && isExistingDestination(cause)) setExistingPath(destinationPath);
      else setError(`导出失败：${messageFrom(cause)}`);
    }
  }

  async function chooseAndExport() {
    if (blocked || lock.current) return;
    lock.current = true; setPending('choosing'); setError(''); setExistingPath('');
    const current = ++generation.current;
    try {
      const selected = await invoke<string | null>('choose_path', { kind: 'export', suggestedName: `${project.id}.zip` });
      if (!alive.current || current !== generation.current || !selected) return;
      await exportTo(selected, false, current);
    } catch (cause) {
      if (alive.current && current === generation.current) setError(`路径选择失败：${messageFrom(cause)}`);
    } finally {
      if (alive.current && current === generation.current) { lock.current = false; setPending(null); }
    }
  }

  async function replaceAndExport() {
    if (blocked || lock.current || !existingPath) return;
    lock.current = true; setPending('exporting'); setError('');
    const current = ++generation.current;
    try { await exportTo(existingPath, true, current); }
    finally { if (alive.current && current === generation.current) { lock.current = false; setPending(null); } }
  }

  return <Modal title={`导出「${project.name}」`} titleIcon={<span className="project-export-title-icon"><FileArchive size={20} /></span>} className="project-export-modal" closeDisabled={busy} onClose={onClose}>
    <p className="project-export-description">将当前应用保存为 ZIP，可在另一工作区导入。</p>
    <section className="project-export-includes" aria-label="归档包含">
      <h3>归档包含</h3>
      <ul><li><Check size={20} aria-hidden="true" />源码与项目资料</li><li><Check size={20} aria-hidden="true" />当前应用文件</li><li><Check size={20} aria-hidden="true" />数据库和用户文件</li></ul>
    </section>
    <p className="project-export-excludes">不包含 Git 历史、开发依赖、缓存和平台历史备份。</p>
    <p className="project-export-warning"><Info size={20} aria-hidden="true" /><span>归档包含当前业务数据，分享前请核对其中的个人内容。</span></p>
    {existingPath && <section className="project-export-collision" aria-label="替换已有 ZIP 确认">
      <strong>此位置已有文件，是否替换？</strong>
      <code>{existingPath}</code>
      <p>只有新归档生成并校验成功后，才会替换此文件。也可以选择其他位置。</p>
    </section>}
    {error && <p className="task-error" role="alert">{error}</p>}
    <div className="project-export-actions">
      <button type="button" className="refresh-button" disabled={busy} onClick={onClose}>取消</button>
      <button type="button" className={existingPath ? 'refresh-button' : 'primary-button'} disabled={busy || blocked} onClick={() => void chooseAndExport()}><FolderOpen size={20} />{pending === 'choosing' ? '选择中…' : pending === 'exporting' && !existingPath ? '正在提交…' : existingPath ? '选择其他位置' : '选择位置并导出'}</button>
      {existingPath && <button type="button" className="primary-button" disabled={busy || blocked} onClick={() => void replaceAndExport()}>{pending === 'exporting' ? '正在提交…' : '确认替换并导出'}</button>}
    </div>
  </Modal>;
}
