import { invoke } from '@tauri-apps/api/core';
import { Archive, Check, CircleAlert, RefreshCw, RotateCcw, ShieldCheck, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { BackupDetails, BackupSummary, MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { ProjectTask } from './ProjectTask';
import { messageFrom } from './ui-utils';

interface Props {
  project: ProjectInfo;
  disabled?: boolean;
  status?: MaintenanceStatus;
  task?: TaskRecord;
  onTask: (task: TaskRecord) => void;
  onChanged: () => Promise<void>;
  onClose: () => void;
}

function timestamp(value: string | number | null | undefined) {
  if (value == null) return '时间未知';
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? '时间未知' : date.toLocaleString('zh-CN', { hour12: false });
}

function fileSize(value: number | undefined) {
  if (value === undefined || !Number.isFinite(value)) return '大小未知';
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

const kindLabels: Record<string, string> = {
  manual: '手动备份', maintenanceBaseline: '维护基线', maintenance_baseline: '维护基线',
  preRestore: '恢复前备份', pre_restore: '恢复前备份', automatic: '自动备份',
};
const protectionLabels: Record<string, string> = {
  manual: '手动保留', maintenanceBaseline: '当前维护基线', maintenance_baseline: '当前维护基线',
  maintenance: '当前维护基线', lastValidBackup: '最后一份有效备份', last_valid_backup: '最后一份有效备份',
  last_valid: '最后一份有效备份', restoreSource: '恢复来源', restore_source: '恢复来源',
  active_operation: '进行中的操作使用', recovery: '恢复待处理', 'maintenance or recovery baseline': '维护或恢复基线',
};

export function BackupMaintenancePanel({ project, disabled = false, status, task, onTask, onChanged, onClose }: Props) {
  const [backups, setBackups] = useState<BackupSummary[]>([]);
  const [selected, setSelected] = useState<BackupDetails | null>(null);
  const [note, setNote] = useState('');
  const [reason, setReason] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [restoreAcknowledged, setRestoreAcknowledged] = useState(false);
  const [confirmEnd, setConfirmEnd] = useState(false);
  const pending = useRef(false);
  const request = useRef(0);
  const detailRequest = useRef(0);
  const panel = useRef<HTMLElement>(null);
  const inMaintenance = status?.mode === 'maintenance';
  const active = Boolean(task && !task.final) || Boolean(status?.activeTask);
  const blocked = disabled || submitting || active || !status || Boolean(status.recoveryRequired);
  const canChange = !blocked && (!inMaintenance || status.readyForChanges);

  const load = useCallback(async () => {
    const id = ++request.current;
    setReading(true);
    try {
      const list = await invoke<BackupSummary[]>('backup_list', { projectId: project.id });
      if (id !== request.current) return;
      setBackups(list);
      setSelected((current) => current && list.some((backup) => backup.backupId === current.backupId) ? current : null);
      setError(null);
    } catch (cause) { if (id === request.current) setError(messageFrom(cause)); }
    finally { if (id === request.current) setReading(false); }
  }, [project.id]);

  useEffect(() => {
    panel.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    void load();
    return () => { request.current += 1; detailRequest.current += 1; };
  }, [load]);
  useEffect(() => { if (task?.final) void load(); }, [task?.taskId, task?.final, load]);

  async function selectBackup(backup: BackupSummary) {
    const id = ++detailRequest.current;
    setSelected(null);
    setConfirmRestore(false);
    setRestoreAcknowledged(false);
    setError(null);
    if (!backup.verified) { setSelected(backup); return; }
    try {
      const detail = await invoke<BackupDetails>('backup_info', { projectId: project.id, backupId: backup.backupId });
      if (id === detailRequest.current) setSelected(detail);
    } catch (cause) { if (id === detailRequest.current) setError(messageFrom(cause)); }
  }

  async function submit(command: string, input: Record<string, unknown>) {
    if (pending.current || disabled) return;
    pending.current = true;
    setSubmitting(true);
    setError(null);
    setMessage(null);
    try {
      const response = await invoke<TaskRecord | MaintenanceStatus>(command, {
        projectId: project.id,
        input: { ...input, ...(inMaintenance && status.maintenanceId ? { maintenanceId: status.maintenanceId } : {}) },
      });
      if ('taskId' in response) {
        onTask(response);
        setMessage('任务已提交。等待最终结果；此时可以关闭面板，任务会继续执行。');
      } else {
        setMessage('检查已完成。旧页面需要重新加载，请先处理未保存内容。');
      }
      setConfirmRestore(false);
      setRestoreAcknowledged(false);
      setConfirmEnd(false);
      await onChanged();
      await load();
    } catch (cause) {
      setError(messageFrom(cause));
      // Read state after an error; never replay a submission whose result is unknown.
      await onChanged();
    } finally { pending.current = false; setSubmitting(false); }
  }

  const lastError = typeof status?.lastError === 'string' ? status.lastError : status?.lastError?.message;
  return <section className="backup-panel" ref={panel} aria-labelledby="backup-panel-title">
    <div className="section-heading"><div><h2 id="backup-panel-title"><Archive size={17} />备份与维护 · {project.name}</h2><p>整项目快照包含源码、网页、数据库和附件。手动备份不会自动清理。</p></div><button className="icon-button" aria-label="关闭备份与维护" onClick={onClose}><X size={18} /></button></div>
    <div className={`maintenance-summary ${inMaintenance ? 'maintenance-active' : ''}`}>
      <div className="maintenance-heading"><ShieldCheck size={18} /><strong>{status?.recoveryRequired ? '恢复结果需要处理' : inMaintenance ? status.readyForChanges ? '维护中 · 可以管理修改' : '维护中 · 基线尚未准备好' : status ? '正常运行' : '正在读取维护状态'}</strong><button className="text-button" disabled={reading || submitting} onClick={() => { void onChanged(); void load(); }}><RefreshCw size={13} />刷新状态</button></div>
      {status?.recoveryRequired ? <p role="alert">恢复尚未得到明确结果，项目保持停用。请查看任务日志及恢复记录，处理完成前不能结束维护。</p> : inMaintenance ? <><p>业务网页暂停操作。退出 AI 或重启平台不会结束维护；结束维护保留当前修改，撤销需要恢复备份。</p><dl className="maintenance-facts"><div><dt>维护原因</dt><dd>{status.reason || '未填写'}</dd></div><div><dt>开始时间</dt><dd>{timestamp(status.startedAt)}</dd></div><div><dt>维护 ID</dt><dd><code>{status.maintenanceId}</code></dd></div><div><dt>基线备份</dt><dd><code>{status.baselineBackupId || '尚未成功生成，请勿修改项目'}</code></dd></div></dl></> : <p>普通备份会短暂暂停数据操作。创建期间请让外部 AI 和其他工具停止修改项目文件。</p>}
      {status?.pendingBuild && <p className="maintenance-warning">需要构建或检查：{status.pendingBuildReason || '维护中的构建输入发生了变化。'}</p>}
      {lastError && <p className="maintenance-warning">上次检查：{lastError}</p>}
      {inMaintenance ? <div className="maintenance-actions">{!status.readyForChanges && <button className="refresh-button" disabled={blocked} onClick={() => void submit('maintenance_begin', { reason: status.reason || '重试维护基线准备' })}>重试准备基线</button>}<button className="refresh-button" disabled={blocked} onClick={() => setConfirmEnd(true)}>检查并结束维护</button></div> : <form className="maintenance-form" onSubmit={(event) => { event.preventDefault(); void submit('maintenance_begin', { reason: reason.trim() }); }}><label htmlFor="maintenance-reason">维护原因</label><div className="inline-form"><input id="maintenance-reason" value={reason} onChange={(event) => setReason(event.target.value)} placeholder="例如：调整数据表并更新页面" disabled={blocked} required /><button className="refresh-button" type="submit" disabled={blocked || !reason.trim()}>进入维护并备份</button></div></form>}
      {confirmEnd && <div className="operation-confirmation" role="group" aria-label="结束维护确认"><strong>检查通过后恢复项目使用</strong><p>此操作保留所有当前修改，不会回滚数据。已有页面必须明确重新加载；请先复制、保存到别处或决定放弃其中尚未提交的内容。</p><div className="confirmation-actions"><button className="primary-button" disabled={blocked} onClick={() => void submit('maintenance_end', {})}>检查并结束</button><button className="refresh-button" disabled={submitting} onClick={() => setConfirmEnd(false)}>继续维护</button></div></div>}
    </div>
    {error && <div className="panel-message panel-error" role="alert"><CircleAlert size={16} /><span>{error}。请先查看当前状态和日志，勿重复提交结果不明的操作。</span></div>}
    {message && <div className="panel-message" role="status"><Check size={16} /><span>{message}</span></div>}
    {task && <ProjectTask key={task.taskId} task={task} />}
    <div className="backup-content">
      <div className="backup-list-column">
        <form className="backup-create-form" onSubmit={(event) => { event.preventDefault(); void submit('backup_create', { note: note.trim() }); }}><label htmlFor="backup-note">创建手动备份<span className="optional-label">备注选填</span></label><div className="inline-form"><input id="backup-note" value={note} onChange={(event) => setNote(event.target.value)} placeholder="例如：调整之前" disabled={!canChange} /><button className="primary-button" type="submit" disabled={!canChange}><Archive size={14} />创建备份</button></div></form>
        <div className="backup-list-heading"><strong>{reading ? '正在读取备份…' : `${backups.length} 份备份`}</strong><span>点击查看内容与恢复入口</span></div>
        {backups.length ? <ul className="backup-list">{backups.map((backup) => <li key={backup.backupId}><button className={`backup-row ${selected?.backupId === backup.backupId ? 'backup-row-selected' : ''}`} onClick={() => void selectBackup(backup)}><span className="backup-row-top"><strong>{kindLabels[backup.kind] ?? backup.kind}</strong><time>{timestamp(backup.createdAt)}</time></span><span>{backup.note || backup.backupId}</span><span className="backup-row-meta">{fileSize(backup.sizeBytes)} · {backup.verified ? '校验通过' : '需要检查'}{backup.protectionReasons?.length ? ` · ${backup.protectionReasons.map((item) => protectionLabels[item] ?? item).join('、')}` : ''}</span></button></li>)}</ul> : !reading && <p className="backup-empty">还没有完整备份。创建后可以恢复到这个时间点。</p>}
        <p className="backup-retention">普通自动备份保留最近 10 份；手动备份、使用中的基线和受保护备份单独保留。</p>
      </div>
      <div className="backup-detail-column">{selected ? <>
        <h3>备份详情</h3><dl className="backup-facts"><div><dt>备份时间</dt><dd>{timestamp(selected.createdAt)}</dd></div><div><dt>备份 ID</dt><dd><code>{selected.backupId}</code></dd></div><div><dt>类型与体积</dt><dd>{kindLabels[selected.kind] ?? selected.kind} · {fileSize(selected.sizeBytes)}</dd></div><div><dt>快照状态</dt><dd>{selected.snapshotState === 'ready' ? '正常运行' : selected.snapshotState === 'maintenance' ? '维护中' : '来源状态未知'}{selected.pendingBuild?.required ? ' · 带有待构建要求' : ''}</dd></div><div><dt>网页产物</dt><dd>{selected.hasArtifact === true ? '归档包含可复用网页，恢复本身无需 Node' : selected.hasArtifact === false ? '尚未构建，恢复后仍等待构建' : '未能读取归档内容'}</dd></div><div><dt>版本</dt><dd>归档 v{selected.formatVersion ?? '未知'} · 平台 {selected.platformVersion ?? '未知'} · SQLite {selected.sqliteVersion ?? '未知'} · API v{selected.sdkApiVersion ?? '未知'}</dd></div><div><dt>完整性</dt><dd>{selected.verified ? '已验证文件清单与数据库' : '需要查看诊断，无法保证可恢复'}</dd></div>{selected.note && <div><dt>备注</dt><dd>{selected.note}</dd></div>}</dl>
        {selected.issues?.length ? <ul className="project-issues">{selected.issues.map((issue, index) => <li key={index}>{issue}</li>)}</ul> : null}
        <details className="backup-files"><summary>归档文件 · {selected.files?.length ?? 0} 个</summary><ul>{selected.files?.map((file) => <li key={file.path}><code>{file.path}</code><span>{fileSize(file.sizeBytes ?? file.bytes)}</span></li>)}</ul></details>
        {selected.excludedPaths?.length ? <details className="backup-files"><summary>归档排除范围</summary><ul>{selected.excludedPaths.map((path) => <li key={path}><code>{path}</code></li>)}</ul></details> : null}
        <p className="backup-restore-note">恢复会让源码、网页、数据和附件回到此快照。快照之后新增或修改的内容不再出现在恢复后的项目中；恢复前完整现场会被保留。</p>
        {!confirmRestore ? <button className="refresh-button" disabled={blocked || !selected.verified} onClick={() => { setConfirmRestore(true); setRestoreAcknowledged(false); }}><RotateCcw size={14} />恢复这份备份</button> : <div className="operation-confirmation" role="group" aria-label="恢复备份确认"><strong>恢复「{project.name}」到 {timestamp(selected.createdAt)}</strong><p>恢复后保持维护状态。请先停止外部编辑器和数据库工具的写入，并处理旧页面中尚未保存的内容。</p><label className="restore-acknowledgement"><input type="checkbox" checked={restoreAcknowledged} onChange={(event) => setRestoreAcknowledged(event.target.checked)} disabled={submitting} />我已确认项目与备份时间，并理解恢复会替换当前内容。</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !restoreAcknowledged} onClick={() => void submit('backup_restore', { backupId: selected.backupId })}>确认恢复</button><button className="refresh-button" disabled={submitting} onClick={() => setConfirmRestore(false)}>取消</button></div></div>}
      </> : <div className="backup-empty"><Archive size={26} /><p>选择一份备份，查看文件清单、校验结果及恢复信息。</p></div>}</div>
    </div>
  </section>;
}
