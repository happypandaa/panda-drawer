import { invoke } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import { Trash2, TriangleAlert } from 'lucide-react';
import { Modal } from './Modal';
import { messageFrom } from './ui-utils';

const confirmationPhrase = '删除全部本机数据';
interface CleanupPlan {
  planId: string;
  instanceId: string;
  expiresAt: number;
  confirmationText: string;
  deletePaths: { path: string; label: string }[];
  preservedPaths: string[];
  warnings: string[];
}
type CleanupStatus = 'idle' | 'stopping' | 'deleting' | 'completed' | 'blocked' | 'failed';
interface CleanupState {
  operationId: string | null;
  status: CleanupStatus;
  message?: string;
  remainingPaths?: string[];
  receiptPath?: string | null;
  error?: unknown;
}
interface Props {
  managedProjects?: { name: string; path: string }[];
  externalProjectCount?: number;
  instanceId?: string;
  scopeIdentity: string;
  disabled: boolean;
  onPendingChange: (pending: boolean) => void;
  onTerminalLock: () => void;
}
function readPlan(value: CleanupPlan, instanceId?: string): CleanupPlan {
  if (!value || typeof value.planId !== 'string' || !value.planId || value.instanceId !== instanceId
    || typeof value.expiresAt !== 'number' || !Number.isFinite(value.expiresAt) || value.expiresAt <= Date.now()
    || value.confirmationText !== confirmationPhrase || !Array.isArray(value.deletePaths) || !value.deletePaths.length
    || value.deletePaths.some(item => !item || typeof item.path !== 'string' || !item.path || typeof item.label !== 'string')
    || !Array.isArray(value.preservedPaths) || value.preservedPaths.some(path => typeof path !== 'string')
    || !Array.isArray(value.warnings) || value.warnings.some(warning => typeof warning !== 'string')) {
    throw new Error('清理范围或服务身份无法核对，尚未提交删除。请重新检查范围。');
  }
  return value;
}
function readState(value: CleanupState): CleanupState {
  if (!value || !(value.status === 'idle' ? value.operationId === null : typeof value.operationId === 'string' && Boolean(value.operationId))
    || !['idle', 'stopping', 'deleting', 'completed', 'blocked', 'failed'].includes(value.status)
    || value.message !== undefined && typeof value.message !== 'string'
    || value.receiptPath != null && typeof value.receiptPath !== 'string'
    || (value.remainingPaths !== undefined && (!Array.isArray(value.remainingPaths) || value.remainingPaths.some(path => typeof path !== 'string')))) {
    throw new Error('清理状态返回不完整，请核对状态；不会再次提交删除。');
  }
  return value;
}
const stateLabels: Record<CleanupStatus, string> = {
  idle: '清理尚未受理，未开始删除', stopping: '正在关闭应用并停止写入', deleting: '正在永久清理本机内容',
  completed: '本机项目内容已清理', blocked: '清理已停止，需要核对', failed: '清理未完成',
};

