import { invoke } from '@tauri-apps/api/core';
import { CheckCircle2, Copy, FolderOpen, History, RefreshCw, Server } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { AiProjectHelp } from './AiProjectHelp';
import { ImportProgress } from './ImportProgress';
import { NodeDataContractInfo } from './NodeDataContractInfo';
import { isAbsoluteFolderPath, localReviewCategory, localReviewComparison, localReviewReason, normalizedFolderPath, readRecentProjectFolders, removeRecentProjectFolder, saveRecentProjectFolder } from './local-directory-review';
import type { RecentProjectFolder } from './local-directory-review';
import type { NodeAppDetail, NodeDirectoryReview } from './node-app-types';
import { messageFrom } from './ui-utils';

type Operation = 'choose' | 'inspect' | 'initialize' | 'register' | 'copy' | null;

function checkedReview(value: NodeDirectoryReview, sourcePath: string): NodeDirectoryReview {
  if (!value || value.sourcePath !== sourcePath || !['empty', 'ready', 'needs-review'].includes(value.state)
    || typeof value.expectedRevision !== 'string' || !value.expectedRevision
    || !Array.isArray(value.issues) || !value.issues.every(issue => typeof issue === 'string')
    || typeof value.reviewPrompt !== 'string') throw new Error('目录检查结果不完整或路径不符，请重新检查。');
  if (value.existingApp && (typeof value.existingApp.id !== 'string' || !value.existingApp.id || typeof value.existingApp.name !== 'string' || !value.existingApp.name)) throw new Error('已添加应用的信息不完整，请刷新项目列表后继续。');
  if (value.state === 'ready' && (!value.inspection || value.inspection.runtime !== 'node-service'
    || !value.inspection.app || typeof value.inspection.app.name !== 'string' || typeof value.inspection.app.version !== 'string'
    || !Array.isArray(value.inspection.configuration?.fields)
    || !value.inspection.configuration.fields.every(field => field && typeof field.label === 'string' && typeof field.required === 'boolean'))) throw new Error('项目预览不完整，请重新检查目录。');
  if (value.state === 'needs-review' && !value.reviewPrompt) throw new Error('缺少给 AI 的检查说明，请重新检查目录。');
  return value;
}

function checkedAddedApp(value: NodeAppDetail, sourcePath: string): NodeAppDetail {
  if (!value || !value.id || value.sourcePath !== sourcePath || value.runtime !== 'node-service' || !Array.isArray(value.fields) || !value.configuration || typeof value.configuration.canStart !== 'boolean') throw new Error('添加结果尚不能确认。请重新检查登记状态，不要重复添加。');
  return value;
}

