import { invoke, isTauri } from '@tauri-apps/api/core';
import { CircleAlert, Check, FolderLock, Globe, Hammer, Monitor, RefreshCw, ShieldCheck, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import type { ProjectInfo } from './types';
import { CreateProjectForm, ManagerShell, ManagerToolbar, ProjectCollection } from './ManagerView';
import type { ProjectFilter } from './ManagerView';
import { NodeProjectCard, ProjectCard } from './ProjectCard';
import { NodeAppsPanel } from './NodeAppsPanel';
import type { NodeAppRequest } from './NodeAppsPanel';
import type { NodeAppDetail } from './node-app-types';
import { Modal } from './Modal';
import { WindowsDevelopmentTools } from './WindowsDevelopmentTools';
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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [developmentPending, setDevelopmentPending] = useState(false);
  const [developmentRequiresRecheck, setDevelopmentRequiresRecheck] = useState(false);
  const developmentInteraction = useRef(false);
  const reportDevelopmentPending = useCallback((value: boolean) => {
    developmentInteraction.current = value;
    setDevelopmentPending(value);
  }, []);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<ProjectFilter>('all');
  const [selected, setSelected] = useState<ProjectInfo | null>(null);
  const [nodeApps, setNodeApps] = useState<NodeAppDetail[]>([]);
  const [nodeReadError, setNodeReadError] = useState('');
  const [nodeListStale, setNodeListStale] = useState(true);
  const [nodeRequest, setNodeRequest] = useState<NodeAppRequest | null>(null);
  const [nodeAddRequest, setNodeAddRequest] = useState(0);
  const [nodeModalOpen, setNodeModalOpen] = useState(false);
  const [nodeDraft, setNodeDraft] = useState({ dirty: false, pending: false });
  const nodeRequestToken = useRef(0);
  const nodeListRevision = useRef(0);
  const nodeInteraction = useRef({ pending: false, modal: false });
  const nodeAccess = useRef(false);
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
      // A Node registry read is admitted only by the actual ready-root report.
      // Missing or retained roots never initialize the Node subsystem from UI.
      if (capabilities.nodeApps && value.rootStatus === 'ready') {
        try {
          const nodes = await invoke<NodeAppDetail[]>('node_app_list');
          if (current !== request.current) return;
          setNodeApps(nodes); setNodeListStale(false); setNodeReadError('');
        } catch (cause) {
          if (current !== request.current) return;
          setNodeListStale(true); setNodeReadError('Node 应用列表未能读取：' + errorMessage(cause) + ' 请刷新后核对，已有项目保留。');
        }
      } else { setNodeListStale(true); setNodeReadError(''); }
      nodeListRevision.current++;
      setStatus(value);
      setStatusStale(false);
      if (value.rootStatus === 'ready') setDiagnostic(null);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === request.current) { setError(errorMessage(cause)); setStatusStale(true); setNodeListStale(true); }
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
    if (!capabilities.create || !isTauri() || nodeInteraction.current.pending || nodeInteraction.current.modal || createPending.current || openPending.current || refreshPending.current || diagnosticPending.current || !status || status.rootStatus === 'blocked' || statusStale || !name.trim()) return;
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
    if (!(target === 'desktop' ? capabilities.openDesktop : capabilities.openBrowser) || !isTauri() || nodeInteraction.current.pending || nodeInteraction.current.modal || openPending.current || createPending.current || refreshPending.current || diagnosticPending.current || !status || status.rootStatus !== 'ready' || statusStale || currentProject?.state !== 'ready') return;
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
    if (!isTauri() || nodeInteraction.current.pending || nodeInteraction.current.modal || diagnosticPending.current || createPending.current || openPending.current || refreshPending.current || status?.rootStatus === 'ready') return;
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
  const hostBusy = loading || creating || Boolean(opening) || diagnosticLoading;
  const busy = hostBusy || nodeDraft.pending;
  const nodeAllowed = capabilities.nodeApps && isTauri() && status?.rootStatus === 'ready' && !statusStale && !nodeListStale;
  nodeAccess.current = nodeAllowed;
  const nodeUnavailableReason = status?.rootStatus === 'missing'
    ? '请先创建一个可丢弃网页项目，初始化固定工作区后再添加 Node 应用。'
    : '工作区或 Node 状态尚未确认。请关闭此窗口，刷新项目状态后再操作。';
  const deferredReason = '当前 Windows 版本尚未开放此功能。';
  const projects = status?.projects ?? [];
  const currentProject = projects.find(project => project.id === selected?.id);
  const detailsProject = currentProject ?? selected;
  const modalOpen = showCreate || Boolean(selected) || settingsOpen || nodeModalOpen;
  const filteredProjects = projects.filter(project => {
    const matches = `${project.name} ${project.description ?? ''} ${project.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const ready = !blocked && project.state === 'ready';
    return matches && (filter === 'all' || (filter === 'ready' ? ready : !ready));
  });
  const hasFilter = Boolean(query.trim()) || filter !== 'all';
  const filteredNodeApps = nodeApps.filter(app => {
    const matches = `${app.name} ${app.id} ${app.sourcePath} Node 服务项目`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const ready = nodeAllowed && app.configuration.canStart && !['failed', 'starting', 'stopping'].includes(app.state);
    return matches && (filter === 'all' || (filter === 'ready' ? ready : !ready));
  });
  const totalCount = projects.length + nodeApps.length;
  const filteredCount = filteredProjects.length + filteredNodeApps.length;
  const reportNodeDraft = useCallback((value: { dirty: boolean; pending: boolean }) => {
    nodeInteraction.current.pending = value.pending;
    setNodeDraft(previous => previous.dirty === value.dirty && previous.pending === value.pending ? previous : value);
  }, []);
  const reportNodeModal = useCallback((value: boolean) => { nodeInteraction.current.modal = value; setNodeModalOpen(value); }, []);
  const acceptNodeApp = useCallback((app: NodeAppDetail) => {
    if (!nodeAccess.current) return;
    nodeListRevision.current++;
    setNodeApps(previous => previous.some(value => value.id === app.id) ? previous.map(value => value.id === app.id ? app : value) : [...previous, app]);
  }, []);
  const nodeMutationError = useCallback((cause: unknown) => {
    const details = typeof cause === 'object' && cause && 'details' in cause ? cause.details : null;
    const outcome = typeof details === 'object' && details && 'operationOutcome' in details ? details.operationOutcome
      : typeof cause === 'object' && cause && 'operationOutcome' in cause ? cause.operationOutcome : null;
    if (outcome !== 'unknown') return;
    nodeAccess.current = false;
    nodeListRevision.current++;
    setNodeListStale(true);
    setNodeReadError('Node 操作结果尚未确认，系统不会自动重试。请关闭详情并刷新列表，核对实际状态后再操作。');
  }, []);
  useEffect(() => {
    if (!nodeAllowed || hostBusy || !nodeApps.length) return;
    let cancelled = false; let polling = false;
    const timer = window.setInterval(() => {
      if (polling || nodeInteraction.current.pending || nodeInteraction.current.modal || refreshPending.current || createPending.current || openPending.current) return;
      polling = true;
      const revision = nodeListRevision.current; const generation = request.current;
      void invoke<NodeAppDetail[]>('node_app_list').then(values => {
        if (!cancelled && nodeAccess.current && revision === nodeListRevision.current && generation === request.current && !nodeInteraction.current.pending && !nodeInteraction.current.modal) { setNodeApps(values); setNodeReadError(''); }
      }).catch(() => {
        if (!cancelled && revision === nodeListRevision.current && generation === request.current && !nodeInteraction.current.pending && !nodeInteraction.current.modal) { nodeAccess.current = false; setNodeListStale(true); setNodeReadError('Node 应用状态未能更新。请刷新后核对；系统不会自动启动或重试应用。'); }
      }).finally(() => { polling = false; });
    }, 2000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [nodeAllowed, hostBusy, nodeApps.length]);
  function requestNodeApp(app: NodeAppDetail, intent: 'enter' | 'details') {
    if (!nodeAccess.current || hostBusy || showCreate || selected || settingsOpen || nodeInteraction.current.pending || nodeInteraction.current.modal || refreshPending.current || createPending.current || openPending.current || diagnosticPending.current) return;
    nodeInteraction.current.pending = true;
    setNodeRequest({ app, intent, token: ++nodeRequestToken.current });
  }
  function requestAddNode() {
    if (!nodeAccess.current || busy || showCreate || selected || settingsOpen || nodeInteraction.current.pending || nodeInteraction.current.modal || refreshPending.current || createPending.current || openPending.current || diagnosticPending.current) return;
    nodeInteraction.current.modal = true;
    setNodeAddRequest(value => value + 1);
  }
  function requestRefresh() {
    // User actions share one synchronous admission gate. Creation's accepted
    // operation still uses refresh() internally after its real acknowledgement.
    if (developmentInteraction.current || nodeInteraction.current.pending || nodeInteraction.current.modal || createPending.current || openPending.current || diagnosticPending.current || refreshPending.current) return;
    return refresh();
  }
  function requestCreate() {
    if (!capabilities.create || blocked || busy || nodeInteraction.current.pending || nodeInteraction.current.modal || createPending.current || openPending.current || diagnosticPending.current || refreshPending.current) return;
    setShowCreate(true); setNotice(null);
  }
  function showDetails(project: ProjectInfo) {
    if (!capabilities.readOnlyDetails || busy || nodeInteraction.current.pending || nodeInteraction.current.modal) return;
    setSelected(project);
  }
  const feedback = <>
    {error && <div className="notice notice-error" role="alert"><CircleAlert size={20} /><div><strong>操作未完成</strong><p>{error}{status ? ' 下方保留上次成功读取的状态；请刷新确认后再操作。' : ''}</p>
      {showCreate && statusStale && <button className="refresh-button" disabled={busy} onClick={() => void requestRefresh()}><RefreshCw size={14} />刷新状态</button>}
    </div><button className="icon-button" aria-label="关闭错误提示" onClick={() => setError(null)}><X size={16} /></button></div>}
    {notice && <div className="notice notice-success" role="status"><Check size={18} /><p>{notice}</p><button className="icon-button" aria-label="关闭操作提示" onClick={() => setNotice(null)}><X size={16} /></button></div>}
  </>;
  const protection = <div className="notice" role="status"><ShieldCheck size={20} /><div><strong>Windows 内部样本：网页只读打开与 Node 服务应用</strong><p>仅使用可丢弃验证项目。Node 应用从本机源码目录登记，使用包内 Node；完整备份、恢复、回收、迁移与外部分发尚未接入。网页项目的 SQL 写入、附件、构建与资料修改仍未开放。已有目录和未解决现场继续保留。</p>{status?.rootStatus === 'missing' && <p>{nodeUnavailableReason}</p>}</div></div>;
  return <ManagerShell page="projects" onPageChange={() => {}} onSettings={() => { if (!busy && !nodeInteraction.current.pending && !nodeInteraction.current.modal) setSettingsOpen(true); }}
    storageUnavailableReason={deferredReason} settingsDisabled={busy || nodeModalOpen}
    footerCount={hasFilter ? `${filteredCount} / ${totalCount}` : totalCount}
    workspaceRoot={status?.rootStatus === 'ready' ? status.dataRoot + '\\workspace' : undefined}
    toolbar={<ManagerToolbar query={query} onQueryChange={setQuery} filter={filter} onFilterChange={setFilter}
      loading={loading} checkedAt={checkedAt} menuDisabled={busy || nodeModalOpen} refreshDisabled={!isTauri() || busy || nodeModalOpen} createDisabled={!capabilities.create || blocked || busy || showCreate || nodeModalOpen}
      addUnavailableReason={nodeAllowed ? undefined : nodeUnavailableReason} importUnavailableReason={deferredReason} onRefresh={requestRefresh} onCreate={requestCreate} onAdd={requestAddNode} />}
  >
    <div className="radio-notices">
    {!isTauri() && <div className="notice" role="status"><Monitor size={20} /><div><strong>项目管理在桌面应用中进行</strong><p>请从 Windows 桌面应用打开此页面，以读取本机项目状态。</p></div></div>}
    {!modalOpen && feedback}
    {nodeReadError && <p className="task-error" role="alert">{nodeReadError}</p>}
    {protection}
    {status?.rootStatus === 'blocked' && <div className="notice notice-error" role="alert"><FolderLock size={20} /><div><strong>现有目录受保护</strong><p>{status.message}</p><p>请保留现有文件，并从设置查看诊断；本窗口不会创建空工作区覆盖它。</p></div></div>}
    </div>
    <ProjectCollection totalCount={totalCount} filteredCount={filteredCount} hasFilter={hasFilter}
      loading={loading} connected={Boolean(status) && status?.rootStatus !== 'blocked'} createDisabled={blocked || busy}
      onCreate={requestCreate} onResetFilters={() => { setQuery(''); setFilter('all'); }}
      emptyTitle={status?.rootStatus === 'blocked' ? '现有目录受保护' : undefined}
      emptyDescription={status?.rootStatus === 'blocked' ? '未接管当前目录；请先查看设置中的诊断信息。' : undefined}>
      {filteredProjects.map(project => <ProjectCard key={project.id} project={project}
        blocked={busy || blocked || nodeModalOpen} opening={opening === project.id + ':desktop'} deletingDisabled={!capabilities.deleteProject}
        deleteUnavailableReason={deferredReason} readOnly detailsDisabled={busy || nodeModalOpen}
        onEnter={() => void openProject(project, 'desktop')} onDetails={() => showDetails(project)} onDelete={() => {}} />)}
      {filteredNodeApps.map(app => <NodeProjectCard key={'node:' + app.id} app={app} blocked={!nodeAllowed || busy || nodeModalOpen}
        deleteUnavailableReason={deferredReason} detailsDisabled={!nodeAllowed || busy || nodeModalOpen} opening={nodeRequest?.app.id === app.id && nodeDraft.pending} onEnter={() => requestNodeApp(app, 'enter')} onDetails={() => requestNodeApp(app, 'details')} />)}
    </ProjectCollection>
    {settingsOpen && <Modal title="设置" className="radio-settings-modal" closeDisabled={diagnosticLoading || developmentPending} beforeClose={() => !diagnosticPending.current && !developmentInteraction.current} onClose={() => setSettingsOpen(false)}>
      {feedback}
      <WindowsDevelopmentTools disabled={status?.rootStatus !== 'ready' || busy} onPendingChange={reportDevelopmentPending}
        requiresRecheck={developmentRequiresRecheck} onRecheckRequiredChange={setDevelopmentRequiresRecheck} />
      <section className="runtime-view" aria-labelledby="settings-runtime-title">
        <h3 id="settings-runtime-title">运行环境</h3>
        <dl className="radio-settings-summary"><div><dt>Windows 能力阶段</dt><dd>网页只读与 Node 内部样本</dd></div><div><dt>工作区状态</dt><dd>{status ? rootLabels[status.rootStatus] : '尚未读取'}</dd></div></dl>
        <details className="radio-runtime-details"><summary>查看目录与诊断详情</summary>
      <section className="diagnostics-card" aria-labelledby="windows-workspace-title" aria-busy={loading}>
        <div className="section-heading"><div><h2 id="windows-workspace-title">固定开发工作区</h2><p>{status ? rootLabels[status.rootStatus] : '尚未取得项目状态'}</p></div><span className="updated-at">{checkedAt ? '最近成功读取 ' + checkedAt : '尚无成功读取记录'}</span></div>
        <dl className="path-list"><div className="path-row"><dt>开发数据目录</dt><dd><code aria-label="固定开发数据目录">{status?.dataRoot ?? '尚未读取'}</code></dd></div></dl>
        <div className="section-heading"><p>{status?.message ?? '等待真实项目状态。'} 创建和打开均由本机服务检查工作区与项目状态。</p><button className="refresh-button" disabled={busy || !isTauri()} aria-label="刷新设置中的项目状态" onClick={() => void requestRefresh()}><RefreshCw size={14} />刷新状态</button></div>
      </section>
      <section className="diagnostics-card" aria-labelledby="windows-diagnostics-title">
        <div className="section-heading"><div><h2 id="windows-diagnostics-title">目录诊断</h2><p>{status?.rootStatus === 'ready' ? '工作区由当前项目服务持有，以上方项目状态为准。独立浅层诊断不会再次打开数据根。' : '按需只读观察保留现场，不执行恢复或清理。'}</p></div>
          {status?.rootStatus !== 'ready' && <button className="refresh-button" disabled={busy} onClick={() => void readDiagnostic()}>{diagnosticLoading ? '正在检查…' : '读取目录诊断'}</button>}
        </div>
        {diagnostic && <div className="section-heading"><div><ul>{diagnostic.probe.checks.map((check, index) => <li key={check.code + '-' + index}>{check.message}</li>)}</ul>{diagnostic.probe.evidence.map(entry => <p key={entry.kind}>{evidenceLabels[entry.kind] ?? entry.kind}：{entry.count}</p>)}</div></div>}
      </section>
        </details>
      </section>
      <section className="radio-settings-about" aria-labelledby="settings-about-title">
        <h3 id="settings-about-title">关于</h3>
        <div className="radio-about-brand"><img src="/panda-app-drawer.png" alt="" /><div>熊猫应用小抽屉<span>Windows · 内部验证版</span></div></div>
        <p>项目数据保存在本机；Node 仅接入内部源码样本，完整备份、生命周期与外部分发尚未开放。</p>
      </section>
      <details className="radio-settings-guide"><summary>使用指南</summary><div className="section-heading"><div><h2>创建与打开</h2><p>先创建一个可丢弃网页项目初始化固定工作区。网页初始页面已预编译，使用时无需系统 Node。</p><p>工作区就绪后，可从“添加本地应用”登记 Node 样本；先核对声明，再配置保存，最后明确进入项目。保存配置不会启动或重启应用。源码目录需保留。上方开发工具设置不会自动构建或运行项目；应用运行仍使用包内独立 Node。</p></div></div></details>
      <div className="radio-settings-footer"><span>PANDA DRAWER</span><button className="refresh-button" disabled={diagnosticLoading || developmentPending} onClick={() => { if (!diagnosticPending.current && !developmentInteraction.current) setSettingsOpen(false); }}>完成</button></div>
    </Modal>}
    <NodeAppsPanel enabled={nodeAllowed && !hostBusy} request={nodeRequest} addRequest={nodeAddRequest}
      chooseSourceCommand="node_app_choose_source" unavailableReason={nodeUnavailableReason} onMutationError={nodeMutationError}
      directoryOnboarding={capabilities.nodeDirectoryOnboarding} managedInstallation={capabilities.nodeManagedInstall} deletion={capabilities.nodeDelete}
      onAppChanged={acceptNodeApp} onModalStateChange={reportNodeModal} onDraftStateChange={reportNodeDraft} />
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
