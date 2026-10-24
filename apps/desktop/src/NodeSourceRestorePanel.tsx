import { invoke } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import { messageFrom } from './ui-utils';
import { nodeGitFailureMessage } from './node-git-error';

interface Preview { targetCommit: string; head: string; branch: string; snapshotDigest: string; planDigest: string; canApply: boolean; changes: { path: string; kind: string }[] }
interface Recovery { operationId: string; phase: string; targetCommit: string; planDigest: string; journalDigest: string; canContinue: boolean; canUndo: boolean; changes: { path: string; state: string }[] }
interface RecoveryStatus { state: string; operations: Recovery[]; issues: unknown[] }
const digest = /^[a-f0-9]{64}$/;
const commit = /^[a-f0-9]{40}$/;
function validPreview(value: Preview, target: string) { return value?.targetCommit === target && commit.test(value.head) && digest.test(value.planDigest) && digest.test(value.snapshotDigest) && typeof value.canApply === 'boolean' && Array.isArray(value.changes) && value.changes.every(change => typeof change.path === 'string' && ['add', 'modify', 'delete'].includes(change.kind)); }
function validStatus(value: RecoveryStatus) { return value && Array.isArray(value.operations) && Array.isArray(value.issues) && value.operations.every(item => /^restore_[a-f0-9-]{36}$/.test(item.operationId) && commit.test(item.targetCommit) && digest.test(item.planDigest) && digest.test(item.journalDigest) && typeof item.canContinue === 'boolean' && typeof item.canUndo === 'boolean' && Array.isArray(item.changes) && item.changes.every(change => typeof change.path === 'string' && ['before', 'after'].includes(change.state))); }

