import { invoke } from '@tauri-apps/api/core';
import { RefreshCw, Search } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import type { NodeAppDetail } from './node-app-types';
import { bigBytes, bigText, exactBytes, partBytes, timeText } from './storage-utils';
import type { ProjectInfo, StoragePart, TaskRecord } from './types';
import { messageFrom } from './ui-utils';

// Everything 清理 removes; the same set the project detail page offers.
export const regenerableCategories = ['dependencies', 'buildCache', 'logs'] as const;

// Regenerable bytes from the latest project scan, or null when not measured.
export function projectRegenerable(project: ProjectInfo): bigint | null {
  const current = project.storage?.currentProject;
  if (!current || current.status === 'not_scanned') return null;
  const dependencies = bigBytes(partBytes(current.categories?.dependencies));
  const cache = bigBytes(partBytes(current.categories?.cache));
  return dependencies === null || cache === null ? null : dependencies + cache;
}
export const projectTotal = (project: ProjectInfo) => {
  const current = project.storage?.currentProject;
  return !current || current.status === 'not_scanned' ? null : bigBytes(current.totalBytes);
};
export const nodeRowFor = (rows: StoragePart[] | undefined, appId: string) => rows?.find((row) => (row.scope as Record<string, unknown> | undefined)?.appId === appId);

interface Row {
  key: string;
  kind: 'web' | 'node';
  name: string;
  sub: string;
  total: bigint | null;
  regenerable: bigint | null;
  time: string;
  needsScan: boolean;
  project?: ProjectInfo;
  app?: NodeAppDetail;
}
type Filter = 'all' | 'web' | 'node';
type Sort = 'size' | 'regenerable' | 'name';
export interface SpaceProjectsProps {
  projects: ProjectInfo[];
  nodeApps: NodeAppDetail[];
  nodeRows?: StoragePart[];
  enabled: boolean;
  // A project with a running task, recovery or paused workspace cannot change now.
  isProjectBusy?: (project: ProjectInfo) => boolean;
  bulkRequest?: number;
  onProjectTask: (task: TaskRecord, project: ProjectInfo) => void;
  onProjectDetails: (project: ProjectInfo) => void;
  onProjectDelete: (project: ProjectInfo) => void;
  onNodeDetails: (app: NodeAppDetail) => void;
  onNodeDelete: (app: NodeAppDetail) => void;
}
const pageSize = 20;
const compare = (left: bigint | null, right: bigint | null) => left === right ? 0 : left === null ? 1 : right === null ? -1 : left > right ? -1 : 1;
const sumKnown = (values: (bigint | null)[]) => values.reduce<bigint>((sum, value) => sum + (value ?? 0n), 0n);

