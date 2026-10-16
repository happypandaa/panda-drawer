import { invoke } from '@tauri-apps/api/core';
import { Terminal } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { TaskLogs, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText } from './storage-utils';
import type { ByteCount } from './types';

export function ProjectTask({ task }: { task: TaskRecord }) {
  const [logs, setLogs] = useState('');
  const [cursor, setCursor] = useState(0);
  const [gap, setGap] = useState(false);
  const [logsError, setLogsError] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const pending = useRef(false);
  const labels: Record<string, string> = { build: '构建', backup: '备份', restore: '恢复', maintenanceBegin: '准备维护', maintenanceEnd: '结束维护', export: '导出项目', import: '导入项目', copy: '复制项目', delete: '移入回收站', trashRestore: '恢复回收站项目', trashPurge: '永久删除', workspaceMigration: '迁移工作区', storageScan: '扫描项目空间', storageCleanup: '清理可再生成文件' };
  const label = labels[task.type] ?? '项目任务';
  const outcome = task.result?.operationOutcome ?? task.error?.details?.operationOutcome;
  const outcomes: Record<string, string> = { restored: task.type === 'restore' ? '恢复内容已提交；项目保持维护，检查后再结束维护。' : '项目内容已恢复。请查看项目列表中的维护状态，再决定何时重新打开。', rolled_back: '已回退到恢复前的完整项目；项目仍保持维护。', needs_attention: '操作结果需要检查，相关目录已保留。请先查看日志和当前状态，不要重复提交。', committed: '操作已提交，但收尾未全部完成。请核对当前状态，不要重复提交。', not_started: '操作尚未执行，原内容保持不变。', committed_needs_attention: '新工作区路径已提交，但重新连接或收尾失败。原目录仍保留；请核对目标磁盘与日志，修复后重新启动平台。不要重试迁移或切回原目录。', not_committed: '迁移尚未提交，原工作区仍是当前工作区。目标现场可能仍保留，请查看路径和日志。', purge_incomplete: '永久删除未完成，部分文件可能已被清除，此记录不能再恢复。查看剩余目录后可重试永久删除。', registered_maintenance_not_ready: '项目已注册，但维护基线尚未准备好。请先到备份与维护中检查并准备基线，不要继续修改。' };
  const successMessages: Record<string, string> = { storageScan: '扫描已完成，请查看空间分类、完成状态与扫描时间。', storageCleanup: '清理已结束。实际删除逻辑字节与文件系统可用空间变化可能不同，请查看结果和日志。', export: '项目归档已完成，可从下方路径找到 ZIP。', import: '项目已导入，请在项目列表中查看状态并打开。', copy: '独立副本已创建，请在项目列表中查看。', delete: '完整项目已移入回收站，可从回收站恢复。', trashRestore: '项目已恢复，请在项目列表查看维护状态。旧窗口需要处理草稿后明确重新加载。', trashPurge: '此回收站项目的文件已永久删除。历史备份和外部导出文件仍保留。', workspaceMigration: '工作区迁移已完成。旧目录已保留；处理草稿后，从平台重新加载项目，并重新复制 AI 路径。' };
  const resultIssues = Array.isArray(task.result?.issues) ? task.result.issues.map(messageFrom) : [];
  const deletedObjects = Array.isArray(task.result?.deleted) ? task.result.deleted as Record<string, unknown>[] : [];
  const cleanupProblems: unknown[] = task.type === 'storageCleanup' ? [...(Array.isArray(task.result?.skipped) ? task.result.skipped : []), ...(Array.isArray(task.result?.failures) ? task.result.failures : [])] : [];
  const paths = ['destinationPath', 'archivePath', 'path', 'remainingPath', 'sourcePath', 'projectId', 'targetProjectId', 'trashId'].filter((key) => typeof task.result?.[key] === 'string');
  const originalPath = task.result?.originalPath ?? task.error?.details?.originalPath;
  const statuses = { queued: '等待执行', running: '正在执行', succeeded: '已完成', failed: '失败', interrupted: '已中断' };
  const phases: Record<string, string> = { queued: '任务已接受', environment: '检查 Node / npm', 'input-digest': '核对构建输入', dependencies: '准备项目依赖', build: '编译网页', 'artifact-validation': '检查输入和产物', publish: '发布网页', completed: '已完成', 'snapshot-freeze': '生成项目副本', 'archive-publish': '校验与发布归档', 'maintenance-intent': '进入维护', 'maintenance-check': '检查项目完整性', 'maintenance-commit': '结束维护并更新页面身份', 'candidate-validation': '校验恢复候选', 'restore-maintenance': '准备恢复', 'preserve-original': '保留恢复前现场', 'install-candidate': '切换项目目录', 'restore-commit': '提交恢复', snapshot: '复制完整项目', archive: '生成归档', validation: '校验项目', baseline: '准备维护基线', candidate: '校验恢复候选', preserve: '保留恢复前现场', switch: '切换项目目录', commit: '提交恢复结果' };
  useEffect(() => { setLogs(''); setCursor(0); setGap(false); setLogsError(null); setHasMore(false); }, [task.taskId]);
  async function readLogs() {
    if (pending.current) return;
    pending.current = true;
    setReading(true);
    try {
      const chunk = await invoke<TaskLogs>('task_logs', { taskId: task.taskId, cursor });
      setLogs((current) => (current + chunk.text).slice(-120_000));
      setCursor(chunk.nextCursor);
      setGap((current) => current || chunk.truncated || Boolean(chunk.gaps.length) || logs.length + chunk.text.length > 120_000);
      setHasMore(!chunk.eof);
      setLogsError(null);
    } catch (cause) { setLogsError(messageFrom(cause)); }
    finally { pending.current = false; setReading(false); }
  }
  return <section className="build-task" aria-label={`${label}任务`}><div className="build-task-heading"><strong>{label} · {statuses[task.status]}</strong><span>{phases[task.phase] ?? task.phase}</span></div><code className="task-id">{task.taskId}</code>{task.error && <p className="task-error" role="alert">{task.error.message} ({task.error.code})</p>}{typeof outcome === 'string' && outcomes[outcome] && !(outcome === 'committed' && task.status === 'succeeded') && <p className={outcome === 'needs_attention' ? 'task-error' : undefined}>{outcomes[outcome]}</p>}{typeof originalPath === 'string' && <details className="retained-original"><summary>恢复前完整现场已保留</summary><p>此目录包含替换前的完整项目，可供检查和后续处理。</p><code>{originalPath}</code></details>}{task.status === 'succeeded' && <p>{task.type === 'build' ? '新网页已发布。重新加载已有窗口前，请先处理未保存内容。' : task.type === 'restore' ? '项目已恢复，仍处于维护。完成检查并明确结束维护后，再重新加载项目。' : task.type === 'maintenanceBegin' ? '维护准备已完成。请核对基线与可修改状态后再修改项目。' : task.type === 'maintenanceEnd' ? '维护已结束。旧页面需要重新加载，请先复制或放弃未保存内容。' : successMessages[task.type] ?? '备份任务已完成，可在备份列表查看快照。'}</p>}{task.type === 'storageCleanup' && task.result && <p>实际删除：{bytesText((task.result.logicalBytes) as ByteCount | undefined)}</p>}{deletedObjects.length > 0 && <details className="retained-original"><summary>查看实际删除的对象</summary>{deletedObjects.map((item, index) => <p key={index}><code>{String(item.path ?? item.category ?? '已删除对象')}</code>{bytesText(item.logicalBytes as ByteCount | undefined)}</p>)}</details>}{task.result?.mode === 'maintenance' && <p className="maintenance-warning">项目仍在维护中{task.result.readyForChanges === false ? '，基线尚未准备好，请勿修改' : '，请检查后再显式结束维护'}。</p>}{cleanupProblems.map((item, index) => <p key={`cleanup:${index}`} className="task-error">{typeof item === 'object' && item ? [String((item as Record<string, unknown>).path ?? ''), String((item as Record<string, unknown>).message ?? (item as Record<string, unknown>).reason ?? (item as Record<string, unknown>).code ?? '未处理，请查看日志')].filter(Boolean).join(' · ') : String(item)}</p>)}{resultIssues.map((issue) => <p className="task-error" key={issue}>{issue}</p>)}{paths.map((key) => <code className="task-result-path" key={key}>{({ destinationPath: '目标路径', archivePath: '归档路径', path: '路径', remainingPath: '尚未清除的目录', sourcePath: '保留的原路径', projectId: '项目 ID', targetProjectId: '新项目 ID', trashId: '回收站记录' } as Record<string, string>)[key]}：{String(task.result?.[key])}</code>)}{!task.final && <p role="status">任务会在平台中继续执行。只有最终成功才表示操作完成。</p>}<button className="text-button" onClick={() => void readLogs()} disabled={reading}><Terminal size={13} />{reading ? '读取日志…' : cursor ? hasMore ? '继续读取日志' : '刷新日志' : '查看任务日志'}</button>{logsError && <p role="alert">{logsError}</p>}{gap && <p className="log-note">日志包含被截断的片段；此处仅显示最近 120 KB。</p>}{logs && <pre className="task-logs" tabIndex={0}>{logs}</pre>}</section>;
}