export function LocalDataCleanupPanel({ managedProjects = [], externalProjectCount = 0, instanceId, scopeIdentity, disabled, onPendingChange, onTerminalLock }: Props) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState<1 | 2>(1);
  const [plan, setPlan] = useState<CleanupPlan | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [typed, setTyped] = useState('');
  const [previewing, setPreviewing] = useState(false);
  const [locked, setLocked] = useState(false);
  const [status, setStatus] = useState<CleanupState | null>(null);
  const [checking, setChecking] = useState(false);
  const [quitting, setQuitting] = useState(false);
  const [expired, setExpired] = useState(false);
  const [error, setError] = useState('');
  const alive = useRef(true);
  const sequence = useRef(0);
  const operation = useRef(false);
  const lock = useRef(false);
  const checkingRef = useRef(false);
  const currentIdentity = JSON.stringify([instanceId, scopeIdentity]);
  const identity = useRef(currentIdentity); identity.current = currentIdentity;
  const planIdentity = useRef<string | null>(null);
  const operationId = useRef<string | null>(null);
  const callbacks = useRef({ onPendingChange, onTerminalLock }); callbacks.current = { onPendingChange, onTerminalLock };
  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; callbacks.current.onPendingChange(false); }; }, []);
  useEffect(() => {
    if (lock.current) return;
    sequence.current++; planIdentity.current = null;
    setPlan(null); setStep(1); setAcknowledged(false); setTyped(''); setExpired(false);
  }, [currentIdentity]);
  useEffect(() => { if (disabled && !lock.current) { planIdentity.current = null; setPlan(null); setStep(1); setAcknowledged(false); setTyped(''); } }, [disabled]);
  useEffect(() => {
    if (!plan || locked) return;
    const timer = window.setTimeout(() => { setExpired(true); setStep(1); setAcknowledged(false); setTyped(''); }, Math.max(0, plan.expiresAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [plan, locked]);
  async function preview() {
    if (operation.current || lock.current || disabled || !instanceId) return;
    operation.current = true; setOpen(true); setPreviewing(true); setError(''); setPlan(null); setStep(1); setAcknowledged(false); setTyped(''); setExpired(false);
    callbacks.current.onPendingChange(true);
    const request = ++sequence.current; const selected = identity.current;
    try {
      const value = readPlan(await invoke<CleanupPlan>('local_data_cleanup_preview'), instanceId);
      if (alive.current && request === sequence.current && identity.current === selected) { planIdentity.current = selected; setPlan(value); }
    } catch (cause) { if (alive.current && request === sequence.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) { setPreviewing(false); callbacks.current.onPendingChange(false); } }
  }
  const planCurrent = Boolean(plan && planIdentity.current === currentIdentity && !expired && plan.expiresAt > Date.now());
  function close() { if (operation.current || lock.current) return; sequence.current++; setOpen(false); setPlan(null); setStep(1); setAcknowledged(false); setTyped(''); setError(''); }
  async function checkStatus() {
    if (checkingRef.current || !lock.current) return;
    checkingRef.current = true; setChecking(true);
    try {
      const value = readState(await invoke<CleanupState>('local_data_cleanup_status'));
      if (value.status !== 'idle' && value.operationId !== operationId.current) throw new Error('清理任务身份变化，无法确认原任务结果。请保留现场并核对，不会重复删除。');
      if (alive.current) { setStatus(value); setError(''); }
    } catch (cause) { if (alive.current) setError(`${messageFrom(cause)} 清理结果仍需核对，请勿重复提交。`); }
    finally { checkingRef.current = false; if (alive.current) setChecking(false); }
  }
  useEffect(() => {
    if (!locked || !status || !['stopping', 'deleting'].includes(status.status)) return;
    const timer = window.setInterval(() => { void checkStatus(); }, 1000);
    return () => window.clearInterval(timer);
  }, [locked, status?.status]);
  async function start() {
    if (operation.current || lock.current || disabled || !planCurrent || !plan || step !== 2 || !acknowledged || typed !== confirmationPhrase) return;
    operation.current = true; lock.current = true; operationId.current = plan.planId; setLocked(true); setError('');
    // Lock the parent synchronously before any deletion request can be accepted.
    callbacks.current.onTerminalLock(); callbacks.current.onPendingChange(true);
    try {
      const value = readState(await invoke<CleanupState>('local_data_cleanup_start', { input: { planId: plan.planId, firstConfirmation: true, confirmation: typed } }));
      if (value.status !== 'idle' && value.operationId !== operationId.current) throw new Error('清理任务身份与本次确认不符，请核对原任务状态。');
      if (alive.current) setStatus(value);
    } catch (cause) {
      if (alive.current) setError(`${messageFrom(cause)} 提交结果尚不能确认。项目管理已停用，请查看清理状态；不会自动重复提交。`);
    } finally { operation.current = false; if (alive.current) callbacks.current.onPendingChange(false); }
  }
  async function quit() {
    if (!locked || quitting || operation.current || checkingRef.current || status && ['stopping', 'deleting'].includes(status.status)) return;
    operation.current = true; setQuitting(true); setError('');
    try { await invoke('local_data_cleanup_quit'); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setQuitting(false); }
  }
  return <section className="local-data-cleanup" aria-labelledby={`${id}-title`}>
    <div className="local-data-cleanup-heading"><Trash2 size={18} aria-hidden="true" /><h3 id={`${id}-title`}>清理本机项目内容</h3></div>
    <p>通常在卸载熊猫小抽屉之前使用。清理会永久删除本软件管理的所有本机项目代码和数据，无法撤销。</p>
    <button className="danger-button" disabled={disabled || previewing || locked || !instanceId} onClick={() => void preview()}>清理所有本机项目内容</button>
    {open && <Modal title={locked ? '本机内容清理状态' : step === 1 ? '清理所有本机内容 · 第一次确认' : '清理所有本机内容 · 第二次确认'} titleIcon={<TriangleAlert size={22} />} className="local-data-cleanup-modal" closeDisabled={previewing || locked} beforeClose={() => !operation.current && !lock.current} onClose={close}>
      {!locked ? <>
        <p className="local-data-cleanup-warning">通常在卸载软件前才需要执行。将永久删除下方范围内的全部代码和本机数据，不进入回收站，无法撤销。</p>
        <p>包括项目源码、已安装应用及其依赖、数据库、附件、配置和保存的 Key、所有工作区、旧备份、旧回收项与恢复现场。请先把需要保留的内容导出到本次范围以外，并停止外部编辑器、AI 和其他工具访问这些目录。</p>
        {previewing && <p role="status">正在核对清理范围，尚未删除文件…</p>}
        {plan && <>{managedProjects.length > 0 && <div className="local-data-cleanup-paths"><h4>将删除的托管项目</h4><ul>{managedProjects.map(project => <li key={project.path}><strong>{project.name}</strong><code>{project.path}</code></li>)}</ul></div>}<p>外部项目的文件夹不会被删除：{externalProjectCount} 个。平台内的应用数据和设置会删除。</p><div className="local-data-cleanup-paths"><h4>将永久删除</h4><ul>{plan.deletePaths.map((item, index) => <li key={`${item.path}:${index}`}><strong>{item.label}</strong><code>{item.path}</code></li>)}</ul></div>
          <div className="local-data-cleanup-paths"><h4>继续保留</h4><p>软件程序本身保留，可在清理完成并退出后卸载。为阻止并发运行及核对结果，可能保留极少量实例锁与清理回执。</p><p>系统生成的网页存储（可能包含会话和本地存储）、软件缓存及系统偏好也可能保留；本操作不等于卸载后完全无残留。</p>{plan.preservedPaths.length > 0 && <ul>{plan.preservedPaths.map((path, index) => <li key={index}>{path}</li>)}</ul>}</div>
          {plan.warnings.length > 0 && <ul className="local-data-cleanup-warnings">{plan.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul>}
          {expired && <p role="status">清理范围已过期，请重新核对后完成两次确认。</p>}
          {disabled && <p role="status">有其他操作进行中或工作区不可用，请等待处理完成后重新核对。</p>}
          {step === 1 ? <><label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={!planCurrent || disabled} onChange={event => setAcknowledged(event.target.checked)} />我已核对所有目录，理解其中全部本机代码、数据和历史资料将永久删除</label><div className="confirmation-actions"><button className="refresh-button" onClick={close}>取消，保留全部内容</button><button className="danger-button" disabled={!planCurrent || disabled || !acknowledged} onClick={() => { if (planCurrent && !disabled && acknowledged) { setStep(2); setTyped(''); } }}>已了解，继续第二次确认</button></div></> : <><p className="local-data-cleanup-warning">最后确认：这些代码、数据库、附件、Key 和历史资料会永久删除。清理完成后软件将停止项目管理，通常请退出并卸载软件。</p><label className="local-data-cleanup-phrase" htmlFor={`${id}-phrase`}>请输入“{confirmationPhrase}”完成第二次确认<input id={`${id}-phrase`} autoComplete="off" spellCheck={false} value={typed} disabled={!planCurrent || disabled} onChange={event => setTyped(event.target.value)} /></label><div className="confirmation-actions"><button className="refresh-button" onClick={close}>取消，保留全部内容</button><button className="danger-button" disabled={!planCurrent || disabled || typed !== confirmationPhrase} onClick={() => void start()}>永久清理所有本机数据</button></div></>}
        </>}
        {!previewing && (!plan || expired) && <button className="refresh-button" disabled={disabled} onClick={() => void preview()}>重新核对清理范围</button>}
      </> : <div className="local-data-cleanup-result" aria-live="polite" aria-busy={operation.current || checking || Boolean(status && ['stopping', 'deleting'].includes(status.status))}>
        <h3>{status ? stateLabels[status.status] : operation.current ? '正在提交清理，请等待' : '清理结果尚待核对'}</h3>
        {status?.message && <p>{status.message}</p>}
        {status?.status === 'completed' ? <><p>本次清理已完成。软件程序仍在，可退出后卸载；外部保留范围及必要的实例锁和清理回执以核对结果为准。</p><p>系统网页存储、软件缓存和系统偏好可能保留，本操作不等于卸载后完全无残留。</p></> : status?.status === 'idle' ? <p>本次没有受理清理，未开始删除。可以退出软件，重新打开后核对范围并重新确认；不会自动重试。</p> : <p>普通项目操作已停止。请等待并核对实际结果；已删除的文件无法恢复，不会自动再次提交清理。</p>}
        {status?.error != null && <p role="alert">{messageFrom(status.error)}</p>}
        {Boolean(status?.remainingPaths?.length) && <div className="local-data-cleanup-paths"><h4>尚需核对的目录</h4><ul>{status!.remainingPaths!.map(path => <li key={path}><code>{path}</code></li>)}</ul></div>}
        {status?.receiptPath && <div className="local-data-cleanup-paths"><h4>清理回执</h4><code>{status.receiptPath}</code><p>回执用于核对实际结果，此处仅显示位置。</p></div>}
        <div className="confirmation-actions">{status?.status !== 'completed' && <button className="refresh-button" disabled={checking || operation.current} onClick={() => void checkStatus()}>{checking ? '正在核对…' : '查看清理状态'}</button>}<button className="refresh-button" disabled={quitting || operation.current || checking || Boolean(status && ['stopping', 'deleting'].includes(status.status))} onClick={() => void quit()}>{quitting ? '正在退出…' : '退出软件'}</button></div>
      </div>}
      {error && <p className="task-error" role="alert">{error}</p>}
    </Modal>}
  </section>;
}
