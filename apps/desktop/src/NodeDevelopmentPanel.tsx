import { invoke } from '@tauri-apps/api/core';
import { FolderOpen, Hammer } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { CodexHandoffPanel } from './CodexHandoffPanel';
import type { HandoffAction, ProjectHandoff } from './CodexHandoffPanel';
import { ProjectTask } from './ProjectTask';
import type { SourceInstallCandidate } from './NodeInstallPanel';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';

export interface NodeDevelopmentInfo { sourceRoot: string; runtimeIsSource: boolean; instanceId: string }
let clipboard = Promise.resolve();

export function NodeDevelopmentPanel({ appId, instanceId, installedGeneration, disabled, running, onPendingChange, onPrepared, onTask, onInfo }: {
  appId: string; instanceId?: string; installedGeneration?: string; disabled: boolean; running: boolean;
  onPendingChange: (value: boolean) => void; onPrepared: (value: SourceInstallCandidate) => void; onTask?: (value: TaskRecord) => void; onInfo: (value: NodeDevelopmentInfo) => void;
}) {
  const [info, setInfo] = useState<NodeDevelopmentInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [handoff, setHandoff] = useState<ProjectHandoff | null>(null);
  const [action, setAction] = useState<HandoffAction | null>(null);
  const [feedback, setFeedback] = useState<{ error?: string; notice?: string } | null>(null);
  const alive = useRef(false);
  const pending = useRef(false);
  const polling = useRef(false);
  const generation = useRef(0);
  const infoRead = useRef(0);
  const current = (sequence: number) => alive.current && generation.current === sequence;
  const callbacks = useRef({ onPendingChange, onPrepared, onTask, onInfo }); callbacks.current = { onPendingChange, onPrepared, onTask, onInfo };
  useEffect(() => {
    alive.current = true;
    const sequence = ++generation.current;
    const request = ++infoRead.current;
    pending.current = false; polling.current = false; setBusy(false); setInfo(null); setTask(null); setHandoff(null); setFeedback(null); setAction(null); setError('');
    void invoke<NodeDevelopmentInfo>('node_development_info', { appId }).then(value => { if (current(sequence) && request === infoRead.current) { setInfo(value); callbacks.current.onInfo(value); } }).catch(cause => { if (current(sequence) && request === infoRead.current) setError(messageFrom(cause)); });
    return () => { alive.current = false; generation.current++; callbacks.current.onPendingChange(false); };
  }, [appId, instanceId, installedGeneration]);
  useEffect(() => { callbacks.current.onPendingChange(busy || Boolean(task && !task.final)); }, [busy, task]);
  useEffect(() => {
    if (!task || task.final) return;
    let cancelled = false;
    const sequence = generation.current;
    const poll = async () => {
      if (cancelled || !current(sequence) || polling.current) return;
      polling.current = true;
      try {
        const value = await invoke<TaskRecord>('task_get', { taskId: task.taskId });
        if (cancelled || !current(sequence)) return;
        if (value.taskId !== task.taskId || value.type !== 'nodeDevelopmentBuild') throw new Error('构建任务身份不符，请重新核对。');
        setTask(value); callbacks.current.onTask?.(value); setError('');
        if (value.final && value.status === 'succeeded') {
          const source = value.result?.source as SourceInstallCandidate | undefined;
          if (value.result?.appId !== appId || !source || source.status !== 'ready' || source.inspection?.kind !== 'node-service-source' || !/^src_[a-f0-9]{32}$/.test(source.sourceId) || !/^[a-f0-9]{64}$/.test(source.contentDigest ?? '')) throw new Error('构建结果不完整，请保留现场并核对。');
          callbacks.current.onPrepared(source);
        }
      } catch (cause) { if (!cancelled && current(sequence)) setError(messageFrom(cause)); }
      finally { if (current(sequence)) polling.current = false; }
    };
    void poll(); const timer = window.setInterval(() => void poll(), 500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [task?.taskId, task?.final, appId]);
  async function run(command: 'bind' | 'reveal' | 'build' | 'cancel') {
    if (disabled || pending.current) return;
    const sequence = generation.current;
    pending.current = true; setBusy(true); setError('');
    try {
      if (command === 'bind') {
        const sourcePath = await invoke<string | null>('choose_path', { kind: 'nodeapp' });
        if (!current(sequence) || !sourcePath) return;
        const request = ++infoRead.current;
        await invoke('node_development_bind', { appId, sourcePath });
        if (!current(sequence)) return;
        const value = await invoke<NodeDevelopmentInfo>('node_development_info', { appId });
        if (current(sequence) && request === infoRead.current) { setInfo(value); callbacks.current.onInfo(value); }
      } else if (command === 'reveal') await invoke('node_development_reveal', { appId });
      else if (command === 'build') {
        const value = await invoke<TaskRecord>('node_development_build', { appId });
        if (current(sequence)) {
          if (value.type !== 'nodeDevelopmentBuild') throw new Error('构建任务身份不符，请重新核对。');
          setTask(value); callbacks.current.onTask?.(value); }
      } else if (task) await invoke('node_development_cancel', { taskId: task.taskId });
    } catch (cause) { if (current(sequence)) setError(messageFrom(cause)); }
    finally { if (current(sequence)) { pending.current = false; setBusy(false); } }
  }
  async function prepare(value: HandoffAction) {
    if (disabled || pending.current) return;
    const sequence = generation.current;
    pending.current = true; setBusy(true); setAction(value); setFeedback(null);
    try {
      const data = await invoke<ProjectHandoff>('node_development_handoff', { appId });
      if (!current(sequence)) return;
      if (!data.projectRoot || !data.instanceId || data.instanceId !== instanceId) throw new Error('平台已变化，请重新打开详情。');
      setHandoff(data);
      let copied = false;
      const write = clipboard.then(async () => { if (current(sequence)) { await navigator.clipboard.writeText(data.text); copied = true; } });
      clipboard = write.catch(() => {});
      await write.catch(() => {});
      if (!current(sequence)) return;
      if (value === 'codex') {
        const opened = await invoke<{ projectRoot: string; requested: boolean }>('node_development_open_codex', { appId, expectedProjectRoot: data.projectRoot, expectedInstanceId: data.instanceId });
        if (!opened.requested || opened.projectRoot !== data.projectRoot) throw new Error('无法确认 Codex 已打开。');
      }
      if (current(sequence)) setFeedback({ notice: value === 'codex' ? '已打开 Codex 草稿，请粘贴说明并发送需求。' : copied ? '说明已复制。' : '请手动复制下方说明。' });
    } catch (cause) { if (current(sequence)) setFeedback({ error: messageFrom(cause) }); }
    finally { if (current(sequence)) { pending.current = false; setBusy(false); setAction(null); } }
  }
  const active = Boolean(task && !task.final);
  const locked = disabled || busy || active;
  return <section className="node-development-panel" aria-label="Node 开发工具">
    {error && <p className="task-error" role="alert">{error}</p>}
    {info && <code className="node-development-path">{info.sourceRoot}</code>}
    <div className="project-dialog-actions"><button className="refresh-button" disabled={locked || !info} onClick={() => void run('reveal')}><FolderOpen size={15} />打开源码</button><button className="text-button" disabled={locked} onClick={() => void run('bind')}>{info ? '更换源码目录' : '选择源码目录'}</button><button className="primary-button" disabled={locked || !info || running} onClick={() => void run('build')}><Hammer size={15} />构建并更新</button>{active && <button className="text-button" disabled={disabled || busy} onClick={() => void run('cancel')}>取消构建</button>}</div>
    <p className="project-help">{running ? '先关闭应用，再构建更新。' : info?.runtimeIsSource ? '首次更新会将程序与源码分开，数据和配置保留。' : '构建后核对并应用，当前程序在确认前保持原状。'}</p>
    {task && <ProjectTask task={task} />}
    <CodexHandoffPanel handoff={handoff} action={action} feedback={feedback && (feedback.error ? <p className="task-error" role="alert">{feedback.error}</p> : <p role="status">{feedback.notice}</p>)} disabled={locked || !info || info.runtimeIsSource} onPrepare={value => void prepare(value)} onClear={() => { setHandoff(null); setFeedback(null); }} />
  </section>;
}