export function LocalDirectoryPanel({ enabled, instanceId, workspacePath, platformVersion, apps = [], onPendingChange, onAdded, onEnter, onViewDetails, onImportArchive, onClose }: {
  enabled: boolean;
  instanceId?: string;
  workspacePath?: string;
  platformVersion?: string;
  apps?: NodeAppDetail[];
  onPendingChange: (pending: boolean) => void;
  onAdded: (app: NodeAppDetail) => void;
  onEnter?: (app: NodeAppDetail) => void;
  onViewDetails?: (app: NodeAppDetail) => void;
  onImportArchive?: () => void;
  onClose?: () => void;
}) {
  const scope = workspacePath ?? instanceId ?? 'unresolved-workspace';
  const [sourcePath, setSourcePath] = useState('');
  const [phase, setPhase] = useState(0);
  const [completed, setCompleted] = useState<NodeAppDetail | null>(null);
  const [generatedHere, setGeneratedHere] = useState(false);
  const [existing, setExisting] = useState<{ id: string; name: string } | null>(null);
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
  const context = useRef({ enabled, instanceId, scope, apps }); context.current = { enabled, instanceId, scope, apps };
  const callbacks = useRef({ onPendingChange, onAdded, onEnter, onViewDetails, onImportArchive, onClose }); callbacks.current = { onPendingChange, onAdded, onEnter, onViewDetails, onImportArchive, onClose };
  const selectDropped = useRef<(path: string) => void>(() => {});
  const id = useId();
  const blocked = !enabled || pending !== null;

  function revokeReview() { focusPreview.current = false; setReview(null); setExisting(null); setComparison(null); }
  function remember(path: string, result: RecentProjectFolder['result'], issues: string[]) {
    const item = { path, result, issues, selectedAt: new Date().toISOString() };
    setRecent(saveRecentProjectFolder(context.current.scope, item));
  }
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; operation.current = false; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => {
    sequence.current++; operation.current = false; setPending(null); callbacks.current.onPendingChange(false);
    revokeReview(); setCopyNotice(''); setCompleted(null); setGeneratedHere(false); setPhase(sourcePath ? 1 : 0);
    if (sourcePath) setNotice('接入状态已变化，请重新检查目录后继续。');
  }, [instanceId, enabled, scope]);
  useEffect(() => { setRecent(readRecentProjectFolders(scope)); }, [scope]);
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
        setError(`${messageFrom(cause)}${kind === 'initialize' ? ' 已生成的文件会保留；请重新检查并核对项目列表，不要重复生成。' : ''}`);
      }
    }
    finally { if (current()) { operation.current = false; setPending(null); callbacks.current.onPendingChange(false); } }
  }
  async function inspect(source: string, current: () => boolean) {
    setPhase(1);
    const already = context.current.apps.find(app => normalizedFolderPath(app.development?.sourcePath ?? app.sourcePath) === normalizedFolderPath(source));
    if (already) { setExisting(already); remember(source, '已添加', []); return; }
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
    if (checked.existingApp) { setExisting(checked.existingApp); remember(source, '已添加', []); return; }
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
      if (!source) { setNotice('已取消选择。已选目录仍保留，重新检查后才能继续。'); return; }
      if (!isAbsoluteFolderPath(source)) throw new Error('选择器未返回有效的本地目录，请重新选择。');
      setSourcePath(source); setGeneratedHere(false); setName(source.split(/[\\/]/).filter(Boolean).at(-1) ?? '新项目'); setReviewPrompt(''); setCheckedAt(undefined);
      await inspect(source, current);
    });
  }
  function selectSource(source: string) {
    if (blocked || operation.current) return;
    if (!isAbsoluteFolderPath(source)) { setNotice('请使用“选择文件夹…”选择电脑上的项目文件夹。'); return; }
    revokeReview(); setSourcePath(source); setGeneratedHere(false); setReviewPrompt(''); setCheckedAt(undefined); setName(source.split(/[\\/]/).filter(Boolean).at(-1) ?? '新项目');
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
      setCompleted(added); setGeneratedHere(true); setPhase(2); remember(selected.sourcePath, '已添加', []); callbacks.current.onAdded(added);
    });
  }
  function register() {
    if (blocked || !review || review.state !== 'ready' || operation.current) return;
    const selected = review; revokeReview(); setPhase(2);
    void run('register', async current => {
      const value = await invoke<NodeAppDetail>('node_app_directory_register', { sourcePath: selected.sourcePath, expectedRevision: selected.expectedRevision });
      if (!current()) return;
      const added = checkedAddedApp(value, selected.sourcePath);
      setCompleted(added); setPhase(2); remember(selected.sourcePath, '已添加', []); callbacks.current.onAdded(added);
    });
  }
  function copyPath() {
    if (blocked || !sourcePath || operation.current) return;
    void run('copy', async current => {
      try {
        if (!navigator.clipboard?.writeText) throw new Error('剪贴板不可用');
        await navigator.clipboard.writeText(sourcePath);
        if (current()) setCopyNotice('文件夹路径已复制。');
      } catch { if (current()) setCopyNotice('复制未完成，可选中上方完整路径后手动复制。'); }
    });
  }
  async function existingAction(details: boolean) {
    if (blocked || !existing) return;
    const selected = context.current.apps.find(app => app.id === existing.id);
    if (selected) { (details ? callbacks.current.onViewDetails ?? callbacks.current.onEnter : callbacks.current.onEnter)?.(selected); return; }
    void run('inspect', async current => {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: existing.id });
      if (!current()) return;
      if (detail.id !== existing.id) throw new Error('应用信息已变化，请刷新项目列表后继续。');
      (details ? callbacks.current.onViewDetails ?? callbacks.current.onEnter : callbacks.current.onEnter)?.(detail);
    });
  }

  const inspection = review?.state === 'ready' ? review.inspection : null;
  const category = review?.state === 'needs-review' ? localReviewCategory(review) : null;
  const categoryLabel = category === 'unsupported' ? '当前不适合' : category === 'insufficient' ? '信息不足' : '需要修改';
  const reviewTitle = category === 'unsupported' ? '这个项目目前没法直接使用' : category === 'insufficient' ? '还需要进一步确认这个文件夹' : '这个项目需要修改后才能添加';
  const existingDetail = existing ? apps.find(app => app.id === existing.id) : null;
  const helpIssues = review?.issues ?? (error ? [error] : []);
  const canShowPrompt = reviewPrompt && pending !== 'initialize' && pending !== 'register' && (review?.state === 'needs-review' || !review);
  return <div className="node-add-dialog local-directory-panel" aria-label="本地项目文件夹接入" aria-busy={pending !== null}>
    <ImportProgress steps={['选择文件夹', '检查项目', '添加应用']} currentStep={phase} state={completed ? 'succeeded' : error ? 'failed' : review?.state === 'needs-review' ? 'warning' : pending && pending !== 'copy' ? 'running' : 'paused'} label="添加进度" />
    {completed ? <section className="import-result local-directory-result">
      <span className="local-directory-result-label is-ready"><CheckCircle2 size={15} />添加成功</span>
      <h3 ref={resultHeading} tabIndex={-1}>已添加「{completed.name}」</h3>
      <p role="status">应用现在使用新建的数据位置。{completed.configuration.canStart ? '可以打开应用。' : '完成必要设置后就可以使用。'}</p>
      <div className="local-directory-location"><strong>文件在哪里</strong><p>源码：<code>{completed.sourcePath}</code>（{generatedHere ? '原位置，已生成起始项目' : '原位置，未改动'}）</p><p>应用数据另外保存在熊猫小抽屉里，原项目里的数据不会自动带过来。</p></div>
      <p className="project-help">没有临时下载文件需要清理。以后删除应用时，你的文件夹仍会保留。</p>
      <div className="confirmation-actions">{onEnter && <button type="button" className="primary-button" disabled={blocked} onClick={() => callbacks.current.onEnter?.(completed)}>{completed.configuration.canStart ? '打开应用' : '去设置'}</button>}{onClose && <button type="button" className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>完成</button>}</div>
    </section> : <>
      <div ref={dropzone} className={`local-directory-dropzone${sourcePath ? ' has-selection' : ''}`} onDragOver={event => { event.preventDefault(); event.dataTransfer.dropEffect = blocked ? 'none' : 'copy'; }} onDrop={event => {
        event.preventDefault(); if (blocked) return;
        const path = (event.dataTransfer.files[0] as File & { path?: string } | undefined)?.path;
        if (path && event.dataTransfer.files.length === 1) selectSource(path);
        else setNotice('请拖入一个项目文件夹，或点击“选择文件夹…”选择。');
      }}>
        <FolderOpen size={sourcePath ? 20 : 28} aria-hidden="true" /><strong>{sourcePath ? '已选择项目文件夹' : '把项目文件夹拖到这里'}</strong>{!sourcePath && <p>选择后会检查能否在熊猫小抽屉里使用。</p>}
        <button className="refresh-button" disabled={blocked} onClick={choose}>{pending === 'choose' ? '正在选择与检查…' : sourcePath ? '换一个' : '选择文件夹…'}</button>
      </div>
      <ul className="local-directory-guarantees" hidden={Boolean(sourcePath)}><li>检查只读取文件，不修改、不移动你的文件夹。</li><li>添加后直接使用这里的代码，你和 AI 继续在这里修改。</li><li>应用数据另外保存，原项目里的数据不会自动带过来。</li></ul>
      {!sourcePath && recent.length > 0 && <section className="local-directory-recents"><h3><History size={16} />最近选过的文件夹</h3><p className="project-help">每次选择都会重新检查，上次结果仅供参考。</p>{recent.map(item => <div className="local-directory-recent" key={item.path}><button className="text-button" disabled={blocked} onClick={() => selectSource(item.path)}><span>{item.path}</span><small>{item.result}</small></button><button className="text-button" disabled={blocked} aria-label={`移除最近目录 ${item.path}`} onClick={() => setRecent(removeRecentProjectFolder(scope, item.path))}>移除</button></div>)}</section>}
      {sourcePath && <div className="local-directory-path"><label htmlFor={`${id}-path`}>你的文件夹</label><input id={`${id}-path`} className="node-selected-source" readOnly value={sourcePath} /><div className="local-directory-actions"><button className="text-button" disabled={blocked} onClick={copyPath}><Copy size={13} />复制文件夹路径</button><button className="text-button" disabled={blocked} onClick={recheck}><RefreshCw size={13} />{pending === 'inspect' ? '正在检查…' : '重新检查'}</button></div></div>}
      {error && <section className="local-directory-result"><h3>这次添加没有完成</h3><p className="task-error" role="alert">{error} 请重新检查或重新选择文件夹后继续。</p><p>文件在哪里：<code>{sourcePath}</code>。本次结果需要核对，已有文件和登记不会自动删除。</p><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'local', sourcePath: sourcePath || undefined, issues: helpIssues, platformVersion, checkedAt }} label="复制说明给 AI" disabled={blocked} /></section>}
      {(pending === 'initialize' || pending === 'register') && <p className="workspace-inline-notice" role="status">{pending === 'initialize' ? '正在生成并添加项目…' : '正在添加应用…'}</p>}
      {notice && <p className="workspace-inline-notice" role="status">{notice}</p>}
      {comparison && <p className="local-directory-diff" role="status">{comparison}</p>}
      {existing && <section className="local-directory-result local-directory-existing"><span className="local-directory-result-label is-ready">已添加</span><h3>这个文件夹已经添加过了</h3><p>「{existing.name}」使用的就是这个文件夹，同一个文件夹不能添加两次。</p><p>文件在哪里：<code>{sourcePath}</code></p><div className="confirmation-actions"><button className="primary-button" disabled={blocked || (!onEnter && !onViewDetails)} onClick={() => void existingAction(false)}>打开「{existing.name}」</button>{(onViewDetails || existingDetail) && <button className="refresh-button" disabled={blocked} onClick={() => void existingAction(true)}>查看应用详情</button>}<button className="refresh-button" disabled={blocked} onClick={choose}>换一个文件夹</button></div></section>}
      {review?.state === 'empty' && <section className="local-directory-empty local-directory-result" aria-label="空目录初始化"><span className="local-directory-result-label">空文件夹</span><h3>可以在这里开始一个新项目</h3><p id={`${id}-create-hint`}>点击后会在这个空文件夹生成起始文件并添加项目，不安装依赖或自动运行。以后删除应用时，这个文件夹仍会保留。</p><label htmlFor={`${id}-name`}>项目名称<input id={`${id}-name`} value={name} maxLength={200} disabled={blocked} onChange={event => setName(event.target.value)} /></label><button className="primary-button" aria-describedby={`${id}-create-hint`} disabled={blocked || !name.trim()} onClick={initializeAndRegister}>生成并添加项目</button></section>}
      {inspection && <section className="node-inspection-preview local-directory-result" aria-label="可添加项目预览"><span className="local-directory-result-label is-ready"><CheckCircle2 size={15} />可以添加</span><h3>「{inspection.app.name}」可以添加</h3><p id={`${id}-add-hint`}>核对下面的项目和文件夹后，就可以添加。检查通过还不代表已经实际运行。</p><p>{inspection.app.description}</p><span className="node-runtime-label"><Server size={14} />{inspection.runtimeLabel} · {inspection.app.version}</span><div className="local-directory-location"><strong>文件在哪里</strong><p>源码：<code>{sourcePath}</code>（原地使用，不移动、不修改）</p><p>应用数据：新建独立位置，原项目里的数据不会自动带过来。</p></div><p>有 {inspection.configuration.fields.filter(field => field.required).length} 项必填设置，添加后在应用设置中填写。</p><div className="local-directory-ready-action"><button ref={addButton} className="primary-button" aria-describedby={`${id}-add-hint`} disabled={blocked} onClick={register}>添加</button>{onClose && <button className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>取消</button>}</div><details className="local-directory-details"><summary>检查详情</summary><NodeDataContractInfo value={inspection.dataContract} legacyNotice="平台未验证实际数据格式。" /><p>此入口直接使用你的文件夹。删除应用只移除平台登记及独立的配置、应用数据和缓存，你的文件夹会保留。</p><ul>{inspection.configuration.fields.map((field, index) => <li key={index}>{field.label}{field.required ? '（必填）' : '（选填）'}{field.secret ? ' · 密钥' : ''}</li>)}</ul></details></section>}
      {review?.state === 'needs-review' && <section className="local-directory-review local-directory-result"><span className={`local-directory-result-label is-${category}`}>{categoryLabel}</span><h3 ref={resultHeading} tabIndex={-1}>{reviewTitle}</h3><ul className="local-directory-reasons" aria-label="目录检查说明">{(review.issues.length ? review.issues : ['无法确定这个文件夹里要运行的应用。']).slice(0, 3).map((issue, index) => { const reason = localReviewReason(issue); return <li key={index}><strong>{reason.title}</strong><p>{reason.description}</p></li>; })}</ul><div className="local-directory-location"><strong>文件在哪里</strong><p>你的文件夹保持原样，没有添加任何内容。</p><code>{sourcePath}</code></div><p>可以把说明发给原来开发这个项目的 AI。修改后，重新选择这个文件夹或点击“重新检查”。</p><div className="confirmation-actions">{review.projectFormat === 'project-v1' && onImportArchive ? <button className="primary-button" disabled={blocked} onClick={() => callbacks.current.onImportArchive?.()}>改用导入完整项目</button> : <AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'local', sourcePath, issues: review.issues, platformVersion, checkedAt }} label={category === 'unsupported' ? '复制说明问 AI 有没有替代办法' : '复制说明给 AI'} disabled={blocked} className="primary-button" />}{onClose && <button className="refresh-button" disabled={blocked} onClick={() => callbacks.current.onClose?.()}>关闭</button>}</div></section>}
      {canShowPrompt && <details className="local-directory-prompt local-directory-details"><summary>检查详情与完整说明</summary>{review?.issues.map((issue, index) => <p key={index}>{issue}</p>)}<label htmlFor={`${id}-prompt`}>{review ? '给 AI 的完整检查说明' : '上次保留的检查说明（重新检查后更新）'}</label><textarea id={`${id}-prompt`} aria-label="给 AI 的检查说明" readOnly rows={7} value={reviewPrompt} /></details>}
      {copyNotice && <p className="workspace-inline-notice" role="status">{copyNotice}</p>}
      {pending !== 'initialize' && pending !== 'register' && <div className="add-project-ai-footer"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'local', sourcePath: sourcePath || undefined, platformVersion }} disabled={blocked} /></div>}
    </>}
  </div>;
}
