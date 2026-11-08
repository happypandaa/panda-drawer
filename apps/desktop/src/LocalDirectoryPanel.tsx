import { localizePlatformPrompt } from './platform-prompt';
import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { CheckCircle2, Copy, FolderOpen, History, RefreshCw, Server } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { AiProjectHelp } from './AiProjectHelp';
import { CodexHandoffPanel } from './CodexHandoffPanel';
import type { ProjectHandoff } from './CodexHandoffPanel';
import { ImportProgress } from './ImportProgress';
import { NodeDependencyWarnings } from './NodeDependencyWarnings';
import { useFrontendPlatform } from './frontend-platform';
import { displayPath, folderName, isAbsoluteFolderPath, localReviewCategory, localReviewComparison, localReviewReason, normalizedFolderPath, parentPath, readRecentProjectFolders, recentStatus, recentTimeLabel, removeRecentProjectFolder, reviewCategoryLabels, reviewCategoryTitle, saveRecentProjectFolder } from './local-directory-review';
import type { RecentProjectFolder } from './local-directory-review';
import type { NodeAppDetail, NodeDirectoryReview } from './node-app-types';
import type { ProjectInfo } from './types';
type AddedApp = NodeAppDetail | ProjectInfo;
const sourceOf = (app: AddedApp) => app.runtime === 'web' ? app.projectRoot : app.sourcePath;
import { messageFrom } from './ui-utils';
import { deletedSourceMatches, onProjectDeleted, projectWasDeleted } from './project-deletion';

type Operation = 'choose' | 'inspect' | 'initialize' | 'register' | 'copy' | null;

function checkedReview(value: NodeDirectoryReview, sourcePath: string): NodeDirectoryReview {
  if (!value || value.sourcePath !== sourcePath || !['empty', 'ready', 'needs-review'].includes(value.state)
    || typeof value.expectedRevision !== 'string' || !value.expectedRevision
    || !Array.isArray(value.issues) || !value.issues.every(issue => typeof issue === 'string')
    || typeof value.reviewPrompt !== 'string') throw new Error(t("目录检查结果不完整或路径不符，请重新检查。"));
  if (value.existingApp && (typeof value.existingApp.id !== 'string' || !value.existingApp.id || typeof value.existingApp.name !== 'string' || !value.existingApp.name)) throw new Error(t("已添加应用的信息不完整，请刷新项目列表后继续。"));
  if (value.state === 'ready' && (!value.inspection || !['node', 'web'].includes(value.inspection.runtime)
    || !value.inspection.app || typeof value.inspection.app.name !== 'string' || (value.inspection.app.version != null && typeof value.inspection.app.version !== 'string')
    || !Array.isArray(value.inspection.configuration?.fields)
    || !value.inspection.configuration.fields.every(field => field && typeof field.label === 'string' && typeof field.required === 'boolean'))) throw new Error(t("项目预览不完整，请重新检查目录。"));
  if (value.state === 'needs-review' && !value.reviewPrompt) throw new Error(t("缺少给 AI 的检查说明，请重新检查目录。"));
  return value;
}

function checkedAddedApp(value: AddedApp, sourcePath: string): AddedApp {
  if (value?.runtime === 'web') {
    if (!value.id || !value.name || value.projectRoot !== sourcePath || value.ownership !== 'user' || !['ready', 'needsBuild', 'entryMissing', 'needsConfiguration'].includes(value.state)) throw new Error(t("网页项目添加结果尚不能确认，请重新检查登记状态。"));
    return value;
  }
  if (!value || !value.id || value.sourcePath !== sourcePath || value.runtime !== 'node' || !Array.isArray(value.fields) || !value.configuration || typeof value.configuration.canStart !== 'boolean') throw new Error(t("添加结果尚不能确认。请重新检查登记状态，不要重复添加。"));
  return value;
}

