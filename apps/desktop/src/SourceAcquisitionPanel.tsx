import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import { NodeInstallPanel } from './NodeInstallPanel';
import { SourceBuildPanel } from './SourceBuildPanel';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type Inspection = { kind: string; name?: string; issues: unknown[]; executionState: string; nextStep?: string };
type Candidate = { sourceId: string; taskId: string; name: string; status: string; url: string; provider?: string; localSourcePath?: string; captureNotice?: string; excludedPaths?: string[]; commit?: string; sourcePath?: string; evidencePath: string; inspection?: Inspection; error?: unknown; contentDigest?: string };
const statusText: Record<string, string> = { acquiring: '正在获取', ready: '已获取，待接入', failed: '获取失败', cancelled: '已取消', interrupted: '已中断', unavailable: '记录需要检查' };
const kindText: Record<string, string> = { 'project-v1-source': '平台网页项目源码', 'node-service-source': '带本地服务的应用源码', 'needs-adaptation': '尚待评估的源码' };

type Props = { disabled: boolean; instanceId?: string; onClose: () => void; onTask: (task: TaskRecord) => void; onInstalled?: () => void;
  chooseSourceCommand?: 'choose_path' | 'source_choose_directory'; managedInstallation?: boolean; candidateBuild?: boolean;
};

export function SourceAcquisitionPanel(props: Props) {
  const [url, setUrl] = useState('');
  const [name, setName] = useState('');
  return <SourceAcquisitionWorkflow key={props.instanceId ?? ''} {...props} url={url} setUrl={setUrl} name={name} setName={setName} />;
}

