import { invoke } from '@tauri-apps/api/core';
import { RotateCcw } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { messageFrom } from './ui-utils';

interface Preview {
  appId: string;
  canRollback: boolean;
  name?: string;
  currentVersion?: string;
  previousVersion?: string;
  currentGeneration?: string;
  previousGeneration?: string;
  expectedRevision?: string;
}
function checked(value: Preview, appId: string): Preview | null {
  if (!value || value.appId !== appId || value.canRollback !== true
    || !/^[a-f0-9]{64}$/.test(value.expectedRevision ?? '')
    || !/^gen_[a-f0-9]{32}$/.test(value.currentGeneration ?? '')
    || !/^gen_[a-f0-9]{32}$/.test(value.previousGeneration ?? '')
    || value.currentGeneration === value.previousGeneration) return null;
  return value;
}

export function NodeRollbackButton({ appId, instanceId, disabled, onApplied }: {
  appId: string; instanceId?: string; disabled: boolean; onApplied: () => void;
}) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState('');
  const alive = useRef(false);
  const sequence = useRef(0);
  const operation = useRef(false);
  const callback = useRef(onApplied); callback.current = onApplied;
  const current = (request: number) => alive.current && sequence.current === request;
  useEffect(() => {
    alive.current = true; const request = ++sequence.current;
    operation.current = false; setOpen(false); setPending(false); setPreview(null); setError(''); setUncertain(false);
    if (!disabled) void invoke<Preview>('node_program_rollback_preview', { appId }).then(value => {
      if (current(request)) setPreview(checked(value, appId));
    }).catch(() => { /* The main startup error remains visible; no valid old version means no button. */ });
    return () => { alive.current = false; sequence.current++; };
  }, [appId, instanceId, disabled]);

  async function refresh() {
    if (operation.current || disabled) return;
    const request = sequence.current; operation.current = true; setPending(true); setError('');
    try {
      const value = await invoke<Preview>('node_program_rollback_preview', { appId });
      if (!current(request)) return;
      const next = checked(value, appId); setPreview(next);
      if (next) setUncertain(false);
      else { setUncertain(true); setError('当前没有可继续退回的旧程序。请回到应用详情核对当前版本。'); }
    } catch (cause) { if (current(request)) setError(messageFrom(cause)); }
    finally { if (current(request)) { operation.current = false; setPending(false); } }
  }
  async function apply() {
    if (operation.current || disabled || uncertain || !preview) return;
    const reviewed = preview; const request = sequence.current;
    operation.current = true; setPending(true); setError('');
    try {
      const result = await invoke<{ appId: string; operationOutcome: string; generation: string; dataChanged: boolean; configurationChanged: boolean; started: boolean }>('node_program_rollback_apply', { appId, input: { expectedRevision: reviewed.expectedRevision } });
      if (!current(request)) return;
      if (result.appId !== appId || result.operationOutcome !== 'committed' || result.generation !== reviewed.previousGeneration
        || result.dataChanged !== false || result.configurationChanged !== false || result.started !== false) throw new Error('退回结果还需要核对，请勿重复操作。');
      setOpen(false); setPreview(null); callback.current();
    } catch (cause) {
      if (!current(request)) return;
      setUncertain(true); setError(`退回结果需要重新核对：${messageFrom(cause)}`);
    } finally { if (current(request)) { operation.current = false; setPending(false); } }
  }
  if (!preview && !open) return null;
  return <><button type="button" className="refresh-button" disabled={disabled || pending || !preview} onClick={() => { setOpen(true); setError(''); }}><RotateCcw size={14} />退回上一个版本</button>
    {open && <Modal title="退回上一个程序版本？" description="仅切换程序版本，当前数据和设置继续保留。" onClose={() => { if (!pending) setOpen(false); }} closeDisabled={pending} className="node-program-rollback-modal">
      <p>{preview?.name ?? '此应用'}：{preview?.currentVersion ?? '当前版本'} → {preview?.previousVersion ?? '上一个版本'}。</p>
      <p>这会退回应用程序，不会把记录、附件或设置恢复到旧时点，也不会修改源码。即使数据声明相同，旧程序能否读取当前数据仍需实际核对。</p>
      <p>退回后不会自动打开应用。</p>
      {error && <p className="task-error" role="alert">{error}</p>}
      <div className="confirmation-actions">{uncertain ? <button type="button" className="primary-button" disabled={pending || disabled} onClick={() => void refresh()}>重新核对</button> : <button type="button" className="primary-button" disabled={pending || disabled || !preview} onClick={() => void apply()}>{pending ? '正在退回程序…' : '确认退回程序'}</button>}<button type="button" className="refresh-button" disabled={pending} onClick={() => setOpen(false)}>取消</button></div>
    </Modal>}
  </>;
}
