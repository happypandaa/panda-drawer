import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { bytesText } from './storage-utils';
import type { ByteCount } from './types';
import { ProcessFailureDetails } from './ProcessFailureDetails';

const labels: Record<string, string> = { nodeGitImport: '从 Git 添加', gitSourceRestoreApply: '取回已保存源码', gitSourceRestoreContinue: '继续源码取回', gitSourceRestoreUndo: '撤回源码取回', gitBranchCreate: '创建定制分支', gitInitialize: '初始化源码版本', gitSave: '保存源码版本', gitDataSave: '保存源码和数据版本', gitDataSaveFinalize: '完成数据版本保存', build: '构建', export: '导出项目', import: '导入项目', copy: '复制项目', delete: '永久删除项目', workspaceMigration: '迁移工作区', workspaceMigrationCleanup: '删除迁移原目录', workspaceCleanupPreview: '扫描历史资料', workspaceCleanup: '清理历史资料', storageScan: '统计项目占用', storageCleanup: '清理可重新生成文件' };
Object.assign(labels, { gitDataRestoreApply: '取回业务数据', gitDataRestoreContinue: '继续数据取回', gitDataRestoreUndo: '撤回数据取回' });
export const taskLabel = (type: string) => labels[type] ?? '项目任务';

function BuildArtifactCleanup({ task, outcome }: { task: TaskRecord; outcome: unknown }) {
  if (task.type !== 'build' || (task.status !== 'succeeded' && outcome !== 'committed')) return null;
  const candidate = task.result?.artifactCleanup ?? task.error?.details?.artifactCleanup;
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) return null;
  const report = candidate as Record<string, unknown>;
  if (report.cleanupPending !== true) return null;
  const paths = [...new Set((Array.isArray(report.remainingPaths) ? report.remainingPaths : []).filter((path): path is string => typeof path === 'string'))];
  const issues = [...new Set((Array.isArray(report.issues) ? report.issues : []).map(messageFrom))];
  return <div className="artifact-cleanup-pending" role="status">
    <p>网页发布已提交，旧构建收尾尚未完成。下次构建或平台启动时会重新检查。</p>
    {paths.map(path => <code className="task-result-path" key={path}>需要检查的位置：{path}</code>)}
    {issues.map(issue => <p className="task-error" key={issue}>{issue}</p>)}
  </div>;
}