export function NodeSourceRestorePanel({ appId, instanceId, sourceRoot, disabled, runtimeIsSource, running, onDraftStateChange, onChanged }: {
  appId: string; instanceId?: string; sourceRoot?: string; disabled: boolean; runtimeIsSource?: boolean; running?: boolean;
  onDraftStateChange: (value: { dirty: boolean; pending: boolean }) => void; onChanged: () => void;
}) {
  const id = useId();
  const [target, setTarget] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [recovery, setRecovery] = useState<RecoveryStatus | null>(null);
  const [accepted, setAccepted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const previousApp = useRef(appId);
  const mounted = useRef(false); const generation = useRef(0); const pending = useRef(false); const choice = useRef(0);
  const callbacks = useRef({ onDraftStateChange, onChanged }); callbacks.current = { onDraftStateChange, onChanged };
  const current = (sequence: number) => mounted.current && sequence === generation.current;
  const unavailable = disabled || runtimeIsSource || running;
  async function status(sequence: number) {
    const value = await invoke<RecoveryStatus>('node_development_git', { appId, operation: 'restore-status' });
    if (!current(sequence)) return;
    if (!validStatus(value)) throw new Error('源码取回状态不完整，请保留现场并重新核对。');
    setRecovery(value);
  }
  useEffect(() => {
    mounted.current = true; const sequence = ++generation.current;
    if (previousApp.current !== appId) { setTarget(''); previousApp.current = appId; }
    choice.current++;
    pending.current = false; setBusy(false); setPreview(null); setRecovery(null); setAccepted(false); setError(''); setNotice('');
    if (!runtimeIsSource && !running) void status(sequence).catch(cause => { if (current(sequence)) setError(nodeGitFailureMessage(cause, { operation: 'restore-status' })); });
    return () => { mounted.current = false; generation.current++; callbacks.current.onDraftStateChange({ dirty: false, pending: false }); };
  }, [appId, instanceId, sourceRoot, runtimeIsSource, running]);
  useEffect(() => { callbacks.current.onDraftStateChange({ dirty: Boolean(target), pending: busy }); }, [target, busy, appId, instanceId, sourceRoot]);
  async function run(operation: 'restore-preview' | 'restore-apply' | 'restore-status' | 'restore-recover', record?: Recovery, action?: 'continue' | 'undo') {
    if (unavailable || pending.current) return;
    if (operation === 'restore-preview' && !commit.test(target.trim())) return;
    if (operation === 'restore-apply' && (!preview?.canApply || !accepted)) return;
    if (operation === 'restore-recover' && (!record || (action === 'continue' ? !record.canContinue : !record.canUndo))) return;
    const sequence = generation.current; const selected = choice.current;
    let failureOperation = operation;
    pending.current = true; setBusy(true); setError(''); setNotice('');
    try {
      if (operation === 'restore-status') await status(sequence);
      else if (operation === 'restore-preview') {
        const requested = target.trim();
        const value = await invoke<Preview>('node_development_git', { appId, operation, input: { targetCommit: requested } });
        if (!current(sequence) || selected !== choice.current) return;
        if (!validPreview(value, requested)) throw new Error('源码预览与当前提交不一致，请重新读取。');
        setPreview(value); setAccepted(false);
      } else {
        const input = operation === 'restore-apply'
          ? { targetCommit: preview!.targetCommit, expectedSnapshot: preview!.snapshotDigest, expectedPlanDigest: preview!.planDigest }
          : { operationId: record!.operationId, expectedPlanDigest: record!.planDigest, expectedJournalDigest: record!.journalDigest, action };
        const result = await invoke<{ operationOutcome: string; state: string; controlUpdateWarning?: boolean }>('node_development_git', { appId, operation, input });
        if (!current(sequence)) return;
        if (result.operationOutcome !== 'committed' || !['completed', 'undone'].includes(result.state)) throw new Error('源码操作结果尚未确认，请刷新恢复状态，勿重复应用。');
        setPreview(null); setAccepted(false); setTarget('');
        const message = result.state === 'undone' ? '本次源码取回已撤回，当前程序和业务数据保留。' : '源码已取回为未保存修改。请检查差异，再保存版本并构建更新。当前程序和业务数据保留。';
        setNotice(result.controlUpdateWarning ? `${message} 恢复状态标记尚待核对，请保留本次现场。` : message);
        failureOperation = 'restore-status';
        await status(sequence); if (current(sequence)) callbacks.current.onChanged();
      }
    } catch (cause) {
      if (current(sequence)) {
        setError(nodeGitFailureMessage(cause, { operation: failureOperation, operationId: record?.operationId })); setPreview(null); setAccepted(false);
        if (operation === 'restore-apply' || operation === 'restore-recover') await status(sequence).catch(() => {});
      }
    } finally { if (current(sequence)) { pending.current = false; setBusy(false); } }
  }
  const locked = Boolean(unavailable || busy);
  const recoveryRequired = Boolean(recovery?.operations.length || recovery?.issues.length);
  return <section className="node-source-restore-panel" aria-label="Node 源码取回" aria-busy={busy}>
    <p className="project-help">仅取回源码文件，保留当前分支、Git 提交、暂存区、正在安装的程序、业务数据和配置。取回后不会自动保存版本或构建。</p>
    {runtimeIsSource && <p>请先构建并更新，将运行副本与作者源码分开。</p>}{running && <p>请先正常关闭应用，等待服务停止后再取回源码。</p>}
    {error && <p className="task-error" role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    <button className="refresh-button" disabled={locked} onClick={() => void run('restore-status')}>核对取回状态</button>
    {recovery?.issues.map((issue, index) => <p className="task-error" role="alert" key={index}>{messageFrom(issue)}</p>)}
    {recovery?.operations.map(item => <section className="operation-confirmation" key={item.operationId}><strong>有未完成的源码取回</strong><p>目标提交：<code>{item.targetCommit}</code></p><p>继续或撤回前会重新核对每个文件；发现计划外变化会停止并保留现场。</p><ul>{item.changes.map(change => <li key={change.path}><code>{change.path}</code> · {change.state === 'after' ? '已取回' : '尚未取回'}</li>)}</ul><div className="confirmation-actions"><button className="primary-button" disabled={locked || !item.canContinue} onClick={() => void run('restore-recover', item, 'continue')}>继续本次取回</button><button className="refresh-button" disabled={locked || !item.canUndo} onClick={() => void run('restore-recover', item, 'undo')}>撤回本次取回</button></div></section>)}
    <form onSubmit={event => { event.preventDefault(); void run('restore-preview'); }}>
      <label htmlFor={`${id}-target`}>本机完整提交号</label><input id={`${id}-target`} value={target} maxLength={40} disabled={locked || recoveryRequired} autoCapitalize="none" autoCorrect="off" spellCheck={false} onChange={event => { choice.current++; setTarget(event.target.value); setPreview(null); setAccepted(false); setError(''); }} />
      <button className="refresh-button" disabled={locked || recoveryRequired || !commit.test(target.trim())}>预览源码取回</button>
    </form>
    {preview && <section className="operation-confirmation" aria-label="Node 源码取回预览"><p>目标提交：<code>{preview.targetCommit}</code></p><ul>{preview.changes.map(change => <li key={change.path}><code>{change.path}</code> · {{ add: '新增', modify: '修改', delete: '移除' }[change.kind] ?? change.kind}</li>)}</ul>{!preview.canApply && <p>源码与目标版本相同。</p>}<label className="checkbox-label"><input type="checkbox" checked={accepted} disabled={locked} onChange={event => setAccepted(event.target.checked)} />已核对文件范围，并停止外部编辑器与 Git 操作</label><button className="primary-button" disabled={locked || !preview.canApply || !accepted} onClick={() => void run('restore-apply')}>确认取回源码</button></section>}
  </section>;
}
