import { invoke } from '@tauri-apps/api/core';
import { ArrowRight, Copy, FolderOpen, RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { RetainedWorkspaceRoot, TaskRecord, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';

// A submitted operation stays pending until its durable task reaches a final state.
function useTrackedTask(onTask: (task: TaskRecord) => void, onFinished: () => Promise<void>) {
  const [task, setTask] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const callbacks = useRef({ onTask, onFinished }); callbacks.current = { onTask, onFinished };
  const completed = useRef(new Set<string>());
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const track = useCallback((value: TaskRecord) => {
    if (!mounted.current) return;
    setTask(value); setError(''); callbacks.current.onTask(value);
    if (value.final && !completed.current.has(value.taskId)) {
      completed.current.add(value.taskId);
      void callbacks.current.onFinished().catch((cause) => { if (mounted.current) setError(`刷新操作结果失败：${messageFrom(cause)}`); });
    }
  }, []);
  useEffect(() => {
    if (!task || task.final) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const value = await invoke<TaskRecord>('task_get', { taskId: task!.taskId });
        if (cancelled) return;
        track(value);
        if (!value.final) timer = setTimeout(() => void poll(), 1000);
      } catch (cause) {
        if (cancelled) return;
        setError(`任务状态暂未读取，操作仍可能进行中：${messageFrom(cause)}`);
        timer = setTimeout(() => void poll(), 2000);
      }
    }
    timer = setTimeout(() => void poll(), 500);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [task?.taskId, task?.final, track]);
  return { task, track, error, running: Boolean(task && !task.final) };
}

const retainedRecord = (value: string | RetainedWorkspaceRoot): RetainedWorkspaceRoot => typeof value === 'string' ? { path: value } : value;

export interface WorkspaceLocationProps {
  workspace: WorkspaceInfo | null;
  dataRoot?: string | null;
  enabled: boolean;
  active: boolean;
  migrating: boolean;
  onTask: (task: TaskRecord) => void;
  onRefresh: () => Promise<void>;
  onOpenHistory?: () => void;
}

// Settings › 存储位置: where things live, and moving the whole workspace.
export function WorkspaceLocation({ workspace, dataRoot, enabled, active, migrating, onTask, onRefresh, onOpenHistory }: WorkspaceLocationProps) {
  const [expanded, setExpanded] = useState(false);
  const [destination, setDestination] = useState('');
  const [confirmedPath, setConfirmedPath] = useState<string | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const operation = useRef(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const inputId = useId();
  const tracker = useTrackedTask(onTask, onRefresh);
  const blocked = !enabled || !active || migrating || pending || choosing || tracker.running;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; generation.current++; }; }, []);
  useEffect(() => { if (!active) { generation.current++; setChoosing(false); setConfirmedPath(null); setAcknowledged(false); } }, [active]);
  function editDestination(value: string) { setDestination(value); setConfirmedPath(null); setAcknowledged(false); }
  async function choose() {
    if (blocked || operation.current) return;
    const sequence = ++generation.current; setChoosing(true); setError('');
    try { const value = await invoke<string | null>('choose_path', { kind: 'workspace' }); if (sequence === generation.current && value) editDestination(value); }
    catch (cause) { if (sequence === generation.current) setError(`路径选择失败，可直接填写绝对路径：${messageFrom(cause)}`); }
    finally { if (sequence === generation.current) setChoosing(false); }
  }
  async function migrate() {
    if (blocked || operation.current || !acknowledged || confirmedPath !== destination.trim() || !confirmedPath?.startsWith('/')) return;
    operation.current = true; setPending(true); setError('');
    try { const task = await invoke<TaskRecord>('workspace_migrate', { input: { destinationPath: confirmedPath } }); if (mounted.current) tracker.track(task); else onTask(task); }
    catch (cause) { if (mounted.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (mounted.current) setPending(false); }
  }
  async function refresh() { setRefreshing(true); setError(''); try { await onRefresh(); } catch (cause) { setError(messageFrom(cause)); } finally { setRefreshing(false); } }
  async function copyPath(value: string | null | undefined, label: string) {
    if (!value) return;
    try { await navigator.clipboard.writeText(value); setNotice(`${label}路径已复制。`); } catch { setError('无法复制，请选择显示的路径手动复制。'); }
  }
  const records = (workspace?.retainedRoots ?? []).map(retainedRecord);
  const presentRecords = records.filter((record) => record.absent !== true);
  const step = tracker.running || pending ? 3 : confirmedPath ? 2 : 1;
  return <section className="lifecycle-card workspace-location-card" aria-labelledby="workspace-location-title">
    <div className="section-heading"><div><h2 id="workspace-location-title">存储位置</h2><p>软件管理的内容保存在下面两个位置。</p></div><button className="refresh-button" disabled={!enabled || refreshing || pending} onClick={() => void refresh()}><RefreshCw size={14} />{refreshing ? '刷新中…' : '刷新状态'}</button></div>
    <div className="settings-location-row"><div><h3>项目工作区</h3><p>所有网页项目的源码、数据库和文件保存在这里。</p><code>{workspace?.workspaceRoot ?? '尚未读取'}</code>{workspace?.filesystem && <small>文件系统 {workspace.filesystem}</small>}</div><button className="refresh-button" disabled={!workspace} onClick={() => void copyPath(workspace?.workspaceRoot, '工作区')}><Copy size={14} />复制路径</button></div>
    <div className="settings-location-row"><div><h3>软件数据目录</h3><p>存放 Node 应用、软件设置和运行记录。迁移工作区时不会移动。</p><code>{dataRoot ?? '尚未读取'}</code></div><button className="refresh-button" disabled={!dataRoot} onClick={() => void copyPath(dataRoot, '数据目录')}><Copy size={14} />复制路径</button></div>
    <div className="settings-migration">
      <div className="settings-migration-heading"><div><h3>迁移工作区</h3><p>把所有网页项目移到新位置，比如外置磁盘。原目录会保留，之后可以在“空间 › 历史资料”里清理。</p></div><button className="refresh-button" disabled={blocked} aria-expanded={expanded} aria-controls={`${inputId}-migration`} onClick={() => { setExpanded(!expanded); setConfirmedPath(null); setAcknowledged(false); }}><FolderOpen size={14} />{expanded ? '收起' : '迁移工作区'}</button></div>
      {expanded && <div className="workspace-migration-form" id={`${inputId}-migration`}>
        <ol className="settings-steps" aria-label="迁移步骤">{['选择位置', '核对', '迁移中'].map((label, index) => <li key={label} aria-current={step === index + 1 ? 'step' : undefined}>{index + 1} {label}</li>)}</ol>
        <form onSubmit={(event) => { event.preventDefault(); if (!blocked && destination.trim().startsWith('/')) { setConfirmedPath(destination.trim()); setAcknowledged(false); } }}>
          <div className="path-input"><label htmlFor={inputId}>新的工作区位置</label><div className="path-input-controls"><input id={inputId} value={destination} disabled={blocked} onChange={(event) => editDestination(event.target.value)} placeholder="/Volumes/磁盘/新工作区" autoComplete="off" /><button className="refresh-button" type="button" disabled={blocked} onClick={() => void choose()}><FolderOpen size={14} />{choosing ? '选择中…' : '选择…'}</button></div></div>
          <div className="settings-migration-notes"><div><strong>位置要求</strong><p>空文件夹、可写入、APFS 格式，在本机或直连磁盘上。</p></div><div><strong>开始前请</strong><p>保存各项目窗口的草稿，关闭正在访问项目的 AI 或编辑器。迁移会短暂暂停服务。</p></div></div>
          <button className="refresh-button" type="submit" disabled={blocked || !destination.trim().startsWith('/')}>下一步：核对<ArrowRight size={14} /></button>
        </form>
        {confirmedPath && <div className="operation-confirmation workspace-migration-confirmation"><strong>确认迁移整个工作区</strong><p>来源：<code>{workspace?.workspaceRoot}</code><br />目标：<code>{confirmedPath}</code></p><p>迁移包含所有网页项目；原目录会保留。成功后需要从平台重新进入项目，并重新复制 AI 路径。</p><label className="checkbox-label"><input type="checkbox" checked={acknowledged} disabled={blocked} onChange={(event) => setAcknowledged(event.target.checked)} />我已核对来源和目标，并处理各窗口草稿与外部写入</label><div className="confirmation-actions"><button className="primary-button" disabled={blocked || !acknowledged} onClick={() => void migrate()}>{pending ? '提交中…' : '确认开始迁移'}</button><button className="refresh-button" disabled={pending || tracker.running} onClick={() => { setConfirmedPath(null); setAcknowledged(false); }}>取消</button></div></div>}
      </div>}
    </div>
    {presentRecords.length > 0 && <div className="settings-history-notice"><span>有 {presentRecords.length} 个迁移前的旧工作区还没清理。</span>{onOpenHistory && <button className="refresh-button" onClick={onOpenHistory}>去清理</button>}</div>}
    <details className="workspace-disclosure"><summary>技术信息与迁移记录</summary><div className="workspace-disclosure-body"><dl className="path-list"><div className="path-row"><dt>工作区格式</dt><dd>{workspace?.formatVersion ?? '未知'}</dd></div><div className="path-row"><dt>文件系统</dt><dd>{workspace?.filesystem ?? '尚未读取'}</dd></div></dl>{records.length ? <div className="retained-roots"><h3>迁移原目录记录</h3><p>这里显示迁移前的位置及当前是否可读取。平台使用上方当前工作区，不会自动切回原目录。</p>{records.map((record, index) => {
      const absent = record.absent === true;
      return <article className={`retained-root-record${absent ? ' is-absent' : ''}`} key={record.taskId ?? index}><strong>{absent ? '目录当前不存在' : '保留目录'}</strong><code>{record.path ?? record.sourcePath ?? '原目录路径未记录'}</code><p>{absent ? '原磁盘上的此路径当前不存在；平台不保留删除历史，也不据此认定是谁删除了目录。' : record.available === false ? '当前无法读取此原目录，请在“空间 › 历史资料”检查目录状态；这条记录本身不能证明目录仍然存在或占用空间。' : '此目录尚未清理。可在“空间 › 历史资料”扫描并核对。'}</p>{record.reason && <details><summary>查看记录说明</summary><p>{record.reason}</p></details>}</article>;
    })}</div> : <p>没有迁移原目录记录。</p>}</div></details>
    {tracker.running && <p className="workspace-operation-status" role="status">迁移任务已提交，正在等待最终结果。请勿重复提交。</p>}
    {(error || tracker.error) && <p role="alert" className="task-error">{error || tracker.error}</p>}{notice && <p role="status" className="workspace-inline-notice">{notice}</p>}
  </section>;
}
