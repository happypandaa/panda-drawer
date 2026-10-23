import { invoke } from '@tauri-apps/api/core';
import { FolderOpen } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { messageFrom } from './ui-utils';

export function LocalDirectoryButton({ projectId, disabled }: { projectId: string; disabled: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const pending = useRef(false);
  const lifetime = useRef(0);
  useEffect(() => {
    lifetime.current++; pending.current = false; setBusy(false); setError('');
    return () => { lifetime.current++; };
  }, [projectId]);
  async function open() {
    if (disabled || pending.current) return;
    const generation = lifetime.current;
    pending.current = true; setBusy(true); setError('');
    try { await invoke('project_reveal', { projectId, target: 'root' }); }
    catch (cause) { if (generation === lifetime.current) setError(messageFrom(cause)); }
    finally { if (generation === lifetime.current) { pending.current = false; setBusy(false); } }
  }
  return <><button className="refresh-button" disabled={disabled || busy} onClick={() => void open()}><FolderOpen size={15} />{busy ? '正在打开…' : '打开本地目录'}</button>{error && <p className="task-error detail-directory-error" role="alert">{error}</p>}</>;
}
