import { t } from './i18n';
import type { TaskRecord, ByteCount } from './types';
import { messageFrom } from './ui-utils';
import { bytesText } from './storage-utils';
import { ProcessFailureDetails } from './ProcessFailureDetails';
import { deletionTaskOwnership } from './project-deletion';
import type { DeletePresentation } from './project-deletion';

const labels: Record<string, string> = { nodeGitImport: '从 Git 添加', build: '构建', workspaceMigration: '移动存放位置', workspaceMigrationCleanup: '删除移动原目录', storageScan: '统计应用占用', storageCleanup: '清理可重新生成文件' };
export function taskLabel(type: string, ownership?: DeletePresentation['ownership']) {
  if (type === 'delete') return ownership === 'platform' ? t("永久删除应用") : ownership === 'user' ? t("移出应用") : t("处理应用");
  return t(labels[type] ?? '应用任务');
}

export function ProjectTask({ task, deletionOwnership }: { task: TaskRecord; deletionOwnership?: DeletePresentation['ownership'] }) {
  const ownership = deletionTaskOwnership(task, deletionOwnership);
  const platformDelete = ownership === 'platform';
  const userRemove = ownership === 'user';
  const label = taskLabel(task.type, ownership);
  const result = task.result;
  const outcome = result?.operationOutcome ?? task.error?.details?.operationOutcome;
  const sourceCleanup = result?.sourceCleanup && typeof result.sourceCleanup === 'object' ? result.sourceCleanup as Record<string, unknown> : null;
  const migrationPending = result?.cleanupPending === true || sourceCleanup?.cleanupPending === true;
  const migrationDone = sourceCleanup?.completed === true || result?.sourceRetained === false;
  const outcomes: Record<string, string> = {
    deleted: userRemove ? t("应用已移出，小抽屉已清除应用登记和浏览器数据。")
      : platformDelete ? t("应用文件已永久删除，浏览器数据已清除，本次删除不可撤销。") : t("应用清理已完成。"),
    delete_incomplete: platformDelete ? t("永久删除未完成，部分应用文件可能已删除。请核对剩余目录与错误后重试。")
      : userRemove ? t("移出未完成，请核对小抽屉里的剩余材料后重试。") : t("清理未完成，请核对剩余材料后重试。"),
    not_deleted: platformDelete ? t("本次尚未执行删除，请核对当前状态后重试。")
      : userRemove ? t("本次尚未移出应用，请核对当前状态后重试。") : t("本次尚未执行清理，请核对当前状态后重试。"),
    needs_attention: t("操作结果需要检查，相关目录已保留，请勿重复提交。"),
    committed: t("操作已提交，收尾尚未全部完成，请核对当前状态。"),
    not_started: t("操作尚未执行，原内容保持不变。"),
    committed_needs_attention: t("新位置已提交，重新连接或收尾尚未完成，请核对当前存放位置和残留路径。"),
    unknown: t("操作结果尚不能确认，请查询同一任务，不要重复提交。"),
    not_committed: t("操作尚未提交，当前存放位置保持，请核对任务结果。"),
  };
  const success: Record<string, string> = {
    nodeGitImport: t("应用已添加，临时文件已清理。"),
    delete: typeof outcome === 'string' && outcomes[outcome] ? outcomes[outcome] : platformDelete
      ? t("删除任务已结束，但永久删除结果尚不能确认。请核对应用状态，不要重复提交。")
      : t("任务已结束，结果仍待核对。请查看应用状态，不要重复提交。"),
    storageScan: t("空间统计已完成。"),
    storageCleanup: t("可重新生成文件清理已完成，请核对实际结果。"),
    workspaceMigrationCleanup: t("移动原目录收尾已完成。"),
    workspaceMigration: migrationPending ? t("新位置可继续使用，原目录删除尚未完成，请到设置中核对并重试。") : migrationDone ? t("存放位置移动和原目录收尾已完成，请重新打开应用。") : t("移动任务已结束，请核对当前存放位置和原目录状态。"),
  };
  const status = { queued: t("等待执行"), running: t("正在执行"), waitingConfirmation: t("需要确认"), succeeded: t("已完成"), failed: t("失败"), interrupted: t("已中断") };
  const phases: Record<string, string> = {
    'cleaning-old-workspace': t("删除移动原目录"), 'waiting-for-old-io': t("等待原目录访问结束"), 'waiting-for-native-drain': t("等待应用停止访问原目录"), reopening: t("在新位置重新打开"),
    downloading: t("下载源码"), checking: t("检查应用"), publishing: t("发布应用文件夹"), registering: t("登记应用"), cleanup: t("清理临时文件"),
    'delete-freeze': t("停止应用访问"),
    'delete-prepared': platformDelete ? t("核对永久删除范围") : t("核对清理范围"),
    'delete-intent': platformDelete ? t("永久删除应用文件") : userRemove ? t("正在清除小抽屉里的配置和浏览器数据") : t("正在清理已核实的材料"),
    'delete-finalizing': t("正在清除应用登记与关联材料"),
    'awaited-build-confirmation': t("等待构建并添加确认"), queued: t("任务已接受"), environment: t("检查 Node / npm"), dependencies: t("准备应用依赖"), build: t("编译网页"), 'entry-validation': t("检查网页入口"), completed: t("已完成"),
  };
  const issues = [...new Set([
    ...(Array.isArray(result?.issues) ? result.issues : []),
    ...(result?.cleanupWarning ? [result.cleanupWarning] : []),
    ...(task.error?.details?.durabilityWarning ? [task.error.details.durabilityWarning] : []),
  ].map(messageFrom))];
  const deleted = Array.isArray(result?.deleted) ? result.deleted as Record<string, unknown>[] : [];
  const problems = task.type === 'storageCleanup' ? [...(Array.isArray(result?.skipped) ? result.skipped : []), ...(Array.isArray(result?.failures) ? result.failures : [])] : [];
  const pathLabels: Record<string, string> = { destinationPath: t("目标路径"), path: t("路径"), remainingPath: t("尚未清除的目录"), sourcePath: t("原目录"), projectId: t("应用 ID"), appId: t("应用实例") };
  const cancelled = task.type === 'nodeGitImport' && task.final && task.error?.code === 'NODE_GIT_IMPORT_CANCELLED';
  return <section className="build-task" aria-label={t("{p0}任务", { p0: String(label) })}>
    <div className="build-task-heading"><strong>{label} · {cancelled ? t("已取消") : status[task.status]}</strong><span>{cancelled ? t("添加已停止") : phases[task.phase] ?? task.phase}</span></div>
    <code className="task-id">{task.taskId}</code>
    {task.error && <><p className="task-error" role="alert">{t(task.error.message)} ({task.error.code})</p><ProcessFailureDetails value={task.error.details?.processDiagnostics} /></>}
    {typeof outcome === 'string' && outcomes[outcome] && !(task.status === 'succeeded' && (outcome === 'committed' || task.type === 'delete')) && <p>{outcomes[outcome]}</p>}
    {task.status === 'succeeded' && <p>{task.type === 'build' ? t("构建已完成，打开应用即可使用当前文件。") : success[task.type] ?? t("任务已完成。")}</p>}
    {task.type === 'storageCleanup' && result && <p>{t("实际删除：")}{bytesText(result.logicalBytes as ByteCount | undefined)}</p>}
    {deleted.length > 0 && <details className="retained-original"><summary>{t("查看实际删除的对象")}</summary>{deleted.map((item, index) => <p key={index}><code>{String(item.path ?? item.category ?? t("已删除对象"))}</code>{bytesText(item.logicalBytes as ByteCount | undefined)}</p>)}</details>}
    {problems.map((item, index) => <p className="task-error" key={index}>{t(messageFrom(item))}</p>)}
    {issues.map(issue => <p className="task-error" role="alert" key={issue}>{t(issue)}</p>)}
    {Object.entries(pathLabels).filter(([key]) => typeof result?.[key] === 'string').map(([key, title]) => <code className="task-result-path" key={key}>{t("{label}：{value}", { label: title, value: String(result?.[key]) })}</code>)}
    {!task.final && <p role="status">{t("任务会在平台中继续执行。只有最终成功才表示操作完成。")}</p>}
  </section>;
}
