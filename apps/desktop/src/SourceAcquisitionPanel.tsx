import { invoke } from '@tauri-apps/api/core';
import { CheckCircle2, Download, LoaderCircle } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import { SourceLibraryPanel } from './SourceLibraryPanel';
import { SourceImportError, SourceImportSession } from './source-import';
import type { InstalledNodeResult } from './NodeInstallPanel';
import type { NodeAppDetail } from './node-app-types';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type Props = { disabled: boolean; instanceId?: string; onClose: () => void; onTask: (task: TaskRecord) => void; onInstalled?: () => void; onEnter?: (app: NodeAppDetail) => void };
type View = 'form' | 'running' | 'build' | 'failed' | 'recovery' | 'success';
const phases = ['下载源码', '检查兼容性', '准备应用', '完成导入'];
const phaseDetails: Record<string, string> = { environment: '正在检查运行环境', downloading: '正在从仓库下载源码', 'validating-objects': '正在校验下载内容', materializing: '正在整理源码文件', inspecting: '正在检查应用格式', copying: '正在复制应用文件', dependencies: '正在安装应用依赖', validating: '正在校验应用材料', committing: '正在完成应用登记', 'build-dependencies': '正在准备构建依赖', building: '正在构建应用' };

export function SourceAcquisitionPanel(props: Props) {
  const [url, setUrl] = useState('');
  const [branch, setBranch] = useState('');
  const [name, setName] = useState('');
  return <GitImportWorkflow key={props.instanceId ?? ''} {...props} url={url} setUrl={setUrl} branch={branch} setBranch={setBranch} name={name} setName={setName} />;
}
function GitImportWorkflow({ disabled, instanceId, onClose, onTask, onInstalled, onEnter, url, setUrl, branch, setBranch, name, setName }: Props & {
  url: string; setUrl: (value: string) => void; branch: string; setBranch: (value: string) => void; name: string; setName: (value: string) => void;
}) {
  const [view, setView] = useState<View>('form');
  const [busy, setBusy] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState('');
  const [cleanupError, setCleanupError] = useState('');
  const [cleanupDone, setCleanupDone] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [app, setApp] = useState<NodeAppDetail | null>(null);
  const [showLibrary, setShowLibrary] = useState(false);
  const [, redraw] = useState(0);
  const alive = useRef(false);
  const lifetime = useRef(0);
  const operation = useRef(false);
  const session = useRef<SourceImportSession | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const callbacks = useRef({ onTask, onInstalled, onEnter, onClose }); callbacks.current = { onTask, onInstalled, onEnter, onClose };
  useEffect(() => { alive.current = true; lifetime.current++; return () => { alive.current = false; lifetime.current++; }; }, []);
  useEffect(() => { if (view !== 'form' && view !== 'running') heading.current?.focus(); }, [view]);
  const current = (generation: number) => alive.current && lifetime.current === generation;
  const flow = session.current;
  const hasDownload = Boolean(flow?.sourceTaskId);
  const unsupported = Boolean(flow?.source && flow.source.inspection?.kind !== 'node-service-source');
  const active = busy || view === 'running';
  const step = flow?.phase === 'done' || flow?.phase === 'install' ? 3 : flow?.phase === 'prepare' ? 2 : flow?.phase === 'check' ? 1 : 0;

  function fail(cause: unknown) {
    setError(messageFrom(cause));
    setView(cause instanceof SourceImportError && cause.recovery ? 'recovery' : 'failed');
  }
  async function clean(value: SourceImportSession) {
    setCleanupError(''); setCleaning(true);
    try { await value.cleanup(); setCleanupDone(true); return true; }
    catch (cause) { if (alive.current) setCleanupError(messageFrom(cause)); return false; }
    finally { if (alive.current) setCleaning(false); }
  }
  async function readInstalled(value: SourceImportSession, result: InstalledNodeResult) {
    if (!alive.current || session.current !== value) return;
    callbacks.current.onInstalled?.();
    setView('success'); setError('');
    await clean(value);
    if (!alive.current || session.current !== value) return;
    try {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: result.appId });
      if (!value || session.current !== value || !alive.current) return;
      if (detail.id !== result.appId) throw new Error('应用信息身份不符，请重新读取。');
      setApp(detail);
    } catch (cause) { if (alive.current && session.current === value) setError(`应用已导入，配置暂时无法读取：${messageFrom(cause)}`); }
  }
  async function install(value: SourceImportSession, buildApproved: boolean) {
    const result = await value.install(buildApproved);
    await readInstalled(value, result);
  }
  async function start() {
    if (operation.current || disabled || !url.trim()) return;
    operation.current = true; setBusy(true); setError(''); setCleanupError('');
    const generation = lifetime.current;
    try {
      // A retry first accounts for the previous import's exact materials. It
      // never scans or deletes unrelated sources or installation records.
      if (session.current?.sourceTaskId && !cleanupDone && !await clean(session.current)) return;
      if (!current(generation)) return;
      const value: SourceImportSession = new SourceImportSession(invoke, {
        isCurrent: (): boolean => current(generation) && session.current === value,
        onChange: () => redraw(count => count + 1), onTask: task => callbacks.current.onTask(task),
      });
      session.current = value; setCleanupDone(false); setApp(null); setCancelling(false); setView('running');
      const source = await value.acquire({ url: url.trim(), branch: branch.trim() || null, ...(name.trim() ? { name: name.trim() } : {}) });
      if (source.inspection?.requiresBuild && !source.buildSourceDigest) setView('build');
      else await install(value, false);
    } catch (cause) { if (current(generation)) fail(cause); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function build() {
    if (operation.current || disabled || !session.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true); setView('running'); setError('');
    try { await install(session.current, true); }
    catch (cause) { if (current(generation)) fail(cause); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function cancel() {
    if (!session.current || cancelling || session.current.phase === 'install') return;
    setCancelling(true); setError('');
    try { await session.current.cancel(); }
    catch (cause) { if (alive.current) { setError(`取消请求暂未确认：${messageFrom(cause)}`); setCancelling(false); } }
  }
  async function cleanAndClose() {
    if (operation.current) return;
    if (!session.current?.sourceTaskId || cleanupDone) { callbacks.current.onClose(); return; }
    operation.current = true; setBusy(true);
    const value = session.current; const generation = lifetime.current;
    try { if (await clean(value) && current(generation)) callbacks.current.onClose(); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function editAddress() {
    if (operation.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true);
    try {
      if (session.current?.sourceTaskId && !cleanupDone && !await clean(session.current)) return;
      if (!current(generation)) return;
      session.current = null; setError(''); setCleanupError(''); setCleanupDone(false); setView('form');
    } finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function retryCleanup() {
    if (operation.current || !session.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true);
    try { await clean(session.current); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function reconcile() {
    if (operation.current || !session.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true); setError('');
    try { await readInstalled(session.current, await session.current.reconcile()); }
    catch (cause) { if (current(generation)) fail(cause); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function enter() {
    if (operation.current || !session.current?.installed) return;
    if (app) { callbacks.current.onClose(); callbacks.current.onEnter?.(app); return; }
    const value = session.current; const generation = lifetime.current;
    operation.current = true; setBusy(true);
    try {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: value.installed!.appId });
      if (!current(generation)) return;
      if (detail.id !== value.installed!.appId) throw new Error('应用信息身份不符，请重新读取。');
      setApp(detail); setError('');
      callbacks.current.onClose(); callbacks.current.onEnter?.(detail);
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  function close() {
    if (active) return;
    if (view === 'recovery') callbacks.current.onClose();
    else void cleanAndClose();
  }
  if (showLibrary) return <SourceLibraryPanel disabled={disabled} instanceId={instanceId} onTask={onTask} onInstalled={onInstalled} onClose={() => setShowLibrary(false)} />;
  return <Modal title="从 Git 导入应用" titleIcon={<Download size={20} />} description="粘贴仓库地址，下载、检查并添加到应用列表。" onClose={close} closeDisabled={active} className="git-import-modal">
    {view === 'form' && <form className="create-project-form" onSubmit={event => { event.preventDefault(); void start(); }}>
      <label htmlFor="git-import-url">Git 仓库地址</label>
      <input id="git-import-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/作者/应用.git" autoCapitalize="none" autoCorrect="off" spellCheck={false} />
      <p className="git-import-hint">支持 GitHub、Gitee 的公开 HTTPS 仓库。</p>
      <details className="git-import-options"><summary>高级选项</summary>
        <label htmlFor="git-import-branch">分支（选填）</label><input id="git-import-branch" value={branch} onChange={event => setBranch(event.target.value)} placeholder="留空使用默认分支" autoCapitalize="none" autoCorrect="off" spellCheck={false} />
        <label htmlFor="git-import-name">备注名称（选填）</label><input id="git-import-name" value={name} onChange={event => setName(event.target.value)} />
      </details>
      <p className="git-import-hint">导入会下载依赖并安装为一个新应用。需要执行构建脚本时会先请你确认，完成后由你打开。</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || !url.trim()}>导入应用</button></div>
    </form>}
    {view !== 'form' && <>
      <ol className="git-import-steps" aria-label="导入进度">{phases.map((label, index) => <li key={label} aria-current={step === index && view !== 'success' ? 'step' : undefined} className={view === 'success' || index < step ? 'is-complete' : ''}><span>{view === 'success' || index < step ? <CheckCircle2 size={16} /> : index + 1}</span>{label}</li>)}</ol>
      <section className="git-import-result" aria-busy={active}>
        <h3 ref={heading} tabIndex={-1}>{view === 'success' ? '导入成功' : view === 'build' ? '此应用需要构建' : view === 'recovery' ? '导入结果需要核对' : view === 'failed' ? flow?.cancelled ? '导入已取消' : unsupported ? '这个仓库暂不能直接导入' : '导入未完成' : cancelling ? '正在取消导入…' : phases[step]}</h3>
        {view === 'running' && <p role="status"><LoaderCircle size={16} className="git-import-spinner" />{cancelling ? '正在等待任务停止，停止后即可清理临时文件。' : phaseDetails[flow?.task?.phase ?? ''] ?? '正在处理，请稍候…'}</p>}
        {flow?.source && <p>{flow.source.inspection?.name ?? flow.source.name}</p>}
        {view === 'build' && <><p>将执行这个仓库声明的构建脚本，完成后继续安装。请确认你信任这个来源。</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy || disabled} onClick={() => void build()}>构建并继续导入</button><button type="button" className="refresh-button" disabled={busy} onClick={() => void cleanAndClose()}>取消并清理</button></div></>}
        {view === 'success' && <><p role="status">已添加到应用列表。{app ? app.configuration.canStart ? '现在可以打开使用。' : '完成必要配置后即可使用。' : busy ? '正在读取应用配置。' : '可以点击查看应用重新读取配置。'}</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void enter()}>{app ? app.configuration.canStart ? '打开应用' : '去配置' : '查看应用'}</button><button type="button" className="refresh-button" disabled={busy} onClick={() => void cleanAndClose()}>完成</button></div>{cleanupDone && <p className="git-import-hint">临时文件已清理，源码已保留，可用于后续修改和版本管理。</p>}</>}
        {error && <p role="alert" className="task-error">{error}</p>}
        {flow?.warning && <p role="status">{flow.warning}</p>}
        {cleaning && <p role="status">正在清理本次临时文件…</p>}
        {cleanupError && <div role="alert"><p className="task-error">{cleanupError}</p><button type="button" className="refresh-button" disabled={busy} onClick={() => void retryCleanup()}>重试清理</button></div>}
        {view === 'failed' && <div className="confirmation-actions">{!unsupported && <button type="button" className="primary-button" disabled={busy || disabled} onClick={() => void start()}>重试导入</button>}<button type="button" className={unsupported ? 'primary-button' : 'refresh-button'} disabled={busy} onClick={() => void editAddress()}>修改地址</button><button type="button" className="refresh-button" disabled={busy} onClick={() => void cleanAndClose()}>{hasDownload && !cleanupDone ? '清理并关闭' : '关闭'}</button>{unsupported && <button type="button" className="text-button" disabled={busy} onClick={() => setShowLibrary(true)}>查看源码记录</button>}</div>}
        {view === 'recovery' && <><p>先核对本次任务，确认前不会重新安装或删除材料。也可以从“源码记录与恢复”继续处理。</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void reconcile()}>核对安装结果</button><button type="button" className="refresh-button" disabled={busy} onClick={() => setShowLibrary(true)}>源码记录与恢复</button></div></>}
        {view === 'running' && flow?.phase !== 'install' && <button type="button" className="refresh-button" disabled={cancelling} onClick={() => void cancel()}>取消导入</button>}
        {flow?.task && <details className="git-import-task"><summary>任务详情与日志</summary><ProjectTask key={flow.task.taskId} task={flow.task} /></details>}
      </section>
    </>}
    {view === 'form' && <div className="git-import-library"><p>本机源码、之前的获取记录或中断任务</p><button type="button" className="text-button" disabled={busy || disabled} onClick={() => setShowLibrary(true)}>源码记录与恢复</button></div>}
  </Modal>;
}
