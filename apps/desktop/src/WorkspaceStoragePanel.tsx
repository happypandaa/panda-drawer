import { invoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import { WorkspaceLocation } from './WorkspaceLocation';
import { RegenerableCleanupPanel } from './RegenerableCleanupPanel';
import type { NodeAppDetail } from './node-app-types';
import { bigBytes, bigText, percentOf } from './storage-utils';
import type { ProjectInfo, TaskRecord, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';
import { applicationStorageRows, projectRegenerable } from './workspace-storage-data';
import type { WorkspaceStorage } from './workspace-storage-data';

export interface WorkspaceStoragePanelProps {
  workspace: WorkspaceInfo | null;
  instanceId?: string;
  dataRoot?: string;
  revision: number;
  enabled: boolean;
  active: boolean;
  migrating: boolean;
  projects: ProjectInfo[];
  nodeApps: NodeAppDetail[];
  isProjectBusy?: (project: ProjectInfo) => boolean;
  onTask: (task: TaskRecord) => void;
  onProjectTask: (task: TaskRecord, project: ProjectInfo) => void;
  onRefresh: () => Promise<void>;
  onPendingChange?: (pending: boolean) => void;
}
export function WorkspaceStoragePanel(props: WorkspaceStoragePanelProps) {
  const [storage, setStorage] = useState<WorkspaceStorage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [expanded, setExpanded] = useState(false);
  const request = useRef(0);
  const root = props.workspace?.workspaceRoot ?? null;
  const enabled = props.enabled && props.active && !props.migrating;
  const read = useCallback(async () => {
    if (!enabled) return;
    const sequence = ++request.current; setBusy(true);
    try { const value = await invoke<WorkspaceStorage>('workspace_storage'); if (sequence === request.current) { setStorage(value); setError(''); } }
    catch (cause) { if (sequence === request.current) setError(messageFrom(cause)); }
    finally { if (sequence === request.current) setBusy(false); }
  }, [enabled, root, props.instanceId]);
  useEffect(() => { void read(); return () => { request.current++; }; }, [read, props.revision]);
  useEffect(() => { setStorage(null); setExpanded(false); }, [root, props.instanceId]);
  const rows = applicationStorageRows(storage, props.projects, props.nodeApps);
  const total = rows.reduce((sum, row) => sum + (row.bytes ?? 0n), 0n);
  const incomplete = rows.some(row => row.bytes === null || !row.complete);
  const measured = rows.length === 0 || rows.some(row => row.bytes !== null);
  const regenerable = props.projects.map(projectRegenerable);
  const temporary = regenerable.reduce<bigint>((sum, value) => sum + (value ?? 0n), 0n);
  const releasable = temporary;
  const disk = storage?.disks?.find(value => root && value.exampleRoot?.startsWith(root)) ?? storage?.disks?.[0];
  const maximum = rows.find(row => row.bytes !== null)?.bytes ?? 0n;
  return <section className="settings-section workspace-storage-panel" aria-labelledby="settings-storage-title">
    <h3 id="settings-storage-title">存储与清理</h3>
    <WorkspaceLocation workspace={props.workspace} dataRoot={props.dataRoot} enabled={props.enabled} active={props.active} migrating={props.migrating} onTask={props.onTask} onRefresh={props.onRefresh} />
    <section className="lifecycle-card space-summary" aria-label="应用空间统计">
      <dl className="space-totals">
        <div><dt>应用共占用</dt><dd>{measured ? `${incomplete ? '至少 ' : ''}${bigText(total)}` : '尚未统计'}</dd>{incomplete && <small>部分统计</small>}</div>
        <div><dt>可以清理</dt><dd>约 {bigText(releasable)}</dd><small>{regenerable.some(value => value === null) ? '部分应用尚未统计 · ' : ''}可重新生成的依赖、缓存和日志</small></div>
        <div><dt>磁盘剩余</dt><dd>{bigText(bigBytes(disk?.availableBytes ?? props.workspace?.availableBytes))}</dd></div>
      </dl>
      {busy && <p role="status">正在统计应用占用…</p>}
      <p className="space-note">按文件逻辑大小统计；可以清理的大小是估计，实际删除量以任务结果为准。</p>
      {error && <p className="task-error" role="alert">{error}</p>}
    </section>
    <RegenerableCleanupPanel instanceId={props.instanceId} workspaceRoot={root} projects={props.projects} enabled={enabled} isProjectBusy={props.isProjectBusy} onProjectTask={props.onProjectTask} onRefresh={async () => { await props.onRefresh(); await read(); }} onPendingChange={props.onPendingChange} />
    <section className="lifecycle-card space-top-projects" aria-labelledby="storage-apps-title">
      <div className="space-top-heading"><h2 id="storage-apps-title">各应用占用</h2>{rows.length > 5 && <button type="button" className="text-button" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? '收起' : `查看全部 ${rows.length} 个`}</button>}</div>
      {(expanded ? rows : rows.slice(0, 5)).map(row => <div className="space-top-row storage-application-row" key={row.key}>
        <span className="space-project-name">{row.icon ? <img src={row.icon} alt="" width={24} height={24} /> : <span className="radio-app-icon" aria-hidden="true">{row.name.trim().slice(0, 1)}</span>}<strong>{row.name}</strong></span>
        <span className="space-bar" role="img" aria-label={`${row.name}：${bigText(row.bytes)}${row.complete ? '' : '，部分统计'}`}><span className="space-color-mine" style={{ width: `${percentOf(row.bytes === null ? null : { $int64: row.bytes.toString() }, { $int64: maximum.toString() }) ?? 0}%` }} /></span>
        <strong className="space-number">{bigText(row.bytes)}{!row.complete && row.bytes !== null && <small>部分统计</small>}</strong>
      </div>)}
      {!rows.length && <p>还没有应用。</p>}
    </section>
  </section>;
}
