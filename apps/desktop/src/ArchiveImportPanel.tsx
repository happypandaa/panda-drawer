import { invoke } from '@tauri-apps/api/core';
import { FileArchive, FolderOpen, LoaderCircle, Upload } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ImportProgress } from './ImportProgress';
import { Modal } from './Modal';
import { ProjectTask } from './ProjectTask';
import type { ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

type Props = {
  onTask: (task: TaskRecord) => void; onClose: () => void; disabled: boolean; instanceId?: string;
  onCompleted?: () => void; onImported?: (project: ProjectInfo) => void;
};
type View = 'form' | 'running' | 'success' | 'cleanup-warning' | 'failed' | 'recovery';
type Cleanup = { completed: boolean; projectId: string; operationOutcome: string; removedPaths: string[]; failures: { path: string; error?: unknown; message?: string }[] };
const steps = ['读取归档', '检查内容', '导入项目', '清理临时文件'];
const phaseSteps: Record<string, number> = { 'archive-copy': 0, 'archive-validate': 1, 'candidate-initialize': 2, 'install-intent': 2, cleanup: 3 };
const phaseMessages: Record<string, string> = { 'archive-copy': '正在读取归档文件', 'archive-validate': '正在检查项目、数据库和附件', 'candidate-initialize': '正在准备独立项目', 'install-intent': '正在添加到项目列表', cleanup: '正在清理本次导入的临时文件' };
const record = (value: unknown): value is Record<string, unknown> => Boolean(value && typeof value === 'object' && !Array.isArray(value));
function taskResult(task: TaskRecord): Record<string, unknown> | null {
  if (record(task.result) && task.result.operationOutcome === 'imported') return task.result;
  const recovered = task.error?.details?.lifecycleResult;
  return record(recovered) && recovered.operationOutcome === 'imported' ? recovered : null;
}
function failedOutcome(task: TaskRecord | null) { return task?.result?.operationOutcome ?? task?.error?.details?.operationOutcome; }

export function ImportPanel(props: Props) {
  const [path, setPath] = useState(''); const [mode, setMode] = useState<'original' | 'copy'>('copy'); const [name, setName] = useState('');
  return <ArchiveImportWorkflow key={props.instanceId ?? ''} {...props} path={path} setPath={setPath} mode={mode} setMode={setMode} name={name} setName={setName} />;
}
function ArchiveImportWorkflow({ onTask, onClose, onCompleted, onImported, disabled, path, setPath, mode, setMode, name, setName }: Props & {
  path: string; setPath: (value: string) => void; mode: 'original' | 'copy'; setMode: (value: 'original' | 'copy') => void; name: string; setName: (value: string) => void;
}) {
  const [view, setView] = useState<View>('form'); const [busy, setBusy] = useState(false); const [choosing, setChoosing] = useState(false);
  const [task, setTask] = useState<TaskRecord | null>(null); const [step, setStep] = useState(0); const [error, setError] = useState(''); const [pollError, setPollError] = useState('');
  const [failedStep, setFailedStep] = useState<number | null>(null); const observedStep = useRef(0);
  const [projectId, setProjectId] = useState<string | null>(null); const [cleaned, setCleaned] = useState(false);
  const alive = useRef(false); const generation = useRef(0); const pending = useRef(false); const polling = useRef(false); const selectedTask = useRef<TaskRecord | null>(null);
  const completed = useRef(new Set<string>()); const callbacks = useRef({ onTask, onClose, onCompleted, onImported }); callbacks.current = { onTask, onClose, onCompleted, onImported };
  const heading = useRef<HTMLHeadingElement>(null); const id = useId();
  useEffect(() => { alive.current = true; generation.current++; return () => { alive.current = false; generation.current++; }; }, []);
  useEffect(() => { if (!['form', 'running'].includes(view)) heading.current?.focus(); }, [view]);
  const current = useCallback((value: number) => alive.current && generation.current === value, []);
  const active = busy || choosing || view === 'running';
  const accept = useCallback((value: TaskRecord, expected?: TaskRecord) => {
    if (!value || value.type !== 'import' || typeof value.taskId !== 'string' || !value.taskId || typeof value.projectId !== 'string' || !value.projectId.startsWith('prj_') || typeof value.final !== 'boolean'
      || expected && (value.taskId !== expected.taskId || value.projectId !== expected.projectId)) throw new Error('导入任务身份不符，请核对任务记录，勿重复导入。');
    if (selectedTask.current?.taskId === value.taskId && selectedTask.current.final && !value.final) return;
    selectedTask.current = value; setTask(value); callbacks.current.onTask(value); setPollError('');
    if (phaseSteps[value.phase] !== undefined) { observedStep.current = Math.max(observedStep.current, phaseSteps[value.phase]); setStep(observedStep.current); }
    if (!value.final) { setView('running'); return; }
    const result = taskResult(value);
    if (result) {
      if (typeof result.projectId !== 'string' || result.projectId !== value.projectId || result.targetProjectId !== undefined && result.targetProjectId !== value.projectId) throw new Error('已提交结果的项目身份无法确认，请先核对项目列表。');
      setProjectId(result.projectId); setStep(3); setFailedStep(null);
      if (!completed.current.has(value.taskId)) { completed.current.add(value.taskId); callbacks.current.onCompleted?.(); }
      const cleanup = record(result.cleanup) ? result.cleanup : null;
      if (value.status === 'succeeded' && result.cleanupPending === false && cleanup?.completed === true) {
        setCleaned(true); setView('success'); setError('');
      } else if (value.status !== 'succeeded' && result.cleanupPending !== true) {
        setView('recovery'); setError(value.error ? messageFrom(value.error) : '应用登记已提交，但操作记录仍需核对，请勿重复导入或清理。');
      } else {
        setView('cleanup-warning');
        setError(value.error ? messageFrom(value.error) : '项目已经添加，临时文件还没有清理完成。请重试清理，无需再次导入。');
      }
    } else {
      const outcome = failedOutcome(value);
      setFailedStep(observedStep.current);
      setError(value.error ? messageFrom(value.error) : '导入结果尚不能确认，请核对任务记录。');
      setView(['not_registered', 'not_started'].includes(String(outcome)) ? 'failed' : 'recovery');
    }
  }, []);
  const poll = useCallback(async () => {
    const previous = selectedTask.current;
    if (!previous || polling.current) return;
    const started = generation.current; polling.current = true;
    try {
      const value = await invoke<TaskRecord>('task_get', { taskId: previous.taskId });
      if (!current(started) || selectedTask.current?.taskId !== previous.taskId) return;
      try { accept(value, previous); }
      catch (cause) { setView('recovery'); setError(messageFrom(cause)); }
    } catch (cause) { if (current(started)) setPollError(`暂时读不到进度，仍在核对本次任务：${messageFrom(cause)}`); }
    finally { polling.current = false; }
  }, [accept, current]);
  useEffect(() => {
    if (!task || task.final || view !== 'running') return;
    const timer = window.setInterval(() => { void poll(); }, 1000);
    return () => window.clearInterval(timer);
  }, [task?.taskId, task?.final, view, poll]);
  async function choose() {
    if (active || disabled || pending.current) return;
    const started = generation.current; pending.current = true; setChoosing(true); setError('');
    try { const value = await invoke<string | null>('choose_path', { kind: 'import' }); if (current(started) && value) setPath(value); }
    catch (cause) { if (current(started)) setError(`路径选择失败，可填写完整路径：${messageFrom(cause)}`); }
    finally { if (current(started)) { pending.current = false; setChoosing(false); } }
  }
  async function cleanup(): Promise<boolean> {
    const previous = selectedTask.current;
    if (!previous?.final) throw new Error('任务尚未结束，不能清理。');
    setStep(3); setView('running'); setError('');
    const started = generation.current;
    const result = await invoke<Cleanup>('project_import_cleanup', { taskId: previous.taskId });
    if (!current(started)) return false;
    if (!result || result.projectId !== previous.projectId || typeof result.completed !== 'boolean' || !['imported', 'not_registered'].includes(result.operationOutcome) || !Array.isArray(result.failures)) throw new Error('清理结果身份或范围无法确认，请重试核对。');
    if (!result.completed) throw new Error(`部分临时文件未清理：${result.failures.map(item => messageFrom(item.error ?? item.message ?? item.path)).join('；') || '请重试清理。'}`);
    setCleaned(true);
    if (result.operationOutcome === 'imported' && !projectId) {
      setProjectId(result.projectId); setFailedStep(null); setView('success'); setError('');
      if (!completed.current.has(previous.taskId)) { completed.current.add(previous.taskId); callbacks.current.onCompleted?.(); }
      return false;
    }
    return true;
  }
  async function submit() {
    if (active || disabled || pending.current || !path.trim().startsWith('/')) return;
    const started = generation.current; let submitting = false; pending.current = true; setBusy(true); setError(''); setPollError('');
    try {
      if (selectedTask.current && !cleaned && !await cleanup()) return;
      if (!current(started)) return;
      selectedTask.current = null; setTask(null); setCleaned(false); setProjectId(null); setFailedStep(null); observedStep.current = 0; setStep(0); setView('running');
      submitting = true;
      const value = await invoke<TaskRecord>('project_import', { input: { archivePath: path.trim(), mode, cleanupTemporaryFiles: true, ...(mode === 'copy' && name.trim() ? { name: name.trim() } : {}) } });
      if (!current(started)) return;
      try { accept(value); }
      catch (cause) { setView('recovery'); setError(messageFrom(cause)); }
    } catch (cause) {
      if (current(started)) {
        setError(messageFrom(cause));
        setView(!submitting && selectedTask.current ? 'failed' : !record(cause) || typeof cause.code !== 'string' ? 'recovery' : 'failed');
      }
    } finally { if (current(started)) { pending.current = false; setBusy(false); } }
  }
  async function retryCleanup(closeAfter = false) {
    if (active || pending.current) return;
    const started = generation.current; const installed = projectId !== null;
    pending.current = true; setBusy(true);
    try {
      if (!await cleanup() || !current(started)) return;
      if (closeAfter) callbacks.current.onClose();
      else if (installed) { setView('success'); setError(''); }
      else setView('failed');
    } catch (cause) { if (current(started)) { setError(messageFrom(cause)); setView(installed ? 'cleanup-warning' : 'failed'); } }
    finally { if (current(started)) { pending.current = false; setBusy(false); } }
  }
  async function edit() {
    if (active || pending.current) return;
    if (selectedTask.current && !cleaned) {
      const started = generation.current; pending.current = true; setBusy(true);
      try { if (!await cleanup() || !current(started)) return; }
      catch (cause) { if (current(started)) { setError(messageFrom(cause)); setView('failed'); } return; }
      finally { if (current(started)) { pending.current = false; setBusy(false); } }
    }
    selectedTask.current = null; setTask(null); setError(''); setView('form');
  }
  function close() {
    if (active) return;
    if (view === 'failed' && task && !cleaned) void retryCleanup(true);
    else callbacks.current.onClose();
  }
  async function showProject() {
    if (!projectId || active || pending.current) return;
    const started = generation.current; pending.current = true; setBusy(true); setError('');
    try {
      const list = await invoke<ProjectInfo[]>('project_list');
      if (!current(started)) return;
      const project = list.find(item => item.id === projectId);
      if (!project) throw new Error('导入已完成，项目列表暂时无法定位该项目，请刷新列表。');
      callbacks.current.onClose(); callbacks.current.onImported?.(project);
    } catch (cause) { if (current(started)) setError(messageFrom(cause)); }
    finally { if (current(started)) { pending.current = false; setBusy(false); } }
  }
  return <Modal title="导入完整项目" titleIcon={<FileArchive size={20} />} description="选择平台导出的 ZIP，检查后添加到项目列表。" onClose={close} closeDisabled={active} className="import-dialog">
    {view === 'form' && <form className="create-project-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <div className="path-input"><label htmlFor={`${id}-path`}>完整项目 ZIP</label><span className="path-input-controls"><input id={`${id}-path`} value={path} onChange={event => setPath(event.target.value)} disabled={active || disabled} placeholder="/Users/你的名字/Documents/项目.zip" autoComplete="off" /><button type="button" className="refresh-button" disabled={active || disabled} onClick={() => void choose()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择路径'}</button></span></div>
      <fieldset disabled={active || disabled} className="choice-group"><legend>导入方式</legend><label><input type="radio" name="import-mode" value="copy" checked={mode === 'copy'} onChange={() => setMode('copy')} />作为新项目导入（默认）</label><label><input type="radio" name="import-mode" value="original" checked={mode === 'original'} onChange={() => setMode('original')} />保留原 ID（用于恢复）</label></fieldset>
      <p className="project-help">{mode === 'original' ? '用于接回原项目身份。如果该 ID 已在项目列表或历史保留项目中，平台会拒绝导入，不会覆盖或合并。' : '每次导入都生成新 ID，重复导入可以并存；数据库、附件和源码各自独立。'}</p>
      {mode === 'copy' && <label>副本名称（选填）<input value={name} onChange={event => setName(event.target.value)} disabled={active || disabled} /></label>}
      <p className="project-help">导入不会运行项目脚本；维护中的快照继续保持维护。完成后清理临时副本，保留原 ZIP 文件。</p>
      <div className="confirmation-actions"><button className="primary-button" type="submit" disabled={active || disabled || !path.trim().startsWith('/')}><Upload size={14} />校验并导入</button></div>
    </form>}
    {view !== 'form' && <><ImportProgress steps={steps} currentStep={step} completedSteps={step === 3 && failedStep !== null ? steps.map((_, index) => index).filter(index => index < failedStep) : undefined} state={view === 'success' ? 'succeeded' : view === 'cleanup-warning' ? 'warning' : view === 'failed' || view === 'recovery' ? 'failed' : 'running'} /><section className="import-result" aria-busy={active}>
      <h3 ref={heading} tabIndex={-1}>{view === 'success' ? '导入成功' : view === 'cleanup-warning' ? '项目已导入，临时文件待清理' : view === 'failed' ? '导入未完成' : view === 'recovery' ? '导入结果需要核对' : steps[step]}</h3>
      {view === 'running' && <p role="status"><LoaderCircle size={16} className="import-spinner" />{busy && step === 3 ? '正在清理本次导入的临时文件' : phaseMessages[task?.phase ?? ''] ?? '正在检查归档，请稍候…'}</p>}
      {view === 'success' && <><p role="status">已添加到项目列表，临时文件已清理。原 ZIP 文件保留。</p><div className="confirmation-actions">{onImported && <button type="button" className="primary-button" disabled={active} onClick={() => void showProject()}>查看项目</button>}<button type="button" className={onImported ? 'refresh-button' : 'primary-button'} disabled={active} onClick={close}>完成</button></div></>}
      {view === 'cleanup-warning' && <><p>项目已经添加，请重试清理临时文件，无需再次导入。</p><div className="confirmation-actions"><button type="button" className="primary-button" disabled={active} onClick={() => void retryCleanup()}>重试清理</button><button type="button" className="refresh-button" disabled={active} onClick={close}>稍后处理</button></div></>}
      {view === 'failed' && <div className="confirmation-actions"><button type="button" className="primary-button" disabled={active || disabled} onClick={() => void submit()}>重试导入</button><button type="button" className="refresh-button" disabled={active} onClick={() => void edit()}>重新选择文件</button><button type="button" className="refresh-button" disabled={active} onClick={close}>{task && !cleaned ? '清理并关闭' : '关闭'}</button></div>}
      {view === 'recovery' && <><p>先核对本次任务和项目列表，确认前不会重复导入或删除材料。</p><div className="confirmation-actions">{task && <button type="button" className="primary-button" disabled={active} onClick={() => void poll()}>核对导入结果</button>}<button type="button" className="refresh-button" disabled={active} onClick={close}>返回项目列表</button></div></>}
      {task && <details className="import-task"><summary>任务详情与日志</summary><ProjectTask key={task.taskId} task={task} /></details>}
    </section></>}
    {error && <p className="task-error" role="alert">{error}</p>}{pollError && <p role="status">{pollError}</p>}
  </Modal>;
}
