import type { TaskRecord, ByteCount } from './types';
import { messageFrom } from './ui-utils';
import { bytesText } from './storage-utils';
import { ProcessFailureDetails } from './ProcessFailureDetails';

const labels: Record<string, string> = { nodeGitImport: '从 Git 添加', build: '构建', delete: '永久删除项目', workspaceMigration: '迁移工作区', workspaceMigrationCleanup: '删除迁移原目录', storageScan: '统计项目占用', storageCleanup: '清理可重新生成文件' };
export const taskLabel = (type: string) => labels[type] ?? '项目任务';

export function ProjectTask({ task }: { task: TaskRecord }) {
  const label = taskLabel(task.type);
  const result = task.result;
  const outcome = result?.operationOutcome ?? task.error?.details?.operationOutcome;
  const sourceCleanup = result?.sourceCleanup && typeof result.sourceCleanup === 'object' ? result.sourceCleanup as Record<string, unknown> : null;
  const migrationPending = result?.cleanupPending === true || sourceCleanup?.cleanupPending === true;
  const migrationDone = sourceCleanup?.completed === true || result?.sourceRetained === false;
  const outcomes: Record<string, string> = {
    deleted: result?.sourcePreserved === true ? '平台登记和浏览器数据已清除，项目文件夹与文件数据保留。' : '项目文件已永久删除，浏览器数据已清除，本次删除不可撤销。',
    delete_incomplete: '永久删除未完成，部分项目文件可能已删除。请核对剩余目录与错误后重试。',
    not_deleted: '本次尚未执行删除，请核对当前状态后重试。',
    needs_attention: '操作结果需要检查，相关目录已保留，请勿重复提交。',
    committed: '操作已提交，收尾尚未全部完成，请核对当前状态。',
    not_started: '操作尚未执行，原内容保持不变。',
    committed_needs_attention: '新位置已提交，重新连接或收尾尚未完成，请核对当前工作区和残留路径。',
    unknown: '操作结果尚不能确认，请查询同一任务，不要重复提交。',
    not_committed: '操作尚未提交，当前工作区保持，请核对任务结果。',
  };
  const success: Record<string, string> = {
    nodeGitImport: '项目已添加，临时文件已清理。',
    delete: typeof outcome === 'string' && outcomes[outcome] ? outcomes[outcome] : '删除任务已结束，但永久删除结果尚不能确认。请核对项目状态，不要重复提交。',
    storageScan: '空间统计已完成。',
    storageCleanup: '可重新生成文件清理已完成，请核对实际结果。',
    workspaceMigrationCleanup: '迁移原目录收尾已完成。',
    workspaceMigration: migrationPending ? '新工作区可继续使用，原目录删除尚未完成，请到设置中核对并重试。' : migrationDone ? '工作区迁移和原目录收尾已完成，请重新打开应用。' : '迁移任务已结束，请核对当前工作区和原目录状态。',
  };
  const status = { queued: '等待执行', running: '正在执行', waitingConfirmation: '需要确认', succeeded: '已完成', failed: '失败', interrupted: '已中断' };
  const phases: Record<string, string> = {
    'cleaning-old-workspace': '删除迁移原目录', 'waiting-for-old-io': '等待原目录访问结束', 'waiting-for-native-drain': '等待应用停止访问原目录', reopening: '在新位置重开工作区',
    downloading: '下载源码', checking: '检查项目', publishing: '发布项目文件夹', registering: '登记项目', cleanup: '清理临时文件',
    'delete-freeze': '停止项目访问', 'delete-prepared': '核对永久删除范围', 'delete-intent': '永久删除项目文件',
    'awaited-build-confirmation': '等待构建并添加确认', queued: '任务已接受', environment: '检查 Node / npm', dependencies: '准备项目依赖', build: '编译网页', 'entry-validation': '检查网页入口', completed: '已完成',
  };
  const issues = [...new Set([
    ...(Array.isArray(result?.issues) ? result.issues : []),
    ...(result?.cleanupWarning ? [result.cleanupWarning] : []),
    ...(task.error?.details?.durabilityWarning ? [task.error.details.durabilityWarning] : []),
  ].map(messageFrom))];
  const deleted = Array.isArray(result?.deleted) ? result.deleted as Record<string, unknown>[] : [];
  const problems = task.type === 'storageCleanup' ? [...(Array.isArray(result?.skipped) ? result.skipped : []), ...(Array.isArray(result?.failures) ? result.failures : [])] : [];
  const pathLabels: Record<string, string> = { destinationPath: '目标路径', path: '路径', remainingPath: '尚未清除的目录', sourcePath: '原目录', projectId: '项目 ID', appId: '应用实例' };
  const cancelled = task.type === 'nodeGitImport' && task.final && task.error?.code === 'NODE_GIT_IMPORT_CANCELLED';
  return <section className="build-task" aria-label={`${label}任务`}>
    <div className="build-task-heading"><strong>{label} · {cancelled ? '已取消' : status[task.status]}</strong><span>{cancelled ? '添加已停止' : phases[task.phase] ?? task.phase}</span></div>
    <code className="task-id">{task.taskId}</code>
    {task.error && <><p className="task-error" role="alert">{task.error.message} ({task.error.code})</p><ProcessFailureDetails value={task.error.details?.processDiagnostics} /></>}
    {typeof outcome === 'string' && outcomes[outcome] && !(task.status === 'succeeded' && (outcome === 'committed' || task.type === 'delete')) && <p>{outcomes[outcome]}</p>}
    {task.status === 'succeeded' && <p>{task.type === 'build' ? '构建已完成，打开应用即可使用当前文件。' : success[task.type] ?? '任务已完成。'}</p>}
    {task.type === 'storageCleanup' && result && <p>实际删除：{bytesText(result.logicalBytes as ByteCount | undefined)}</p>}
    {deleted.length > 0 && <details className="retained-original"><summary>查看实际删除的对象</summary>{deleted.map((item, index) => <p key={index}><code>{String(item.path ?? item.category ?? '已删除对象')}</code>{bytesText(item.logicalBytes as ByteCount | undefined)}</p>)}</details>}
    {problems.map((item, index) => <p className="task-error" key={index}>{messageFrom(item)}</p>)}
    {issues.map(issue => <p className="task-error" role="alert" key={issue}>{issue}</p>)}
    {Object.entries(pathLabels).filter(([key]) => typeof result?.[key] === 'string').map(([key, title]) => <code className="task-result-path" key={key}>{title}：{String(result?.[key])}</code>)}
    {!task.final && <p role="status">任务会在平台中继续执行。只有最终成功才表示操作完成。</p>}
  </section>;
}
