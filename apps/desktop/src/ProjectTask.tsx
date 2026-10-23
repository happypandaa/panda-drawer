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
  const labels: Record<string, string> = { nodeDevelopmentBuild: '构建应用', gitSourceRestoreApply: '取回已保存源码', gitSourceRestoreContinue: '继续源码取回', gitSourceRestoreUndo: '撤回源码取回', gitBranchCreate: '创建定制分支', nodeInstallPrepare: '准备应用安装', nodeInstallApply: '安装或更新应用', sourceCapture: '接入本机源码', gitInitialize: '初始化源码版本', gitSave: '保存源码版本', sourceAcquire: '获取源码', build: '构建', backup: '备份', restore: '恢复', maintenanceBegin: '准备维护', maintenanceEnd: '结束维护', export: '导出项目', import: '导入项目', copy: '复制项目', delete: '删除项目', trashRestore: '旧版恢复任务', trashPurge: '旧版清除任务', workspaceMigration: '迁移工作区', workspaceCleanupPreview: '检查历史清理范围', workspaceCleanup: '清理历史', storageScan: '扫描项目空间', storageCleanup: '清理可再生成文件' };
  const label = labels[task.type] ?? '项目任务';
  const outcome = task.result?.operationOutcome ?? task.error?.details?.operationOutcome;
  const gitOperations: Record<string, string> = { gitInitialize: 'initialize', gitSave: 'save', gitBranchCreate: 'create-branch', gitSourceRestoreApply: 'source-restore', gitSourceRestoreContinue: 'source-restore-continue', gitSourceRestoreUndo: 'source-restore-undo' };
  const isSourceRestore = task.type.startsWith('gitSourceRestore');
  const isGit = Boolean(gitOperations[task.type]);
  const retainedResult = task.error?.details?.result;
  const displayResult = task.result ?? (isGit && outcome === 'committed' && retainedResult && typeof retainedResult === 'object' && !Array.isArray(retainedResult) && (retainedResult as Record<string, unknown>).operationOutcome === 'committed' && (retainedResult as Record<string, unknown>).operation === gitOperations[task.type] ? retainedResult as Record<string, unknown> : null);
  const candidateCommit = isGit && outcome === 'unknown' && typeof task.error?.details?.candidateCommit === 'string' ? task.error.details.candidateCommit : null;
  const outcomes: Record<string, string> = { restored: task.type === 'restore' ? '恢复内容已提交；项目保持维护，检查后再结束维护。' : '项目内容已恢复。请查看项目列表中的维护状态，再决定何时重新打开。', rolled_back: isSourceRestore ? '已撤回这次源码取回，请核对恢复状态。数据库和当前程序不受影响。' : '已回退到恢复前的完整项目；项目仍保持维护。', needs_attention: '操作结果需要检查，相关目录已保留。请先查看日志和当前状态，不要重复提交。', committed: isSourceRestore ? '源码操作已应用，但收尾未全部确认。请核对源码恢复状态，不要重复提交；这不代表已经保存 Git 版本或构建。' : '操作已提交，但收尾未全部完成。请核对当前状态，不要重复提交。', not_started: '操作尚未执行，原内容保持不变。', committed_needs_attention: '新工作区路径已提交，但重新连接或收尾失败。原目录仍保留；请核对目标磁盘与日志，修复后重新启动平台。不要重试迁移或切回原目录。', unknown: '操作结果尚不能确认，现场已保留。请核对当前状态和日志，不要重复提交。', 'not-committed': '安装或更新尚未提交，当前程序与数据保持原状；候选现场可能仍保留。', not_committed: isSourceRestore ? '已确认本次没有应用源码修改。请核对恢复状态；未自动保存版本或构建。' : isGit ? '本次版本操作未提交。源码和数据保持原状；请检查保留现场。' : '迁移尚未提交，原工作区仍是当前工作区。目标现场可能仍保留，请查看路径和日志。', purge_incomplete: '永久删除未完成，部分文件可能已被清除，此记录不能再恢复。查看剩余目录后可重试永久删除。', registered_maintenance_not_ready: '项目已注册，但维护基线尚未准备好。请先到备份与维护中检查并准备基线，不要继续修改。' };
  const sourceRestoreSuccess = displayResult?.state === 'not-applied' ? '本次没有应用源码修改。请核对恢复状态。' : displayResult?.state === 'undone' ? '这次源码取回已撤回。源码恢复到操作前内容；数据库和当前程序保持不变。' : displayResult?.state === 'completed' ? '源码已取回为待保存修改。当前分支、提交和暂存区保持不变；未自动保存版本或构建，数据库和当前程序保持不变。' : '源码任务已结束，请核对恢复状态；未自动保存版本或构建。';
  const successMessages: Record<string, string> = { nodeDevelopmentBuild: '构建完成，请核对并应用更新。', gitSourceRestoreApply: sourceRestoreSuccess, gitSourceRestoreContinue: sourceRestoreSuccess, gitSourceRestoreUndo: sourceRestoreSuccess, gitBranchCreate: '已创建并切换定制分支。源码、数据库和当前运行网页保持不变。', nodeInstallPrepare: '安装候选已准备。尚未安装或启动；请核对后明确应用。', nodeInstallApply: '应用程序已安装或更新，请核对配置后明确打开。', sourceCapture: '独立源码副本已创建。请核对排除范围；尚未安装或运行。', gitInitialize: '本机源码仓库已建立；尚未保存首个版本。', gitSave: '源码版本已保存。业务数据库、附件和运行产物未纳入此次提交。', sourceAcquire: '源码已获取。请核对来源和检查结果；尚未安装或运行。', workspaceCleanupPreview: '检查已完成。核对所列范围并明确选择后，才会永久清理。', workspaceCleanup: '历史清理任务已结束，请核对实际删除、失败和残留范围。逻辑字节不等于磁盘实际释放量。', storageScan: '扫描已完成，请查看空间分类、完成状态与扫描时间。', storageCleanup: '清理已结束。实际删除逻辑字节与文件系统可用空间变化可能不同，请查看结果和日志。', export: '项目归档已完成，可从下方路径找到 ZIP。', import: '项目已导入，请在项目列表中查看状态并打开。', copy: '独立副本已创建，请在项目列表中查看。', delete: outcome === 'system_trashed' ? '完整项目已移入系统废纸篓。文件找回请使用 Finder，应用不会自动恢复。' : '这是旧版删除任务结果，仅供历史核对；新版不再提供平台回收站入口。', trashRestore: '项目已恢复，请在项目列表查看维护状态。旧窗口需要处理草稿后明确重新加载。', trashPurge: '此回收站项目的文件已永久删除。历史备份和外部导出文件仍保留。', workspaceMigration: '工作区迁移已完成。旧目录已保留；处理草稿后，从平台重新加载项目，并重新复制 AI 路径。' };
  const durabilityWarning = task.error?.details?.durabilityWarning;
  const resultIssues = [...new Set([
    ...(Array.isArray(displayResult?.issues) ? displayResult.issues : []),
    ...(durabilityWarning ? [durabilityWarning] : []),
  ].map(messageFrom))];
  const deletedObjects = Array.isArray(task.result?.deleted) ? task.result.deleted as Record<string, unknown>[] : [];
  const cleanupProblems: unknown[] = task.type === 'storageCleanup' ? [...(Array.isArray(task.result?.skipped) ? task.result.skipped : []), ...(Array.isArray(task.result?.failures) ? task.result.failures : [])] : [];
  const paths = ['restoreTaskId', 'targetCommit', 'evidencePath', 'destinationPath', 'archivePath', 'path', 'remainingPath', 'sourcePath', 'projectId', 'targetProjectId', 'trashId', 'repositoryRoot', 'commit', 'head', 'previousBranch', 'branch', 'candidateId', 'appId', 'systemTrashPath', ...(task.type === 'delete' ? ['originalPath'] : [])].filter((key) => typeof displayResult?.[key] === 'string');
  const originalPath = task.result?.originalPath ?? task.error?.details?.originalPath;
  const cancelledPreparation = task.type === 'nodeInstallPrepare' && task.final && task.status === 'failed' && task.error?.code === 'NODE_INSTALL_CANCELLED';
  const statuses = { queued: '等待执行', running: '正在执行', succeeded: '已完成', failed: '失败', interrupted: '已中断' };
  const phases: Record<string, string> = { 'building-source': '构建源码', 'capturing-program': '准备更新来源', 'system-trash-freeze': '暂停项目写入', 'system-trash-intent': '准备移入系统废纸篓', 'system-trash-commit': '核对系统废纸篓结果', 'source-restore-preview': '核对源码取回范围', 'source-restore-materialize': '准备源码取回现场', 'source-restore-apply': '取回源码文件', 'source-restore-verify': '核对取回结果', 'source-restore-undo': '撤回源码取回', 'git-build-check': '检查源码版本状态', 'git-preview': '核对源码与版本范围', 'git-branch-commit': '创建分支并核对结果', 'git-initialize-candidate': '准备本机仓库', 'git-source-snapshot': '生成源码候选', 'git-write-objects': '写入源码版本', 'git-commit': '提交版本并核对结果', queued: '任务已接受', environment: '检查 Node / npm', 'input-digest': '核对构建输入', dependencies: '准备项目依赖', build: '编译网页', 'artifact-validation': '检查输入和产物', publish: '发布网页', completed: '已完成', 'snapshot-freeze': '生成项目副本', 'archive-publish': '校验与发布归档', 'maintenance-intent': '进入维护', 'maintenance-check': '检查项目完整性', 'maintenance-commit': '结束维护并更新页面身份', 'candidate-validation': '校验恢复候选', 'restore-maintenance': '准备恢复', 'preserve-original': '保留恢复前现场', 'install-candidate': '切换项目目录', 'restore-commit': '提交恢复', snapshot: '复制完整项目', archive: '生成归档', validation: '校验项目', baseline: '准备维护基线', candidate: '校验恢复候选', preserve: '保留恢复前现场', switch: '切换项目目录', commit: '提交恢复结果' };
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
  return <section className="build-task" aria-label={`${label}任务`}><div className="build-task-heading"><strong>{label} · {cancelledPreparation ? '已取消' : statuses[task.status]}</strong><span>{cancelledPreparation ? '安装准备已停止' : phases[task.phase] ?? task.phase}</span></div><code className="task-id">{task.taskId}</code>{task.error && <p className="task-error" role="alert">{task.error.message} ({task.error.code})</p>}{typeof outcome === 'string' && outcomes[outcome] && !(outcome === 'committed' && task.status === 'succeeded') && <p className={outcome === 'needs_attention' ? 'task-error' : undefined}>{outcomes[outcome]}</p>}{typeof originalPath === 'string' && task.type !== 'delete' && <details className="retained-original"><summary>恢复前完整现场已保留</summary><p>此目录包含替换前的完整项目，可供检查和后续处理。</p><code>{originalPath}</code></details>}{task.status === 'succeeded' && <p>{task.type === 'build' ? '新网页已发布。重新加载已有窗口前，请先处理未保存内容。' : task.type === 'restore' ? '项目已恢复，仍处于维护。完成检查并明确结束维护后，再重新加载项目。' : task.type === 'maintenanceBegin' ? '维护准备已完成。请核对基线与可修改状态后再修改项目。' : task.type === 'maintenanceEnd' ? '维护已结束。旧页面需要重新加载，请先复制或放弃未保存内容。' : successMessages[task.type] ?? '备份任务已完成，可在备份列表查看快照。'}</p>}{task.type === 'storageCleanup' && task.result && <p>实际删除：{bytesText((task.result.logicalBytes) as ByteCount | undefined)}</p>}{deletedObjects.length > 0 && <details className="retained-original"><summary>查看实际删除的对象</summary>{deletedObjects.map((item, index) => <p key={index}><code>{String(item.path ?? item.category ?? '已删除对象')}</code>{bytesText(item.logicalBytes as ByteCount | undefined)}</p>)}</details>}{task.result?.mode === 'maintenance' && <p className="maintenance-warning">项目仍在维护中{task.result.readyForChanges === false ? '，基线尚未准备好，请勿修改' : '，请检查后再显式结束维护'}。</p>}{cleanupProblems.map((item, index) => <p key={`cleanup:${index}`} className="task-error">{typeof item === 'object' && item ? [String((item as Record<string, unknown>).path ?? ''), String((item as Record<string, unknown>).message ?? (item as Record<string, unknown>).reason ?? (item as Record<string, unknown>).code ?? '未处理，请查看日志')].filter(Boolean).join(' · ') : String(item)}</p>)}{resultIssues.map((issue) => <p className="task-error" role="alert" key={issue}>{issue}</p>)}{paths.map((key) => <code className="task-result-path" key={key}>{({ restoreTaskId: '源码取回任务', targetCommit: '取回来源提交', head: '当前源码提交', previousBranch: '原分支', branch: '当前分支', candidateId: '安装候选', appId: '应用实例', repositoryRoot: '仓库目录', commit: '源码提交', evidencePath: '保留现场', destinationPath: '目标路径', archivePath: '归档路径', path: '路径', remainingPath: '尚未清除的目录', sourcePath: '保留的原路径', projectId: '项目 ID', targetProjectId: '新项目 ID', trashId: '旧版回收记录', systemTrashPath: '系统废纸篓位置', originalPath: '删除前目录' } as Record<string, string>)[key]}：{String(displayResult?.[key])}</code>)}{candidateCommit && <p>候选提交（是否已保存尚未确认）：<code>{candidateCommit}</code></p>}{!task.final && <p role="status">任务会在平台中继续执行。只有最终成功才表示操作完成。</p>}<button className="text-button" onClick={() => void readLogs()} disabled={reading}><Terminal size={13} />{reading ? '读取日志…' : cursor ? hasMore ? '继续读取日志' : '刷新日志' : '查看任务日志'}</button>{logsError && <p role="alert">{logsError}</p>}{gap && <p className="log-note">日志包含被截断的片段；此处仅显示最近 120 KB。</p>}{logs && <pre className="task-logs" tabIndex={0}>{logs}</pre>}</section>;
}

