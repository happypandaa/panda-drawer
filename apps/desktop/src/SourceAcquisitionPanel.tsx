import { invoke } from '@tauri-apps/api/core';
import { Download, FolderOpen, LoaderCircle } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ImportProgress } from './ImportProgress';
import { ProjectTask } from './ProjectTask';
import { SourceLibraryPanel } from './SourceLibraryPanel';
import { AiProjectHelp } from './AiProjectHelp';
import { SourceImportError, SourceImportSession } from './source-import';
import type { InstalledNodeResult } from './NodeInstallPanel';
import type { NodeAppDetail } from './node-app-types';
import type { TaskLogs, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type ExistingRepository = { url: string; appId: string; name: string };
export type SourceAcquisitionPanelProps = {
  disabled: boolean; instanceId?: string; open?: boolean; projectDirectory?: string; platformVersion?: string;
  existingRepositories?: ExistingRepository[];
  onClose: () => void; onBackground?: () => void; onAttention?: () => void;
  onOpenLocal?: () => void; onOpenSettings?: () => void; onOpenExisting?: (appId: string) => void;
  onTask: (task: TaskRecord) => void; onInstalled?: () => void; onEnter?: (app: NodeAppDetail) => void;
};
type View = 'form' | 'duplicate' | 'running' | 'build' | 'failed' | 'recovery' | 'finishing' | 'publish-failed' | 'cleanup-failed' | 'success';
const phases = ['下载源码', '检查项目', '准备应用', '添加应用', '清理临时文件'];
const phaseDetails: Record<string, string> = {
  environment: '正在检查运行环境…', downloading: '正在从仓库下载源码…', 'validating-objects': '正在检查下载内容…',
  materializing: '正在整理源码文件…', inspecting: '正在检查项目，只读取文件，不会修改。', copying: '正在准备应用文件…',
  dependencies: '正在准备应用依赖，可能需要几分钟。', validating: '正在检查应用材料…', committing: '正在添加应用，完成前不能取消。',
  'build-dependencies': '正在准备构建所需的依赖…', building: '正在构建应用…',
};

export function gitRepositoryAddress(value: string): { url: string; provider: string; repository: string } | null {
  const match = /^(https:\/\/)(github\.com|gitee\.com)\/([A-Za-z0-9][A-Za-z0-9._-]{0,254})\/([A-Za-z0-9][A-Za-z0-9._-]{0,254})\/?$/.exec(value.trim());
  if (!match) return null;
  const repository = match[4].replace(/\.git$/, '');
  if (!repository) return null;
  return { url: `${match[1]}${match[2]}/${match[3]}/${repository}.git`, provider: match[2] === 'github.com' ? 'GitHub' : 'Gitee', repository };
}
function lastAddressKey(instanceId?: string) { return `panda-drawer:add-git:last-address:${instanceId ?? 'local'}`; }
function readLastAddress(instanceId?: string) { try { return window.localStorage.getItem(lastAddressKey(instanceId)) ?? ''; } catch { return ''; } }
function rememberAddress(instanceId: string | undefined, value: string) { try { window.localStorage.setItem(lastAddressKey(instanceId), value); } catch { /* Keeping an address is optional. */ } }
function ordinaryReason(value: string) {
  if (/local-app\.json|应用声明|应用格式|声明文件/.test(value)) return '缺少有效的应用说明，平台还不知道怎样打开这个项目。';
  if (/lock|锁文件/i.test(value)) return '缺少有效的依赖版本记录，暂时无法准备应用。';
  if (/timeout|timed.out|超时/i.test(value)) return '连接仓库或准备依赖超时，请检查网络后重试。';
  if (/disk|space|磁盘|空间不足/i.test(value)) return '磁盘空间不足，请先释放空间后再试。';
  if (/permission|denied|权限|不能写入/i.test(value)) return '无法访问所需位置，请检查目录权限或存储位置。';
  if (/\.node|原生扩展/.test(value)) return '项目依赖需要单独准备的本机组件，当前不能直接使用。';
  if (/Python|Java\b|Docker|多服务/i.test(value)) return '主要功能依赖平台当前不支持的运行环境，需要先评估替代办法。';
  if (/依赖|npm|DEPENDENCY/i.test(value)) return '准备应用所需的依赖时出错，可以把检查详情交给 AI 处理。';
  if (/构建|build/i.test(value)) return '没有生成可运行的应用，请让 AI 检查构建问题。';
  return value;
}

export function SourceAcquisitionPanel(props: SourceAcquisitionPanelProps) {
  return <GitImportWorkflow key={props.instanceId ?? ''} {...props} />;
}
function GitImportWorkflow({ disabled, instanceId, open = true, projectDirectory: suppliedDirectory, platformVersion, existingRepositories, onClose, onBackground, onAttention, onOpenLocal, onOpenSettings, onOpenExisting, onTask, onInstalled, onEnter }: SourceAcquisitionPanelProps) {
  const [url, setUrl] = useState(() => readLastAddress(suppliedDirectory ?? instanceId));
  const [branch, setBranch] = useState('');
  const [name, setName] = useState('');
  const [projectDirectory, setProjectDirectory] = useState(suppliedDirectory ?? '');
  const [locationError, setLocationError] = useState('');
  const [view, setView] = useState<View>('form');
  const [duplicate, setDuplicate] = useState<ExistingRepository | null>(null);
  const [busy, setBusy] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [cancelConfirm, setCancelConfirm] = useState(false);
  const [error, setError] = useState('');
  const [failurePhase, setFailurePhase] = useState<'download' | 'check' | 'prepare' | 'install'>('download');
  const [cleanupError, setCleanupError] = useState('');
  const [cleanupDone, setCleanupDone] = useState(false);
  const [sourceRetained, setSourceRetained] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [logs, setLogs] = useState('');
  const [app, setApp] = useState<NodeAppDetail | null>(null);
  const [showLibrary, setShowLibrary] = useState(false);
  const [buildInfo, setBuildInfo] = useState<{ scriptName: string; script: string } | null>(null);
  const [buildInfoError, setBuildInfoError] = useState('');
  const [, redraw] = useState(0);
  const alive = useRef(false);
  const lifetime = useRef(0);
  const operation = useRef(false);
  const session = useRef<SourceImportSession | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const callbacks = useRef({ onTask, onInstalled, onEnter, onClose, onBackground, onAttention });
  callbacks.current = { onTask, onInstalled, onEnter, onClose, onBackground, onAttention };
  useEffect(() => { alive.current = true; lifetime.current++; return () => { alive.current = false; lifetime.current++; }; }, []);
  useEffect(() => {
    if (suppliedDirectory) { setProjectDirectory(suppliedDirectory); setLocationError(''); return; }
    let valid = true;
    void invoke<{ projectDirectory: string }>('source_import_location').then(value => {
      if (valid && typeof value?.projectDirectory === 'string' && value.projectDirectory) { setProjectDirectory(value.projectDirectory); setLocationError(''); }
      else if (valid) setLocationError('项目目录暂时无法读取，请稍后重新打开。');
    }).catch(cause => { if (valid) setLocationError(messageFrom(cause)); });
    return () => { valid = false; };
  }, [suppliedDirectory]);
  useEffect(() => { if (open && view !== 'form' && view !== 'running') heading.current?.focus(); }, [open, view]);
  useEffect(() => { if (['build', 'failed', 'recovery', 'publish-failed', 'cleanup-failed', 'success'].includes(view)) callbacks.current.onAttention?.(); }, [view]);
  const current = (generation: number) => alive.current && lifetime.current === generation;
  const flow = session.current;
  const address = gitRepositoryAddress(url);
  const unsupported = Boolean(flow?.source && flow.source.inspection?.kind !== 'node-service-source');
  const active = busy || view === 'running' || view === 'finishing';
  const canBackground = Boolean(onBackground) && active && !cleaning && view !== 'finishing' && flow?.phase !== 'install' && !flow?.installed;
  const sourceStep = flow?.phase === 'cleanup' || flow?.phase === 'done' ? 4 : flow?.phase === 'install' ? 3 : flow?.phase === 'prepare' ? 2 : flow?.phase === 'check' ? 1 : 0;
  const step = cleaning || view === 'finishing' || view === 'cleanup-failed' ? 4 : sourceStep;
  const completedSteps = !flow?.installed && (cleaning || cleanupDone) ? [...Array.from({ length: sourceStep }, (_, index) => index), ...(cleanupDone ? [4] : [])] : undefined;
  const issues = [...new Set([...(flow?.source?.inspection?.issues.map(messageFrom) ?? []), ...(error ? [error] : [])])];
  const sourceLocation = flow?.published?.sourcePath;
  const plannedLocation = projectDirectory ? `${projectDirectory.replace(/[\\/]$/, '')}/${address?.repository ?? '仓库名'}` : '正在读取项目目录…';

  async function clean(value: SourceImportSession) {
    setCleanupError(''); setCleaning(true);
    try {
      const result = await value.cleanup();
      if (!alive.current || session.current !== value) return false;
      setCleanupDone(true); setSourceRetained(result.sourceRetained); return true;
    } catch (cause) { if (alive.current && session.current === value) setCleanupError(messageFrom(cause)); return false; }
    finally { if (alive.current && session.current === value) setCleaning(false); }
  }
  async function failure(cause: unknown, generation: number) {
    if (!current(generation)) return;
    const value = session.current;
    setError(messageFrom(cause)); setCancelConfirm(false);
    if (value?.installed) { setView('publish-failed'); return; }
    if (cause instanceof SourceImportError && cause.recovery) { setView('recovery'); return; }
    setFailurePhase(value?.phase === 'check' ? 'check' : value?.phase === 'prepare' ? 'prepare' : value?.phase === 'install' ? 'install' : 'download');
    setView('failed');
    // An unsuccessful attempt is not a project. Clean only its verified,
    // stopped task; unknown commits and unknown process ownership stay protected.
    if (value?.sourceTaskId) await clean(value);
    else { setCleanupDone(true); setSourceRetained(false); }
    if (!current(generation)) return;
    if (value?.task?.taskId) {
      try {
        const chunk = await invoke<TaskLogs>('task_logs', { taskId: value.task.taskId, cursor: 0 });
        if (current(generation) && session.current === value && typeof chunk?.text === 'string') setLogs(chunk.text.slice(-12000));
      } catch { /* Error and source identity still make a usable handoff. */ }
    }
  }
  async function readInstalled(value: SourceImportSession, result: InstalledNodeResult, notify = true) {
    if (!alive.current || session.current !== value) return;
    if (notify) callbacks.current.onInstalled?.();
    setView('finishing'); setError('');
    await value.publishSource();
    const cleaned = await clean(value);
    if (!alive.current || session.current !== value) return;
    try {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: result.appId });
      if (!alive.current || session.current !== value) return;
      if (detail.id !== result.appId) throw new Error('应用信息身份不符，请重新读取。');
      setApp(detail);
    } catch (cause) { if (alive.current && session.current === value) setError(`应用已添加，设置暂时无法读取：${messageFrom(cause)}`); }
    if (alive.current && session.current === value) setView(cleaned ? 'success' : 'cleanup-failed');
  }
  async function install(value: SourceImportSession, buildApproved: boolean) { await readInstalled(value, await value.install(buildApproved)); }
  async function start(allowDuplicate = false) {
    if (operation.current || disabled || !address) return;
    const found = existingRepositories?.find(item => gitRepositoryAddress(item.url)?.url === address.url);
    if (found && !allowDuplicate) { setDuplicate(found); setView('duplicate'); return; }
    operation.current = true; setBusy(true); setError(''); setCleanupError('');
    rememberAddress(suppliedDirectory ?? instanceId, url.trim());
    const generation = lifetime.current;
    try {
      if (session.current?.sourceTaskId && !cleanupDone && !await clean(session.current)) return;
      if (!current(generation)) return;
      const value: SourceImportSession = new SourceImportSession(invoke, {
        isCurrent: (): boolean => current(generation) && session.current === value,
        onChange: () => redraw(count => count + 1), onTask: task => callbacks.current.onTask(task),
      });
      session.current = value; setCleanupDone(false); setSourceRetained(false); setApp(null); setLogs(''); setBuildInfo(null); setBuildInfoError(''); setCancelling(false); setView('running');
      const source = await value.acquire({ url: address.url, branch: branch.trim() || null, ...(name.trim() ? { name: name.trim() } : {}) });
      if (source.inspection?.requiresBuild && !source.buildSourceDigest) {
        setView('build');
        try {
          const info = await invoke<{ scriptName: string; script: string }>('source_import_build_info', { sourceTaskId: value.sourceTaskId });
          if (current(generation) && typeof info?.scriptName === 'string' && typeof info?.script === 'string') setBuildInfo(info);
          else if (current(generation)) setBuildInfoError('脚本内容暂时无法读取。请核对来源后再继续。');
        } catch (cause) { if (current(generation)) setBuildInfoError(messageFrom(cause)); }
      } else await install(value, false);
    } catch (cause) { await failure(cause, generation); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function build() {
    if (operation.current || disabled || !session.current || !buildInfo) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true); setView('running'); setError('');
    try { await install(session.current, true); }
    catch (cause) { await failure(cause, generation); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function cancel() {
    if (!session.current || cancelling || session.current.phase === 'install') return;
    setCancelConfirm(false); setCancelling(true); setError('');
    try { await session.current.cancel(); }
    catch (cause) { if (alive.current) { setError(`取消请求暂未确认：${messageFrom(cause)}`); setCancelling(false); } }
  }
  async function cleanAndClose() {
    if (operation.current) return;
    if (!session.current?.sourceTaskId || cleanupDone || view === 'recovery') { callbacks.current.onClose(); return; }
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
      session.current = null; setError(''); setCleanupError(''); setCleanupDone(false); setDuplicate(null); setView('form');
    } finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function retryCleanup() {
    if (operation.current || !session.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true);
    try {
      if (session.current.installed) await readInstalled(session.current, session.current.installed, false);
      else await clean(session.current);
    } catch (cause) { await failure(cause, generation); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function reconcile() {
    if (operation.current || !session.current) return;
    const generation = lifetime.current;
    operation.current = true; setBusy(true); setError('');
    try { await readInstalled(session.current, await session.current.reconcile()); }
    catch (cause) { await failure(cause, generation); }
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
      setApp(detail); setError(''); callbacks.current.onClose(); callbacks.current.onEnter?.(detail);
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { if (current(generation)) { operation.current = false; setBusy(false); } }
  }
  async function openDuplicate() {
    if (!duplicate) return;
    if (onOpenExisting) { callbacks.current.onClose(); onOpenExisting(duplicate.appId); return; }
    setBusy(true);
    try {
      const detail = await invoke<NodeAppDetail>('node_app_configuration', { appId: duplicate.appId });
      if (alive.current && detail.id === duplicate.appId) { callbacks.current.onClose(); callbacks.current.onEnter?.(detail); }
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { if (alive.current) setBusy(false); }
  }
  async function revealSource() {
    if (!flow?.installed) return;
    try { await invoke('node_development_reveal', { appId: flow.installed.appId }); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
  }
  function close() {
    if (canBackground) { callbacks.current.onBackground?.(); return; }
    if (active) return;
    if (view === 'recovery' || view === 'publish-failed' || view === 'cleanup-failed') callbacks.current.onClose();
    else void cleanAndClose();
  }
  if (!open) return null;
  if (showLibrary) return <SourceLibraryPanel disabled={disabled} instanceId={instanceId} onTask={onTask} onInstalled={onInstalled} onClose={() => setShowLibrary(false)} />;
  return <Modal title="从 Git 添加项目" titleIcon={<Download size={20} />} description="粘贴仓库地址，检查能否使用，再添加到你的项目列表。" onClose={close} closeDisabled={active && !canBackground} className="import-dialog git-import-modal">
    {view === 'form' && <form className="create-project-form" onSubmit={event => { event.preventDefault(); void start(); }}>
      <label htmlFor="git-import-url">Git 仓库地址</label>
      <input id="git-import-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/作者/项目" autoCapitalize="none" autoCorrect="off" spellCheck={false} aria-describedby="git-address-hint" aria-invalid={Boolean(url.trim() && !address)} />
      <p id="git-address-hint" className={url.trim() && !address ? 'task-error' : 'git-import-hint'}>{address ? `${address.provider} 公开仓库 · ${branch.trim() ? `将使用分支 ${branch.trim()}` : '将使用默认分支'}` : url.trim() ? '这不像仓库地址。请复制以 https://github.com/ 或 https://gitee.com/ 开头的公开仓库地址。' : '支持 GitHub、Gitee 的公开 HTTPS 仓库。'}</p>
      <div className="add-project-location"><strong>添加成功后，源码放在</strong><code>{plannedLocation}</code><p>同名文件夹会另取名字，不会覆盖。需要换位置时，在设置的存储位置修改项目目录。</p>{onOpenSettings && <button type="button" className="text-button" onClick={onOpenSettings}>修改项目目录</button>}{locationError && <p role="status">{locationError}</p>}</div>
      <div className="add-project-storage-cards"><div><strong>源码</strong><p>保存在项目目录，你和 AI 都可以修改。卸载前清理会删除它。</p></div><div><strong>应用</strong><p>使用平台准备好的版本，修改源码后需要明确更新。</p></div><div><strong>应用数据</strong><p>记录和设置单独保存，重要数据请自行备份。</p></div></div>
      <details className="git-import-options"><summary>高级选项</summary><label htmlFor="git-import-branch">分支（选填）</label><input id="git-import-branch" value={branch} onChange={event => setBranch(event.target.value)} placeholder="留空使用默认分支" autoCapitalize="none" autoCorrect="off" spellCheck={false} /><label htmlFor="git-import-name">显示名称（选填）</label><input id="git-import-name" value={name} onChange={event => setName(event.target.value)} placeholder="留空使用仓库名" /></details>
      <p className="git-import-hint">下载源码 → 检查项目 → 准备应用 → 添加应用 → 清理临时文件。需要运行构建脚本时会先问你；失败时可复制说明交给 AI。</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || !address || !projectDirectory}>开始添加</button><button type="button" className="refresh-button" onClick={close}>取消</button></div>
    </form>}
    {view === 'duplicate' && duplicate && <section className="import-result"><h3 ref={heading} tabIndex={-1}>这个仓库已经添加过：「{duplicate.name}」</h3><p>另外添加的一份会从空数据开始，两份互不影响。更新原来的应用，请进入应用详情。</p><div className="confirmation-actions"><button className="primary-button" disabled={busy} onClick={() => void openDuplicate()}>打开已添加的</button><button className="refresh-button" disabled={busy || disabled} onClick={() => void start(true)}>仍然另外添加一份</button><button className="text-button" disabled={busy} onClick={() => setView('form')}>取消</button></div>{error && <p role="alert" className="task-error">{error}</p>}</section>}
    {!['form', 'duplicate'].includes(view) && <>
      <ImportProgress label="添加进度" steps={phases} currentStep={step} state={view === 'success' ? 'succeeded' : view === 'cleanup-failed' || view === 'publish-failed' ? 'warning' : view === 'failed' && !cleaning ? 'failed' : view === 'build' ? 'paused' : 'running'} completedSteps={completedSteps} />
      <section className="import-result git-import-result" aria-busy={active}>
        <h3 ref={heading} tabIndex={-1}>{cleaning ? '正在清理本次临时文件…' : view === 'finishing' ? '正在整理源码和完成添加…' : view === 'cleanup-failed' ? '应用已添加，临时文件还没清理完' : view === 'publish-failed' ? '应用已添加，源码位置还需处理' : view === 'success' ? `已添加「${app?.name ?? flow?.source?.inspection?.name ?? flow?.source?.name ?? '应用'}」` : view === 'build' ? '这个项目需要先构建一次' : view === 'recovery' ? '还不能确定应用是否已经添加' : view === 'failed' ? flow?.cancelled ? '添加已取消' : failurePhase === 'prepare' ? '准备应用时出错' : failurePhase === 'download' ? '下载没有完成' : '这个项目暂时不能添加' : cancelling ? '正在停止任务…' : phases[step]}</h3>
        {view === 'running' && !cleaning && <p role="status"><LoaderCircle size={16} className="import-spinner" />{cancelling ? '正在等待任务停止，停止后会清理本次临时文件。' : phaseDetails[flow?.task?.phase ?? ''] ?? '正在处理，请稍候…'}</p>}
        {view === 'running' && flow?.phase === 'download' && <p>全部检查通过并添加后，源码才会放进项目目录；现在的文件在临时位置。</p>}
        {view === 'build' && <><p>项目要求运行构建脚本{buildInfo ? `「${buildInfo.scriptName}」` : ''}，生成可以运行的网页和程序。在单独的临时位置运行，不改动源码，不读取或注入应用设置与密钥。</p><p>脚本来自仓库作者，会以你的身份在这台电脑上运行。只在你信任这个项目时继续。</p>{buildInfo && <details className="add-project-check-details"><summary>查看脚本内容</summary><pre>{buildInfo.script}</pre><p>只执行声明的构建脚本，依赖包自带的安装脚本不会运行。</p></details>}{buildInfoError && <p role="alert" className="task-error">{buildInfoError}</p>}<div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy || disabled || !buildInfo} onClick={() => void build()}>同意并继续</button><button type="button" className="refresh-button" disabled={busy} onClick={() => void cleanAndClose()}>不构建，关闭</button></div></>}
        {view === 'success' && <><p role="status">临时文件已清理。{app ? app.configuration.canStart ? '可以直接使用。' : `还差 ${app.configuration.missingRequired.length} 项设置就可以使用。` : '可以查看应用并完成必要设置。'}</p><div className="add-project-location"><strong>文件在哪里</strong><dl><dt>源码</dt><dd><code>{sourceLocation}</code><button type="button" className="text-button" disabled={busy} onClick={() => void revealSource()}><FolderOpen size={14} />在 Finder 中显示</button></dd><dt>应用</dt><dd>在“我的项目”里打开。</dd><dt>应用数据</dt><dd>单独保存在熊猫小抽屉里。</dd></dl></div><p>想改功能时，在应用详情的“开发与更新”中把源码交给 AI。</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void enter()}>{app ? app.configuration.canStart ? '打开应用' : '去设置' : '查看应用'}</button><button type="button" className="refresh-button" disabled={busy} onClick={close}>完成</button></div></>}
        {view === 'cleanup-failed' && <p>应用已经添加。可以稍后处理临时文件；重试清理不会再次添加应用。</p>}
        {view === 'publish-failed' && <><p>应用已经添加，源码还没整理到项目目录。请重试整理，原应用和数据会保留，不会重复添加。</p><button type="button" className="primary-button" disabled={busy} onClick={() => void retryCleanup()}>重试整理源码</button></>}
        {view === 'failed' && <><span className="add-project-result-label">{unsupported ? /Python|Java\b|Docker|多服务|原生扩展|\.node/i.test(issues.join(' ')) ? '当前不适合' : '需要修改' : '需要处理'}</span>{[...new Set(issues.map(ordinaryReason))].slice(0, 3).map(reason => <p key={reason}>{reason}</p>)}<div className="add-project-location"><strong>文件在哪里</strong><p>{cleaning ? '正在清理本次下载的临时文件。' : cleanupDone ? sourceRetained ? '本次临时文件已清理；仍被其他操作使用的文件已保留，请核对检查详情。' : flow?.sourceTaskId ? '下载的文件已经删除，项目目录里没有留下这个项目。' : '尚未下载项目文件。' : '临时文件尚未清理完成，请重试清理。'}</p></div>{failurePhase !== 'download' && <p>把说明交给能读取项目的 AI，先评估再修改。改好后，让 AI 告诉你本机文件夹位置，再用“添加本地项目”添加。</p>}<div className="confirmation-actions"><AiProjectHelp context={{ mode: 'add-failed', sourceKind: 'git', url: url.trim(), branch: branch.trim() || undefined, commit: flow?.source?.commit ?? undefined, issues, logs, platformVersion }} label="复制说明给 AI" className="primary-button" disabled={busy} />{failurePhase === 'download' && <button type="button" className="refresh-button" disabled={busy || disabled || !cleanupDone} onClick={() => void start()}>重试下载</button>}{failurePhase === 'download' && <button type="button" className="refresh-button" disabled={busy} onClick={() => void editAddress()}>修改地址</button>}{onOpenLocal && <button type="button" className="refresh-button" disabled={busy} onClick={() => { close(); onOpenLocal(); }}>改用本地添加</button>}<button type="button" className="text-button" disabled={busy} onClick={close}>{failurePhase === 'download' ? '稍后再试' : '关闭'}</button></div></>}
        {view !== 'failed' && error && <p role="alert" className="task-error">{error}</p>}
        {flow?.warning && <p role="status">{flow.warning}</p>}
        {cleaning && <p role="status"><LoaderCircle size={16} className="import-spinner" />正在清理本次临时文件…</p>}
        {cleanupError && <div role="alert"><p className="task-error">{cleanupError}</p><button type="button" className="refresh-button" disabled={busy} onClick={() => void retryCleanup()}>重试清理</button></div>}
        {(view === 'cleanup-failed' || view === 'publish-failed') && <button type="button" className="refresh-button" disabled={busy} onClick={() => callbacks.current.onClose()}>稍后</button>}
        {view === 'recovery' && <><p>正在核对，请不要重复添加。结果明确前会保留必要文件。</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void reconcile()}>重新核对</button><button type="button" className="text-button" disabled={busy} onClick={() => setShowLibrary(true)}>检查详情</button></div></>}
        {view === 'running' && !cleaning && flow?.phase !== 'install' && <div className="confirmation-actions">{onBackground && <button type="button" className="refresh-button" disabled={!canBackground} onClick={() => callbacks.current.onBackground?.()}>在后台继续</button>}<button type="button" className="text-button" disabled={cancelling} onClick={() => setCancelConfirm(true)}>{flow?.phase === 'download' ? '取消下载' : '取消'}</button></div>}
        {flow?.task && <details className="import-task"><summary>检查详情与日志</summary><ProjectTask key={flow.task.taskId} task={flow.task} /></details>}
      </section>
    </>}
    {['form', 'duplicate'].includes(view) && <div className="git-import-library"><AiProjectHelp context={{ mode: 'evaluation', sourceKind: 'git', url: url.trim() || undefined, branch: branch.trim() || undefined, platformVersion }} /><details><summary>高级检查与恢复</summary><button type="button" className="text-button" disabled={busy || disabled} onClick={() => setShowLibrary(true)}>查看源码记录与恢复</button></details></div>}
    {cancelConfirm && <Modal title={flow?.phase === 'download' ? '停止下载？' : '停止准备？'} description="任务停止后，已下载的部分和本次临时文件会被清理。" onClose={() => setCancelConfirm(false)}><div className="confirmation-actions"><button type="button" className="primary-button" onClick={() => setCancelConfirm(false)}>{flow?.phase === 'download' ? '继续下载' : '继续准备'}</button><button type="button" className="refresh-button" onClick={() => void cancel()}>{flow?.phase === 'download' ? '停止下载' : '停止准备'}</button></div></Modal>}
  </Modal>;
}
