import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ProjectTask } from './ProjectTask';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface SourceInstallCandidate {
  sourceId: string;
  name: string;
  status: string;
  contentDigest?: string;
  inspection?: { kind: string; name?: string; issues?: unknown[] };
}
export interface PreparedNodeInstall {
  candidateId: string;
  appId: string;
  generation: string;
  manifest: { runtime: string; app: { id: string; name: string; version: string }; [key: string]: unknown };
  programDigest: string;
  provenance: {
    sourceId: string; sourceDigest: string; sourceCommit: string | null; buildSourceDigest?: string | null;
    node: { path: string; version: string }; npm: { path: string; version: string }; preparedAt: number;
  };
}
export interface InstalledNodeResult {
  candidateId: string;
  appId: string;
  generation: string;
  operationOutcome: 'committed';
}
interface InstallRecord {
  candidateId: string; appId: string | null; targetAppId: string | null; state: string;
  prepareTaskId: string | null; applyTaskId: string | null;
  prepared: PreparedNodeInstall | null; error: unknown; issues: unknown[];
}
export interface NodeInstallPanelProps {
  sources: SourceInstallCandidate[];
  targetAppId?: string;
  disabled: boolean;
  instanceId?: string;
  task?: TaskRecord | null;
  onTask: (task: TaskRecord) => void;
  onInstalled?: (result: InstalledNodeResult) => void;
  onPendingChange?: (pending: boolean) => void;
}

const digestPattern = /^[a-f0-9]{64}$/;
const sourcePattern = /^src_[a-f0-9]{32}$/;
const candidatePattern = /^install_[a-f0-9]{32}$/;
const generationPattern = /^gen_[a-f0-9]{32}$/;
const appPattern = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
function eligible(source: SourceInstallCandidate) {
  return source.status === 'ready' && source.inspection?.kind === 'node-service-source'
    && sourcePattern.test(source.sourceId) && typeof source.contentDigest === 'string' && digestPattern.test(source.contentDigest);
}
function record(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }
function cancelledPreparationRecord(item: InstallRecord) {
  return item.state === 'failed' && Boolean(item.prepareTaskId) && item.applyTaskId === null
    && record(item.error) && item.error.code === 'NODE_INSTALL_CANCELLED';
}
function preparedResult(value: unknown): PreparedNodeInstall | null {
  if (!record(value) || typeof value.candidateId !== 'string' || !candidatePattern.test(value.candidateId)
    || typeof value.appId !== 'string' || !appPattern.test(value.appId)
    || typeof value.generation !== 'string' || !generationPattern.test(value.generation)
    || typeof value.programDigest !== 'string' || !digestPattern.test(value.programDigest)
    || !record(value.manifest) || value.manifest.runtime !== 'node-service' || !record(value.manifest.app)
    || !['id', 'name', 'version'].every(key => typeof value.manifest === 'object' && record(value.manifest) && record(value.manifest.app) && typeof value.manifest.app[key] === 'string' && value.manifest.app[key])) return null;
  const source = value.provenance;
  if (!record(source) || typeof source.sourceId !== 'string' || !sourcePattern.test(source.sourceId)
    || typeof source.sourceDigest !== 'string' || !digestPattern.test(source.sourceDigest)
    || !(source.sourceCommit === null || typeof source.sourceCommit === 'string' && /^[a-f0-9]{40}(?:[a-f0-9]{24})?$/.test(source.sourceCommit))
    || !(source.buildSourceDigest == null || typeof source.buildSourceDigest === 'string' && digestPattern.test(source.buildSourceDigest))
    || typeof source.preparedAt !== 'number' || !Number.isFinite(source.preparedAt)
    || ![source.node, source.npm].every(tool => record(tool) && typeof tool.path === 'string' && typeof tool.version === 'string')) return null;
  return value as unknown as PreparedNodeInstall;
}
function installedResult(value: unknown): InstalledNodeResult | null {
  if (!record(value) || value.operationOutcome !== 'committed'
    || typeof value.candidateId !== 'string' || !candidatePattern.test(value.candidateId)
    || typeof value.appId !== 'string' || !appPattern.test(value.appId)
    || typeof value.generation !== 'string' || !generationPattern.test(value.generation)) return null;
  return value as unknown as InstalledNodeResult;
}
function installTask(value: TaskRecord | null | undefined): value is TaskRecord {
  return Boolean(value && value.projectId === null && ['nodeInstallPrepare', 'nodeInstallApply'].includes(value.type));
}