export function ProjectTask({ task }: { task: TaskRecord }) {
  const label = taskLabel(task.type);
  const outcome = task.result?.operationOutcome ?? task.error?.details?.operationOutcome;
  const gitOperations: Record<string, string> = { gitInitialize: 'initialize', gitSave: 'save', gitDataSave: 'save-data', gitDataSaveFinalize: 'save-data-finalize', gitBranchCreate: 'create-branch', gitSourceRestoreApply: 'source-restore', gitSourceRestoreContinue: 'source-restore-continue', gitSourceRestoreUndo: 'source-restore-undo' };
  Object.assign(gitOperations, { gitDataRestoreApply: 'data-restore-apply', gitDataRestoreContinue: 'data-restore-continue', gitDataRestoreUndo: 'data-restore-undo' });
  const isDataRestore = task.type.startsWith('gitDataRestore');
  const isSourceRestore = task.type.startsWith('gitSourceRestore');
  const isGit = Boolean(gitOperations[task.type]);
  const retainedResult = task.error?.details?.result;
  const displayResult = task.result ?? (isGit && (outcome === 'committed' || isDataRestore) && retainedResult && typeof retainedResult === 'object' && !Array.isArray(retainedResult) && (retainedResult as Record<string, unknown>).operationOutcome === outcome && (retainedResult as Record<string, unknown>).operation === gitOperations[task.type] ? retainedResult as Record<string, unknown> : null);
  const candidateCommit = isGit && outcome === 'unknown' && typeof task.error?.details?.candidateCommit === 'string' ? task.error.details.candidateCommit : null;
  const outcomes: Record<string, string> = { deleted: '项目文件已永久删除，本次删除不可撤销。平台未为本次删除保存可恢复副本；其他项目和操作记录不受影响。', delete_incomplete: '永久删除未完成，部分项目文件可能已删除，不能恢复为完整项目。请核对剩余目录和错误信息，处理失败原因后明确重试；不会自动继续删除。', not_deleted: '本次尚未执行删除。请核对错误与保护状态后，再明确重试。', rolled_back: '已撤回这次源码取回，请核对恢复状态。数据库和当前程序不受影响。', needs_attention: '操作结果需要检查，相关目录已保留。请先查看错误信息和当前状态，不要重复提交。', committed: isSourceRestore ? '源码操作已应用，但收尾未全部确认。请核对源码恢复状态，不要重复提交；这不代表已经保存 Git 版本或构建。' : '操作已提交，但收尾未全部完成。请核对当前状态，不要重复提交。', not_started: '操作尚未执行，原内容保持不变。', committed_needs_attention: '新工作区路径已提交，但重新连接或收尾失败。原目录仍保留；请核对目标磁盘与错误信息，修复后重新启动平台。不要重试迁移或切回原目录。', unknown: '操作结果尚不能确认，现场已保留。请核对当前状态和错误信息，不要重复提交。', not_committed: isSourceRestore ? '已确认本次没有应用源码修改。请核对恢复状态；未自动保存版本或构建。' : isGit ? '本次版本操作未提交。源码和数据保持原状；请检查保留现场。' : '迁移尚未提交，原工作区仍是当前工作区。目标现场可能仍保留，请查看路径和错误信息。' };
  if (isDataRestore) Object.assign(outcomes, {
    committed: '数据切换已提交，但收尾尚未完全确认。请核对数据恢复状态，勿重复提交；旧页面暂不能继续使用。',
    not_committed: '页面身份尚未提交，部分数据可能已移动。请核对数据恢复状态并明确继续或撤回，勿直接使用旧页面或重复提交。',
    unknown: '数据取回结果尚不能确认。现场保留，业务访问暂时停止；请刷新数据恢复状态并核对处理信息。',
    rolled_back: '本次数据取回已撤回。请核对恢复状态，处理未保存内容后重新打开项目。',
  });
  if (task.type === 'workspaceMigrationCleanup') Object.assign(outcomes, {
    committed: '当前工作区可继续使用，原目录删除尚未完成。请刷新存储位置，核对残留路径后重试。',
    committed_needs_attention: '当前工作区可继续使用，原目录删除需要检查。请刷新存储位置并核对错误和残留路径。',
    unknown: '原目录删除结果尚未确认。当前工作区可继续使用；请查询同一任务和存储位置，不自动重复提交。',
    not_committed: '本次尚未完成原目录删除。当前工作区可继续使用，请核对错误后重试。',
  });
  const dataRestoreSuccess = displayResult?.pending === true || displayResult?.businessBlocked === true
    ? '数据取回仍需完成收尾，业务访问暂时停止。请核对数据恢复状态。'
    : displayResult?.dataState === 'undone' ? '本次数据取回已撤回。原数据恢复后已更新页面身份，请重新打开项目。'
      : displayResult?.dataState === 'applied' ? '所选版本的数据库和附件已取回，原数据现场保留。请处理未保存内容后重新打开项目。'
        : '数据任务已结束，请核对数据恢复状态后再使用项目。';
  const sourceRestoreSuccess = displayResult?.state === 'not-applied' ? '本次没有应用源码修改。请核对恢复状态。' : displayResult?.state === 'undone' ? '这次源码取回已撤回。源码恢复到操作前内容；数据库和当前程序保持不变。' : displayResult?.state === 'completed' ? '源码已取回为待保存修改。当前分支、提交和暂存区保持不变；未自动保存版本或构建，数据库和当前程序保持不变。' : '源码任务已结束，请核对恢复状态；未自动保存版本或构建。';
  const sourceCleanup = task.result?.sourceCleanup && typeof task.result.sourceCleanup === 'object' ? task.result.sourceCleanup as Record<string, unknown> : null;
  const migrationCleanupPending = task.result?.cleanupPending === true || sourceCleanup?.cleanupPending === true;
  const migrationSourceCompleted = sourceCleanup?.completed === true || task.result?.sourceRetained === false;
  const migrationSuccess = migrationCleanupPending ? '新工作区可继续使用，原目录删除尚未完成。请在存储位置核对残留路径并重试。' : migrationSourceCompleted ? '工作区迁移已完成，原目录收尾已完成。处理草稿后，请重新进入项目并重新复制 AI 路径。' : '迁移任务已结束，请在存储位置核对当前工作区和原目录状态。';
  const successMessages: Record<string, string> = { nodeGitImport: '项目已添加，临时文件已清理。', gitSourceRestoreApply: sourceRestoreSuccess, gitSourceRestoreContinue: sourceRestoreSuccess, gitSourceRestoreUndo: sourceRestoreSuccess, gitBranchCreate: '已创建并切换定制分支。源码、数据库和当前运行网页保持不变。', gitInitialize: '本机源码仓库已建立；尚未保存首个版本。', gitSave: '源码版本已保存。本次未重新捕获业务数据库和附件，已有数据版本保持。', gitDataSave: '源码及一致性数据库、附件版本已保存在本机 Git。当前业务数据和运行程序保持。', gitDataSaveFinalize: outcome === 'not_committed' ? '已确认此前保存没有提交。当前源码、业务数据库和附件保持，候选现场保留。' : '已完成此前已提交数据版本的保存收尾，当前业务数据库和附件保持。', workspaceCleanupPreview: '检查已完成。核对所列范围并明确选择后，才会永久清理。', workspaceCleanup: '历史清理任务已结束，请核对实际删除、失败和残留范围。逻辑字节不等于磁盘实际释放量。', storageScan: '扫描已完成，请查看空间分类、完成状态与扫描时间。', storageCleanup: '清理已结束。实际删除逻辑字节与文件系统可用空间变化可能不同，请查看结果和错误信息。', export: '项目归档已完成，可从下方路径找到 ZIP。', import: '项目已导入，请在项目列表中查看状态并打开。', copy: '独立副本已创建，请在项目列表中查看。', delete: typeof outcome === 'string' && outcomes[outcome] ? outcomes[outcome] : '删除任务已结束，但永久删除结果尚不能确认。请核对项目状态和错误信息，不要重复提交。', workspaceMigration: migrationSuccess, workspaceMigrationCleanup: migrationCleanupPending ? '新工作区可继续使用，原目录删除仍需检查。请刷新状态并核对残留路径。' : '迁移原目录收尾已完成，当前工作区可继续使用。' };
  Object.assign(successMessages, { gitDataRestoreApply: dataRestoreSuccess, gitDataRestoreContinue: dataRestoreSuccess, gitDataRestoreUndo: dataRestoreSuccess });
  const durabilityWarning = task.error?.details?.durabilityWarning;
  const resultIssues = [...new Set([
    ...(Array.isArray(displayResult?.issues) ? displayResult.issues : []),
    ...(durabilityWarning ? [durabilityWarning] : []),
    ...(displayResult?.cleanupWarning ? [displayResult.cleanupWarning] : []),
  ].map(messageFrom))];
  const deletedObjects = Array.isArray(task.result?.deleted) ? task.result.deleted as Record<string, unknown>[] : [];
  const cleanupProblems: unknown[] = task.type === 'storageCleanup' ? [...(Array.isArray(task.result?.skipped) ? task.result.skipped : []), ...(Array.isArray(task.result?.failures) ? task.result.failures : [])] : [];
  const pathResult = displayResult ?? (isDataRestore ? task.error?.details : null);
  const paths = ['restoreTaskId', 'targetCommit', 'retainedPath', 'evidencePath', 'destinationPath', 'archivePath', 'path', 'remainingPath', 'sourcePath', 'projectId', 'targetProjectId', 'repositoryRoot', 'commit', 'head', 'previousBranch', 'branch', 'candidateId', 'appId'].filter((key) => typeof pathResult?.[key] === 'string');
  const originalPath = task.result?.originalPath ?? task.error?.details?.originalPath;
  const cancelledPreparation = task.type === 'nodeGitImport' && task.final && task.status === 'failed' && task.error?.code === 'NODE_GIT_IMPORT_CANCELLED';
  const statuses = { queued: '等待执行', running: '正在执行', succeeded: '已完成', failed: '失败', interrupted: '已中断' };
  const phases: Record<string, string> = { 'cleaning-old-workspace': '删除迁移原目录', 'waiting-for-old-io': '等待原目录访问结束', 'waiting-for-native-drain': '等待应用停止访问原目录', reopening: '在新位置重开工作区', downloading: '下载源码', checking: '检查项目', publishing: '发布项目文件夹', registering: '登记项目', cleanup: '清理临时文件', 'building-source': '构建源码', 'capturing-program': '准备更新来源', 'delete-freeze': '停止项目访问', 'delete-prepared': '核对永久删除范围', 'delete-intent': '永久删除项目文件', 'source-restore-preview': '核对源码取回范围', 'source-restore-materialize': '准备源码取回现场', 'source-restore-apply': '取回源码文件', 'source-restore-verify': '核对取回结果', 'source-restore-undo': '撤回源码取回', 'git-build-check': '检查源码版本状态', 'git-preview': '核对源码与版本范围', 'git-branch-commit': '创建分支并核对结果', 'git-initialize-candidate': '准备本机仓库', 'git-source-snapshot': '生成源码候选', 'git-data-snapshot': '捕获数据库与附件', 'git-data-finalize': '核对并完成数据版本保存', 'git-write-objects': '写入源码版本', 'git-commit': '提交版本并核对结果', queued: '任务已接受', environment: '检查 Node / npm', 'input-digest': '核对构建输入', dependencies: '准备项目依赖', build: '编译网页', 'artifact-validation': '检查输入和产物', publish: '发布网页', completed: '已完成', 'snapshot-freeze': '生成项目副本', 'archive-publish': '校验与发布归档', snapshot: '复制完整项目', archive: '生成归档', validation: '校验项目' };
  return <section className="build-task" aria-label={`${label}任务`}><div className="build-task-heading"><strong>{label} · {cancelledPreparation ? '已取消' : statuses[task.status]}</strong><span>{cancelledPreparation ? '添加已停止' : phases[task.phase] ?? task.phase}</span></div><code className="task-id">{task.taskId}</code>{task.error && <p className="task-error" role="alert">{task.error.message} ({task.error.code})</p>}{task.error && <ProcessFailureDetails value={task.error.details?.processDiagnostics} />}{typeof outcome === 'string' && outcomes[outcome] && !(task.status === 'succeeded' && (outcome === 'committed' || task.type === 'delete')) && <p className={outcome === 'needs_attention' ? 'task-error' : undefined}>{outcomes[outcome]}</p>}{typeof originalPath === 'string' && <details className="retained-original"><summary>恢复前完整现场已保留</summary><p>此目录包含替换前的完整项目，可供检查和后续处理。</p><code>{originalPath}</code></details>}{task.status === 'succeeded' && <p>{task.type === 'build' ? '新网页已发布。重新加载已有窗口前，请先处理未保存内容。' : successMessages[task.type] ?? '任务已完成。'}</p>}<BuildArtifactCleanup task={task} outcome={outcome} />{task.type === 'storageCleanup' && task.result && <p>实际删除：{bytesText((task.result.logicalBytes) as ByteCount | undefined)}</p>}{deletedObjects.length > 0 && <details className="retained-original"><summary>查看实际删除的对象</summary>{deletedObjects.map((item, index) => <p key={index}><code>{String(item.path ?? item.category ?? '已删除对象')}</code>{bytesText(item.logicalBytes as ByteCount | undefined)}</p>)}</details>}{cleanupProblems.map((item, index) => <p key={`cleanup:${index}`} className="task-error">{typeof item === 'object' && item ? [String((item as Record<string, unknown>).path ?? ''), String((item as Record<string, unknown>).message ?? (item as Record<string, unknown>).reason ?? (item as Record<string, unknown>).code ?? '未处理，请查看错误信息')].filter(Boolean).join(' · ') : String(item)}</p>)}{resultIssues.map((issue) => <p className="task-error" role="alert" key={issue}>{issue}</p>)}{paths.map((key) => <code className="task-result-path" key={key}>{({ restoreTaskId: isDataRestore ? '数据取回任务' : '源码取回任务', targetCommit: '取回来源提交', head: '当前源码提交', previousBranch: '原分支', branch: '当前分支', candidateId: '安装候选', appId: '应用实例', repositoryRoot: '仓库目录', commit: '源码提交', retainedPath: '保留的数据恢复现场', evidencePath: '保留现场', destinationPath: '目标路径', archivePath: '归档路径', path: '路径', remainingPath: '尚未清除的目录', sourcePath: ['workspaceMigration', 'workspaceMigrationCleanup'].includes(task.type) ? '迁移原目录' : '保留的原路径', projectId: '项目 ID', targetProjectId: '新项目 ID' } as Record<string, string>)[key]}：{String(pathResult?.[key])}</code>)}{candidateCommit && <p>候选提交（是否已保存尚未确认）：<code>{candidateCommit}</code></p>}{!task.final && <p role="status">任务会在平台中继续执行。只有最终成功才表示操作完成。</p>}</section>;
}

