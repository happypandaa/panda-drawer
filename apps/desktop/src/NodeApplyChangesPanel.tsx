import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import { AiProjectHelp } from './AiProjectHelp';
import { NodeInstallPanel } from './NodeInstallPanel';
import type { SourceInstallCandidate } from './NodeInstallPanel';
import { ProjectTask } from './ProjectTask';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

/** The temporary managed-copy bridge. It never binds a different source. */
export function NodeApplyChangesPanel({ appId, instanceId, sourcePath, appName, appVersion, platformVersion, runtimeLogPaths, disabled, running, onPendingChange, onTask, onInstalled }: {
  appId: string; instanceId?: string; sourcePath?: string; appName?: string; appVersion?: string; platformVersion?: string;
  runtimeLogPaths?: { current: string; previous: string };
  disabled: boolean; running: boolean;
  onPendingChange: (value: boolean) => void; onTask?: (value: TaskRecord) => void; onInstalled: () => void;
}) {
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [source, setSource] = useState<SourceInstallCandidate | null>(null);
  const [busy, setBusy] = useState(false);
  const [installBusy, setInstallBusy] = useState(false);
  const [error, setError] = useState('');
  const alive = useRef(false);
  const lifetime = useRef(0);
  const submitting = useRef(false);
  const polling = useRef(false);
  const published = useRef(new Set<string>());
  const callbacks = useRef({ onPendingChange, onTask, onInstalled }); callbacks.current = { onPendingChange, onTask, onInstalled };
  const active = Boolean(task && !task.final);
  const locked = disabled || busy || active || installBusy;
  const current = (version: number) => alive.current && lifetime.current === version;
  useEffect(() => {
    alive.current = true; lifetime.current++;
    return () => { alive.current = false; lifetime.current++; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => { callbacks.current.onPendingChange(busy || active || installBusy); }, [busy, active, installBusy]);

  function accept(value: TaskRecord) {
    if (value.type !== 'nodeDevelopmentBuild' || value.projectId !== null) throw new Error('应用修改任务身份不符，请重新核对。');
    setTask(value); callbacks.current.onTask?.(value);
    if (value.final && value.status === 'succeeded' && !published.current.has(value.taskId)) {
      const result = value.result?.source as SourceInstallCandidate | undefined;
      if (value.result?.appId !== appId || !result || result.status !== 'ready' || result.inspection?.kind !== 'node-service-source' || !/^src_[a-f0-9]{32}$/.test(result.sourceId) || !/^[a-f0-9]{64}$/.test(result.contentDigest ?? '')) throw new Error('应用修改结果不完整，请保留现场并核对。');
      published.current.add(value.taskId); setSource(result);
    }
  }
  useEffect(() => {
    if (!task || task.final) return;
    let cancelled = false; const version = lifetime.current;
    const poll = async () => {
      if (cancelled || !current(version) || polling.current) return;
      polling.current = true;
      try {
        const value = await invoke<TaskRecord>('task_get', { taskId: task.taskId });
        if (cancelled || !current(version)) return;
        if (value.taskId !== task.taskId) throw new Error('应用修改任务身份不符，请重新核对。');
        accept(value); setError('');
      } catch (cause) { if (!cancelled && current(version)) setError(messageFrom(cause)); }
      finally { if (current(version)) polling.current = false; }
    };
    void poll(); const timer = window.setInterval(() => void poll(), 500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [task?.taskId, task?.final]);

  async function build() {
    if (locked || running || submitting.current) return;
    submitting.current = true; setBusy(true); setSource(null); setTask(null); setError('');
    const version = lifetime.current;
    try {
      const value = await invoke<TaskRecord>('node_development_build', { appId });
      if (current(version)) accept(value);
    } catch (cause) { if (current(version)) setError(messageFrom(cause)); }
    finally { if (current(version)) { submitting.current = false; setBusy(false); } }
  }
  async function cancel() {
    if (disabled || busy || submitting.current || !task || task.final) return;
    submitting.current = true; setBusy(true); const version = lifetime.current;
    try { await invoke('node_development_cancel', { taskId: task.taskId }); }
    catch (cause) { if (current(version)) setError(messageFrom(cause)); }
    finally { if (current(version)) { submitting.current = false; setBusy(false); } }
  }
  return <div className="node-apply-changes">
    <div className="confirmation-actions"><button className="refresh-button" disabled={locked || running || Boolean(source)} onClick={() => void build()}>应用修改</button>{active && <button className="text-button" disabled={disabled || busy} onClick={() => void cancel()}>取消应用修改</button>}{source && <button className="text-button" disabled={locked} onClick={() => { setSource(null); setTask(null); setError(''); }}>收起应用修改</button>}</div>
    <p className="detail-section-note">{running ? '先关闭应用，再应用 AI 对项目文件夹的修改。' : '此应用使用安装副本。AI 改完源码后，点“应用修改”，核对并确认后生效。'}</p>
    {error && <p className="task-error" role="alert">{error}</p>}
    {!active && (error || task?.status === 'failed') && <AiProjectHelp allowCodex={false} context={{ mode: 'update-failed', sourceKind: 'managed', sourcePath, appName, appVersion, platformVersion, runtimeLogPaths, issues: [error, task?.error ? messageFrom(task.error) : ''].filter(Boolean) }} label="复制说明给 AI" disabled={locked} />}
    {task && <ProjectTask task={task} />}
    {source && <NodeInstallPanel mode="apply-changes" sources={[source]} targetAppId={appId} instanceId={instanceId} sourcePath={sourcePath} sourceKind="managed" appName={appName} appVersion={appVersion} platformVersion={platformVersion} runtimeLogPaths={runtimeLogPaths} disabled={disabled || busy || active} onPendingChange={setInstallBusy} onTask={value => callbacks.current.onTask?.(value)} onInstalled={() => callbacks.current.onInstalled()} />}
  </div>;
}
