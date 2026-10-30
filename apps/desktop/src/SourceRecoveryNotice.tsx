import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { messageFrom } from './ui-utils';

interface Attempt { taskId: string; status: 'running' | 'cleaned' | 'installed' | 'blocked'; message: string; appId?: string | null; url?: string; cleanupPending?: boolean; remainingPaths?: string[] }
export function SourceRecoveryNotice({ instanceId, reportErrors = false, onFound, onOpenExisting, onOpenReady, onAttentionChange }: {
  instanceId?: string; reportErrors?: boolean; onFound?: () => void; onOpenExisting: (appId: string) => void;
  onOpenReady?: (open: (() => void) | null) => void;
  onAttentionChange?: (attention: boolean) => void;
}) {
  const [attempts, setAttempts] = useState<Attempt[]>([]);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const report = useRef(reportErrors); report.current = reportErrors;
  const callbacks = useRef({ onFound, onOpenReady, onAttentionChange }); callbacks.current = { onFound, onOpenReady, onAttentionChange };
  const alive = useRef(true); const generation = useRef(0); const pending = useRef(false);
  async function check() {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError('');
    const sequence = generation.current;
    try {
      const values = await invoke<Attempt[]>('source_import_recover');
      if (!Array.isArray(values) || values.some(value => !value || typeof value.taskId !== 'string' || !['running', 'cleaned', 'installed', 'blocked'].includes(value.status) || typeof value.message !== 'string')) throw new Error('上次添加的结果无法核对，请查看任务。');
      if (alive.current && sequence === generation.current) {
        setAttempts(values); setOpen(values.length > 0);
        if (values.length) callbacks.current.onFound?.();
        callbacks.current.onAttentionChange?.(values.length > 0);
      }
    } catch (cause) { if (alive.current && sequence === generation.current) { setError(messageFrom(cause)); callbacks.current.onAttentionChange?.(true); if (report.current) setOpen(true); } }
    finally { if (alive.current && sequence === generation.current) { pending.current = false; setBusy(false); } }
  }
  async function retry(taskId: string) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError('');
    const sequence = generation.current;
    try { await invoke('node_git_import_retry_cleanup', { taskId }); }
    catch (cause) { if (alive.current && sequence === generation.current) setError(messageFrom(cause)); }
    finally { if (alive.current && sequence === generation.current) { pending.current = false; setBusy(false); await check(); } }
  }
  useEffect(() => { alive.current = true; generation.current++; pending.current = false; setOpen(false); setAttempts([]); setError(''); void check(); return () => { alive.current = false; generation.current++; }; }, [instanceId]);
  useEffect(() => {
    callbacks.current.onOpenReady?.(() => setOpen(true));
    return () => { callbacks.current.onOpenReady?.(null); };
  }, [instanceId]);
  useEffect(() => { if (reportErrors && error) setOpen(true); }, [reportErrors, error]);
  function dismiss() {
    setOpen(false);
    callbacks.current.onAttentionChange?.(Boolean(error) || attempts.some(attempt => attempt.cleanupPending || ['blocked', 'running'].includes(attempt.status)));
  }
  if (!open) return null;
  return <Modal title="上次添加需要查看" description="先核对原来的任务，再决定是否重新添加。" closeDisabled={busy} onClose={dismiss}>
    {attempts.map(attempt => <section className="source-recovered-attempt" key={attempt.taskId}><h3>{attempt.status === 'cleaned' ? '添加已中断，临时文件已清理' : attempt.status === 'installed' ? '应用已经添加' : '添加结果仍需核对'}</h3>{attempt.url && <code>{attempt.url}</code>}<p>{attempt.message}</p>{attempt.remainingPaths?.map(path => <p key={path}>待清理：<code>{path}</code></p>)}{(attempt.cleanupPending || attempt.status === 'installed' && attempt.appId) && <div className="confirmation-actions">{attempt.cleanupPending && <button className="refresh-button" disabled={busy} onClick={() => void retry(attempt.taskId)}>重试清理</button>}{attempt.status === 'installed' && attempt.appId && <button className="primary-button" onClick={() => { dismiss(); onOpenExisting(attempt.appId!); }}>查看已添加的应用</button>}</div>}</section>)}
    {error && <p role="alert" className="task-error">{error}</p>}
    <div className="confirmation-actions">{(error || attempts.some(attempt => ['blocked', 'running'].includes(attempt.status))) ? <button className="primary-button" disabled={busy} onClick={() => void check()}>{busy ? '正在核对…' : '重新核对'}</button> : <button className="primary-button" onClick={dismiss}>继续添加</button>}<button className="refresh-button" disabled={busy} onClick={dismiss}>关闭</button></div>
  </Modal>;
}
