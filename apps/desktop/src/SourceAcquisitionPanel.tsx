import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { Download, FolderOpen } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ImportProgress } from './ImportProgress';
import { ProjectTask } from './ProjectTask';
import { ProcessFailureDetails } from './ProcessFailureDetails';
import { checkedProcessDiagnostics } from './process-diagnostics';
import { AiProjectHelp } from './AiProjectHelp';
import { SourceImportError, SourceImportSession } from './source-import';
import { gitRepositoryAddress, sameGitRepository } from './git-repository';
import type { GitRepositorySource } from './git-repository';
import type { NodeAppDetail } from './node-app-types';
import type { ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { onProjectDeleted } from './project-deletion';
import { canDismissGitImport } from './git-import-view-state';
import type { GitImportViewState } from './git-import-view-state';
import { useFrontendPlatform } from './frontend-platform';
export { gitRepositoryAddress } from './git-repository';

type ExistingRepository = GitRepositorySource & { appId: string; name: string; runtime?: 'web' | 'node' };
export type SourceAcquisitionPanelProps = {
  disabled: boolean; instanceId?: string; open?: boolean; projectDirectory?: string; platformVersion?: string;
  existingRepositories?: ExistingRepository[];
  onClose: (options?: { keepSession: boolean }) => void; onBackground?: () => void; onAttention?: () => void;
  onOpenLocal?: () => void; onOpenSettings?: () => void; onOpenExisting?: (appId: string) => void;
  onTask: (task: TaskRecord) => void; onInstalled?: () => void; onEnter?: (app: NodeAppDetail) => void;
  onWebEnter?: (app: ProjectInfo) => void;
  onStateChange?: (state: GitImportViewState | null) => void;
};
const phases = ['下载源码', '检查项目', '准备依赖', '添加应用', '清理临时文件'];
function phaseIndex(phase?: string) {
  return phase === 'completed' || phase === 'cleanup' ? 4 : phase === 'registering' ? 3 : phase === 'dependencies' ? 2 : ['checking', 'publishing', 'inspecting'].includes(phase ?? '') ? 1 : 0;
}
export function SourceAcquisitionPanel(props: SourceAcquisitionPanelProps) {
  return <GitImportWorkflow key={props.instanceId ?? ''} {...props} />;
}
function GitImportWorkflow({ disabled, instanceId, open = true, projectDirectory: suppliedDirectory, platformVersion, existingRepositories, onClose, onBackground, onAttention, onOpenLocal, onOpenSettings, onOpenExisting, onTask, onInstalled, onEnter, onWebEnter, onStateChange }: SourceAcquisitionPanelProps) {
  const frontendPlatform = useFrontendPlatform();
  const [url, setUrl] = useState(''); const [branch, setBranch] = useState('');
  const [directory, setDirectory] = useState(suppliedDirectory ?? '');
  const [view, setView] = useState<'form' | 'duplicate' | 'running' | 'waiting' | 'failed' | 'unknown' | 'success'>('form');
  const [duplicate, setDuplicate] = useState<ExistingRepository | null>(null);
  const [error, setError] = useState(''); const [busy, setBusy] = useState(false); const [cancelling, setCancelling] = useState(false);
  const [, redraw] = useState(0); const mounted = useRef(true); const generation = useRef(0); const pending = useRef(false);
  const session = useRef<SourceImportSession | null>(null);
  const callbacks = useRef({ onTask, onInstalled, onAttention, onClose }); callbacks.current = { onTask, onInstalled, onAttention, onClose };
  const report = useRef(onStateChange); report.current = onStateChange;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current++; }; }, []);
  useEffect(() => onProjectDeleted(notice => {
    if (instanceId && notice.instanceId !== instanceId) return;
    session.current?.revoke(notice);
  }), [instanceId]);
  useEffect(() => {
    if (suppliedDirectory) { setDirectory(suppliedDirectory); return; }
    let live = true;
    void invoke<{ projectDirectory: string }>('source_import_location').then(value => { if (live) setDirectory(value.projectDirectory); }).catch(cause => { if (live) setError(messageFrom(cause)); });
    return () => { live = false; };
  }, [suppliedDirectory]);
  const flow = session.current; const snapshot = flow?.snapshot; const task = flow?.task;
  const address = gitRepositoryAddress(url); const committed = snapshot?.commitState === 'committed';
  const sessionId = flow?.importId;
  const warning = flow?.warning ?? '';
  const canLeaveFailedFlow = canDismissGitImport({ sessionId: sessionId ?? '', url, name: address?.repository ?? '', task: task ?? null, snapshot: snapshot ?? null, view: 'failed', busy, error, warning });
  useEffect(() => {
    report.current?.(sessionId && !['form', 'duplicate'].includes(view) ? {
      sessionId, url, name: address?.repository || t("正在添加的应用"), task: task ?? null,
      snapshot: snapshot ?? null, view: view as GitImportViewState['view'], busy, error, warning,
    } : null);
  }, [sessionId, url, address?.repository, task, snapshot, view, busy, error, warning]);
  const diagnostics = checkedProcessDiagnostics(task?.error?.details?.processDiagnostics);
  const current = (id: number) => mounted.current && generation.current === id;
  function acceptResult() {
    const value = session.current?.snapshot;
    if (!value) { setView('unknown'); return; }
    if (value.task.status === 'waitingConfirmation') setView('waiting');
    else if (!value.task.final) setView('running');
    else if (value.commitState === 'unknown') setView('unknown');
    else if (value.commitState === 'committed') { setView('success'); callbacks.current.onInstalled?.(); }
    else { setView('failed'); setError(messageFrom(value.task.error ?? t("添加未完成。"))); }
    if (value.task.status === 'waitingConfirmation' || value.commitState === 'unknown' || value.cleanupPending || value.task.status === 'failed' || value.task.status === 'interrupted') callbacks.current.onAttention?.();
  }
  async function start(allowDuplicate = false) {
    if (pending.current || disabled || !address) return;
    const found = existingRepositories?.find(item => sameGitRepository(item, { url: address.url, branch }));
    if (found && !allowDuplicate) { setDuplicate(found); setView('duplicate'); return; }
    pending.current = true; setBusy(true); setError(''); setCancelling(false); setView('running');
    const id = generation.current;
    const value: SourceImportSession = new SourceImportSession(invoke, {
      isCurrent: (): boolean => current(id) && session.current === value,
      onTask: task => callbacks.current.onTask(task), onChange: () => redraw(count => count + 1),
      onRevoked: () => { generation.current++; session.current = null; pending.current = false; setBusy(false); callbacks.current.onClose(); },
    });
    session.current = value;
    try { await value.run({ url: address.url, branch: branch.trim() || null }); if (current(id)) acceptResult(); }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); setView(cause instanceof SourceImportError && cause.recovery ? 'unknown' : 'failed'); callbacks.current.onAttention?.(); } }
    finally { if (current(id)) { pending.current = false; setBusy(false); } }
  }
  async function reconcile(cleanup = false) {
    if (pending.current || !session.current) return;
    pending.current = true; setBusy(true); setError(''); const id = generation.current;
    try {
      const value = cleanup ? await session.current.retryCleanup() : await session.current.reconcile();
      if (!current(id)) return;
      if (!value.task.final) { setView('running'); await session.current.wait(); }
      if (current(id)) acceptResult();
    }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); if (cause instanceof SourceImportError) setView(cause.recovery ? 'unknown' : 'failed'); } }
    finally { if (current(id)) { pending.current = false; setBusy(false); } }
  }
  async function cancel() {
    if (!flow || cancelling) return; setCancelling(true);
    const id = generation.current;
    const wasWaiting = view === 'waiting';
    if (wasWaiting) { pending.current = true; setBusy(true); setView('running'); }
    try { await flow.cancel(wasWaiting); if (current(id)) acceptResult(); }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); acceptResult(); } }
    finally { if (current(id)) { setCancelling(false); if (wasWaiting) { pending.current = false; setBusy(false); } } }
  }
  async function confirmBuild() {
    const revision = flow?.snapshot?.buildConfirmation?.revision;
    if (pending.current || disabled || !flow || !revision) return;
    pending.current = true; setBusy(true); setView('running'); setError(''); const id = generation.current;
    try { await flow.confirmBuild(revision); if (current(id)) acceptResult(); }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); setView(cause instanceof SourceImportError && cause.recovery ? 'unknown' : 'failed'); callbacks.current.onAttention?.(); } }
    finally { if (current(id)) { pending.current = false; setBusy(false); } }
  }
  async function enter() {
    if (!snapshot || !committed || busy) return;
    try {
      const app = snapshot.app ?? (snapshot.runtime === 'web'
        ? (await invoke<ProjectInfo[]>('project_list')).find(value => value.id === snapshot.appId)
        : await invoke<NodeAppDetail>('node_app_configuration', { appId: snapshot.appId }));
      if (!app) throw new Error(t("已添加的项目详情暂时读不到，请重新核对。"));
      if (!mounted.current || session.current?.snapshot?.appId !== app.id) return;
      if (app.runtime === 'web') onWebEnter?.(app); else onEnter?.(app);
      onClose({ keepSession: Boolean(snapshot.cleanupPending) || snapshot.remainingPaths.length > 0 });
    } catch (cause) { if (mounted.current) setError(messageFrom(cause)); }
  }
  function dismiss() {
    if ((busy || view === 'waiting') && onBackground) { onBackground(); return; }
    onClose({ keepSession: busy || view === 'waiting' || view === 'failed' || view === 'unknown' || Boolean(snapshot?.cleanupPending) });
  }
  if (!open) return null;
  return <Modal title={t("从 Git 添加项目")} titleIcon={<Download size={20} />} className="source-acquisition-modal git-import-modal" wide onClose={dismiss}>
    {view === 'form' && <form onSubmit={event => { event.preventDefault(); void start(); }}>
      <label htmlFor="git-import-url">{t("公开仓库地址")}</label><input id="git-import-url" value={url} onChange={event => setUrl(event.target.value)} placeholder={t("https://github.com/作者/仓库")} autoCapitalize="none" autoCorrect="off" spellCheck={false} />
      <p>{t("支持 GitHub、Gitee 的公开 HTTPS 仓库。")}</p>
      <section className="add-project-after" aria-label={t("添加后文件在哪里")}><strong>{t("添加后")}</strong><dl><dt>{t("项目文件夹")}</dt><dd><code>{directory || t("正在读取项目目录…")}/{address?.repository ?? t("仓库名")}</code>{t("平台和 AI 直接使用这一份代码。同名时自动另取名字，删除项目时一起删除。")}</dd><dt>{t("文件数据")}</dt><dd>{t("保存在项目文件夹的 ")}<code>data/</code> {t(" 下，删除项目时一起删除。")}</dd><dt>{t("浏览器数据")}</dt><dd>{t("保存在平台窗口存储中，删除项目时清除。配置和密钥由平台保存。")}</dd></dl>{onOpenSettings && <button type="button" className="text-button" onClick={onOpenSettings}>{t(frontendPlatform.projectDirectorySettingsLabel)}</button>}</section>
      <details className="git-import-options"><summary>{t("高级选项")}</summary><label htmlFor="git-import-branch">{t("分支（选填）")}</label><input id="git-import-branch" value={branch} onChange={event => setBranch(event.target.value)} placeholder={t("留空使用默认分支")} /></details>
      <p>{t("Node 项目只准备生产依赖，不执行仓库脚本。网页项目已有运行文件时直接添加；需要构建时，先由你确认。")}</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || !address || !directory}>{t("开始添加")}</button><button type="button" className="refresh-button" onClick={() => onClose()}>{t("取消")}</button></div>
      <div className="add-project-ai-footer"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'git', url: url.trim() || undefined, branch: branch.trim() || undefined, platformVersion }} /></div>
    </form>}
    {view === 'duplicate' && duplicate && <section className="import-result"><h3>{t("这个仓库和分支已经添加过：「{name}」", { name: duplicate.name })}</h3><p>{t("另外添加的一份会从空数据开始，两份互不影响。")}</p><div className="confirmation-actions"><button className="primary-button" onClick={() => { onOpenExisting?.(duplicate.appId); onClose(); }}>{t("打开已添加的")}</button><button className="refresh-button" disabled={disabled} onClick={() => void start(true)}>{t("仍然另外添加一份")}</button><button className="text-button" onClick={() => setView('form')}>{t("取消")}</button></div></section>}
    {!['form', 'duplicate'].includes(view) && <>
      <ImportProgress label={t("添加进度")} steps={phases.map(label => t(label))} currentStep={phaseIndex(task?.phase)} state={busy ? 'running' : view === 'success' ? snapshot?.cleanupPending ? 'warning' : 'succeeded' : view === 'failed' ? 'failed' : 'warning'} />
      <section className="import-result" aria-busy={busy}><h3>{view === 'success' ? snapshot?.cleanupPending ? t("已添加，临时文件还没清理完") : t("已添加「{value0}」", { value0: snapshot?.app?.name ?? t("应用") }) : view === 'waiting' ? t("需要确认构建") : view === 'unknown' ? t("添加结果待核对") : view === 'failed' ? task?.error?.code === 'NODE_GIT_IMPORT_CANCELLED' ? t("已取消添加") : t("这个项目暂时不能添加") : cancelling ? t("正在停止并清理…") : task?.phase === 'building' || task?.phase === 'build' ? t("正在构建项目") : t(phases[phaseIndex(task?.phase)])}</h3>
        {error && <p role="alert" className="task-error">{t(error)}</p>}{flow?.warning && <p role="status">{t(flow.warning)}</p>}
        {view === 'unknown' && <p>{t("结果明确前保留必要材料，请不要重复添加。")}</p>}
        {view === 'waiting' && snapshot?.buildConfirmation && <section aria-label={t("构建确认")}><p>{t("这个项目还没有构建好的文件，需要平台构建。构建会运行该仓库自带的脚本（包括依赖安装脚本），可能需要几分钟。")}</p><p>{t("仓库：")}<code>{snapshot.buildConfirmation.sourceUrl}</code></p><p>{t("构建脚本：")}<code>npm run {snapshot.buildConfirmation.script}</code></p></section>}
        {snapshot?.remainingPaths.map(path => <p key={path}>{t("待清理：")}<code>{path}</code></p>)}
        {view === 'failed' && <><ProcessFailureDetails value={diagnostics} /><p>{!snapshot ? t("尚未开始下载项目文件。") : snapshot.cleanupPending ? t("临时文件尚未清理完成。") : t("本次下载及临时材料已清理。")}</p></>}
        {view === 'success' && <><p>{snapshot?.cleanupPending ? t("重试清理不会再次添加项目。") : t("临时文件已清理。")}</p><div className="git-import-source"><p>{t("项目文件夹：")}<code>{snapshot?.sourcePath}</code></p><button className="text-button" disabled={!snapshot?.sourcePath} onClick={() => void invoke('open_project_folder', { path: snapshot?.sourcePath }).catch(cause => setError(messageFrom(cause)))}><FolderOpen size={14} />{t("打开文件夹")}</button></div></>}
        <div className="confirmation-actions">
          {view === 'waiting' && <><button className="primary-button" disabled={disabled || busy || cancelling} onClick={() => void confirmBuild()}>{t("构建并添加")}</button><button className="refresh-button" disabled={busy || cancelling} onClick={() => void cancel()}>{t("取消")}</button></>}
          {view === 'unknown' && <button className="primary-button" disabled={busy} onClick={() => void reconcile()}>{t("重新核对")}</button>}
          {snapshot?.cleanupPending && <button className="refresh-button" disabled={busy} onClick={() => void reconcile(true)}>{t("重试清理")}</button>}
          {view === 'failed' && <><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'git', url, branch: branch.trim() || undefined, issues: [error].filter(Boolean), processDiagnostics: diagnostics, platformVersion }} label={t("复制说明给 AI")} className="primary-button" />{!snapshot && <button className="refresh-button" onClick={() => { session.current = null; setView('form'); setError(''); }}>{t("返回填写")}</button>}{onOpenLocal && <button className="refresh-button" onClick={() => { onClose({ keepSession: !canLeaveFailedFlow }); onOpenLocal(); }}>{t("改用本地添加")}</button>}</>}
          {view === 'success' && <button className="primary-button" disabled={busy} onClick={() => void enter()}>{snapshot?.app ? snapshot.app.runtime === 'web' ? snapshot.app.state === 'ready' ? t("打开应用") : t("查看应用") : snapshot.app.configuration.canStart ? t("打开应用") : t("去设置") : t("查看应用")}</button>}
          {busy ? <><button className="refresh-button" onClick={dismiss}>{t("在后台继续")}</button><button className="text-button" disabled={cancelling || task?.phase === 'registering' || committed} onClick={() => void cancel()}>{t("取消添加")}</button></> : <button className="refresh-button" onClick={dismiss}>{view === 'success' ? t("完成") : t("关闭")}</button>}
        </div>
        {task && <details className="import-task"><summary>{t("检查详情")}</summary><ProjectTask task={task} /></details>}
      </section>
    </>}
  </Modal>;
}
