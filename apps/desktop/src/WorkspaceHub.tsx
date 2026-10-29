import { invoke } from '@tauri-apps/api/core';
import { RefreshCw } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { SpaceProjects, nodeRowFor, projectRegenerable, projectTotal } from './SpaceProjects';
import { StorageScopeDetails } from './StoragePanels';
import type { WorkspaceStorage } from './StoragePanels';
import { WorkspaceCleanupPanel } from './WorkspaceCleanupPanel';
import { useHistoryScan } from './history-scan-store';
import type { HistoryScanSummary } from './history-scan-store';
import type { NodeAppDetail } from './node-app-types';
import { bigBytes, bigText, partBytes, percentOf, timeText } from './storage-utils';
import type { ProjectInfo, TaskRecord, WorkspaceInfo } from './types';
import { messageFrom } from './ui-utils';
import { deriveWorkspaceTaskState, mergeLifecycleTask } from './workspace-task-state';

export type SpaceTab = 'overview' | 'projects' | 'history';
interface WorkspaceHubProps {
  workspace: WorkspaceInfo | null;
  revision: number;
  enabled: boolean;
  migrating?: boolean;
  paused?: boolean;
  cleanupTask?: TaskRecord | null;
  projects: ProjectInfo[];
  nodeApps?: NodeAppDetail[];
  tab?: SpaceTab;
  onTabChange?: (tab: SpaceTab) => void;
  isProjectBusy?: (project: ProjectInfo) => boolean;
  onTask: (task: TaskRecord) => void;
  onProjectTask?: (task: TaskRecord, project: ProjectInfo) => void;
  onRefresh: () => Promise<void>;
  onProjectDetails?: (project: ProjectInfo) => void;
  onProjectDelete?: (project: ProjectInfo) => void;
  onNodeDetails?: (app: NodeAppDetail) => void;
  onNodeDelete?: (app: NodeAppDetail) => void;
  onOpenSettings?: (section: 'uninstall') => void;
}
const hubTabs = [
  { id: 'overview' as const, label: '概览' },
  { id: 'projects' as const, label: '按项目' },
  { id: 'history' as const, label: '历史资料' },
];
const ignore = () => {};