export function SpaceProjects(props: SpaceProjectsProps) {
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [sort, setSort] = useState<Sort>('size');
  const [expanded, setExpanded] = useState(false);
  const [cleanTarget, setCleanTarget] = useState<ProjectInfo[] | null>(null);
  const [pending, setPending] = useState<'scan' | 'clean' | null>(null);
  const [notice, setNotice] = useState('');
  const [errors, setErrors] = useState<string[]>([]);
  const mounted = useRef(true);
  const lastBulk = useRef(props.bulkRequest ?? 0);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const busy = (project: ProjectInfo) => !props.enabled || Boolean(pending) || Boolean(props.isProjectBusy?.(project));
  const rows: Row[] = [
    ...props.projects.map((project): Row => {
      const current = project.storage?.currentProject;
      const scanned = Boolean(current && current.status !== 'not_scanned');
      return { key: `web:${project.id}`, kind: 'web', name: project.name, sub: project.id, total: projectTotal(project), regenerable: projectRegenerable(project), time: scanned ? current?.stale ? '需要重新统计' : timeText(current?.completedAt) : '尚未统计', needsScan: !scanned || Boolean(current?.stale), project };
    }),
    ...props.nodeApps.map((app): Row => {
      const row = nodeRowFor(props.nodeRows, app.id);
      return { key: `node:${app.id}`, kind: 'node', name: app.name, sub: app.installation ? '平台安装副本' : '外部目录接入', total: bigBytes(partBytes(row)), regenerable: null, time: row?.completedAt ? timeText(row.completedAt as string | number) : '尚未统计', needsScan: false, app };
    }),
  ];
  const cleanable = props.projects.filter((project) => (projectRegenerable(project) ?? 0n) > 0n && !busy(project));
  const cleanableTotal = sumKnown(cleanable.map(projectRegenerable));
  useEffect(() => {
    const token = props.bulkRequest ?? 0;
    if (token === lastBulk.current) return;
    lastBulk.current = token;
    if (cleanable.length) setCleanTarget(cleanable);
  }, [props.bulkRequest, cleanable]);
  const needle = query.trim().toLocaleLowerCase();
  const visibleRows = rows
    .filter((row) => filter === 'all' || row.kind === filter)
    .filter((row) => !needle || `${row.name} ${row.sub}`.toLocaleLowerCase().includes(needle))
    .sort((left, right) => sort === 'name' ? left.name.localeCompare(right.name, 'zh-CN') : compare(sort === 'size' ? left.total : left.regenerable, sort === 'size' ? right.total : right.regenerable) || left.name.localeCompare(right.name, 'zh-CN'));
  const shown = expanded ? visibleRows : visibleRows.slice(0, pageSize);
  const webCount = props.projects.length; const nodeCount = props.nodeApps.length;
  async function scanAll() {
    const targets = props.projects.filter((project) => !busy(project));
    if (!targets.length || pending) return;
    setPending('scan'); setNotice(''); setErrors([]);
    const failures: string[] = []; let accepted = 0;
    for (const project of targets) {
      try { props.onProjectTask(await invoke<TaskRecord>('storage_scan', { projectId: project.id, input: {} }), project); accepted++; }
      catch (cause) { failures.push(`「${project.name}」：${messageFrom(cause)}`); }
    }
    if (!mounted.current) return;
    setPending(null); setErrors(failures);
    setNotice(accepted ? `已开始统计 ${accepted} 个网页项目，完成后数字会自动更新。` : '');
  }
  async function scanOne(project: ProjectInfo) {
    if (busy(project)) return;
    setPending('scan'); setNotice(''); setErrors([]);
    try { props.onProjectTask(await invoke<TaskRecord>('storage_scan', { projectId: project.id, input: {} }), project); }
    catch (cause) { if (mounted.current) setErrors([`「${project.name}」：${messageFrom(cause)}`]); }
    finally { if (mounted.current) setPending(null); }
  }
  async function clean(targets: ProjectInfo[]) {
    if (pending) return;
    setPending('clean'); setNotice(''); setErrors([]);
    const failures: string[] = []; let accepted = 0;
    for (const project of targets) {
      try { props.onProjectTask(await invoke<TaskRecord>('storage_cleanup', { projectId: project.id, input: { categories: [...regenerableCategories] } }), project); accepted++; }
      catch (cause) { failures.push(`「${project.name}」：${messageFrom(cause)}`); }
    }
    if (!mounted.current) return;
    setPending(null); setCleanTarget(null); setErrors(failures);
    setNotice(accepted ? `已开始清理 ${accepted} 个项目的可重新生成文件。实际释放量以任务结果为准。` : '');
  }
  return <section className="space-projects" aria-label="按项目查看占用">
    <div className="space-projects-toolbar">
      <label className="project-search"><Search size={16} /><span className="sr-only">搜索项目</span><input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索项目" /></label>
      <div className="project-filter-chips" role="group" aria-label="类型">{([['all', `全部 ${webCount + nodeCount}`], ['web', `网页项目 ${webCount}`], ['node', `Node 应用 ${nodeCount}`]] as const).map(([value, label]) => <button key={value} type="button" aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}</button>)}</div>
      <label className="space-sort">排序<select value={sort} onChange={(event) => setSort(event.target.value as Sort)}><option value="size">占用从大到小</option><option value="regenerable">可释放从大到小</option><option value="name">名称</option></select></label>
      <div className="radio-toolbar-spacer" />
      <button className="refresh-button" disabled={!props.enabled || Boolean(pending) || !props.projects.some((project) => !busy(project))} onClick={() => void scanAll()}><RefreshCw size={14} />{pending === 'scan' ? '正在提交…' : '统计全部项目'}</button>
      <button className="primary-button" disabled={!props.enabled || Boolean(pending) || !cleanable.length} onClick={() => setCleanTarget(cleanable)}>清理全部可重新生成文件{cleanable.length ? ` · ${bigText(cleanableTotal)}` : ''}</button>
    </div>
    {notice && <p className="workspace-inline-notice" role="status">{notice}</p>}
    {errors.length > 0 && <div className="task-error" role="alert">{errors.map((error) => <p key={error}>{error}</p>)}</div>}
    <div className="space-project-table" role="table" aria-label="项目占用列表">
      <div className="space-project-row is-header" role="row"><span role="columnheader">项目</span><span role="columnheader">类型</span><span role="columnheader">总占用</span><span role="columnheader">可重新生成</span><span role="columnheader">统计时间</span><span role="columnheader">操作</span></div>
      {shown.map((row) => <div className="space-project-row" role="row" key={row.key}>
        <span role="cell" className="space-project-name"><strong>{row.name}</strong><small>{row.sub}</small></span>
        <span role="cell"><span className="history-chip">{row.kind === 'web' ? '网页项目' : 'Node 应用'}</span></span>
        <span role="cell" className="space-number" title={row.total === null ? undefined : exactBytes({ $int64: row.total.toString() })}>{row.total === null ? '—' : bigText(row.total)}</span>
        <span role="cell" className="space-number is-regenerable">{row.kind === 'node' ? '—' : row.regenerable === null ? '—' : bigText(row.regenerable)}</span>
        <span role="cell" className={row.needsScan ? 'space-time needs-scan' : 'space-time'}>{row.time}</span>
        <span role="cell" className="space-project-actions">
          <button className="refresh-button" aria-label={`查看「${row.name}」的详情`} disabled={!props.enabled} onClick={() => row.project ? props.onProjectDetails(row.project) : row.app && props.onNodeDetails(row.app)}>详情</button>
          {row.project && (row.regenerable ?? 0n) > 0n && <button className="refresh-button" aria-label={`清理「${row.name}」的可重新生成文件`} disabled={busy(row.project)} onClick={() => setCleanTarget([row.project!])}>清理</button>}
          {row.project && row.needsScan && <button className="refresh-button" aria-label={`统计「${row.name}」的占用`} disabled={busy(row.project)} onClick={() => void scanOne(row.project!)}>统计</button>}
          <button className="text-button danger-text" aria-label={`删除「${row.name}」`} disabled={!props.enabled || Boolean(pending) || Boolean(row.project && props.isProjectBusy?.(row.project))} onClick={() => row.project ? props.onProjectDelete(row.project) : row.app && props.onNodeDelete(row.app)}>删除…</button>
        </span>
      </div>)}
      {!rows.length && <p className="space-empty">还没有项目。</p>}
      {rows.length > 0 && !visibleRows.length && <p className="space-empty">没有匹配的项目。</p>}
    </div>
    <div className="space-projects-footer"><span>显示 {shown.length} / 共 {visibleRows.length} 个项目 · 合计 {bigText(sumKnown(visibleRows.map((row) => row.total)))}{visibleRows.some((row) => row.total === null) ? '（不含尚未统计的项目）' : ''}</span>{visibleRows.length > pageSize && <button className="refresh-button" onClick={() => setExpanded((value) => !value)}>{expanded ? '收起' : `显示其余 ${visibleRows.length - pageSize} 个`}</button>}</div>
    <p className="space-note">“清理”只删除开发依赖、构建缓存和日志，保留源码、数据库、文件和当前可运行的应用。Node 应用的占用在其详情中查看；删除会打开对应的删除确认。</p>
    {cleanTarget && <Modal title={cleanTarget.length === 1 ? `清理「${cleanTarget[0].name}」的可重新生成文件` : `清理 ${cleanTarget.length} 个项目的可重新生成文件`} className="space-clean-dialog" closeDisabled={pending === 'clean'} onClose={() => setCleanTarget(null)}>
      {cleanTarget.length === 1 ? <dl className="space-clean-list">{[['dependencies', '开发依赖'], ['cache', '构建缓存与日志']].map(([key, label]) => <div key={key}><dt>{label}</dt><dd>{bigText(bigBytes(partBytes(cleanTarget[0].storage?.currentProject?.categories?.[key])))}</dd></div>)}</dl>
        : <dl className="space-clean-list">{cleanTarget.map((project) => <div key={project.id}><dt>{project.name}</dt><dd>{bigText(projectRegenerable(project))}</dd></div>)}</dl>}
      <p>依赖和缓存会在下次构建时重新生成；日志删除后无法找回。</p>
      <p>保留：源码、数据库、文件、当前可运行的应用。数字来自最近一次统计，实际释放量以任务结果为准。</p>
      <div className="confirmation-actions"><button className="refresh-button" disabled={pending === 'clean'} onClick={() => setCleanTarget(null)}>取消</button><button className="primary-button" disabled={!props.enabled || pending === 'clean'} onClick={() => void clean(cleanTarget)}>{pending === 'clean' ? '正在提交…' : `清理 ${bigText(sumKnown(cleanTarget.map(projectRegenerable)))}`}</button></div>
    </Modal>}
  </section>;
}
