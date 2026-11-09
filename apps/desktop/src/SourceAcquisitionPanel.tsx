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
import { AppConfiguration } from './AppConfiguration';
import { SourceImportError, SourceImportSession } from './source-import';
import type { SelectedImportDirectory } from './source-import';
import { gitRepositoryAddress, sameGitRepository } from './git-repository';
import type { GitRepositorySource } from './git-repository';
import type { ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { onProjectDeleted, projectWasDeleted } from './project-deletion';
import type { GitImportViewState } from './git-import-view-state';
import { useFrontendPlatform } from './frontend-platform';
import type { ApplicationSource } from './AddApplicationDialog';
import { downloadRoutes, otherDownloadRoute, useDownloadRoutes } from './download-routes';
import { DownloadRouteFailure } from './DownloadRouteFailure';
export { gitRepositoryAddress } from './git-repository';

type ExistingRepository = GitRepositorySource & { appId: string; name: string };
export type SourceAcquisitionPanelProps = {
  disabled: boolean; instanceId?: string; open?: boolean; projectDirectory?: string; platformVersion?: string;
  existingRepositories?: ExistingRepository[];
  onClose: (options?: { keepSession: boolean }) => void; onBackground?: () => void; onAttention?: () => void;
  onOpenLocal?: () => void; onOpenSettings?: () => void; onOpenExisting?: (appId: string) => void;
  onTask: (task: TaskRecord) => void; onInstalled?: () => void; onEnter?: (app: ProjectInfo) => void;
  onWebEnter?: (app: ProjectInfo) => void;
  onStateChange?: (state: GitImportViewState | null) => void;
  initialSource?: ApplicationSource; requestToken?: number; waitForCompletion?: boolean;
};
const phases = ['下载应用', '检查应用', '准备依赖', '添加应用', '清理临时文件'];
function phaseIndex(phase?: string) {
  return phase === 'completed' || phase === 'cleanup' ? 4 : phase === 'registering' ? 3 : phase === 'dependencies' ? 2 : ['checking', 'publishing', 'inspecting'].includes(phase ?? '') ? 1 : 0;
}
export function SourceAcquisitionPanel(props: SourceAcquisitionPanelProps) {
  return <GitImportWorkflow key={`${props.instanceId ?? ''}:${props.requestToken ?? 0}`} {...props} />;
}
function GitImportWorkflow({ disabled, instanceId, open = true, projectDirectory: suppliedDirectory, platformVersion, existingRepositories, onClose, onBackground, onAttention, onOpenSettings, onOpenExisting, onTask, onInstalled, onEnter, onWebEnter, onStateChange, initialSource, waitForCompletion = false }: SourceAcquisitionPanelProps) {
  const frontendPlatform = useFrontendPlatform();
  const [discoveredAgent, setDiscoveredAgent] = useState(false);
  const agentPlacement = Boolean(frontendPlatform.agentDirectorySelection && (discoveredAgent || initialSource?.agent && initialSource.catalogId));
  const routeState = useDownloadRoutes();
  const recommendation = useRef<ApplicationSource | undefined>(initialSource);
  const startedDownloadMode = useRef(downloadRoutes.getSnapshot().mode);
  const [url, setUrl] = useState(initialSource?.url ?? ''); const [branch, setBranch] = useState(initialSource?.branch ?? '');
  const autoStarted = useRef(false);
  const [directory, setDirectory] = useState(suppliedDirectory ?? '');
  const [view, setView] = useState<'placement' | 'form' | 'duplicate' | 'running' | 'waiting' | 'failed' | 'unknown' | 'success'>(agentPlacement ? 'placement' : 'form');
  const [placement, setPlacement] = useState<'default' | 'selected'>('default');
  const [selectedDirectory, setSelectedDirectory] = useState<SelectedImportDirectory | null>(null);
  const [choosingDirectory, setChoosingDirectory] = useState(false);
  const [duplicate, setDuplicate] = useState<ExistingRepository | null>(null);
  const [error, setError] = useState(''); const [busy, setBusy] = useState(false); const [cancelling, setCancelling] = useState(false);
  const [configurationOpen, setConfigurationOpen] = useState(false);
  const [configurationState, setConfigurationState] = useState({dirty: false, pending: false});
  const [savedApp, setSavedApp] = useState<ProjectInfo | null>(null);
  const configurationRequested = useRef(new Set<string>());
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
  const targetDirectory = agentPlacement && placement === 'selected' ? selectedDirectory : null;
  const address = gitRepositoryAddress(url); const committed = snapshot?.commitState === 'committed';
  const sessionId = flow?.importId;
  const warning = flow?.warning ?? '';
  const addedApp = savedApp ?? snapshot?.app;
  const configurationLocked = configurationState.dirty || configurationState.pending;
  useEffect(() => {
    if (disabled || !open || busy || view !== 'success' || !snapshot || snapshot.cleanupPending || snapshot.remainingPaths.length
      || !snapshot.task.final || snapshot.commitState !== 'committed' || addedApp?.state !== 'needsConfiguration'
      || projectWasDeleted(addedApp.id) || configurationRequested.current.has(addedApp.id)) return;
    configurationRequested.current.add(addedApp.id); setConfigurationOpen(true);
  }, [disabled, open, busy, view, snapshot, addedApp]);
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
    if (value.placementRequired && frontendPlatform.agentDirectorySelection) { setDiscoveredAgent(true); setView('placement'); }
    else if (value.task.status === 'waitingConfirmation') setView('waiting');
    else if (!value.task.final) setView('running');
    else if (value.commitState === 'unknown') setView('unknown');
    else if (value.commitState === 'committed') { setView('success'); callbacks.current.onInstalled?.(); }
    else { setView('failed'); setError(messageFrom(value.task.error ?? t("添加未完成。"))); }
    if (value.task.status === 'waitingConfirmation' || value.commitState === 'unknown' || value.cleanupPending || value.task.status === 'failed' || value.task.status === 'interrupted') callbacks.current.onAttention?.();
  }
  async function chooseDirectory() {
    if (pending.current || disabled || !agentPlacement) return;
    pending.current = true; setChoosingDirectory(true); setError('');
    const id = generation.current;
    try {
      const path = await invoke<string | null>('choose_path', { kind: 'agent' });
      if (!current(id) || path === null) return;
      if (typeof path !== 'string' || !frontendPlatform.isAbsolutePath(path)) throw new Error(t("文件夹检查结果不完整，请重新选择。"));
      setSelectedDirectory(null);
      const checked = await invoke<SelectedImportDirectory>('source_import_directory_inspect', { path });
      if (!current(id)) return;
      if (!checked || typeof checked.path !== 'string' || !frontendPlatform.isAbsolutePath(checked.path)
        || typeof checked.revision !== 'string' || !checked.revision) throw new Error(t("文件夹检查结果不完整，请重新选择。"));
      setSelectedDirectory({ path: checked.path, revision: checked.revision }); setPlacement('selected');
    } catch (cause) { if (current(id)) setError(messageFrom(cause)); }
    finally { if (current(id)) { pending.current = false; setChoosingDirectory(false); } }
  }
  async function start(allowDuplicate = false, sourceOverride?: ApplicationSource) {
    const selectedSource = sourceOverride ?? recommendation.current;
    const requestedUrl = sourceOverride?.url ?? url;
    const selectedAddress = gitRepositoryAddress(requestedUrl);
    if (pending.current || disabled || !selectedAddress) return;
    const selectedRecommendation = selectedSource?.catalogId && selectedSource.url === requestedUrl ? selectedSource : undefined;
    if (agentPlacement && placement === 'selected' && !selectedDirectory) return;
    recommendation.current = selectedRecommendation;
    startedDownloadMode.current = downloadRoutes.getSnapshot().mode;
    const found = existingRepositories?.find(item => sameGitRepository(item, { url: selectedAddress.url, branch }));
    if (found && !allowDuplicate) { setDuplicate(found); setView('duplicate'); return; }
    pending.current = true; setBusy(true); setError(''); setCancelling(false); setView('running');
    const id = generation.current;
    const value: SourceImportSession = new SourceImportSession(invoke, {
      isCurrent: (): boolean => current(id) && session.current === value,
      onTask: task => callbacks.current.onTask(task), onChange: () => redraw(count => count + 1),
      onDownload: downloadedUrl => {
        const repositories = selectedRecommendation?.catalogRepositories;
        if (!repositories || downloadedUrl === selectedAddress.url) return;
        const provider = gitRepositoryAddress(downloadedUrl)?.provider;
        const route = provider === 'Gitee' ? 'cn' : 'global';
        downloadRoutes.recordSuccess(route);
      },
      onRevoked: () => { generation.current++; session.current = null; pending.current = false; setBusy(false); callbacks.current.onClose(); },
    });
    session.current = value;
    try {
      await value.run({
        url: selectedAddress.url,
        branch: branch.trim() || null,
        ...(selectedSource?.name ? { name: selectedSource.name } : {}),
        ...(selectedRecommendation ? { catalogId: selectedRecommendation.catalogId } : {}),
        ...(agentPlacement && placement === 'selected' && selectedDirectory ? { selectedDirectory } : {}),
        ...(selectedRecommendation?.catalogRepositories && downloadRoutes.getSnapshot().mode === 'auto'
          ? { alternateUrl: selectedAddress.provider === 'Gitee' ? selectedRecommendation.catalogRepositories.github : selectedRecommendation.catalogRepositories.gitee }
          : {}),
      });
      if (current(id)) acceptResult();
    }
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
    const wasWaiting = view === 'waiting' || view === 'placement';
    if (wasWaiting) { pending.current = true; setBusy(true); setView('running'); }
    try { await flow.cancel(wasWaiting); if (current(id)) acceptResult(); }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); acceptResult(); } }
    finally { if (current(id)) { setCancelling(false); if (wasWaiting) { pending.current = false; setBusy(false); } } }
  }
  async function confirmPlacement() {
    if (!flow || pending.current || disabled || placement === 'selected' && !selectedDirectory) return;
    pending.current = true; setBusy(true); setError(''); setView('running'); const id = generation.current;
    try { await flow.confirmPlacement(placement === 'selected' ? selectedDirectory : null); if (current(id)) acceptResult(); }
    catch (cause) { if (current(id)) { setError(messageFrom(cause)); setView('unknown'); } }
    finally { if (current(id)) { pending.current = false; setBusy(false); } }
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
    if (!snapshot || !committed || busy || mustWait) return;
    try {
      const app = addedApp ?? (await invoke<ProjectInfo[]>('app_list')).find(value => value.id === snapshot.appId);
      if (!app) throw new Error(t("已添加的项目详情暂时读不到，请重新核对。"));
      if (!mounted.current || session.current?.snapshot?.appId !== app.id) return;
      (onEnter ?? onWebEnter)?.(app);
      onClose({ keepSession: Boolean(snapshot.cleanupPending) || snapshot.remainingPaths.length > 0 });
    } catch (cause) { if (mounted.current) setError(messageFrom(cause)); }
  }
  async function configurationSaved() {
    if (!addedApp) return;
    const app = addedApp; const id = generation.current;
    const values = await invoke<ProjectInfo[]>('app_list');
    if (!current(id) || projectWasDeleted(app.id) || session.current?.snapshot?.appId !== app.id) return;
    const updated = values.find(value => value.id === app.id && value.projectRoot === app.projectRoot);
    if (updated) setSavedApp(updated);
    callbacks.current.onInstalled?.();
  }
  const failedRoute = recommendation.current?.downloadRoute;
  const retryRoute = failedRoute ? otherDownloadRoute(failedRoute) : null;
  const retryUrl = retryRoute ? recommendation.current?.catalogRepositories?.[retryRoute === 'cn' ? 'gitee' : 'github'] : null;
  const routeFailure = snapshot?.task.error?.details?.downloadFailure === true
    && snapshot.task.final && snapshot.commitState === 'not_committed' && !snapshot.cleanupPending
    && snapshot.remainingPaths.length === 0 && retryUrl && routeState.mode !== 'auto' && startedDownloadMode.current !== 'auto';
  function retryOtherRoute() {
    if (!retryRoute || !retryUrl || !recommendation.current || busy) return;
    downloadRoutes.setMode(retryRoute);
    const source = { ...recommendation.current, url: retryUrl, downloadRoute: retryRoute };
    setUrl(retryUrl);
    void start(false, source);
  }
  const mustWait = configurationLocked || waitForCompletion && (busy || view === 'waiting' || view === 'unknown' || task?.final === false || Boolean(snapshot?.cleanupPending) || Boolean(snapshot?.remainingPaths.length));
  function dismiss() {
    if (mustWait || choosingDirectory) return;
    if ((busy || view === 'waiting') && onBackground) { onBackground(); return; }
    onClose({ keepSession: busy || view === 'waiting' || view === 'failed' || view === 'unknown' || Boolean(snapshot?.cleanupPending) });
  }
  useEffect(() => {
    if (!initialSource || !directory || disabled || autoStarted.current || agentPlacement) return;
    // Deferral lets StrictMode finish its mount cleanup before native work starts.
    const timer = window.setTimeout(() => { if (!autoStarted.current) { autoStarted.current = true; void start(); } }, 0);
    return () => window.clearTimeout(timer);
  }, [initialSource, directory, disabled, agentPlacement]);
  if (!open) return null;
  return <Modal title={t("添加应用")} titleIcon={<Download size={20} />} className="source-acquisition-modal git-import-modal" wide onClose={dismiss} closeDisabled={mustWait || choosingDirectory}>
    {view === 'placement' && <section className="agent-install-placement" aria-label={t("放在哪里")}>
      <h3>{t("放在哪里")}</h3>
      {initialSource?.name && <p className="agent-install-name">{initialSource.name}</p>}
      <p>{t("你的 AI 会在这个文件夹里工作。选好后不方便移动。")}</p>
      <div className={`agent-install-option${placement === 'default' ? ' is-selected' : ''}`}>
        <label><input type="radio" name="agent-install-location" checked={placement === 'default'} disabled={choosingDirectory || disabled} onChange={() => setPlacement('default')} /><strong>{t("默认位置")}</strong></label>
        <code>{directory || t("正在读取项目文件夹…")}</code>
        <p>{t("应用会放在这个位置下的独立文件夹，具体路径在添加完成后显示。由平台管理，删除应用时整个文件夹和 data/ 一起删除。")}</p>
      </div>
      <div className={`agent-install-option${placement === 'selected' ? ' is-selected' : ''}`}>
        <label><input type="radio" name="agent-install-location" checked={placement === 'selected'} disabled={choosingDirectory || disabled} onChange={() => setPlacement('selected')} /><strong>{t("选择一个空文件夹")}</strong></label>
        {selectedDirectory && <code>{selectedDirectory.path}</code>}
        <p>{t("应用直接放进你选择的文件夹，不再套一层。删除应用时，文件夹、代码和 data/ 会保留，平台配置和浏览器数据会清除。")}</p>
        <button type="button" className="refresh-button" disabled={choosingDirectory || disabled} onClick={() => { setPlacement('selected'); void chooseDirectory(); }}>{choosingDirectory ? t("正在检查文件夹…") : selectedDirectory ? t("重新选择文件夹…") : t("选择文件夹…")}</button>
      </div>
      {error && <p role="alert" className="task-error">{t(error)}</p>}
      <div className="confirmation-actions"><button type="button" className="primary-button" disabled={disabled || choosingDirectory || !directory || placement === 'selected' && !selectedDirectory} onClick={() => void (snapshot?.placementRequired ? confirmPlacement() : start())}>{t("开始添加")}</button><button type="button" className="refresh-button" disabled={choosingDirectory} onClick={() => { if (snapshot?.placementRequired) void cancel(); else onClose(); }}>{t("取消")}</button></div>
    </section>}
    {view === 'form' && <form onSubmit={event => { event.preventDefault(); void start(); }}>
      <label htmlFor="git-import-url">{t("有应用链接")}</label><input id="git-import-url" value={url} onChange={event => setUrl(event.target.value)} placeholder={t("粘贴 GitHub 或 Gitee 上的应用地址")} autoCapitalize="none" autoCorrect="off" spellCheck={false} />
      <p>{t("支持 GitHub、Gitee 上的公开应用地址。")}</p>
      <section className="add-project-after" aria-label={t("添加后文件在哪里")}><strong>{t("添加后")}</strong><dl><dt>{t("项目文件夹")}</dt><dd><code>{directory || t("正在读取项目文件夹…")}/{address?.repository ?? t("仓库名")}</code>{t("平台和 AI 直接使用这一份代码。同名时自动另取名字，删除应用时一起删除。")}</dd><dt>{t("文件数据")}</dt><dd>{t("保存在项目文件夹的 ")}<code>data/</code> {t(" 下，删除应用时一起删除。")}</dd><dt>{t("浏览器数据")}</dt><dd>{t("保存在平台窗口存储中，删除应用时清除。配置和密钥由平台保存。")}</dd></dl>{onOpenSettings && <button type="button" className="text-button" onClick={onOpenSettings}>{t(frontendPlatform.projectDirectorySettingsLabel)}</button>}</section>
      <details className="git-import-options"><summary>{t("高级选项")}</summary><label htmlFor="git-import-branch">{t("分支（选填）")}</label><input id="git-import-branch" value={branch} onChange={event => setBranch(event.target.value)} placeholder={t("留空使用默认分支")} /></details>
      <p>{t("已有运行文件时直接添加；需要运行应用自带的准备脚本时，先由你确认。")}</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || !address || !directory}>{t("开始添加")}</button><button type="button" className="refresh-button" onClick={() => onClose()}>{t("取消")}</button></div>
      <div className="add-project-ai-footer"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'git', url: url.trim() || undefined, branch: branch.trim() || undefined, platformVersion }} /></div>
    </form>}
    {view === 'duplicate' && duplicate && <section className="import-result"><h3>{t("这个应用已经添加过：「{name}」", { name: duplicate.name })}</h3><p>{t("另外添加的一份会从空数据开始，两份互不影响。")}</p><div className="confirmation-actions"><button className="primary-button" onClick={() => { onOpenExisting?.(duplicate.appId); onClose(); }}>{t("打开已添加的")}</button><button className="refresh-button" disabled={disabled} onClick={() => void start(true)}>{t("仍然另外添加一份")}</button><button className="text-button" onClick={() => setView(agentPlacement ? 'placement' : 'form')}>{t("取消")}</button></div></section>}
    {!['placement', 'form', 'duplicate'].includes(view) && <>
      <ImportProgress label={t("添加进度")} steps={phases.map(label => t(label))} currentStep={phaseIndex(task?.phase)} state={busy ? 'running' : view === 'success' ? snapshot?.cleanupPending ? 'warning' : 'succeeded' : view === 'failed' ? 'failed' : 'warning'} />
      <section className="import-result" aria-busy={busy}><h3>{view === 'success' ? snapshot?.cleanupPending ? t("已添加，临时文件还没清理完") : t("已添加「{value0}」", { value0: snapshot?.app?.name ?? t("应用") }) : view === 'waiting' ? t("需要确认准备") : view === 'unknown' ? t("添加结果待核对") : view === 'failed' ? task?.error?.code === 'NODE_GIT_IMPORT_CANCELLED' ? t("已取消添加") : t("这个应用暂时不能添加") : cancelling ? t("正在停止并清理…") : task?.phase === 'building' || task?.phase === 'build' ? t("正在准备应用") : t(phases[phaseIndex(task?.phase)])}</h3>
        {targetDirectory && view !== 'success' && <p className="agent-install-target">{t("添加后的应用文件夹：")}<code>{targetDirectory.path}</code></p>}
        {error && <p role="alert" className="task-error">{t(error)}</p>}{flow?.warning && <p role="status">{t(flow.warning)}</p>}
        {view === 'unknown' && <p>{t("结果明确前保留必要材料，请不要重复添加。")}</p>}
        {view === 'waiting' && snapshot?.buildConfirmation && <section aria-label={t("准备确认")}><p>{t("这个应用需要先准备运行文件。继续会运行应用自带的脚本（包括依赖安装脚本），可能需要几分钟。")}</p><p>{t("应用地址：")}<code>{snapshot.buildConfirmation.sourceUrl}</code></p>{targetDirectory && <p>{t("准备运行文件的位置：")}<code>{snapshot.buildConfirmation.sourcePath}</code></p>}<p>{t("准备脚本：")}<code>npm run {snapshot.buildConfirmation.script}</code></p></section>}
        {snapshot?.remainingPaths.map(path => <p key={path}>{t("待清理：")}<code>{path}</code></p>)}
        {view === 'failed' && <><ProcessFailureDetails value={diagnostics} /><p>{!snapshot ? t("尚未开始下载应用文件。") : snapshot.cleanupPending ? t("临时文件尚未清理完成。") : t("本次下载及临时材料已清理。")}</p></>}
        {view === 'success' && <><p>{snapshot?.cleanupPending ? t("重试清理不会再次添加项目。") : t("临时文件已清理。")}</p><p className="project-help">{t("打开后想改哪里，点应用窗口上的「按你的想法改」，交给你的 AI。")}</p><div className="git-import-source"><p>{t("项目文件夹：")}<code>{snapshot?.sourcePath}</code></p><button className="text-button" disabled={!snapshot?.sourcePath} onClick={() => void invoke('open_project_folder', { path: snapshot?.sourcePath }).catch(cause => setError(messageFrom(cause)))}><FolderOpen size={14} />{t("打开文件夹")}</button></div></>}
        {view === 'success' && configurationOpen && addedApp && <AppConfiguration returnToAdded projectId={addedApp.id} instanceId={instanceId} disabled={disabled || busy} onState={setConfigurationState} onBack={() => setConfigurationOpen(false)} onSaved={configurationSaved} />}
        <div className="confirmation-actions">
          {view === 'waiting' && <><button className="primary-button" disabled={disabled || busy || cancelling} onClick={() => void confirmBuild()}>{t("准备并添加")}</button><button className="refresh-button" disabled={busy || cancelling} onClick={() => void cancel()}>{t("取消")}</button></>}
          {view === 'unknown' && <button className="primary-button" disabled={busy} onClick={() => void reconcile()}>{t("重新核对")}</button>}
          {snapshot?.cleanupPending && <button className="refresh-button" disabled={busy} onClick={() => void reconcile(true)}>{t("重试清理")}</button>}
          {view === 'failed' && routeFailure && failedRoute && <DownloadRouteFailure route={failedRoute} disabled={busy || disabled} onRetry={retryOtherRoute} />}
          {view === 'failed' && <><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'git', url, branch: branch.trim() || undefined, issues: [error].filter(Boolean), processDiagnostics: diagnostics, platformVersion, downloadDirectory: directory || undefined }} label={t("交给 AI 处理")} className="primary-button" />{!snapshot && <button className="refresh-button" onClick={() => { session.current = null; setView(agentPlacement ? 'placement' : 'form'); setError(''); }}>{t("返回填写")}</button>}</>}
          {view === 'success' && !configurationOpen && <button className="primary-button" disabled={busy || mustWait} onClick={() => { if (addedApp?.state === 'needsConfiguration') setConfigurationOpen(true); else void enter(); }}>{addedApp ? addedApp.state === 'ready' ? t("打开应用") : addedApp.state === 'needsConfiguration' ? t("填写配置") : t("查看应用") : t("查看应用")}</button>}
          {view === 'failed' && task?.final === false && <button className="refresh-button" disabled={busy} onClick={() => void reconcile()}>{t("重新核对")}</button>}
          {busy ? <>{waitForCompletion ? <span className="project-help" role="status">{t("请在窗口中等待添加完成。")}</span> : <button className="refresh-button" onClick={dismiss}>{t("在后台继续")}</button>}<button className="text-button" disabled={cancelling || task?.phase === 'registering' || committed} onClick={() => void cancel()}>{t("取消添加")}</button></> : <button className="refresh-button" disabled={mustWait} onClick={dismiss}>{view === 'success' ? t("完成") : t("关闭")}</button>}
        </div>
        {task && <details className="import-task"><summary>{t("检查详情")}</summary><ProjectTask task={task} /></details>}
      </section>
    </>}
  </Modal>;
}
