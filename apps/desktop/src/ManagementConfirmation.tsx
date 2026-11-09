import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { t } from './i18n';
import { messageFrom } from './ui-utils';

export interface ManagementRequest {
  requestId: string;
  requester: string;
  action: string;
  impact: string;
  expiresAt: number;
}
export function checkedManagementRequest(value: unknown): value is ManagementRequest {
  if (!value || typeof value !== 'object') return false;
  const request = value as ManagementRequest;
  return ['requestId', 'requester', 'action', 'impact'].every(key => typeof request[key as keyof ManagementRequest] === 'string' && Boolean(request[key as keyof ManagementRequest]))
    && Number.isFinite(request.expiresAt) && request.expiresAt > Date.now();
}

/** Only this manager-window action can approve a pending CLI operation. */
export function ManagementConfirmation({ instanceId }: {instanceId?: string}) {
  const [requests, setRequests] = useState<ManagementRequest[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const [now, setNow] = useState(Date.now);
  const generation = useRef(0);
  const submitting = useRef(false);
  const selected = requests[0];
  useEffect(() => {
    const current = ++generation.current;
    setRequests([]); setPending(false); setError(''); submitting.current = false;
    if (!instanceId || !isTauri()) return;
    let disposed = false; let stop: (() => void) | undefined;
    void listen<unknown>('panda-management-confirmation', event => {
      if (disposed || generation.current !== current || !checkedManagementRequest(event.payload)) return;
      const request = event.payload;
      setRequests(previous => previous.some(item => item.requestId === request.requestId) ? previous : [...previous, request]);
    }).then(unlisten => { if (disposed) unlisten(); else stop = unlisten; }).catch(cause => setError(messageFrom(cause)));
    return () => { disposed = true; generation.current++; stop?.(); };
  }, [instanceId]);
  useEffect(() => {
    if (!selected) return;
    const timer = window.setInterval(() => {
      const time = Date.now(); setNow(time);
      setRequests(previous => previous.filter(request => request.expiresAt > time));
    }, 250);
    return () => window.clearInterval(timer);
  }, [selected?.requestId]);
  async function resolve(approved: boolean) {
    if (!selected || submitting.current || selected.expiresAt <= Date.now()) return;
    const current = generation.current; const requestId = selected.requestId;
    submitting.current = true; setPending(true); setError('');
    try {
      await invoke('management_request_resolve', {requestId, approved});
      if (current === generation.current) setRequests(previous => previous.filter(request => request.requestId !== requestId));
    } catch (cause) { if (current === generation.current) setError(messageFrom(cause)); }
    finally { if (current === generation.current) { submitting.current = false; setPending(false); } }
  }
  if (!selected) return null;
  return <Modal title={t('确认命令行请求')} closeDisabled={pending} onClose={() => void resolve(false)}>
    <dl className="path-list"><div><dt>{t('请求者')}</dt><dd>{selected.requester}</dd></div><div><dt>{t('操作')}</dt><dd>{t(selected.action)}</dd></div></dl>
    <p>{t(selected.impact)}</p>
    <p role="status">{t('请在 {seconds} 秒内确认；超时将取消，不执行操作。', {seconds: Math.max(0, Math.ceil((selected.expiresAt - now) / 1000))})}</p>
    {error && <p role="alert" className="task-error">{t(error)}</p>}
    <div className="confirmation-actions"><button type="button" className="refresh-button" disabled={pending} onClick={() => void resolve(false)}>{t('拒绝')}</button><button type="button" className="danger-button" disabled={pending || selected.expiresAt <= Date.now()} onClick={() => void resolve(true)}>{t('确认执行')}</button></div>
  </Modal>;
}