function SourceAcquisitionWorkflow({ disabled, instanceId, onClose, onTask, onInstalled, url, setUrl, name, setName,
  chooseSourceCommand = 'choose_path', managedInstallation = true, candidateBuild = false,
}: Props & { url: string; setUrl: (value: string) => void; name: string; setName: (value: string) => void }) {
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [inspection, setInspection] = useState<{ sourcePath: string; inspection: Inspection } | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState(false);
  const [showInstall, setShowInstall] = useState(false);
  const [installPending, setInstallPending] = useState(false);
  const [installTask, setInstallTask] = useState<TaskRecord | null>(null);
  const [buildPending, setBuildPending] = useState(false);
  const work = useRef({ install: false, build: false });
  const enabled = useRef(!disabled); enabled.current = !disabled;
  const pending = useRef(false);
  const alive = useRef(false);
  const generation = useRef(0);
  const readSequence = useRef(0);
  const currentTask = useRef<TaskRecord | null>(null);
  const callbacks = useRef({ onTask }); callbacks.current = { onTask };
  const isCurrent = useCallback((started: number) => alive.current && generation.current === started, []);
  const acceptTask = useCallback((value: TaskRecord, expectedId?: string) => {
    if (expectedId && value.taskId !== expectedId) return false;
    if (currentTask.current?.taskId === value.taskId && currentTask.current.final && !value.final) return false;
    currentTask.current = value;
    setTask(value); callbacks.current.onTask(value);
    return true;
  }, []);
  const read = useCallback(async (refreshCompleted = true): Promise<void> => {
    if (!alive.current || !enabled.current) return;
    const started = generation.current;
    const request = ++readSequence.current;
    const current = () => isCurrent(started) && request === readSequence.current;
    setReading(true);
    try {
      const list = await invoke<Candidate[]>('source_list');
      if (!current()) return;
      setCandidates(list);
      const active = list.find(item => item.status === 'acquiring');
      if (active) {
        const value = await invoke<TaskRecord>('task_get', { taskId: active.taskId });
        if (!current()) return;
        if (acceptTask(value, active.taskId) && value.final && refreshCompleted) await read(false);
      }
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally { if (current()) setReading(false); }
  }, [acceptTask, isCurrent]);
  useEffect(() => {
    generation.current++; alive.current = true; void read();
    return () => { alive.current = false; generation.current++; };
  }, [read]);
  useEffect(() => {
    if (!task || task.final) return;
    const started = generation.current;
    let disposed = false; let polling = false;
    const timer = window.setInterval(() => {
      if (polling || !enabled.current) return; polling = true;
      void invoke<TaskRecord>('task_get', { taskId: task.taskId }).then(value => {
        if (disposed || !isCurrent(started)) return;
        if (acceptTask(value, task.taskId) && value.final) void read();
      }).catch(cause => { if (!disposed && isCurrent(started)) setError(`任务状态暂时无法读取：${messageFrom(cause)}。请刷新，勿重复提交。`); }).finally(() => { polling = false; });
    }, 1000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [task, read, acceptTask, isCurrent]);
  const acquiring = candidates.some(item => item.status === 'acquiring') || Boolean(task && !task.final);
  async function acquire() {
    if (pending.current || work.current.install || work.current.build || disabled || acquiring || !url.trim()) return;
    const started = generation.current;
    readSequence.current++; setReading(false);
    pending.current = true; setBusy(true); setError(''); setNotice('');
    try {
      const value = await invoke<TaskRecord>('source_acquire', { input: { url: url.trim(), ...(name.trim() ? { name: name.trim() } : {}) } });
      if (!isCurrent(started)) return;
      acceptTask(value);
      setNotice('获取任务已接受，尚未安装或运行。可以关闭此页，稍后回来查看结果。');
      await read();
    } catch (cause) { if (isCurrent(started)) setError(messageFrom(cause)); }
    finally { if (isCurrent(started)) { pending.current = false; setBusy(false); } }
  }
  async function cancel() {
    if (disabled || pending.current || work.current.install || work.current.build || !task || task.final) return;
    const started = generation.current;
    pending.current = true; setBusy(true); setError('');
    try {
      const value = await invoke<TaskRecord>('source_cancel', { taskId: task.taskId });
      if (!isCurrent(started) || !acceptTask(value, task.taskId)) return;
      setNotice(value.final ? '任务已经结束，请核对最终结果。' : '已请求取消，正在等待任务停止；部分下载现场会保留。');
      if (value.final) await read();
    } catch (cause) { if (isCurrent(started)) setError(messageFrom(cause)); }
    finally { if (isCurrent(started)) { pending.current = false; setBusy(false); } }
  }
  async function inspect() {
    if (pending.current || work.current.install || work.current.build || disabled) return;
    const started = generation.current;
    pending.current = true; setBusy(true); setError('');
    try {
      const sourcePath = await invoke<string | null>(chooseSourceCommand, chooseSourceCommand === 'choose_path' ? { kind: 'source' } : undefined);
      if (!sourcePath || !isCurrent(started) || !enabled.current) return;
      const value = await invoke<{ sourcePath: string; inspection: Inspection }>('source_inspect', { sourcePath });
      if (isCurrent(started)) setInspection(value);
    } catch (cause) { if (isCurrent(started)) setError(messageFrom(cause)); }
    finally { if (isCurrent(started)) { pending.current = false; setBusy(false); } }
  }
  async function capture() {
    if (pending.current || work.current.install || work.current.build || disabled || acquiring || !inspection) return;
    const started = generation.current;
    readSequence.current++; setReading(false);
    pending.current = true; setBusy(true); setError(''); setNotice('');
    try {
      const value = await invoke<TaskRecord>('source_capture', { input: { sourcePath: inspection.sourcePath, ...(name.trim() ? { name: name.trim() } : {}) } });
      if (!isCurrent(started)) return;
      acceptTask(value);
      setNotice('已接受接入任务，将创建独立源码副本；原目录不改动。');
      await read();
    } catch (cause) { if (isCurrent(started)) setError(messageFrom(cause)); }
    finally { if (isCurrent(started)) { pending.current = false; setBusy(false); } }
  }
  return <Modal title="获取项目源码" description="从公开仓库获取，或先检查本机源码。" wide closeDisabled={busy || installPending || buildPending} beforeClose={() => !pending.current && !work.current.install && !work.current.build} onClose={onClose}>
    {error && <p role="alert" className="task-error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    <form className="create-project-form" onSubmit={event => { event.preventDefault(); void acquire(); }}>
      <label htmlFor="source-url">公开仓库链接</label><input id="source-url" value={url} onChange={event => setUrl(event.target.value)} placeholder="https://github.com/作者/项目.git" readOnly={disabled || busy || buildPending || installPending} />
      <label htmlFor="source-name">备注名称（选填）</label><input id="source-name" value={name} onChange={event => setName(event.target.value)} readOnly={disabled || busy || buildPending || installPending} />
      <p>支持 GitHub、Gitee 的公开 HTTPS 仓库。仅下载当前版本并检查，不运行项目代码。私有仓库及 SSH 尚未接入。</p>
      <div className="confirmation-actions"><button type="submit" className="primary-button" disabled={disabled || busy || acquiring || buildPending || installPending || !url.trim()}>{busy ? '正在提交…' : '获取源码'}</button><button type="button" className="refresh-button" disabled={disabled || busy || buildPending || installPending} onClick={() => void inspect()}>检查本机源码目录</button></div>
    </form>
    {inspection && <section><h3>本机检查结果</h3><code className="task-result-path">{inspection.sourcePath}</code><InspectionView value={inspection.inspection} candidateBuild={candidateBuild} /><p>接入会创建独立源码副本，按路径排除数据、已知凭据、依赖和本机状态。不能识别任意源码里的秘密；请先核对选择的目录。不会执行代码或改动原目录。</p><button className="refresh-button" disabled={disabled || busy || acquiring || buildPending || installPending} onClick={() => void capture()}>接入此目录的源码副本</button></section>}
    {task && <><ProjectTask task={task} disabled={disabled} />{!task.final && <button className="refresh-button" disabled={disabled || busy || buildPending || installPending} onClick={() => void cancel()}>取消获取</button>}</>}
    <section><div className="storage-heading"><h3>已获取与保留的现场</h3><button className="refresh-button" disabled={disabled || reading || busy || buildPending || installPending} onClick={() => void read()}>刷新结果</button></div>
      <p>{managedInstallation ? '获取完成不等于可以运行；需先核对声明并完成适配或安装。' : '获取完成不等于可以运行；网页项目可在明确同意执行脚本后准备独立候选，安装与发布尚未开放。'}失败、中断的目录保留，重试使用新目录。</p>
      {!candidates.length && <p>暂无获取记录。</p>}
      {candidates.map(item => <article className="operation-confirmation" key={item.sourceId}><h4>{item.name} · {statusText[item.status] ?? item.status}</h4><code className="task-result-path">{item.provider === 'local' ? `本机来源：${item.localSourcePath ?? '路径未记录'}` : item.url}</code>{item.commit && <p>源码版本：<code>{item.commit}</code></p>}<code className="task-result-path">{item.sourcePath ?? item.evidencePath}</code>{item.captureNotice && <p>{item.captureNotice}</p>}{Boolean(item.excludedPaths?.length) && <details><summary>接入时排除的路径（{item.excludedPaths?.length}）</summary><ul>{item.excludedPaths?.map(path => <li key={path}><code>{path}</code></li>)}</ul></details>}{item.inspection && <InspectionView value={item.inspection} candidateBuild={candidateBuild} />}{Boolean(item.error) && <p role="alert">{messageFrom(item.error)}</p>}</article>)}
    </section>
    {candidateBuild && <SourceBuildPanel sources={candidates} instanceId={instanceId} disabled={disabled || busy || acquiring || installPending} beforeMutation={() => !pending.current && !work.current.install} onPendingChange={value => { work.current.build = value; setBuildPending(value); }} onTask={onTask} />}
    {managedInstallation && <section><h3>安装带本地服务的应用</h3><p>支持已核验的 Node 应用源码；静态项目和需适配的目录请先完成适配。</p><button type="button" className="refresh-button" disabled={disabled || busy || installPending || buildPending} onClick={() => setShowInstall(true)}>查看安装候选与准备安装</button>{showInstall && <NodeInstallPanel sources={candidates} instanceId={instanceId} disabled={disabled || busy || buildPending} task={installTask} onPendingChange={value => { work.current.install = value; setInstallPending(value); }} onTask={value => { setInstallTask(value); onTask(value); }} onInstalled={() => onInstalled?.()} />}</section>}
  </Modal>;
}
function InspectionView({ value, candidateBuild = false }: { value: Inspection; candidateBuild?: boolean }) {
  const nodeStep = candidateBuild ? '当前先保留这份 Node 源码。Windows 安装与更新尚未开放，请先核对运行方式与接入要求。' : '请先核对 Node 运行方式、应用配置与安装候选，再明确安装。';
  const adaptationStep = '请先让 AI 只读检查公开材料并完成适配，再重新获取源码；不会自动执行或安装。';
  const projectStep = candidateBuild ? '请先核对项目声明与来源；准备独立网页候选会执行依赖与构建脚本，需要单独确认。' : '请先核对项目声明与来源，完成必要适配后再接入。';
  const steps: Record<string, string> = { 'prepare-node-runtime-before-registration': nodeStep, 'review-and-adapt-source': adaptationStep, 'review-project-source-before-integration': projectStep };
  const fallback = candidateBuild ? value.kind === 'node-service-source' ? nodeStep : value.kind === 'needs-adaptation' ? adaptationStep : value.kind === 'project-v1-source' ? projectStep : null : null;
  const nextStep = value.nextStep ? steps[value.nextStep] ?? (/^[a-z0-9]+(?:-[a-z0-9]+)+$/i.test(value.nextStep) ? fallback : value.nextStep) : fallback;
  return <><p>{kindText[value.kind] ?? value.kind} · 未执行代码</p>{value.kind === 'needs-adaptation' && <p>尚未满足当前接入格式，平台未判断能否适配。需要进一步检查公开项目材料后再决定。</p>}{nextStep && <p>{nextStep}</p>}{value.issues.map((issue, index) => <p key={index}>{messageFrom(issue)}</p>)}</>;
}