export function WorkspaceHub(props: WorkspaceHubProps) {
  const [ownTab, setOwnTab] = useState<SpaceTab>('overview');
  const tab = props.tab ?? ownTab;
  const setTab = (value: SpaceTab) => { setOwnTab(value); props.onTabChange?.(value); };
  const [storage, setStorage] = useState<WorkspaceStorage | null>(null);
  const [storageBusy, setStorageBusy] = useState(false);
  const [storageError, setStorageError] = useState('');
  const [bulkRequest, setBulkRequest] = useState(0);
  const storageRequest = useRef(0);
  const [knownCleanupTasks, setKnownCleanupTasks] = useState<Record<string, TaskRecord>>(() => {
    const task = props.cleanupTask ?? props.workspace?.cleanupTask;
    return task ? { [task.taskId]: task } : {};
  });
  const [cleanupPending, setCleanupPending] = useState(false);
  const alive = useRef(true);
  const taskCallback = useRef(props.onTask); taskCallback.current = props.onTask;
  const taskFromWorkspace = props.cleanupTask ?? props.workspace?.cleanupTask;
  useEffect(() => { alive.current = true; return () => { alive.current = false; storageRequest.current++; }; }, []);
  useEffect(() => { if (taskFromWorkspace) {
    setKnownCleanupTasks((current) => mergeLifecycleTask(current, taskFromWorkspace));
  } }, [taskFromWorkspace]);
  const trackCleanupTask = useCallback((task: TaskRecord) => {
    if (alive.current && task.type === 'workspaceCleanup') {
      setKnownCleanupTasks((current) => mergeLifecycleTask(current, task));
    }
    taskCallback.current(task);
  }, []);
  const id = useId();
  const taskState = deriveWorkspaceTaskState(props.workspace && taskFromWorkspace ? { ...props.workspace, cleanupTask: taskFromWorkspace } : props.workspace, knownCleanupTasks);
  const migrating = props.migrating ?? taskState.migrating;
  const paused = props.paused ?? (props.migrating === undefined ? taskState.paused : false);
  const workspaceBlocked = migrating || paused;
  const root = props.workspace?.workspaceRoot ?? 'disconnected';
  const cleanupBusy = cleanupPending || taskState.cleanupBusy;
  const storageEnabled = props.enabled && !workspaceBlocked;
  const readStorage = useCallback(async () => {
    if (!storageEnabled) return;
    const sequence = ++storageRequest.current; setStorageBusy(true);
    try { const value = await invoke<WorkspaceStorage>('workspace_storage'); if (sequence === storageRequest.current) { setStorage(value); setStorageError(''); } }
    catch (cause) { if (sequence === storageRequest.current) setStorageError(messageFrom(cause)); }
    finally { if (sequence === storageRequest.current) setStorageBusy(false); }
  }, [storageEnabled]);
  useEffect(() => { void readStorage(); }, [readStorage, props.revision, root]);
  const history = useHistoryScan(props.workspace?.workspaceRoot ?? null);
  function activateTab(value: SpaceTab) {
    setTab(value);
    document.getElementById(`${id}-${value}-tab`)?.focus();
  }
  function navigateTabs(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const next = event.key === 'ArrowRight' ? (index + 1) % hubTabs.length
      : event.key === 'ArrowLeft' ? (index + hubTabs.length - 1) % hubTabs.length
        : event.key === 'Home' ? 0 : event.key === 'End' ? hubTabs.length - 1 : null;
    if (next === null) return;
    event.preventDefault();
    activateTab(hubTabs[next].id);
  }
  const historyBadge = history && history.itemCount > 0 && !history.stale ? `${history.totalComplete ? '' : '至少 '}${bigText(history.totalBytes)}` : null;
  return <div className="workspace-hub">
    <div className="workspace-hub-tabs" role="tablist" aria-label="空间">
      {hubTabs.map(({ id: value, label }, index) => <button key={value} id={`${id}-${value}-tab`} type="button" role="tab" aria-selected={tab === value} aria-controls={`${id}-${value}-panel`} tabIndex={tab === value ? 0 : -1} className={tab === value ? 'selected' : ''} onClick={() => setTab(value)} onKeyDown={(event) => navigateTabs(event, index)}>{label}{value === 'history' && historyBadge && <span className="history-chip is-warning" aria-label={`，共 ${historyBadge}`}>{historyBadge}</span>}</button>)}
    </div>
    {migrating && <p className="workspace-operation-status" role="status">工作区正在迁移，新操作暂时停用。请等待任务最终结果；已有窗口的草稿会保留。</p>}
    {paused && <p className="workspace-operation-status" role="status">工作区已暂停，请查看迁移任务结果并刷新状态；已有窗口的草稿会保留。</p>}
    {cleanupBusy && <p className="workspace-operation-status" role="status">正在清理历史资料。完成前，删除、迁移和其他清理操作暂时不可用；项目可以正常打开。</p>}
    <div className="workspace-hub-panel" id={`${id}-overview-panel`} role="tabpanel" aria-labelledby={`${id}-overview-tab`} hidden={tab !== 'overview'}>
      <SpaceOverview storage={storage} busy={storageBusy} error={storageError} enabled={storageEnabled} history={history} projects={props.projects} nodeApps={props.nodeApps ?? []} workspaceRoot={props.workspace?.workspaceRoot ?? null} onRescan={() => void readStorage()} onTab={activateTab} onRelease={() => { setTab('projects'); setBulkRequest((value) => value + 1); }} onOpenSettings={props.onOpenSettings} />
    </div>
    <div className="workspace-hub-panel" id={`${id}-projects-panel`} role="tabpanel" aria-labelledby={`${id}-projects-tab`} hidden={tab !== 'projects'}>
      <SpaceProjects projects={props.projects} nodeApps={props.nodeApps ?? []} nodeRows={storage?.nodeApps} enabled={props.enabled && !workspaceBlocked && !cleanupBusy} isProjectBusy={props.isProjectBusy} bulkRequest={bulkRequest} onProjectTask={props.onProjectTask ?? ignore} onProjectDetails={props.onProjectDetails ?? ignore} onProjectDelete={props.onProjectDelete ?? ignore} onNodeDetails={props.onNodeDetails ?? ignore} onNodeDelete={props.onNodeDelete ?? ignore} />
    </div>
    <div className="workspace-hub-panel" id={`${id}-history-panel`} role="tabpanel" aria-labelledby={`${id}-history-tab`} hidden={tab !== 'history'}>
      <WorkspaceCleanupPanel key={root} workspaceRoot={props.workspace?.workspaceRoot ?? null} revision={props.revision} enabled={props.enabled} active={tab === 'history'} autoScan migrating={workspaceBlocked} cleanupTask={taskState.cleanupTask} onTask={trackCleanupTask} onRefresh={props.onRefresh} onPendingChange={setCleanupPending} />
    </div>
  </div>;
}

