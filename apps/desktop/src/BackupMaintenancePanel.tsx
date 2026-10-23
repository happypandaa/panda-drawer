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
  embedded?: boolean;
  active?: boolean;
  onDraftStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
}

function timestamp(value: string | number | null | undefined) {
  if (value == null) return '时间未知';
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? '时间未知' : date.toLocaleString('zh-CN', { hour12: false });
}

function fileSize(value: number | undefined) {
  if (value === undefined || !Number.isFinite(value)) return '大小未知';
  if (value < 1000) return `${value} B`;
  if (value < 1000 * 1000) return `${(value / 1000).toFixed(1)} KB`;
  return `${(value / 1000 / 1000).toFixed(1)} MB`;
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
  'invalid archive retained': '无效归档已保留',
};

export function BackupMaintenancePanel({ project, disabled = false, status, task, onTask, onChanged, onClose, embedded = false, active = true, onDraftStateChange }: Props) {
  const [backups, setBackups] = useState<BackupSummary[]>([]);
  const [selected, setSelected] = useState<BackupDetails | null>(null);
  const [note, setNote] = useState('');
  const [reason, setReason] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
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
  const draftCallback = useRef(onDraftStateChange);
  draftCallback.current = onDraftStateChange;
  const inMaintenance = status?.mode === 'maintenance';
  const legacyMustStayOpen = inMaintenance || Boolean(status?.recoveryRequired);
  const hasProjectGit = Boolean(project.objectLayout?.versioning.markers.some((marker) => marker.scope === 'project' || marker.scope === 'source'));
  const activeTask = Boolean(task && !task.final) || Boolean(status?.activeTask);
  const blocked = !active || disabled || submitting || activeTask || !status || Boolean(status.recoveryRequired);
  const canChange = !blocked && (!inMaintenance || status.readyForChanges);

  useEffect(() => {
    draftCallback.current?.({ dirty: Boolean(note || reason), pending: submitting });
  }, [note, reason, submitting]);
  useEffect(() => () => { draftCallback.current?.({ dirty: false, pending: false }); }, []);
  useEffect(() => {
    if (!active) {
      detailRequest.current += 1;
      setConfirmRestore(false);
      setRestoreAcknowledged(false);
      setConfirmEnd(false);
    }
  }, [active]);

  const load = useCallback(async () => {
    const id = ++request.current;
    setReading(true);
    try {
      const list = await invoke<BackupSummary[]>('backup_list', { projectId: project.id });
      if (id !== request.current) return;
      setBackups(list);
      setSelected((current) => current && list.some((backup) => backup.backupId === current.backupId) ? current : null);
      setReadError(null);
    } catch (cause) { if (id === request.current) setReadError(messageFrom(cause)); }
    finally { if (id === request.current) setReading(false); }
  }, [project.id]);

  useEffect(() => {
    if (!embedded) panel.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    void load();
    return () => { request.current += 1; detailRequest.current += 1; };
  }, [load, embedded]);
  useEffect(() => { if (task?.final) void load(); }, [task?.taskId, task?.final, load]);

  async function selectBackup(backup: BackupSummary) {
    if (!active || submitting) return;
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
    if (pending.current || blocked || (command === 'backup_create' && !canChange) || (command === 'backup_restore' && hasProjectGit)) return;
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
        setMessage('任务已提交，请等待最终结果。关闭面板后任务会继续。');
      } else {
        setMessage('检查已完成。旧页面需要重新加载，请先处理未保存内容。');
      }
      if (command === 'backup_create') setNote('');
      if (command === 'maintenance_begin') setReason('');
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
  return <section className="backup-panel" ref={panel} aria-labelledby={embedded ? undefined : 'backup-panel-title'} aria-label={embedded ? '备份与维护' : undefined}>
    {!embedded && <div className="section-heading"><h2 id="backup-panel-title"><Archive size={17} />旧版备份与维护</h2><button className="icon-button" aria-label="关闭备份与维护" onClick={onClose} disabled={submitting}><X size={18} /></button></div>}
    <p className="detail-legacy-note">旧版完整快照与“源码版本”分开。Git 仅保存本机源码，不含业务数据，尚无远程同步。</p>
    <details className="detail-disclosure legacy-backup-tools" open={legacyMustStayOpen} onToggle={(event) => { if (legacyMustStayOpen && !event.currentTarget.open) event.currentTarget.open = true; }}>
      <summary onClick={(event) => { if (legacyMustStayOpen) event.preventDefault(); }}>旧版工具{status?.recoveryRequired ? ' · 恢复待处理' : inMaintenance ? ' · 维护中' : ''}</summary>
      <div className={`maintenance-summary ${inMaintenance ? 'maintenance-active' : ''}`}>
      <div className="maintenance-heading"><ShieldCheck size={18} /><strong>{status?.recoveryRequired ? '恢复结果需要处理' : inMaintenance ? status.readyForChanges ? '维护中 · 可以管理修改' : '维护中 · 基线尚未准备好' : status ? '正常运行' : '正在读取维护状态'}</strong><button className="text-button" disabled={!active || reading || submitting} onClick={() => { void onChanged(); void load(); }}><RefreshCw size={13} />刷新状态</button></div>
      {status?.recoveryRequired ? <p role="alert">项目保持停用。请先查看任务日志和恢复记录，处理后再结束维护。</p> : inMaintenance ? <><p>业务网页已暂停。结束维护会保留当前修改；退出 AI 或重启不会结束维护。</p><dl className="maintenance-facts"><div><dt>维护原因</dt><dd>{status.reason || '未填写'}</dd></div><div><dt>开始时间</dt><dd>{timestamp(status.startedAt)}</dd></div></dl><details className="detail-disclosure"><summary>维护记录</summary><dl className="maintenance-facts"><div><dt>维护 ID</dt><dd><code>{status.maintenanceId}</code></dd></div><div><dt>基线备份</dt><dd><code>{status.baselineBackupId || '尚未成功生成，请勿修改项目'}</code></dd></div></dl></details></> : <p>生成快照时会短暂停写，请先停止外部编辑器和 AI 修改项目。</p>}
      {status?.pendingBuild && <p className="maintenance-warning">需要构建或检查：{status.pendingBuildReason || '维护中的构建输入发生了变化。'}</p>}
      {lastError && <p className="maintenance-warning">上次检查：{lastError}</p>}
      {inMaintenance ? <div className="maintenance-actions">{!status.readyForChanges && <button className="refresh-button" disabled={blocked} onClick={() => void submit('maintenance_begin', { reason: status.reason || '重试维护基线准备' })}>重试准备基线</button>}<button className="refresh-button" disabled={blocked} onClick={() => setConfirmEnd(true)}>检查并结束维护</button></div> : <form className="maintenance-form" onSubmit={(event) => { event.preventDefault(); void submit('maintenance_begin', { reason: reason.trim() }); }}><label htmlFor="maintenance-reason">维护原因</label><div className="inline-form"><input id="maintenance-reason" value={reason} onChange={(event) => setReason(event.target.value)} placeholder="例如：调整数据表并更新页面" disabled={blocked} required /><button className="refresh-button" type="submit" disabled={blocked || !reason.trim()}>进入维护并备份</button></div></form>}
      {confirmEnd && <div className="operation-confirmation" role="group" aria-label="结束维护确认"><strong>检查通过后恢复项目使用</strong><p>保留当前修改，不回滚数据。请先处理旧页面的未保存内容，再明确重新加载。</p><div className="confirmation-actions"><button className="primary-button" disabled={blocked} onClick={() => void submit('maintenance_end', {})}>检查并结束</button><button className="refresh-button" disabled={submitting} onClick={() => setConfirmEnd(false)}>继续维护</button></div></div>}
      </div>
      <form className="backup-create-form" onSubmit={(event) => { event.preventDefault(); void submit('backup_create', { note: note.trim() }); }}><label htmlFor="backup-note">创建完整快照<span className="optional-label">备注选填</span></label><div className="inline-form"><input id="backup-note" value={note} onChange={(event) => setNote(event.target.value)} placeholder="例如：调整之前" disabled={!canChange} /><button className="primary-button" type="submit" disabled={!canChange}><Archive size={14} />创建备份</button></div></form>
      <p className="backup-retention">快照包含源码、网页、数据库和附件，不含 Git 仓库。手动快照不会自动清理。</p>
    </details>
    {error && <div className="panel-message panel-error" role="alert"><CircleAlert size={16} /><span>{error}。请先查看当前状态和日志，勿重复提交结果不明的操作。</span></div>}
    {readError && <div className="panel-message panel-error" role="alert"><CircleAlert size={16} /><span>备份列表暂时无法读取：{readError}</span></div>}
    {message && <div className="panel-message" role="status"><Check size={16} /><span>{message}</span></div>}
    {task && !embedded && <ProjectTask key={task.taskId} task={task} />}
    <div className="backup-content">
      <div className="backup-list-column">
        <div className="backup-list-heading"><strong>{reading ? '正在读取…' : `完整快照 · ${backups.length}`}</strong></div>
        {backups.length ? <ul className="backup-list">{backups.map((backup) => <li key={backup.backupId}><button className={`backup-row ${selected?.backupId === backup.backupId ? 'backup-row-selected' : ''}`} disabled={!active || submitting} onClick={() => void selectBackup(backup)}><span className="backup-row-top"><strong>{backup.note || kindLabels[backup.kind] || backup.kind}</strong><time>{timestamp(backup.createdAt)}</time></span><span className="backup-row-meta">{fileSize(backup.sizeBytes)} · {backup.verified ? '校验通过' : '需要检查'}{backup.protectionReasons?.length ? ` · ${backup.protectionReasons.map((item) => protectionLabels[item] ?? item).join('、')}` : ''}</span></button></li>)}</ul> : !reading && <p className="backup-empty">暂无旧版完整快照。</p>}
        {backups.length > 0 && <details className="detail-disclosure backup-retention"><summary>保留规则</summary><p>自动生成的基线和恢复前快照合计保留最近 10 份；手动与受保护快照除外。</p></details>}
      </div>
      <div className="backup-detail-column">{selected ? <>
        <h3>快照详情</h3><dl className="backup-facts"><div><dt>备份时间</dt><dd>{timestamp(selected.createdAt)}</dd></div><div><dt>类型与体积</dt><dd>{kindLabels[selected.kind] ?? selected.kind} · {fileSize(selected.sizeBytes)}</dd></div><div><dt>完整性</dt><dd>{selected.verified ? '文件清单与数据库已验证' : '需要检查，无法保证可恢复'}</dd></div>{selected.note && <div><dt>备注</dt><dd>{selected.note}</dd></div>}</dl>
        <details className="detail-disclosure"><summary>快照记录</summary><dl className="backup-facts"><div><dt>备份 ID</dt><dd><code>{selected.backupId}</code></dd></div><div><dt>快照状态</dt><dd>{selected.snapshotState === 'ready' ? '正常运行' : selected.snapshotState === 'maintenance' ? '维护中' : '来源状态未知'}{selected.pendingBuild?.required ? ' · 仍需构建' : ''}</dd></div><div><dt>网页产物</dt><dd>{selected.hasArtifact === true ? '包含可复用网页，恢复无需 Node' : selected.hasArtifact === false ? '尚未构建，恢复后仍需构建' : '未能读取归档内容'}</dd></div><div><dt>版本</dt><dd>归档 v{selected.formatVersion ?? '未知'} · 平台 {selected.platformVersion ?? '未知'} · SQLite {selected.sqliteVersion ?? '未知'} · API v{selected.sdkApiVersion ?? '未知'}</dd></div></dl></details>
        {selected.issues?.length ? <ul className="project-issues">{selected.issues.map((issue, index) => <li key={index}>{issue}</li>)}</ul> : null}
        <details className="backup-files"><summary>归档文件 · {selected.files?.length ?? 0} 个</summary><ul>{selected.files?.map((file) => <li key={file.path}><code>{file.path}</code><span>{fileSize(file.sizeBytes ?? file.bytes)}</span></li>)}</ul></details>
        {selected.excludedPaths?.length ? <details className="backup-files"><summary>归档排除范围</summary><ul>{selected.excludedPaths.map((path) => <li key={path}><code>{path}</code></li>)}</ul></details> : null}
        <p className="backup-restore-note">恢复会替换当前源码、网页、数据库和附件，恢复前现场保留。</p>
        <p className={hasProjectGit ? 'maintenance-warning' : 'backup-restore-note'}>{hasProjectGit ? '检测到本项目 Git 仓库，整项目恢复已停用。Git 取回只恢复源码，不含数据库；业务数据恢复需另行处理。' : '恢复前会重新核实 Git，发现仓库即停止。Git 取回只恢复源码，不能恢复业务数据。'}</p>
        {!confirmRestore ? <button className="refresh-button" disabled={blocked || !selected.verified || hasProjectGit} onClick={() => { setConfirmRestore(true); setRestoreAcknowledged(false); }}><RotateCcw size={14} />恢复这份备份</button> : <div className="operation-confirmation" role="group" aria-label="恢复备份确认"><strong>恢复「{project.name}」到 {timestamp(selected.createdAt)}</strong><p>源码、网页和业务数据将回到所选快照，之后的新增或修改内容将被替换。请停止外部编辑器、AI 和数据库工具写入，并处理旧页面的未保存内容。恢复后保持维护状态。</p><label className="restore-acknowledgement"><input type="checkbox" checked={restoreAcknowledged} onChange={(event) => setRestoreAcknowledged(event.target.checked)} disabled={submitting} />我已确认项目和时间，理解这会替换当前程序与业务数据。</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !restoreAcknowledged || hasProjectGit} onClick={() => void submit('backup_restore', { backupId: selected.backupId })}>确认恢复</button><button className="refresh-button" disabled={submitting} onClick={() => setConfirmRestore(false)}>取消</button></div></div>}
      </> : <div className="backup-empty"><Archive size={26} /><p>选择快照查看详情</p></div>}</div>
    </div>
  </section>;
}
