import { invoke, isTauri } from '@tauri-apps/api/core';
import { CircleAlert, Check, FolderLock, Globe, Hammer, Monitor, RefreshCw, ShieldCheck, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type { ProjectInfo } from './types';
import { CreateProjectForm, ManagerActions, ManagerShell, ProjectCollection } from './ManagerView';
import type { ManagerPage, ProjectFilter } from './ManagerView';
import { Modal } from './Modal';
import { WINDOWS_MANAGER_CAPABILITIES as capabilities } from './manager-capabilities';

interface ProjectStatus {
  stage: 'createReadOnly';
  rootStatus: 'missing' | 'ready' | 'blocked';
  message: string;
  dataRoot: string;
  projects: ProjectInfo[];
}

interface HostReport {
  identifier: string;
  platform: string;
  architecture: string;
  probe: {
    checks: { code: string; status: string; message: string }[];
    evidence: { kind: string; count: number }[];
  };
}

const rootLabels: Record<ProjectStatus['rootStatus'], string> = {
  missing: '等待首次创建',
  ready: '工作区已就绪',
  blocked: '现有目录受保护',
};
const evidenceLabels: Record<string, string> = {
  environmentMarker: '环境标记文件', settings: '工作区设置文件',
  migrationEntries: '迁移目录内条目', cleanupEntries: '清理目录内条目',
  workspaceEntries: '默认工作区顶层条目', runtimeDirectory: '运行时目录（仅观察存在）',
  unknownTopLevelEntries: '其他保留条目',
};

function errorMessage(cause: unknown): string {
  if (typeof cause === 'object' && cause && 'message' in cause && typeof cause.message === 'string') return cause.message;
  return typeof cause === 'string' ? cause : '操作未完成，请刷新状态后查看结果。';
}

export function WindowsHost() {
  const [status, setStatus] = useState<ProjectStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [statusStale, setStatusStale] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [creating, setCreating] = useState(false);
  const [opening, setOpening] = useState<string | null>(null);
  const [diagnostic, setDiagnostic] = useState<HostReport | null>(null);
  const [diagnosticLoading, setDiagnosticLoading] = useState(false);
  const [page, setPage] = useState<ManagerPage>('projects');
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<ProjectFilter>('all');
  const [selected, setSelected] = useState<ProjectInfo | null>(null);
  const refreshPending = useRef(false);
  const diagnosticPending = useRef(false);
  const request = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const mounted = useRef(true);
  const refresh = useCallback(async () => {
    const current = ++request.current;
    refreshPending.current = true;
    setLoading(true);
    setError(null);
    try {
      if (!isTauri()) throw new Error('请从 Windows 桌面应用打开此页面，以读取本机项目状态。');
      const value = await invoke<ProjectStatus>('project_status');
      if (current !== request.current) return;
      setStatus(value);
      setStatusStale(false);
      if (value.rootStatus === 'ready') setDiagnostic(null);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === request.current) { setError(errorMessage(cause)); setStatusStale(true); }
    } finally {
      if (current === request.current) { refreshPending.current = false; setLoading(false); }
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; request.current += 1; };
  }, [refresh]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!capabilities.create || !isTauri() || createPending.current || openPending.current || refreshPending.current || diagnosticPending.current || !status || status.rootStatus === 'blocked' || statusStale || !name.trim()) return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      const project = await invoke<ProjectInfo>('project_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      if (!mounted.current) return;
      setName(''); setDescription(''); setShowCreate(false);
      setNotice('已创建「' + project.name + '」。可以进入项目或从详情在浏览器打开初始应用。');
      await refresh();
    } catch (cause) {
      if (mounted.current) {
        // Never retry a creation after an ambiguous response. A manual refresh
        // lets the user inspect actual projects before making another request.
        setError(errorMessage(cause));
        setStatusStale(true);
      }
    } finally {
      createPending.current = false;
      if (mounted.current) setCreating(false);
    }
  }

  async function openProject(project: ProjectInfo, target: 'desktop' | 'browser') {
    const currentProject = status?.projects.find(value => value.id === project.id);
    if (!(target === 'desktop' ? capabilities.openDesktop : capabilities.openBrowser) || !isTauri() || openPending.current || createPending.current || refreshPending.current || diagnosticPending.current || !status || status.rootStatus !== 'ready' || statusStale || currentProject?.state !== 'ready') return;
    openPending.current = true;
    setOpening(project.id + ':' + target);
    setError(null);
    setNotice(null);
    try {
      await invoke(target === 'desktop' ? 'project_open' : 'project_open_browser', { projectId: project.id });
      if (mounted.current) setNotice(target === 'desktop' ? '已打开「' + project.name + '」项目窗口。' : '已请求默认浏览器打开「' + project.name + '」。');
    } catch (cause) {
      if (mounted.current) {
        setError(errorMessage(cause));
        setStatusStale(true);
      }
    } finally {
      openPending.current = false;
      if (mounted.current) setOpening(null);
    }
  }

  async function readDiagnostic() {
    if (!isTauri() || diagnosticPending.current || createPending.current || openPending.current || refreshPending.current || status?.rootStatus === 'ready') return;
    diagnosticPending.current = true;
    setDiagnosticLoading(true);
    setError(null);
    try {
      const value = await invoke<HostReport>('host_status');
      if (mounted.current) setDiagnostic(value);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      diagnosticPending.current = false;
      if (mounted.current) setDiagnosticLoading(false);
    }
  }

  const blocked = !status || status.rootStatus === 'blocked' || statusStale;
  const busy = loading || creating || Boolean(opening) || diagnosticLoading;
  const deferredReason = '当前 Windows 版本尚未开放此功能。';
  const projects = status?.projects ?? [];
  const currentProject = projects.find(project => project.id === selected?.id);
  const detailsProject = currentProject ?? selected;
  const modalOpen = showCreate || Boolean(selected);
  const title = page === 'runtime' ? '运行环境' : '我的项目';
  function requestRefresh() {
    // User actions share one synchronous admission gate. Creation's accepted
    // operation still uses refresh() internally after its real acknowledgement.
    if (createPending.current || openPending.current || diagnosticPending.current || refreshPending.current) return;
    return refresh();
  }
  function requestCreate() {
    if (!capabilities.create || blocked || busy || createPending.current || openPending.current || diagnosticPending.current || refreshPending.current) return;
    setShowCreate(true); setNotice(null);
  }
  function showDetails(project: ProjectInfo) {
    if (!capabilities.readOnlyDetails || busy) return;
    setSelected(project);
  }
  const feedback = <>
    {error && <div className="notice notice-error" role="alert"><CircleAlert size={20} /><div><strong>操作未完成</strong><p>{error}{status ? ' 下方保留上次成功读取的状态；请刷新确认后再操作。' : ''}</p>
      {showCreate && statusStale && <button className="refresh-button" disabled={busy} onClick={() => void requestRefresh()}><RefreshCw size={14} />刷新状态</button>}
    </div><button className="icon-button" aria-label="关闭错误提示" onClick={() => setError(null)}><X size={16} /></button></div>}
    {notice && <div className="notice notice-success" role="status"><Check size={18} /><p>{notice}</p><button className="icon-button" aria-label="关闭操作提示" onClick={() => setNotice(null)}><X size={16} /></button></div>}
  </>;
  const protection = <div className="notice" role="status"><ShieldCheck size={20} /><div><strong>当前 Windows 版本仅开放创建和只读打开</strong><p>仅使用可丢弃验证项目。项目资料修改、SQL 写入、附件、构建、备份、恢复、回收、迁移与清理尚未开放。已有目录和未解决现场继续保留。</p></div></div>;
  return <ManagerShell page={page} onPageChange={next => { if (next === 'projects' || next === 'runtime') setPage(next); }}
    projectCount={projects.length} title={title} description={page === 'projects' ? '选择项目开始使用，或在详情中查看信息和管理数据。' : '查看平台与本地服务的信息。'}
    stageLabel="Windows · 创建与只读打开" versionLabel="内部验证"
    disabledPages={{ storage: deferredReason, about: deferredReason }}
    actions={<ManagerActions loading={loading} inDesktop={isTauri()} refreshDisabled={busy} createDisabled={!capabilities.create || blocked || busy || showCreate}
      importDisabled={!capabilities.importProject} importUnavailableReason={deferredReason} showProjectActions={page === 'projects'}
      onRefresh={requestRefresh} onCreate={requestCreate} />}
  >
    {!isTauri() && <div className="notice" role="status"><Monitor size={20} /><div><strong>项目管理在桌面应用中进行</strong><p>请从 Windows 桌面应用打开此页面，以读取本机项目状态。</p></div></div>}
    {!modalOpen && feedback}
    {protection}
    {page === 'projects' ? <>
      {status?.rootStatus === 'blocked' && <div className="notice notice-error" role="alert"><FolderLock size={20} /><div><strong>现有目录受保护</strong><p>{status.message}</p><p>请保留现有文件，并从运行环境查看诊断；本窗口不会创建空工作区覆盖它。</p></div></div>}
      <ProjectCollection projects={projects} query={query} onQueryChange={setQuery} filter={filter} onFilterChange={setFilter}
        isProjectReady={project => !blocked && project.state === 'ready'}
        cardProps={project => ({ blocked: busy || blocked, opening: opening === project.id + ':desktop', deletingDisabled: !capabilities.deleteProject,
          deleteUnavailableReason: deferredReason, readOnly: true, detailsDisabled: busy,
          onEnter: () => void openProject(project, 'desktop'), onDetails: () => showDetails(project), onDelete: () => {} })}
        loading={loading} checkedAt={checkedAt} connected={Boolean(status) && status?.rootStatus !== 'blocked'} createDisabled={blocked || busy}
        onCreate={requestCreate} workspaceRoot={status?.rootStatus === 'ready' ? status.dataRoot + '\\workspace' : undefined}
        emptyTitle={status?.rootStatus === 'blocked' ? '现有目录受保护' : undefined}
        emptyDescription={status?.rootStatus === 'blocked' ? '未接管当前目录；请先查看运行环境中的诊断信息。' : undefined}
        footerHint="当前仅支持只读样本，项目删除与回收尚未开放"
        guide={<div className="section-heading"><div><h2>创建与打开</h2><p>创建后可以进入初始应用，也可以从项目详情在浏览器打开。初始页面已预编译，使用时无需 Node。</p><p>当前版本仅使用可丢弃验证项目；源码修改、数据写入与生命周期功能将按步骤开放。</p></div></div>}
      />
    </> : <>
      <section className="diagnostics-card" aria-labelledby="windows-workspace-title" aria-busy={loading}>
        <div className="section-heading"><div><h2 id="windows-workspace-title">固定开发工作区</h2><p>{status ? rootLabels[status.rootStatus] : '尚未取得项目状态'}</p></div><span className="updated-at">{checkedAt ? '最近成功读取 ' + checkedAt : '尚无成功读取记录'}</span></div>
        <dl className="path-list"><div className="path-row"><dt>开发数据目录</dt><dd><code aria-label="固定开发数据目录">{status?.dataRoot ?? '尚未读取'}</code></dd></div></dl>
        <div className="section-heading"><p>{status?.message ?? '等待真实项目状态。'} 创建和打开均由本机服务检查工作区与项目状态。</p></div>
      </section>
      <section className="diagnostics-card" aria-labelledby="windows-diagnostics-title">
        <div className="section-heading"><div><h2 id="windows-diagnostics-title">目录诊断</h2><p>{status?.rootStatus === 'ready' ? '工作区由当前项目服务持有，以上方项目状态为准。独立浅层诊断不会再次打开数据根。' : '按需只读观察保留现场，不执行恢复或清理。'}</p></div>
          {status?.rootStatus !== 'ready' && <button className="refresh-button" disabled={busy} onClick={() => void readDiagnostic()}>{diagnosticLoading ? '正在检查…' : '读取目录诊断'}</button>}
        </div>
        {diagnostic && <div className="section-heading"><div><ul>{diagnostic.probe.checks.map((check, index) => <li key={check.code + '-' + index}>{check.message}</li>)}</ul>{diagnostic.probe.evidence.map(entry => <p key={entry.kind}>{evidenceLabels[entry.kind] ?? entry.kind}：{entry.count}</p>)}</div></div>}
      </section>
    </>}
    {showCreate && <CreateProjectForm name={name} description={description} onNameChange={setName} onDescriptionChange={setDescription}
      onSubmit={createProject} onClose={() => { if (!createPending.current) setShowCreate(false); }} creating={creating} disabled={blocked || busy}
      feedback={feedback} subtitle="仅创建可丢弃验证项目，创建后可只读打开。" footerNote="保存在固定开发工作区，不执行项目脚本" />}
    {detailsProject && selected && <Modal title={detailsProject.name} description="项目详情与只读打开" wide onClose={() => setSelected(null)}>
      {feedback}
      {(!currentProject || blocked) && <div className="notice notice-error" role="status"><CircleAlert size={18} /><div><strong>项目操作已暂停</strong><p>请关闭详情并刷新项目状态，确认后再打开。</p></div></div>}
      <div className="project-dialog-actions">
        <button className="primary-button" disabled={!capabilities.openDesktop || !currentProject || busy || blocked || detailsProject.state !== 'ready'} onClick={() => void openProject(detailsProject, 'desktop')}><Monitor size={16} />进入项目</button>
        <button className="refresh-button" disabled={!capabilities.openBrowser || !currentProject || busy || blocked || detailsProject.state !== 'ready'} onClick={() => void openProject(detailsProject, 'browser')}><Globe size={16} />浏览器打开</button>
        <button className="refresh-button" disabled={!capabilities.build} title={deferredReason}><Hammer size={16} />构建项目</button>
      </div>
      <section className="project-details"><p>{detailsProject.description || '这个项目尚未填写简介。'}</p><p>当前可只读打开初始应用和真实数据库结构；项目资料修改和其他管理功能尚未开放。</p>
        {detailsProject.issues.length > 0 && <ul className="project-issues">{detailsProject.issues.map((issue, index) => <li key={index}>{issue}</li>)}</ul>}
        <dl><div><dt>项目 ID</dt><dd><code>{detailsProject.id}</code></dd></div><div><dt>项目目录</dt><dd><code>{detailsProject.projectRoot}</code></dd></div><div><dt>开发数据目录</dt><dd><code aria-label="固定开发数据目录">{status?.dataRoot ?? '尚未读取'}</code></dd></div><div><dt>源码与网页</dt><dd>{detailsProject.buildInfo?.sourceState === 'matched' ? '源码与网页一致' : '请检查项目状态'}</dd></div></dl>
      </section>
    </Modal>}
  </ManagerShell>;
}