export function NodeInstallPanel(props: NodeInstallPanelProps) {
  return <NodeInstallWorkflow key={JSON.stringify([props.targetAppId ?? null, props.instanceId ?? ''])} {...props} />;
}

function NodeInstallWorkflow({ sources, targetAppId, disabled, task: externalTask, onTask, onInstalled, onPendingChange }: NodeInstallPanelProps) {
  const id = useId();
  const [sourceId, setSourceId] = useState('');
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [candidate, setCandidate] = useState<PreparedNodeInstall | null>(null);
  const [knownCandidateId, setKnownCandidateId] = useState('');
  const [checked, setChecked] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [windowClosed, setWindowClosed] = useState(false);
  const [installed, setInstalled] = useState<InstalledNodeResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [notice, setNotice] = useState('');
  const [records, setRecords] = useState<InstallRecord[]>([]);
  const [listIssues, setListIssues] = useState<unknown[]>([]);
  const [listError, setListError] = useState('');
  const [listing, setListing] = useState(false);
  const alive = useRef(false);
  const lifetime = useRef(0);
  const choiceRevision = useRef(0);
  const lookupSequence = useRef(0);
  const listSequence = useRef(0);
  const pending = useRef(false);
  const polling = useRef(false);
  const currentTask = useRef<TaskRecord | null>(null);
  const currentCandidate = useRef<PreparedNodeInstall | null>(null);
  const sourceRef = useRef(sourceId); sourceRef.current = sourceId;
  const sourcesRef = useRef(sources); sourcesRef.current = sources;
  const callbacks = useRef({ onTask, onInstalled, onPendingChange }); callbacks.current = { onTask, onInstalled, onPendingChange };
  const seenTasks = useRef(new Set<string>());
  const finishedTasks = useRef(new Set<string>());
  const notified = useRef(new Set<string>());
  const binding = useRef<{ sourceId: string; digest: string } | null>(null);
  const availableSources = sources.filter(eligible);
  const selected = availableSources.find(source => source.sourceId === sourceId);
  const active = Boolean(task && !task.final);
  const visibleRecords = records.filter(item => item.state === 'unavailable' || item.targetAppId === (targetAppId ?? null));
  const listActive = visibleRecords.some(item => ['preparing', 'committing'].includes(item.state));
  const targetValid = targetAppId === undefined || appPattern.test(targetAppId);
  const current = useCallback((generation: number) => alive.current && lifetime.current === generation, []);
  const readList = useCallback(async () => {
    if (!alive.current) return;
    const generation = lifetime.current; const request = ++listSequence.current;
    setListing(true);
    try {
      const value = await invoke<{ candidates: InstallRecord[]; issues: unknown[] }>('node_install_list');
      if (!current(generation) || request !== listSequence.current) return;
      setRecords(value.candidates); setListIssues(value.issues); setListError('');
    } catch (cause) { if (current(generation) && request === listSequence.current) setListError(messageFrom(cause)); }
    finally { if (current(generation) && request === listSequence.current) setListing(false); }
  }, [current]);

  const publishInstalled = useCallback((value: InstalledNodeResult, expected?: PreparedNodeInstall | null) => {
    if (targetAppId && value.appId !== targetAppId || expected && (value.candidateId !== expected.candidateId || value.appId !== expected.appId || value.generation !== expected.generation)) throw new Error('安装结果身份不符，请保留现场并重新核验。');
    setInstalled(value); setChecked(false); setConfirmed(false);
    const key = `${value.candidateId}:${value.generation}`;
    if (!notified.current.has(key)) { notified.current.add(key); callbacks.current.onInstalled?.(value); }
  }, [targetAppId]);

  const matchPrepared = useCallback((value: PreparedNodeInstall) => {
    const source = sourcesRef.current.find(item => eligible(item) && item.sourceId === value.provenance.sourceId && item.contentDigest === value.provenance.sourceDigest);
    if (!source || sourceRef.current && sourceRef.current !== source.sourceId
      || targetAppId && value.appId !== targetAppId
      || binding.current && (binding.current.sourceId !== source.sourceId || binding.current.digest !== value.provenance.sourceDigest)) throw new Error('安装候选与当前源码或更新目标不一致，请重新核对候选。');
    if (!sourceRef.current) { sourceRef.current = source.sourceId; setSourceId(source.sourceId); }
    binding.current = { sourceId: source.sourceId, digest: value.provenance.sourceDigest };
  }, [targetAppId]);

  const lookup = useCallback(async (candidateId: string, expected?: PreparedNodeInstall | null) => {
    if (!alive.current || !candidatePattern.test(candidateId)) return;
    const generation = lifetime.current; const choice = choiceRevision.current; const request = ++lookupSequence.current;
    setChecking(true); setChecked(false); setConfirmed(false);
    try {
      const value = await invoke<unknown>('node_install_candidate', { candidateId });
      if (!current(generation) || choice !== choiceRevision.current || request !== lookupSequence.current) return;
      if (!record(value)) throw new Error('安装候选记录无效，请保留现场。');
      if (value.state === 'committed') {
        const result = installedResult(value.result);
        if (!result || result.candidateId !== candidateId) throw new Error('安装结果记录无效，请保留现场。');
        publishInstalled(result, expected);
        setNotice('已确认此候选已安装。应用尚未由本面板启动。');
      } else if (value.state === 'prepared') {
        const result = preparedResult(value.candidate);
        if (!result || result.candidateId !== candidateId || expected && (result.programDigest !== expected.programDigest || result.appId !== expected.appId || result.generation !== expected.generation || JSON.stringify(result.manifest) !== JSON.stringify(expected.manifest))) throw new Error('安装候选记录已变化，请重新准备。');
        matchPrepared(result); currentCandidate.current = result; setCandidate(result); setKnownCandidateId(result.candidateId); setChecked(true);
        setNotice('安装材料已准备并核验，尚未安装。请核对后明确确认。');
      } else throw new Error('安装候选状态尚不能确认，请保留现场并查看任务结果。');
    } catch (cause) { if (current(generation) && choice === choiceRevision.current && request === lookupSequence.current) setError(messageFrom(cause)); }
    finally { if (current(generation) && request === lookupSequence.current) setChecking(false); }
  }, [current, publishInstalled, matchPrepared]);

  const acceptTask = useCallback(async (value: TaskRecord, notify = true) => {
    if (!installTask(value)) throw new Error('返回的任务不是受管应用安装任务，请核对状态。');
    const previous = currentTask.current;
    if (previous?.taskId === value.taskId && previous.final && !value.final
      || previous?.taskId !== value.taskId && seenTasks.current.has(value.taskId)) return;
    currentTask.current = value; seenTasks.current.add(value.taskId); setTask(value);
    if (notify) callbacks.current.onTask(value);
    if (!value.final || finishedTasks.current.has(value.taskId)) return;
    finishedTasks.current.add(value.taskId);
    const recordId = value.result?.candidateId;
    if (typeof recordId === 'string' && candidatePattern.test(recordId)) setKnownCandidateId(recordId);
    if (value.type === 'nodeInstallPrepare' && value.status === 'succeeded') {
      const prepared = value.result?.state === 'prepared' && value.result.installed === false ? preparedResult(value.result.candidate) : null;
      if (!prepared) throw new Error('准备任务未返回完整安装候选；不能将任务成功视为已安装。');
      matchPrepared(prepared); currentCandidate.current = prepared; setCandidate(prepared); setKnownCandidateId(prepared.candidateId);
      await lookup(prepared.candidateId, prepared);
    } else if (value.type === 'nodeInstallApply') {
      const outcome = value.result?.operationOutcome ?? value.error?.details?.operationOutcome;
      if (outcome === 'committed') {
        const result = installedResult(value.result);
        if (!result) throw new Error('安装提交记录尚不完整，请核验候选结果，勿重复安装。');
        publishInstalled(result, currentCandidate.current);
        setNotice(value.status === 'succeeded' ? '安装已确认。请回到应用列表明确打开应用。' : '安装已提交，但收尾需要检查。请保留现场，勿重复安装。');
      } else {
        setChecked(false); setConfirmed(false);
        setNotice(outcome === 'unknown' ? '安装结果尚不能确认。请保留现场并核验结果，勿重复安装。' : '本次安装未确认提交，请核验候选状态后再决定下一步。');
      }
    }
    await readList();
  }, [matchPrepared, lookup, publishInstalled, readList]);

  const poll = useCallback(async () => {
    const selectedTask = currentTask.current;
    if (!selectedTask || selectedTask.final || polling.current) return;
    polling.current = true; const generation = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('task_get', { taskId: selectedTask.taskId });
      if (!current(generation) || currentTask.current?.taskId !== selectedTask.taskId) return;
      if (value.taskId !== selectedTask.taskId || value.type !== selectedTask.type) throw new Error('任务身份不符，请重新核对。');
      setPollError(''); await acceptTask(value);
    } catch (cause) { if (current(generation)) setPollError(`任务状态暂时无法确认：${messageFrom(cause)}。请刷新状态，勿重复提交。`); }
    finally { polling.current = false; }
  }, [current, acceptTask]);

  useEffect(() => { alive.current = true; lifetime.current += 1; void readList(); return () => { alive.current = false; lifetime.current += 1; }; }, [readList]);
  useEffect(() => {
    if (!installTask(externalTask) || externalTask === currentTask.current) return;
    const previous = currentTask.current;
    if (previous?.taskId !== externalTask.taskId && previous && !previous.final || previous?.taskId === externalTask.taskId && previous.final) return;
    const generation = lifetime.current;
    void acceptTask(externalTask, false).catch(cause => { if (current(generation)) setError(messageFrom(cause)); });
  }, [externalTask, acceptTask, current, task?.taskId, task?.final]);
  useEffect(() => {
    if (!task || task.final) return;
    void poll();
    const timer = window.setInterval(() => { void poll(); }, 1000);
    return () => window.clearInterval(timer);
  }, [task?.taskId, task?.final, poll]);
  useEffect(() => { callbacks.current.onPendingChange?.(busy); }, [busy]);
  useEffect(() => () => { callbacks.current.onPendingChange?.(false); }, []);

  function changeSource(value: string) {
    if (pending.current || active) return;
    choiceRevision.current += 1; lookupSequence.current += 1;
    sourceRef.current = value; setSourceId(value); binding.current = null; currentCandidate.current = null;
    setCandidate(null); setKnownCandidateId(''); setChecked(false); setChecking(false); setConfirmed(false); setWindowClosed(false); setInstalled(null); setError(''); setNotice('');
  }
  async function prepare() {
    if (pending.current || disabled || active || checking || listActive || !selected || !targetValid) return;
    pending.current = true; setBusy(true); setError(''); setPollError(''); setNotice(''); setChecked(false); setConfirmed(false); setInstalled(null);
    const generation = lifetime.current; const choice = choiceRevision.current;
    binding.current = { sourceId: selected.sourceId, digest: selected.contentDigest! };
    try {
      const value = await invoke<TaskRecord>('node_install_prepare', { input: { sourceId: selected.sourceId, expectedSourceDigest: selected.contentDigest, targetAppId: targetAppId ?? null } });
      if (!current(generation) || choice !== choiceRevision.current) return;
      if (value.type !== 'nodeInstallPrepare') throw new Error('返回的任务不是安装准备任务。');
      currentCandidate.current = null; setCandidate(null); setKnownCandidateId('');
      setNotice('准备任务已接受；依赖准备完成后仍需单独确认安装。');
      await acceptTask(value);
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { pending.current = false; if (current(generation)) setBusy(false); }
  }
  const sameSource = Boolean(selected && candidate && selected.sourceId === candidate.provenance.sourceId && selected.contentDigest === candidate.provenance.sourceDigest);
  const canApply = Boolean(candidate && checked && sameSource && confirmed && (!targetAppId || windowClosed) && !installed && !disabled && !busy && !checking && !active && targetValid);
  async function apply() {
    if (pending.current || !canApply || !candidate) return;
    pending.current = true; setBusy(true); setError(''); setNotice(''); setChecked(false); setConfirmed(false);
    const generation = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('node_install_apply', { candidateId: candidate.candidateId });
      if (!current(generation)) return;
      if (value.type !== 'nodeInstallApply') throw new Error('返回的任务不是安装提交任务，请核验结果。');
      await acceptTask(value);
    } catch (cause) {
      if (current(generation)) { setError(messageFrom(cause)); setNotice('请先核验候选与实际安装结果，再决定是否重试。'); }
    } finally { pending.current = false; if (current(generation)) setBusy(false); }
  }
  async function cancel() {
    const selectedTask = currentTask.current;
    if (pending.current || !selectedTask || selectedTask.final || selectedTask.type !== 'nodeInstallPrepare') return;
    pending.current = true; setBusy(true); setError(''); const generation = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('node_install_cancel', { taskId: selectedTask.taskId });
      if (!current(generation) || currentTask.current?.taskId !== selectedTask.taskId) return;
      if (value.taskId !== selectedTask.taskId || value.type !== selectedTask.type) throw new Error('取消结果任务身份不符，请核对状态。');
      await acceptTask(value);
      setNotice(currentTask.current?.final ? '准备任务已结束，请核对最终结果。' : '已请求取消准备，正在等待任务停止。准备现场会保留。');
    } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
    finally { pending.current = false; if (current(generation)) setBusy(false); }
  }
  async function resume(item: InstallRecord) {
    if (pending.current || active || disabled || item.targetAppId !== (targetAppId ?? null) || !candidatePattern.test(item.candidateId)) return;
    const generation = lifetime.current;
    if (item.state === 'prepared' || item.state === 'committed') {
      const prepared = preparedResult(item.prepared);
      if (!prepared || prepared.candidateId !== item.candidateId) { setError('记录缺少完整安装材料，请保留现场并检查任务结果。'); return; }
      changeSource(prepared.provenance.sourceId);
      setKnownCandidateId(item.candidateId);
      await lookup(item.candidateId, prepared);
    } else if (item.state === 'preparing' || item.state === 'committing') {
      const taskId = item.state === 'preparing' ? item.prepareTaskId : item.applyTaskId;
      if (!taskId) { setError('记录缺少任务编号，请保留现场。'); return; }
      // A retained task may belong to another source than the previously
      // selected candidate. Bind its result afresh instead of carrying the
      // previous candidate's identity/acknowledgements into this recovery.
      changeSource('');
      setKnownCandidateId(item.candidateId);
      pending.current = true; setBusy(true); setError('');
      try {
        const value = await invoke<TaskRecord>('task_get', { taskId });
        if (!current(generation)) return;
        if (value.taskId !== taskId) throw new Error('任务身份不符，请核对记录。');
        await acceptTask(value);
      } catch (cause) { if (current(generation)) setError(messageFrom(cause)); }
      finally { pending.current = false; if (current(generation)) setBusy(false); }
    }
  }

  return <section className="node-install-panel" aria-labelledby={`${id}-title`}>
    <h3 id={`${id}-title`}>{targetAppId ? '准备应用更新' : '安装本地服务应用'}</h3>
    <p>准备程序依赖，核对后再安装；不会自动启动应用。</p>
    {targetAppId && <p>更新保留数据和配置。请先关闭应用，并确认新版兼容现有数据。</p>}
    <details className="node-install-method"><summary>准备方式</summary><p>在独立目录使用选定的 Node/npm 下载生产依赖，跳过项目与依赖的安装脚本。需要额外编译的源码，请先在开发工具中构建。更新不会自动转换业务数据。</p></details>
    {!targetValid && <p role="alert">更新目标标识无效，请从受管应用详情重新进入。</p>}
    {error && <p className="task-error" role="alert">{error}</p>}
    {pollError && <p className="task-error" role="alert">{pollError}</p>}
    {notice && <p role="status">{notice}</p>}
    <form className="create-project-form" onSubmit={event => { event.preventDefault(); void prepare(); }}>
      <label htmlFor={`${id}-source`}>源码候选</label>
      <select id={`${id}-source`} value={sourceId} disabled={busy || active || disabled} onChange={event => changeSource(event.target.value)}>
        <option value="">请选择已检查的本地服务源码</option>
        {availableSources.map(source => <option key={source.sourceId} value={source.sourceId}>{source.inspection?.name ?? source.name}</option>)}
      </select>
      {!availableSources.length && <p>暂无可准备的源码候选。请先获取或检查带本地服务声明的源码，并确认获取状态为已完成。</p>}
      {sourceId && !selected && <p role="alert">所选源码当前不可用或已改变，请重新选择并准备。</p>}
      <button type="submit" className="primary-button" disabled={disabled || busy || active || checking || listActive || !selected || !targetValid}>{candidate ? '重新准备安装材料' : '准备安装材料'}</button>
    </form>
    {candidate && <section className="operation-confirmation" aria-label="安装候选确认">
      <h4>{candidate.manifest.app.name} · {candidate.manifest.app.version}</h4>
      <details className="node-install-verification"><summary>核验记录</summary>
        <p>应用标识：<code>{candidate.manifest.app.id}</code></p>
        {targetAppId && <p>更新目标：<code>{targetAppId}</code></p>}
        <p>来源候选：<code>{candidate.provenance.sourceId}</code></p>
        {candidate.provenance.sourceCommit && <p>源码基线提交：<code>{candidate.provenance.sourceCommit}</code></p>}
        {candidate.provenance.buildSourceDigest && <p>构建源码摘要：<code>{candidate.provenance.buildSourceDigest}</code></p>}
        <p>准备环境：Node {candidate.provenance.node.version} / npm {candidate.provenance.npm.version}</p>
        <p>候选：<code>{candidate.candidateId}</code></p><p>程序代次：<code>{candidate.generation}</code></p>
        <p>来源摘要：<code>{candidate.provenance.sourceDigest}</code></p><p>程序摘要：<code>{candidate.programDigest}</code></p>
      </details>
      {!sameSource && <p role="alert">候选与当前源码记录不一致，不能直接安装，请重新准备。</p>}
      {!installed && <>
        {targetAppId && <label><input type="checkbox" checked={windowClosed} disabled={busy || active} onChange={event => setWindowClosed(event.target.checked)} />我已正常关闭目标应用窗口</label>}
        <label><input type="checkbox" checked={confirmed} disabled={!checked || busy || active || !sameSource} onChange={event => setConfirmed(event.target.checked)} />已核对应用、来源与{targetAppId ? '更新目标' : '安装范围'}，确认{targetAppId ? '更新此应用' : '安装为新应用'}</label>
        <button type="button" className="primary-button" disabled={!canApply} onClick={() => void apply()}>{targetAppId ? '确认更新' : '确认安装'}</button>
      </>}
    </section>}
    {knownCandidateId && !installed && <button type="button" className="refresh-button" disabled={busy || active || checking || disabled} onClick={() => { setError(''); void lookup(knownCandidateId, currentCandidate.current); }}>{checking ? '正在核验…' : '核验准备结果'}</button>}
    {installed && <p role="status">已确认{targetAppId ? '更新' : '安装'}，应用未自动启动。</p>}
    {task && <><ProjectTask key={task.taskId} task={task} />{!task.final && <div className="confirmation-actions"><button type="button" className="refresh-button" disabled={busy} onClick={() => void poll()}>刷新任务状态</button>{task.type === 'nodeInstallPrepare' && <button type="button" className="refresh-button" disabled={busy} onClick={() => void cancel()}>取消准备</button>}</div>}</>}
    <section aria-label="保留的安装记录"><div className="storage-heading"><h4>保留的安装记录</h4><button type="button" className="refresh-button" disabled={listing || busy} onClick={() => void readList()}>{listing ? '读取记录…' : '刷新安装记录'}</button></div>
      {listError && <p role="alert">{listError}</p>}{listIssues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}
      {!listing && !listError && !visibleRecords.length && <p>暂无此目标的安装记录。</p>}
      {visibleRecords.map(item => <article className="operation-confirmation" key={item.candidateId}><p><code>{item.candidateId}</code> · {cancelledPreparationRecord(item) ? '准备已取消，现场保留' : ({ preparing: '正在准备', prepared: '已准备，尚未安装', committing: '正在提交或需要恢复核对', committed: '已提交安装', failed: '未完成，现场保留', interrupted: '已中断，现场保留', 'needs-attention': '结果需要核对', unavailable: '记录不可用' } as Record<string, string>)[item.state] ?? '状态需要核对'}</p>{Boolean(item.error) && <p role="alert">{messageFrom(item.error)}</p>}{item.issues.map((issue, index) => <p role="alert" key={index}>{messageFrom(issue)}</p>)}{['prepared', 'committed', 'preparing', 'committing'].includes(item.state) && <button type="button" className="refresh-button" disabled={busy || active || checking || disabled} onClick={() => void resume(item)}>{item.state === 'prepared' ? '核验并继续安装' : item.state === 'committed' ? '核验已安装结果' : '继续查看任务'}</button>}</article>)}
    </section>
  </section>;
}