interface Group { key: string; label: string; badge: string; description: string; bytes: bigint | null; color: string }
function SpaceOverview({ storage, busy, error, enabled, history, projects, nodeApps, workspaceRoot, onRescan, onTab, onRelease, onOpenSettings }: {
  storage: WorkspaceStorage | null; busy: boolean; error: string; enabled: boolean; history: HistoryScanSummary | null;
  projects: ProjectInfo[]; nodeApps: NodeAppDetail[]; workspaceRoot: string | null;
  onRescan: () => void; onTab: (tab: SpaceTab) => void; onRelease: () => void; onOpenSettings?: (section: 'uninstall') => void;
}) {
  const total = bigBytes(partBytes(storage?.knownLocalTotal));
  const webBytes = bigBytes(partBytes((storage?.workspace.categories as Record<string, import('./types').StoragePart> | undefined)?.projects));
  const nodeRows = storage?.nodeApps ?? [];
  const nodeBytes = nodeRows.reduce<bigint | null>((sum, row) => { const bytes = bigBytes(partBytes(row)); return sum === null || bytes === null ? null : sum + bytes; }, 0n);
  const regenerableValues = projects.map(projectRegenerable);
  const regenerable = regenerableValues.reduce<bigint>((sum, value) => sum + (value ?? 0n), 0n);
  const unscanned = regenerableValues.filter((value) => value === null).length;
  const usableHistory = history && !history.stale ? history : null;
  const historyBytes = usableHistory ? usableHistory.totalBytes : null;
  // Old Node versions sit inside app folders, so they move from projects to history.
  const projectBytes = webBytes === null || nodeBytes === null ? null : clamp(webBytes + nodeBytes - regenerable - (usableHistory?.nodeGenerationBytes ?? 0n));
  const software = total === null || projectBytes === null ? null : clamp(total - projectBytes - regenerable - (historyBytes ?? 0n));
  const releasable = regenerable + (usableHistory?.cleanableBytes ?? 0n);
  const disk = storage?.disks?.find((value) => workspaceRoot && String(value.exampleRoot ?? '').startsWith(workspaceRoot)) ?? storage?.disks?.[0];
  const groups: Group[] = [
    { key: 'projects', label: '我的项目', badge: '正在使用', description: `${projects.length} 个网页项目和 ${nodeApps.length} 个 Node 应用的当前内容`, bytes: projectBytes, color: 'mine' },
    { key: 'regenerable', label: '可重新生成', badge: '可直接释放', description: `开发依赖、构建缓存和日志，下次构建会自动重新生成。${unscanned ? `另有 ${unscanned} 个项目尚未统计。` : ''}`, bytes: regenerable, color: 'regenerable' },
    { key: 'history', label: '历史资料', badge: '确认后可释放', description: usableHistory ? '网页项目取回数据或源码前保留的原件。' : history?.stale ? '清理后数字已变化，请在“历史资料”重新扫描。' : '尚未扫描。打开“历史资料”会自动扫描，扫描不会删除文件。', bytes: historyBytes, color: 'history' },
    { key: 'software', label: '软件本身与记录', badge: '不在这里处理', description: `程序文件、软件设置、运行记录与共享缓存${usableHistory ? '' : '（含尚未扫描的历史资料）'}。`, bytes: software, color: 'software' },
  ];
  // Unscanned history stays inside the software remainder, so the bar is still whole.
  const ratios = total !== null && total > 0n && groups.every((group) => group.bytes !== null || group.key === 'history');
  const barLabel = `占用分布：${groups.filter((group) => group.bytes !== null).map((group) => `${group.label} ${bigText(group.bytes)}`).join('，')}`;
  const ranked = [
    ...projects.map((project) => ({ key: `web:${project.id}`, name: project.name, kind: '网页项目', total: projectTotal(project), regenerable: projectRegenerable(project) })),
    ...nodeApps.map((app) => ({ key: `node:${app.id}`, name: app.name, kind: 'Node 应用', total: bigBytes(partBytes(nodeRowFor(nodeRows, app.id))), regenerable: null as bigint | null })),
  ].filter((row) => row.total !== null).sort((left, right) => left.total! > right.total! ? -1 : left.total! < right.total! ? 1 : 0).slice(0, 3);
  const scannedAt = storage?.workspace.completedAt ?? storage?.knownLocalTotal?.completedAt;
  return <>
    <section className="lifecycle-card space-summary" aria-labelledby="space-summary-total">
      <div className="space-summary-heading">
        <dl className="space-totals">
          <div><dt>共占用</dt><dd id="space-summary-total">{bigText(total)}</dd>{storage?.knownLocalTotal && !storage.knownLocalTotal.complete && <small>部分统计</small>}</div>
          <div><dt>可释放</dt><dd className="is-releasable">约 {bigText(releasable)}</dd><small>{bigText(regenerable)} 可直接释放 · {usableHistory ? `${bigText(usableHistory.cleanableBytes)} 需确认` : '历史资料尚未扫描'}</small></div>
          <div><dt>磁盘剩余</dt><dd>{bigText(bigBytes((disk?.availableBytes ?? null) as import('./types').ByteCount))}</dd></div>
        </dl>
        <div className="space-scan-status">{busy ? <span role="status">正在统计…</span> : scannedAt ? <span>统计于 {timeText(scannedAt as string | number)}</span> : null}<button className="refresh-button" disabled={!enabled || busy} onClick={onRescan}><RefreshCw size={14} />{busy ? '统计中…' : '重新统计'}</button></div>
      </div>
      <div className={`space-bar${ratios ? '' : ' is-incomplete'}`} role="img" aria-label={ratios ? barLabel : '占用分布暂不可用，需要完整的统计结果'}>{ratios && groups.filter((group) => group.bytes !== null).map((group) => <span key={group.key} className={`space-color-${group.color}`} style={{ width: `${percentOf({ $int64: group.bytes!.toString() }, { $int64: total!.toString() }) ?? 0}%` }} />)}</div>
      <div className="space-groups">{groups.map((group) => <div className="space-group" key={group.key}>
        <i aria-hidden="true" className={`space-color-${group.color}`} />
        <div><div className="space-group-title"><strong>{group.label}</strong><span className={`history-chip is-${group.color}`}>{group.badge}</span></div><p>{group.description}</p></div>
        <strong className="space-number">{group.key === 'history' ? usableHistory ? `${usableHistory.totalComplete ? '' : '至少 '}${bigText(group.bytes)}` : '尚未扫描' : bigText(group.bytes)}</strong>
        <div className="space-group-action">
          {group.key === 'projects' && <button className="refresh-button" onClick={() => onTab('projects')}>查看项目</button>}
          {group.key === 'regenerable' && <button className="primary-button" disabled={!enabled || regenerable === 0n} onClick={onRelease}>{regenerable === 0n ? '暂无可释放' : `释放 ${bigText(regenerable)}`}</button>}
          {group.key === 'history' && <button className="refresh-button" onClick={() => onTab('history')}>查看并清理</button>}
          {group.key === 'software' && onOpenSettings && <button className="text-button" onClick={() => onOpenSettings('uninstall')}>卸载时处理</button>}
        </div>
      </div>)}</div>
    </section>
    <section className="lifecycle-card space-top-projects" aria-labelledby="space-top-title">
      <div className="space-top-heading"><h2 id="space-top-title">占用最多的项目</h2><button className="text-button" onClick={() => onTab('projects')}>查看全部 {projects.length + nodeApps.length} 个</button></div>
      {ranked.length ? ranked.map((row) => <div className="space-top-row" key={row.key}><span><strong>{row.name}</strong> <span className="history-chip">{row.kind}</span></span><span className="space-top-regen">{row.regenerable ? `可释放 ${bigText(row.regenerable)}` : ''}</span><strong className="space-number">{bigText(row.total)}</strong></div>) : <p>还没有统计结果。可在“按项目”中统计全部项目。</p>}
    </section>
    <details className="workspace-disclosure space-measurement"><summary>统计说明与各位置明细</summary><div className="workspace-disclosure-body"><p>按文件大小统计，可能与磁盘实际占用略有差异。“可释放”只是估计；实际释放量以清理任务结果为准。</p><StorageScopeDetails data={storage} /></div></details>
    {error && <p role="alert" className="environment-issue">{error}</p>}
  </>;
}
const clamp = (value: bigint) => value < 0n ? 0n : value;