export function LocalDirectoryPanel({ enabled, instanceId, workspacePath, platformVersion, apps = [], onPendingChange, onAdded, onEnter, onViewDetails, onWebAdded, onWebEnter, onWebDetails, onClose }: {
  enabled: boolean;
  instanceId?: string;
  workspacePath?: string;
  platformVersion?: string;
  apps?: NodeAppDetail[];
  onPendingChange: (pending: boolean) => void;
  onAdded: (app: NodeAppDetail) => void;
  onWebAdded?: (app: ProjectInfo) => void;
  onWebEnter?: (app: ProjectInfo) => void;
  onWebDetails?: (app: ProjectInfo) => void;
  onEnter?: (app: NodeAppDetail) => void;
  onViewDetails?: (app: NodeAppDetail) => void;
  onClose?: () => void;
}) {
  const platform = useFrontendPlatform();
  const scope = workspacePath ?? instanceId ?? 'unresolved-workspace';
  const [sourcePath, setSourcePath] = useState('');
  const [phase, setPhase] = useState(0);
  const [completed, setCompleted] = useState<AddedApp | null>(null);
  const [generatedHere, setGeneratedHere] = useState(false);
  const [handoff, setHandoff] = useState<ProjectHandoff | null>(null);
  const [existing, setExisting] = useState<{ id: string; name: string; runtime?: 'node' | 'web' } | null>(null);
  const [name, setName] = useState('');
  const [review, setReview] = useState<NodeDirectoryReview | null>(null);
  const [reviewPrompt, setReviewPrompt] = useState('');
  const [checkedAt, setCheckedAt] = useState<string | undefined>();
  const [recent, setRecent] = useState(() => readRecentProjectFolders(scope));
  const [comparison, setComparison] = useState<string | null>(null);
  const [pending, setPending] = useState<Operation>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [copyNotice, setCopyNotice] = useState('');
  const dropzone = useRef<HTMLDivElement>(null);
  const resultHeading = useRef<HTMLHeadingElement>(null);
  const addButton = useRef<HTMLButtonElement>(null);
  const focusPreview = useRef(false);
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const context = useRef({ enabled, instanceId, scope, apps, sourcePath, completed, existing }); context.current = { enabled, instanceId, scope, apps, sourcePath, completed, existing };
  const callbacks = useRef({ onPendingChange, onAdded, onEnter, onViewDetails, onWebAdded, onWebEnter, onWebDetails, onClose }); callbacks.current = { onPendingChange, onAdded, onEnter, onViewDetails, onWebAdded, onWebEnter, onWebDetails, onClose };
  const selectDropped = useRef<(path: string) => void>(() => {});
  const id = useId();
  const blocked = !enabled || pending !== null;

  function revokeReview() { focusPreview.current = false; setReview(null); setExisting(null); setComparison(null); }
  function remember(path: string, result: RecentProjectFolder['result'], issues: string[], ownerId?: string) {
    const item = { path, result, issues, selectedAt: new Date().toISOString(), ownerId };
    setRecent(saveRecentProjectFolder(context.current.scope, item));
  }
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; operation.current = false; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => { setHandoff(null); }, [completed?.id]);
  useEffect(() => {
    sequence.current++; operation.current = false; setPending(null); callbacks.current.onPendingChange(false);
    revokeReview(); setCopyNotice(''); setCompleted(null); setGeneratedHere(false); setPhase(sourcePath ? 1 : 0);
    if (sourcePath) setNotice(t("接入状态已变化，请重新检查目录后继续。"));
  }, [instanceId, enabled, scope]);
  useEffect(() => { setRecent(readRecentProjectFolders(scope)); }, [scope]);
  useEffect(() => onProjectDeleted(notice => {
    const value = context.current;
    if (value.instanceId && value.instanceId !== notice.instanceId) return;
    setRecent(readRecentProjectFolders(value.scope));
    const knownOwner = value.completed?.id ?? value.existing?.id ?? value.apps.find(app => normalizedFolderPath(app.sourcePath) === normalizedFolderPath(value.sourcePath))?.id;
    if (!deletedSourceMatches(notice, { appId: knownOwner, sourceRoots: [value.sourcePath] })) return;
    sequence.current++; operation.current = false; callbacks.current.onPendingChange(false);
    setPending(null); revokeReview(); setCompleted(null); setSourcePath(''); setName(''); setPhase(0);
    setGeneratedHere(false); setReviewPrompt(''); setCheckedAt(undefined); setError(''); setNotice(''); setCopyNotice('');
  }), []);
  useEffect(() => {
    if (enabled && !pending && review?.state === 'ready' && focusPreview.current) {
      focusPreview.current = false; addButton.current?.focus();
    }
  }, [enabled, pending, review]);
  useEffect(() => { if (!pending && (completed || review?.state === 'needs-review')) resultHeading.current?.focus(); }, [completed, pending, review]);
  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    let disposed = false; let unlisten: (() => void) | undefined;
    void import('@tauri-apps/api/webview').then(async ({ getCurrentWebview }) => {
      const stop = await getCurrentWebview().onDragDropEvent(event => {
        if (event.payload.type !== 'drop' || event.payload.paths.length !== 1 || !dropzone.current) return;
        const bounds = dropzone.current.getBoundingClientRect();
        const x = event.payload.position.x / window.devicePixelRatio; const y = event.payload.position.y / window.devicePixelRatio;
        if (x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom) selectDropped.current(event.payload.paths[0]);
      });
      if (disposed) stop(); else unlisten = stop;
    }).catch(() => { /* The directory picker remains usable when drag events are unavailable. */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  async function run(kind: Exclude<Operation, null>, perform: (current: () => boolean) => Promise<void>) {
    if (operation.current || !context.current.enabled) return;
    operation.current = true; setPending(kind); callbacks.current.onPendingChange(true);
    if (kind !== 'copy') { setError(''); setNotice(''); }
    setCopyNotice('');
    if (kind !== 'copy') setCompleted(null);
    const request = ++sequence.current; const service = context.current.instanceId; const workspace = context.current.scope;
    const current = () => alive.current && sequence.current === request && context.current.enabled && context.current.instanceId === service && context.current.scope === workspace;
    try { await perform(current); }
    catch (cause) {
      if (current()) {
        if (kind !== 'copy') revokeReview();
        setError(`${messageFrom(cause)}${kind === 'initialize' ? t(" 已生成的文件会保留；请重新检查并核对项目列表，不要重复生成。") : ''}`);
      }
    }
    finally { if (current()) { operation.current = false; setPending(null); callbacks.current.onPendingChange(false); } }
  }
  async function inspect(source: string, current: () => boolean) {
    setPhase(1);
    const already = context.current.apps.find(app => normalizedFolderPath(app.sourcePath) === normalizedFolderPath(source));
    if (already && !projectWasDeleted(already.id)) { setExisting(already); remember(source, '已添加', [], already.id); return; }
    const previous = readRecentProjectFolders(context.current.scope).find(item => normalizedFolderPath(item.path) === normalizedFolderPath(source));
    let value: NodeDirectoryReview;
    try { value = await invoke<NodeDirectoryReview>('node_app_directory_inspect', { sourcePath: source }); }
    catch (cause) {
      if (current()) {
        const message = messageFrom(cause);
        remember(source, /not found|不存在|已移动|No such file|NOT_FOUND/i.test(message) ? '已移动或删除' : '检查未完成', [message]);
      }
      throw cause;
    }
    if (!current()) return;
    const checked = checkedReview(value, source);
    if (checked.existingApp) { if (!projectWasDeleted(checked.existingApp.id)) { setExisting(checked.existingApp); remember(source, '已添加', [], checked.existingApp.id); } return; }
    focusPreview.current = checked.state === 'ready';
    setReview(checked); setReviewPrompt(checked.reviewPrompt); setCheckedAt(new Date().toISOString()); setPhase(checked.state === 'needs-review' ? 1 : 2);
    setComparison(localReviewComparison(previous?.issues, checked.issues));
    remember(source, checked.state === 'ready' ? '可以添加' : checked.state === 'empty' ? '空文件夹' : '上次未通过', checked.issues);
  }
  function choose() {
    if (blocked || operation.current) return;
    revokeReview();
    void run('choose', async current => {
      setPhase(0);
      const source = await invoke<string | null>('choose_path', { kind: 'nodeapp' });
      if (!current()) return;
      if (!source) { setNotice(t("已取消选择。已选目录仍保留，重新检查后才能继续。")); return; }
      if (!isAbsoluteFolderPath(source)) throw new Error(t("选择器未返回有效的本地目录，请重新选择。"));
      setSourcePath(source); setGeneratedHere(false); setName(source.split(/[\\/]/).filter(Boolean).at(-1) ?? t("新项目")); setReviewPrompt(''); setCheckedAt(undefined);
      await inspect(source, current);
    });
  }
  function selectSource(source: string) {
    if (blocked || operation.current) return;
    if (!isAbsoluteFolderPath(source)) { setNotice(t("请使用“选择文件夹…”选择电脑上的项目文件夹。")); return; }
    revokeReview(); setSourcePath(source); setGeneratedHere(false); setReviewPrompt(''); setCheckedAt(undefined); setName(source.split(/[\\/]/).filter(Boolean).at(-1) ?? t("新项目"));
    void run('inspect', current => inspect(source, current));
  }
  selectDropped.current = selectSource;
  function recheck() {
    if (blocked || !sourcePath || operation.current) return;
    revokeReview(); void run('inspect', current => inspect(sourcePath, current));
  }
  function initializeAndRegister() {
    if (blocked || !review || review.state !== 'empty' || !name.trim() || operation.current) return;
    const selected = review; revokeReview(); setPhase(2);
    void run('initialize', async current => {
      const value = await invoke<NodeAppDetail>('node_app_directory_initialize_and_register', { sourcePath: selected.sourcePath, input: { expectedRevision: selected.expectedRevision, name: name.trim() } });
      if (!current()) return;
      const added = checkedAddedApp(value, selected.sourcePath);
      if (projectWasDeleted(added.id)) return;
      setCompleted(added); setGeneratedHere(true); setPhase(2); remember(selected.sourcePath, '已添加', [], added.id); if (added.runtime === 'web') callbacks.current.onWebAdded?.(added); else callbacks.current.onAdded(added);
    });
  }
  function register() {
    if (blocked || !review || review.state !== 'ready' || operation.current) return;
    const selected = review; revokeReview(); setPhase(2);
    void run('register', async current => {
      const value = await invoke<AddedApp>('node_app_directory_register', { sourcePath: selected.sourcePath, expectedRevision: selected.expectedRevision });
      if (!current()) return;
      const added = checkedAddedApp(value, selected.sourcePath);
      if (projectWasDeleted(added.id)) return;
      setCompleted(added); setPhase(2); remember(selected.sourcePath, '已添加', [], added.id); if (added.runtime === 'web') callbacks.current.onWebAdded?.(added); else callbacks.current.onAdded(added);
    });
  }
  function copyPath() {
    if (blocked || !sourcePath || operation.current) return;
    void run('copy', async current => {
      try {
        if (!navigator.clipboard?.writeText) throw new Error(t("剪贴板不可用"));
        await navigator.clipboard.writeText(sourcePath);
        if (current()) setCopyNotice(t("文件夹路径已复制。"));
      } catch { if (current()) setCopyNotice(t("复制未完成，可选中上方完整路径后手动复制。")); }
    });
  }
  async function prepareCreateHandoff(copy: boolean) {
    if (blocked || !completed || !generatedHere || operation.current) return;
    const added = completed;
    await run('copy', async current => {
      const value = await invoke<ProjectHandoff>(added.runtime === 'web' ? 'project_handoff' : 'node_app_handoff', {
        ...(added.runtime === 'web' ? { projectId: added.id } : { appId: added.id }), purpose: 'create',
      });
      if (!current()) return;
      if (value.projectRoot !== sourceOf(added) || value.instanceId !== context.current.instanceId) throw new Error(t("平台已变化，请重新打开详情。"));
      setHandoff(value);
      if (copy) {
        try {
          await navigator.clipboard.writeText(localizePlatformPrompt(value.text));
          if (current()) setCopyNotice(t("提示词已复制。"));
        } catch { if (current()) setCopyNotice(t("请手动复制下方完整说明。")); }
      }
    });
  }
  async function existingAction(details: boolean) {
    if (blocked || !existing) return;
    if (existing.runtime === 'web' || existing.id.startsWith('prj_')) {
      void run('inspect', async current => {
        const projects = await invoke<ProjectInfo[]>('project_list');
        if (!current()) return;
        const project = projects.find(project => project.id === existing.id);
        if (!project) throw new Error(t("项目列表已变化，请刷新后继续。"));
        (details ? callbacks.current.onWebDetails ?? callbacks.current.onWebEnter : callbacks.current.onWebEnter)?.(project);
      });
      return;
    }
    const selected = context.current.apps.find(app => app.id === existing.id);
    if (selected) { (details ? callbacks.current.onViewDetails ?? callbacks.current.onEnter : callbacks.current.onEnter)?.(selected); return; }
    void run('inspect', async current => {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: existing.id });
      if (!current()) return;
      if (detail.id !== existing.id) throw new Error(t("应用信息已变化，请刷新项目列表后继续。"));
      (details ? callbacks.current.onViewDetails ?? callbacks.current.onEnter : callbacks.current.onEnter)?.(detail);
    });
  }
  async function revealCompleted() {
    if (!enabled || !completed) return;
    const appId = completed.id;
    try { await invoke('open_project_folder', { path: sourceOf(completed) }); }
    catch (cause) { if (alive.current && context.current.completed?.id === appId) setError(messageFrom(cause)); }
  }

  const inspection = review?.state === 'ready' ? review.inspection : null;
  const category = review?.state === 'needs-review' ? localReviewCategory(review) : null;
  const categoryLabel = reviewCategoryLabels[category ?? 'needs-changes'];
  const reviewTitle = category === 'insufficient' ? t("还需要进一步确认这个文件夹") : reviewCategoryTitle(category ?? 'needs-changes');
  const recentList = !sourcePath && recent.length > 0 ? <section className="local-directory-recents" aria-label={t("最近选过的文件夹")}><h3><History size={16} />{t("最近选过的文件夹")}</h3>{recent.map(item => <div className="local-directory-recent" key={item.path}><button type="button" className="local-directory-recent-open" disabled={blocked} title={item.path} onClick={() => selectSource(item.path)}><FolderOpen size={18} aria-hidden="true" /><span className="local-directory-recent-text"><strong>{folderName(item.path)}</strong><span>{displayPath(parentPath(item.path))}</span></span><small>{[t(recentStatus(item)), recentTimeLabel(item.selectedAt)].filter(Boolean).join(' · ')}</small><span className="local-directory-recent-check" aria-hidden="true">{t("检查")}</span></button><button type="button" className="text-button local-directory-recent-remove" disabled={blocked} aria-label={t("移除最近目录 {value0}", { value0: item.path })} onClick={() => setRecent(removeRecentProjectFolder(scope, item.path))}>{t("移除")}</button></div>)}<p className="project-help">{t("每次都会重新检查，上次结果仅供参考。")}</p></section> : null;
  const existingDetail = existing ? apps.find(app => app.id === existing.id) : null;
  const helpIssues = review?.issues ?? (error ? [error] : []);
  const canShowPrompt = reviewPrompt && pending !== 'initialize' && pending !== 'register' && (review?.state === 'needs-review' || !review);
  return <div className="node-add-dialog local-directory-panel" aria-label={t("本地项目文件夹接入")} aria-busy={pending !== null}>
    <ImportProgress steps={[t("选择文件夹"), t("检查项目"), t("添加应用")]} currentStep={phase} state={completed ? 'succeeded' : error ? 'failed' : review?.state === 'needs-review' ? 'warning' : pending && pending !== 'copy' ? 'running' : 'paused'} label={t("添加进度")} />
    {completed ? <section className="import-result local-directory-result">
      <span className="local-directory-result-label is-ready"><CheckCircle2 size={15} />{t("添加成功")}</span>
      <h3 ref={resultHeading} tabIndex={-1}>{t("已添加「{name}」", { name: completed.name })}</h3>
      <p role="status">{completed.runtime === 'web' ? completed.state === 'needsBuild' ? t("添加完成，请在应用详情中构建后打开。") : completed.state === 'entryMissing' ? t("添加完成，网页入口缺失，请让 AI 补齐后打开。") : t("可以打开应用。") : completed.dependencyWarnings?.length ? t("请先准备所需的运行组件，再打开应用。") : completed.configuration.canStart ? t("可以打开应用。") : t("完成必要设置后就可以使用。")}</p>
      {completed.runtime === 'node' && <NodeDependencyWarnings warnings={completed.dependencyWarnings} />}
      <div className="local-directory-location"><strong>{t("文件在哪里")}</strong><p>{t("项目文件夹：")}<code>{sourceOf(completed)}</code>{t("（")}{generatedHere ? t("原位置，已生成起始项目") : t("原位置，未改动")}{t("）")}</p><p>{t("文件数据在这个文件夹的 data/；浏览器数据保存在平台窗口存储中，删除应用时清除。")}</p></div>
      <p className="project-help">{t("删除应用时，这个用户文件夹及其中的文件数据保留，平台登记、配置和浏览器数据清除。")}</p>
      {generatedHere && <CodexHandoffPanel target={{ ...(completed.runtime === 'web' ? { projectId: completed.id } : { appId: completed.id }), appName: completed.name, purpose: 'create' }} handoff={handoff} action={pending === 'copy' ? 'copy' : null} disabled={blocked} onPrepare={() => void prepareCreateHandoff(true)} onShowHelp={() => void prepareCreateHandoff(false)} onClear={() => setHandoff(null)} onOpenFolder={() => void revealCompleted()} feedback={<>{error && <p className="task-error" role="alert">{error}</p>}{copyNotice && <p role="status">{copyNotice}</p>}</>} />}
      <div className="confirmation-actions"><button type="button" className="refresh-button" disabled={!enabled} onClick={() => void revealCompleted()}><FolderOpen size={14} />{t("打开文件夹")}</button>{(onEnter || onWebEnter) && <button type="button" className="primary-button" disabled={blocked} onClick={() => { if (completed.runtime === 'web') { if (completed.state === 'ready') callbacks.current.onWebEnter?.(completed); else callbacks.current.onWebDetails?.(completed); } else callbacks.current.onEnter?.(completed); }}>{completed.runtime === 'web' ? completed.state === 'ready' ? t("打开应用") : t("查看应用详情") : completed.configuration.canStart ? t("打开应用") : t("去设置")}</button>}{onClose && <button type="button" className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>{t("完成")}</button>}</div>
    </section> : <>
      {recentList}
      <div ref={dropzone} className={`local-directory-dropzone${sourcePath ? ' has-selection' : recentList ? ' is-compact' : ''}`} onDragOver={event => { event.preventDefault(); event.dataTransfer.dropEffect = blocked ? 'none' : 'copy'; }} onDrop={event => {
        event.preventDefault(); if (blocked) return;
        const path = (event.dataTransfer.files[0] as File & { path?: string } | undefined)?.path;
        if (path && event.dataTransfer.files.length === 1) selectSource(path);
        else setNotice(t("请拖入一个项目文件夹，或点击“选择文件夹…”选择。"));
      }}>
        <FolderOpen size={sourcePath ? 20 : 28} aria-hidden="true" /><strong>{sourcePath ? t("已选择项目文件夹") : t("把项目文件夹拖到这里")}</strong>{!sourcePath && <p>{t("选择后会检查能否在熊猫小抽屉里使用。")}</p>}
        <button className="refresh-button" disabled={blocked} onClick={choose}>{pending === 'choose' ? t("正在选择与检查…") : sourcePath ? t("换一个") : t("选择文件夹…")}</button>
      </div>
      <ul className="local-directory-guarantees" hidden={Boolean(sourcePath)}><li>{t("检查只读取文件，不修改、不移动你的文件夹。")}</li><li>{t("添加后直接使用这里的代码，你和 AI 继续在这里修改。")}</li><li>{t("文件数据使用这个文件夹的 data/；浏览器数据由平台按实例保存。")}</li></ul>
      {sourcePath && <div className="local-directory-path"><label htmlFor={`${id}-path`}>{t("你的文件夹")}</label><input id={`${id}-path`} className="node-selected-source" readOnly value={sourcePath} /><div className="local-directory-actions"><button className="text-button" disabled={blocked} onClick={copyPath}><Copy size={13} />{t("复制文件夹路径")}</button><button className="text-button" disabled={blocked} onClick={recheck}><RefreshCw size={13} />{pending === 'inspect' ? t("正在检查…") : t("重新检查")}</button></div></div>}
      {error && <section className="local-directory-result"><h3>{t("这次添加没有完成")}</h3><p className="task-error" role="alert">{t(error)} {t(" 请重新检查或重新选择文件夹后继续。")}</p><p>{t("文件在哪里：")}<code>{sourcePath}</code>{t("。本次结果需要核对，已有文件和登记不会自动删除。")}</p><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'local', sourcePath: sourcePath || undefined, issues: helpIssues, platformVersion, checkedAt }} label={t("复制说明给 AI")} disabled={blocked} /></section>}
      {(pending === 'initialize' || pending === 'register') && <p className="workspace-inline-notice" role="status">{pending === 'initialize' ? t("正在生成并添加项目…") : t("正在添加应用…")}</p>}
      {notice && <p className="workspace-inline-notice" role="status">{t(notice)}</p>}
      {comparison && <p className="local-directory-diff" role="status">{t(comparison)}</p>}
      {existing && <section className="local-directory-result local-directory-existing"><span className="local-directory-result-label is-ready">{t("已添加")}</span><h3>{t("这个文件夹已经添加过了")}</h3><p>{t("「{name}」使用的就是这个文件夹，同一个文件夹不能添加两次。", { name: existing.name })}</p><p>{t("文件在哪里：")}<code>{sourcePath}</code></p><div className="confirmation-actions"><button className="primary-button" disabled={blocked || (!onEnter && !onViewDetails && !onWebEnter && !onWebDetails)} onClick={() => void existingAction(false)}>{t("打开「{name}」", { name: existing.name })}</button>{(onViewDetails || onWebDetails || existingDetail) && <button className="refresh-button" disabled={blocked} onClick={() => void existingAction(true)}>{t("查看应用详情")}</button>}<button className="refresh-button" disabled={blocked} onClick={choose}>{t("换一个文件夹")}</button></div></section>}
      {review?.state === 'empty' && <section className="local-directory-empty local-directory-result" aria-label={t("空目录初始化")}><span className="local-directory-result-label">{t("空文件夹")}</span><h3>{t("这个文件夹是空的，要在这里新建一个应用吗？")}</h3><p id={`${id}-create-hint`}>{t(platform.localDirectoryInitializationHint)}</p><label htmlFor={`${id}-name`}>{t("项目名称")}<input id={`${id}-name`} value={name} maxLength={200} disabled={blocked} onChange={event => setName(event.target.value)} /></label><button className="primary-button" aria-describedby={`${id}-create-hint`} disabled={blocked || !name.trim()} onClick={initializeAndRegister}>{t("在这里新建")}</button></section>}
      {inspection && <section className="node-inspection-preview local-directory-result" aria-label={t("可添加项目预览")}><span className="local-directory-result-label is-ready"><CheckCircle2 size={15} />{t("可以添加")}</span><h3>{t("「{name}」可以添加", { name: inspection.app.name })}</h3><p id={`${id}-add-hint`}>{t("核对下面的项目和文件夹后，就可以添加。检查通过还不代表已经实际运行。")}</p><p>{inspection.app.description}</p>{inspection.needsBuild && <p role="status">{t("网页入口尚未生成，添加后可在详情中构建。")}</p>}{inspection.entryMissing && !inspection.needsBuild && <p role="status">{t("网页入口文件缺失，添加后请让 AI 补齐。")}</p>}<NodeDependencyWarnings warnings={inspection.dependencyWarnings} /><span className="node-runtime-label"><Server size={14} />{t(inspection.runtimeLabel)}{inspection.app.version ? ` · ${inspection.app.version}` : ''}</span><div className="local-directory-location"><strong>{t("文件在哪里")}</strong><p>{t("源码：")}<code>{sourcePath}</code>{t("（原地使用，不移动、不修改）")}</p><p>{t("文件数据：这个文件夹的 data/。浏览器数据由平台按实例保存，删除应用时清除。")}</p></div><p>{t("有 {count} 项必填设置，添加后在应用设置中填写。", { count: inspection.configuration.fields.filter(field => field.required).length })}</p><div className="local-directory-ready-action"><button ref={addButton} className="primary-button" aria-describedby={`${id}-add-hint`} disabled={blocked} onClick={register}>{t("添加")}</button>{onClose && <button className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>{t("取消")}</button>}</div><details className="local-directory-details"><summary>{t("检查详情")}</summary><p>{t(platform.localDirectoryReviewDeletion)}</p><ul>{inspection.configuration.fields.map((field, index) => <li key={index}>{field.label}{field.required ? t("（必填）") : t("（选填）")}{field.secret ? t(" · 密钥") : ''}</li>)}</ul></details></section>}
      {review?.state === 'needs-review' && <section className="local-directory-review local-directory-result"><span className={`local-directory-result-label is-${category}`}>{t(categoryLabel)}</span><h3 ref={resultHeading} tabIndex={-1}>{t(reviewTitle)}</h3><ul className="local-directory-reasons" aria-label={t("目录检查说明")}>{(review.issues.length ? review.issues : [t("无法确定这个文件夹里要运行的应用。")]).slice(0, 3).map((issue, index) => { const reason = localReviewReason(issue); return <li key={index}><strong>{t(reason.title)}</strong><p>{t(reason.description)}</p></li>; })}</ul><div className="local-directory-location"><strong>{t("文件在哪里")}</strong><p>{t("你的文件夹保持原样，没有添加任何内容。")}</p><code>{sourcePath}</code></div><p>{t("把说明交给你的 AI：它会先检查，你同意后修改，再直接添加到熊猫小抽屉。AI 没能添加时，点「重新检查」。")}</p><div className="confirmation-actions"><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'local', sourcePath, issues: review.issues, platformVersion, checkedAt }} label={category === 'unsupported' ? t("复制说明问 AI 有没有替代办法") : t("交给 AI 处理")} disabled={blocked} className="primary-button" />{onClose && <button className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>{t("关闭")}</button>}</div></section>}
      {canShowPrompt && <details className="local-directory-prompt local-directory-details"><summary>{t("检查详情与完整说明")}</summary>{review?.issues.map((issue, index) => <p key={index}>{t(issue)}</p>)}<label htmlFor={`${id}-prompt`}>{review ? t("给 AI 的完整检查说明") : t("上次保留的检查说明（重新检查后更新）")}</label><textarea id={`${id}-prompt`} aria-label={t("给 AI 的检查说明")} readOnly rows={7} value={localizePlatformPrompt(reviewPrompt)} /></details>}
      {copyNotice && <p className="workspace-inline-notice" role="status">{t(copyNotice)}</p>}
      {pending !== 'initialize' && pending !== 'register' && <div className="add-project-ai-footer"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'local', sourcePath: sourcePath || undefined, platformVersion }} disabled={blocked} /></div>}
    </>}
  </div>;
}
