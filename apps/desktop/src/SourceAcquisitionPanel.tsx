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
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { onProjectDeleted } from './project-deletion';
import { canDismissGitImport } from './git-import-view-state';
import type { GitImportViewState } from './git-import-view-state';
import { useFrontendPlatform } from './frontend-platform';
export { gitRepositoryAddress } from './git-repository';

type ExistingRepository = GitRepositorySource & { appId: string; name: string };
export type SourceAcquisitionPanelProps = {
  disabled: boolean; instanceId?: string; open?: boolean; projectDirectory?: string; platformVersion?: string;
  existingRepositories?: ExistingRepository[];
  onClose: (options?: { keepSession: boolean }) => void; onBackground?: () => void; onAttention?: () => void;
  onOpenLocal?: () => void; onOpenSettings?: () => void; onOpenExisting?: (appId: string) => void;
  onTask: (task: TaskRecord) => void; onInstalled?: () => void; onEnter?: (app: NodeAppDetail) => void;
  onStateChange?: (state: GitImportViewState | null) => void;
};
const phases = ['下载源码', '检查项目', '准备依赖', '添加应用', '清理临时文件'];
function phaseIndex(phase?: string) {
  return phase === 'completed' || phase === 'cleanup' ? 4 : phase === 'registering' ? 3 : phase === 'dependencies' ? 2 : ['checking', 'publishing', 'inspecting'].includes(phase ?? '') ? 1 : 0;
}
export function SourceAcquisitionPanel(props: SourceAcquisitionPanelProps) {
  return <GitImportWorkflow key={props.instanceId ?? ''} {...props} />;
}
function GitImportWorkflow({ disabled, instanceId, open = true, projectDirectory: suppliedDirectory, platformVersion, existingRepositories, onClose, onBackground, onAttention, onOpenLocal, onOpenSettings, onOpenExisting, onTask, onInstalled, onEnter, onStateChange }: SourceAcquisitionPanelProps) {
  const frontendPlatform = useFrontendPlatform();
  const [url, setUrl] = useState(''); const [branch, setBranch] = useState(''); const [name, setName] = useState('');
  const [directory, setDirectory] = useState(suppliedDirectory ?? '');
  const [view, setView] = useState<'form' | 'duplicate' | 'running' | 'failed' | 'unknown' | 'success'>('form');
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
  const canLeaveFailedFlow = canDismissGitImport({ sessionId: sessionId ?? '', url, name, task: task ?? null, snapshot: snapshot ?? null, view: 'failed', busy, error, warning });
  useEffect(() => {
    report.current?.(sessionId && !['form', 'duplicate'].includes(view) ? {
      sessionId, url, name: name.trim() || address?.repository || '正在添加的应用', task: task ?? null,
      snapshot: snapshot ?? null, view: view as GitImportViewState['view'], busy, error, warning,
    } : null);
  }, [sessionId, url, name, address?.repository, task, snapshot, view, busy, error, warning]);
  const diagnostics = checkedProcessDiagnostics(task?.error?.details?.processDiagnostics);
  const current = (id: number) => mounted.current && generation.current === id;
  function acceptResult() {
    const value = session.current?.snapshot;
    if (!value) { setView('unknown'); return; }
    if (!value.task.final) setView('running');
    else if (value.commitState === 'unknown') setView('unknown');
    else if (value.commitState === 'committed') { setView('success'); callbacks.current.onInstalled?.(); }
    else { setView('failed'); setError(messageFrom(value.task.error ?? '添加未完成。')); }
    if (value.commitState === 'unknown' || value.cleanupPending || value.task.status === 'failed' || value.task.status === 'interrupted') callbacks.current.onAttention?.();
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
    try { await value.run({ url: address.url, branch: branch.trim() || null, ...(name.trim() ? { name: name.trim() } : {}) }); if (current(id)) acceptResult(); }
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
    try { await flow.cancel(); } catch (cause) { if (mounted.current) { setError(messageFrom(cause)); setCancelling(false); } }
  }
  async function enter() {
    if (!snapshot || !committed || busy) return;
    try {
      const app = snapshot.app ?? await invoke<NodeAppDetail>('node_app_configuration', { appId: snapshot.appId });
      if (!mounted.current || session.current?.snapshot?.appId !== app.id) return;
      onEnter?.(app); onClose({ keepSession: Boolean(snapshot.cleanupPending) || snapshot.remainingPaths.length > 0 });
    } catch (cause) { if (mounted.current) setError(messageFrom(cause)); }
  }
  function dismiss() {
    if (busy && onBackground) { onBackground(); return; }
    onClose({ keepSession: busy || view === 'failed' || view === 'unknown' || Boolean(snapshot?.cleanupPending) });
  }
  if (!open) return null;
  return <Modal title="从 Git 添加项目" titleIcon={<Download size={20} />} className="source-acquisition-modal git-import-modal" wide onClose={dismiss}>
    {view === 'form' && <form onSubmit={event => { event.preventDefault(); void start(); }}>
      <label htmlFor="git-import-url">公开仓库地址</label><input id="git-import-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/作者/仓库" autoCapitalize="none" autoCorrect="off" spellCheck={false} />
      <p>支持 GitHub、Gitee 的公开 HTTPS 仓库。</p>
      <section className="add-project-after" aria-label="添加后文件在哪里"><strong>添加后</strong><dl><dt>项目文件夹</dt><dd><code>{directory || '正在读取项目目录…'}/{address?.repository ?? '仓库名'}</code>平台和 AI 直接使用这一份代码。同名时自动另取名字，删除项目时一起删除。</dd><dt>应用数据</dt><dd>配置、密钥、数据和日志单独保存在平台中。</dd></dl>{onOpenSettings && <button type="button" className="text-button" onClick={onOpenSettings}>{frontendPlatform.projectDirectorySettingsLabel}</button>}</section>
      <details className="git-import-options"><summary>高级选项</summary><label htmlFor="git-import-branch">分支（选填）</label><input id="git-import-branch" value={branch} onChange={event => setBranch(event.target.value)} placeholder="留空使用默认分支" /><label htmlFor="git-import-name">显示名称（选填）</label><input id="git-import-name" value={name} onChange={event => setName(event.target.value)} /></details>
      <p>平台只准备生产依赖，不执行仓库脚本。需要构建时，请让 AI 构建后用“添加本地项目”添加。</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || !address || !directory}>开始添加</button><button type="button" className="refresh-button" onClick={() => onClose()}>取消</button></div>
      <div className="add-project-ai-footer"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'git', url: url.trim() || undefined, branch: branch.trim() || undefined, platformVersion }} /></div>
    </form>}
    {view === 'duplicate' && duplicate && <section className="import-result"><h3>这个仓库和分支已经添加过：「{duplicate.name}」</h3><p>另外添加的一份会从空数据开始，两份互不影响。</p><div className="confirmation-actions"><button className="primary-button" onClick={() => { onOpenExisting?.(duplicate.appId); onClose(); }}>打开已添加的</button><button className="refresh-button" disabled={disabled} onClick={() => void start(true)}>仍然另外添加一份</button><button className="text-button" onClick={() => setView('form')}>取消</button></div></section>}
    {!['form', 'duplicate'].includes(view) && <>
      <ImportProgress label="添加进度" steps={phases} currentStep={phaseIndex(task?.phase)} state={busy ? 'running' : view === 'success' ? snapshot?.cleanupPending ? 'warning' : 'succeeded' : view === 'failed' ? 'failed' : 'warning'} />
      <section className="import-result" aria-busy={busy}><h3>{view === 'success' ? snapshot?.cleanupPending ? '已添加，临时文件还没清理完' : `已添加「${snapshot?.app?.name ?? '应用'}」` : view === 'unknown' ? '添加结果待核对' : view === 'failed' ? '这个项目暂时不能添加' : cancelling ? '正在停止并清理…' : phases[phaseIndex(task?.phase)]}</h3>
        {error && <p role="alert" className="task-error">{error}</p>}{flow?.warning && <p role="status">{flow.warning}</p>}
        {view === 'unknown' && <p>结果明确前保留必要材料，请不要重复添加。</p>}
        {snapshot?.remainingPaths.map(path => <p key={path}>待清理：<code>{path}</code></p>)}
        {view === 'failed' && <><ProcessFailureDetails value={diagnostics} /><p>{!snapshot ? '尚未开始下载项目文件。' : snapshot.cleanupPending ? '临时文件尚未清理完成。' : '本次下载及临时材料已清理。'}</p></>}
        {view === 'success' && <><p>{snapshot?.cleanupPending ? '重试清理不会再次添加项目。' : '临时文件已清理。'}</p><div className="git-import-source"><p>项目文件夹：<code>{snapshot?.sourcePath}</code></p><button className="text-button" disabled={!snapshot?.sourcePath} onClick={() => void invoke('open_project_folder', { path: snapshot?.sourcePath }).catch(cause => setError(messageFrom(cause)))}><FolderOpen size={14} />打开文件夹</button></div></>}
        <div className="confirmation-actions">
          {view === 'unknown' && <button className="primary-button" disabled={busy} onClick={() => void reconcile()}>重新核对</button>}
          {snapshot?.cleanupPending && <button className="refresh-button" disabled={busy} onClick={() => void reconcile(true)}>重试清理</button>}
          {view === 'failed' && <><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'git', url, branch: branch.trim() || undefined, issues: [error].filter(Boolean), processDiagnostics: diagnostics, platformVersion }} label="复制说明给 AI" className="primary-button" />{!snapshot && <button className="refresh-button" onClick={() => { session.current = null; setView('form'); setError(''); }}>返回填写</button>}{onOpenLocal && <button className="refresh-button" onClick={() => { onClose({ keepSession: !canLeaveFailedFlow }); onOpenLocal(); }}>改用本地添加</button>}</>}
          {view === 'success' && <button className="primary-button" disabled={busy} onClick={() => void enter()}>{snapshot?.app ? snapshot.app.configuration.canStart ? '打开应用' : '去设置' : '查看应用'}</button>}
          {busy ? <><button className="refresh-button" onClick={dismiss}>在后台继续</button><button className="text-button" disabled={cancelling || task?.phase === 'registering' || committed} onClick={() => void cancel()}>取消添加</button></> : <button className="refresh-button" onClick={dismiss}>{view === 'success' ? '完成' : '关闭'}</button>}
        </div>
        {task && <details className="import-task"><summary>检查详情</summary><ProjectTask task={task} /></details>}
      </section>
    </>}
  </Modal>;
}
