import { invoke, isTauri } from '@tauri-apps/api/core';
import {
  Activity, Check, CircleAlert,
  Database, Folder, FolderOpen, Globe, Monitor, Package,
  RefreshCw, ShieldCheck, Terminal, X, Hammer, Copy, Trash2,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { ProjectTask } from './ProjectTask';
import { ImportPanel } from './LifecyclePanels';
import { ProjectOverview } from './ProjectOverview';
import type { OverviewTab } from './ProjectOverview';
import { WorkspaceHub } from './WorkspaceHub';
import { Modal } from './Modal';
import { AboutPlatform, FirstStartGuide } from './PlatformPanels';
import { ManagerShell, ManagerActions, ProjectCollection, CreateProjectForm } from './ManagerView';
import { MAC_MANAGER_CAPABILITIES as capabilities } from './manager-capabilities';
import { messageFrom } from './ui-utils';
import type { BootstrapInfo, EnvironmentInfo, OpenProjectResult, ProjectInfo, MaintenanceStatus, TaskRecord, WorkspaceInfo } from './types';

const resourceNames: Record<keyof BootstrapInfo['resources'], string> = {
  sdk: 'SDK 0.1.0', template: 'React 项目模板', readme: '开发说明', cli: '原生 CLI',
};


function App() {
  const [info, setInfo] = useState<BootstrapInfo | null>(null);
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [page, setPage] = useState<'projects' | 'runtime' | 'storage' | 'about'>('projects');
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [opening, setOpening] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [projectFilter, setProjectFilter] = useState<'all' | 'ready' | 'attention'>('all');
  const [tasks, setTasks] = useState<Record<string, TaskRecord>>({});
  const [maintenance, setMaintenance] = useState<Record<string, MaintenanceStatus>>({});
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [lifecycleTasks, setLifecycleTasks] = useState<Record<string, TaskRecord>>({});
  const [showImport, setShowImport] = useState(false);
  const [overview, setOverview] = useState<{ project: ProjectInfo; tab: OverviewTab } | null>(null);
  const [requestedDelete, setRequestedDelete] = useState<{ projectId: string; taskId: string } | null>(null);
  const [reloadProjectId, setReloadProjectId] = useState<string | null>(null);
  const [building, setBuilding] = useState<string[]>([]);
  const [handoff, setHandoff] = useState<string | null>(null);
  const [handoffCopying, setHandoffCopying] = useState(false);
  const buildPending = useRef(new Set<string>());
  const requestId = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const inDesktop = isTauri();
  const instanceId = useRef<string | null>(null);
  const handoffRequest = useRef(0);
  const handoffInFlight = useRef<number | null>(null);
  const clipboardQueue = useRef<Promise<void>>(Promise.resolve());
  const selectedOverviewId = useRef<string | null>(null);
  const overviewDismissGuard = useRef<(() => boolean) | null>(null);
  const registerOverviewGuard = useCallback((guard: (() => boolean) | null) => { overviewDismissGuard.current = guard; }, []);
  const clearHandoff = useCallback(() => {
    handoffRequest.current++;
    handoffInFlight.current = null;
    setHandoff(null); setHandoffCopying(false);
  }, []);
  const dismissOverview = useCallback(() => {
    clearHandoff();
    selectedOverviewId.current = null;
    setOverview(null); setReloadProjectId(null);
  }, [clearHandoff]);
  function openOverview(project: ProjectInfo, tab: OverviewTab) {
    clearHandoff();
    selectedOverviewId.current = project.id;
    setReloadProjectId(null); setNotice(null);
    setOverview({ project, tab });
  }
  const migrating = Boolean(workspace?.activeTask) || ['running', 'paused', 'migrating'].includes(workspace?.migrationState ?? '') || Object.values(lifecycleTasks).some((task) => task.type === 'workspaceMigration' && !task.final);
  const cleanupTask = Object.values(lifecycleTasks).filter((task) => task.type === 'workspaceCleanup').at(-1) ?? workspace?.cleanupTask ?? null;
  const cleanupBusy = Boolean(cleanupTask && !cleanupTask.final);
  const workspaceBusy = migrating || cleanupBusy;
  useEffect(() => { if (workspaceBusy) clearHandoff(); }, [workspaceBusy, clearHandoff]);
  const acceptLifecycleTask = useCallback((task: TaskRecord) => {
    setLifecycleTasks((current) => ({ ...current, [task.taskId]: task }));
  }, []);

  const refresh = useCallback(async () => {
    const current = ++requestId.current;
    setLoading(true);
    setError(null);
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      // Read bootstrap first so an unavailable committed workspace reports its
      // concrete startup error, rather than a missing native command state.
      let runtime = await invoke<BootstrapInfo>('bootstrap_info');
      const [list, currentWorkspace] = await Promise.all([
        invoke<ProjectInfo[]>('project_list'), invoke<WorkspaceInfo>('workspace_info'),
      ]);
      if (currentWorkspace.workspaceRoot !== runtime.workspaceRoot) runtime = await invoke<BootstrapInfo>('bootstrap_info');
      if (current !== requestId.current) return;
      if (instanceId.current && instanceId.current !== runtime.instanceId) {
        clearHandoff();
        setNotice('工作区服务已切换。旧窗口的草稿仍保留；处理草稿后请明确重新加载项目，并从新目录重新复制 AI 说明。');
      }
      if (selectedOverviewId.current && !list.some((project) => project.id === selectedOverviewId.current)) clearHandoff();
      instanceId.current = runtime.instanceId;
      setWorkspace(currentWorkspace);
      if (currentWorkspace.cleanupTask) acceptLifecycleTask(currentWorkspace.cleanupTask);
      const workspaceTaskId = (typeof currentWorkspace.activeTask === 'string' ? currentWorkspace.activeTask : currentWorkspace.activeTask?.taskId) ?? currentWorkspace.lastTaskId;
      if (workspaceTaskId) { const task = await invoke<TaskRecord>('task_get', { taskId: workspaceTaskId }); if (current === requestId.current) acceptLifecycleTask(task); }
      if (current !== requestId.current) return;
      setInfo(runtime);
      setProjects(list);
      const statuses = await Promise.allSettled(list.map(async (project) => {
        const status = await invoke<MaintenanceStatus>('maintenance_status', { projectId: project.id });
        const activeId = (typeof status.activeTask === 'string' ? status.activeTask : status.activeTask?.taskId) ?? status.lastTaskId;
        const task = activeId ? await invoke<TaskRecord>('task_get', { taskId: activeId }) : null;
        return { projectId: project.id, status, task };
      }));
      if (current !== requestId.current) return;
      const next: Record<string, MaintenanceStatus> = {};
      const activeTasks: Record<string, TaskRecord> = {};
      for (const project of list) { const scanning = project.storage?.activeTask; if (scanning && typeof scanning === 'object' && 'status' in scanning) activeTasks[project.id] = scanning as TaskRecord; }
      for (const result of statuses) {
        if (result.status === 'fulfilled') {
          next[result.value.projectId] = result.value.status;
          if (result.value.task) activeTasks[result.value.projectId] = result.value.task;
        } else { setError(`维护状态读取失败：${messageFrom(result.reason)}。请刷新状态后再操作。`); }
      }
      setMaintenance(next);
      setTasks((previous) => ({ ...previous, ...activeTasks }));
      setRefreshVersion((version) => version + 1);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === requestId.current) setError(messageFrom(cause));
    } finally {
      if (current === requestId.current) setLoading(false);
    }
  }, [acceptLifecycleTask, clearHandoff]);

  useEffect(() => {
    void refresh();
    return () => { requestId.current += 1; handoffRequest.current++; selectedOverviewId.current = null; };
  }, [refresh]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPending.current || workspaceBusy || !name.trim()) return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      const project = await invoke<ProjectInfo>('project_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      setProjects((previous) => [...previous.filter((item) => item.id !== project.id), project]);
      setName('');
      setDescription('');
      setShowCreate(false);
      openOverview(project, 'overview');
      setNotice(`已创建「${project.name}」。可以直接进入项目；点击“复制 AI 接手说明”，粘贴给其他 AI 并补充你的需求。`);
      await refresh();
    } catch (cause) {
      setError(messageFrom(cause));
    } finally {
      createPending.current = false;
      setCreating(false);
    }
  }

  async function openProject(project: ProjectInfo, target: 'desktop' | 'browser', reload = false) {
    if (openPending.current) return;
    openPending.current = true;
    setOpening(`${project.id}:${target}`);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<OpenProjectResult>('project_open', { projectId: project.id, target, reload });
      setNotice(target === 'browser'
        ? `已将「${project.name}」交给默认浏览器打开。`
        : reload ? `已重新加载「${project.name}」的当前网页产物。` : result.reused ? `已切换到「${project.name}」的现有窗口。如提示需要重载，请先处理未保存内容，再明确重新加载。` : `已在独立窗口打开「${project.name}」。`);
    } catch (cause) {
      setError(messageFrom(cause));
    } finally {
      openPending.current = false;
      setOpening(null);
    }
  }

  useEffect(() => {
    const pending = Object.values(tasks).filter((task) => !task.final);
    if (!pending.length) return;
    let cancelled = false;
    let polling = false;
    const timer = window.setInterval(() => {
      if (polling) return;
      polling = true;
      void Promise.all(pending.map(async (previous) => {
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId });
          if (cancelled) return;
          if (task.projectId) setTasks((current) => ({ ...current, [task.projectId!]: task }));
          if (task.final) {
            setBuilding((current) => current.filter((id) => id !== task.projectId));
            if (task.projectId) buildPending.current.delete(task.projectId);
            void refresh();
          }
        } catch (cause) {
          if (!cancelled) setError(`任务状态暂时无法读取：${messageFrom(cause)}。任务不会因此取消。`);
        }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [tasks, refresh]);

  useEffect(() => {
    const pending = Object.values(lifecycleTasks).filter((task) => !task.final);
    if (!pending.length) return;
    let cancelled = false; let polling = false;
    const timer = window.setInterval(() => {
      if (polling) return;
      polling = true;
      void Promise.all(pending.map(async (previous) => {
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId });
          if (cancelled) return;
          acceptLifecycleTask(task);
          if (task.final) void refresh();
        } catch (cause) { if (!cancelled) setError(`任务状态暂时无法读取：${messageFrom(cause)}。任务仍在继续，请刷新状态；不要重复提交操作。`); }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [lifecycleTasks, acceptLifecycleTask, refresh]);

  async function buildProject(project: ProjectInfo) {
    if (buildPending.current.has(project.id)) return;
    buildPending.current.add(project.id);
    setBuilding((current) => [...current, project.id]);
    setError(null);
    setNotice(null);
    try {
      const status = await invoke<MaintenanceStatus>('maintenance_status', { projectId: project.id });
      const task = await invoke<TaskRecord>('project_build', { projectId: project.id, input: status.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {} });
      setTasks((current) => ({ ...current, [project.id]: task }));
      if (task.final) { buildPending.current.delete(project.id); setBuilding((current) => current.filter((id) => id !== project.id)); }
    } catch (cause) {
      buildPending.current.delete(project.id);
      setBuilding((current) => current.filter((id) => id !== project.id));
      setError(messageFrom(cause));
    }
  }

  async function copyHandoff(project: ProjectInfo) {
    if (selectedOverviewId.current !== project.id || workspaceBusy || !projects.some((value) => value.id === project.id) || handoffInFlight.current !== null) return;
    const sequence = ++handoffRequest.current;
    handoffInFlight.current = sequence;
    const initialInstance = instanceId.current;
    const current = () => sequence === handoffRequest.current && selectedOverviewId.current === project.id && initialInstance === instanceId.current;
    setError(null); setNotice(null); setHandoffCopying(true);
    try {
      const data = await invoke<{ text: string }>('project_handoff', { projectId: project.id });
      if (!current()) return;
      setHandoff(data.text);
      try {
        // A system clipboard write cannot be cancelled after closing a detail.
        // Keep writes ordered across projects so an older write cannot finish last.
        const write = clipboardQueue.current.then(async () => {
          if (current()) await navigator.clipboard.writeText(data.text);
        });
        clipboardQueue.current = write.catch(() => {});
        await write;
        if (current()) setNotice(`已复制「${project.name}」的 AI 接手说明。粘贴给其他 AI，再补充你想做的功能即可。`);
      } catch {
        if (current()) setNotice('说明已准备好，可以选中下面的文字并复制给 AI。');
      }
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally {
      if (handoffInFlight.current === sequence) handoffInFlight.current = null;
      if (current()) setHandoffCopying(false);
    }
  }

  function acceptTask(task: TaskRecord) {
    if (!task.projectId) { acceptLifecycleTask(task); return; }
    if (task.type === 'delete') setRequestedDelete({ projectId: task.projectId, taskId: task.taskId });
    setTasks((current) => ({ ...current, [task.projectId!]: task }));
  }

  useEffect(() => {
    if (!requestedDelete) return;
    const task = lifecycleTasks[requestedDelete.taskId] ?? tasks[requestedDelete.projectId];
    if (task?.taskId !== requestedDelete.taskId || !task.final) return;
    if (task.status === 'succeeded' && selectedOverviewId.current === requestedDelete.projectId) {
      dismissOverview();
      setNotice('项目已移入回收站，可在“空间与工作区 → 回收站”恢复。');
    }
    setRequestedDelete(null);
  }, [requestedDelete, tasks, lifecycleTasks, dismissOverview]);

  function projectBlocked(project: ProjectInfo) {
    const status = maintenance[project.id];
    return workspaceBusy || !status || status.mode === 'maintenance' || status.recoveryRequired;
  }

  const currentOverviewProject = projects.find((project) => project.id === overview?.project.id);
  const overviewProject = currentOverviewProject ?? overview?.project;
  const overviewUnavailable = Boolean(overview && !currentOverviewProject);
  const activeLifecycleTasks = Object.values(lifecycleTasks).filter((task) => !task.final);
  const completedLifecycleTasks = Object.values(lifecycleTasks).filter((task) => task.final).slice(-6).reverse();
  const modalOpen = Boolean(overviewProject && overview) || showCreate;
  const requestCloseOverview = () => { if (!overviewDismissGuard.current || overviewDismissGuard.current()) dismissOverview(); };
  const feedback = <>
    {error && <div className="notice notice-error" role="alert"><CircleAlert size={20} /><div><strong>操作未完成</strong><p>{error}</p></div><button className="icon-button" aria-label="关闭错误提示" onClick={() => setError(null)}><X size={16} /></button></div>}
    {notice && <div className="notice notice-success" role="status"><Check size={18} /><p>{notice}</p><button className="icon-button" aria-label="关闭操作提示" onClick={() => setNotice(null)}><X size={16} /></button></div>}
  </>;
  const resourcesReady = info && Object.values(info.resources).every(Boolean);
  const title = page === 'projects' ? '我的项目' : page === 'storage' ? '空间与工作区' : page === 'about' ? '关于平台' : '运行环境';

  return (
    <ManagerShell page={page} onPageChange={setPage} projectCount={projects.length} title={title}
      description={page === 'projects' ? '选择项目开始使用，或在详情中查看信息和管理数据。' : page === 'storage' ? '查看空间占用、切换工作区位置，或恢复已删除的项目。' : '查看平台与本地服务的信息。'}
      versionLabel={info ? 'v' + info.version : 'M5'}
      actions={<ManagerActions loading={loading} inDesktop={inDesktop} refreshDisabled={false}
        createDisabled={!capabilities.create || !info || creating || workspaceBusy}
        importDisabled={!capabilities.importProject || !info || workspaceBusy} showProjectActions={page === 'projects'}
        onRefresh={refresh} onCreate={() => { setShowCreate(true); setNotice(null); }} onImport={() => setShowImport(true)} />}
    >
          {!inDesktop && <div className="notice" role="status"><Monitor size={20} /><div><strong>项目管理在桌面应用中进行</strong><p>这里是前端开发页面。启动桌面应用后创建项目，再从项目卡片选择“浏览器打开”访问真实项目。</p></div></div>}
          {!modalOpen && feedback}
          {cleanupBusy && <div className="notice" role="status"><Trash2 size={18} /><div><strong>历史清理正在进行，管理写操作暂时停用</strong><p>当前项目及其草稿保留。请等待清理任务的最终结果，不要重复提交。</p></div></div>}
          {migrating && <div className="notice" role="status"><RefreshCw size={18} /><div><strong>工作区正在迁移，新操作暂时暂停</strong><p>各项目窗口保留未保存内容。请等待任务最终结果，不要重复提交迁移。</p></div></div>}
          {activeLifecycleTasks.length > 0 && <div className="lifecycle-tasks" aria-label="进行中的管理任务">{activeLifecycleTasks.map((task) => <ProjectTask key={task.taskId} task={task} />)}</div>}
          {completedLifecycleTasks.length > 0 && <details className="recent-tasks"><summary>最近的管理任务 <span>{completedLifecycleTasks.length}</span></summary><div className="lifecycle-tasks">{completedLifecycleTasks.map((task) => <ProjectTask key={task.taskId} task={task} />)}</div></details>}
          {page === 'projects' ? <>
            {showImport && <ImportPanel onTask={acceptLifecycleTask} onClose={() => setShowImport(false)} disabled={!info || workspaceBusy} />}
            <ProjectCollection projects={projects} query={query} onQueryChange={setQuery} filter={projectFilter} onFilterChange={setProjectFilter}
              isProjectReady={(project) => project.state === 'ready' && !projectBlocked(project)}
              cardProps={(project) => ({ status: maintenance[project.id], task: tasks[project.id], blocked: projectBlocked(project) || opening !== null,
                opening: opening === project.id + ':desktop', deletingDisabled: !capabilities.deleteProject || workspaceBusy,
                onEnter: () => void openProject(project, 'desktop'), onDetails: () => openOverview(project, 'overview'), onDelete: () => openOverview(project, 'manage') })}
              loading={loading} checkedAt={checkedAt} connected={Boolean(info)} createDisabled={workspaceBusy || !capabilities.create}
              onCreate={() => setShowCreate(true)} workspaceRoot={info?.workspaceRoot}
              guide={<FirstStartGuide enabled={Boolean(info) && !workspaceBusy} onCreate={() => setShowCreate(true)} onEnvironment={() => setPage('runtime')} onStorage={() => setPage('storage')} />}
            />
          </> : page === 'storage' ? <WorkspaceHub workspace={workspace} revision={refreshVersion} enabled={Boolean(info)} migrating={migrating} cleanupTask={cleanupTask} projects={projects} onTask={acceptLifecycleTask} onRefresh={refresh} onSelect={(projectId) => { const project = projects.find((value) => value.id === projectId); if (project) openOverview(project, 'storage'); }} /> : page === 'about' ? <AboutPlatform info={info} /> : <>
            <section className="status-grid" aria-label="运行状态">
              <StatusCard icon={<Activity size={18} />} label="本地服务" value={info ? '已启动' : loading ? '读取中' : '未读取'} detail={info ? info.baseUrl : '由 Rust 提供本机服务'} good={Boolean(info)} />
              <StatusCard icon={<Database size={18} />} label="内置 SQLite" value={info ? `SQLite ${info.sqliteVersion}` : loading ? '读取中' : '未读取'} detail="每个项目使用独立数据库文件" good={Boolean(info)} />
              <StatusCard icon={<Package size={18} />} label="包内资源" value={info ? resourcesReady ? '读取正常' : '需要检查' : loading ? '读取中' : '未读取'} detail="CLI、SDK 发布包与 React 模板" good={Boolean(resourcesReady)} />
            </section>
            <BuildEnvironment enabled={Boolean(info)} />
            <section className="diagnostics-card" aria-labelledby="diagnostics-title">
              <div className="section-heading"><div><h2 id="diagnostics-title">运行环境</h2><p>实际目录与实例信息，供开发和打包验证使用。</p></div><span className="updated-at">{checkedAt ? `读取于 ${checkedAt}` : '等待桌面连接'}</span></div>
              <dl className="path-list"><PathRow icon={<Folder size={16} />} label="开发数据目录" value={info?.dataRoot} /><PathRow icon={<FolderOpen size={16} />} label="项目工作区" value={info?.workspaceRoot} /><PathRow icon={<Terminal size={16} />} label="CLI 路径" value={info ? info.cliPath ?? '未找到包内 CLI' : undefined} /></dl>
              <div className="runtime-details"><div><span>运行程序</span><code>{info?.displayName ?? '尚未读取'}</code></div><div><span>应用标识</span><code>{info?.identifier ?? '尚未读取'}</code></div><div><span>实例 ID</span><code>{info?.instanceId ?? '尚未读取'}</code></div><div><span>接口版本</span><code>{info ? `v${info.apiVersion}` : '尚未读取'}</code></div></div>
              <div className="resource-strip"><span>资源探测</span>{(Object.entries(resourceNames) as [keyof BootstrapInfo['resources'], string][]).map(([key, label]) => <span className={`resource-item ${info ? info.resources[key] ? 'resource-ok' : 'resource-missing' : ''}`} key={key}>{info ? info.resources[key] ? <Check size={13} /> : <CircleAlert size={13} /> : <span className="unknown-dot" />}{label}</span>)}</div>
            </section>
            <div className="runtime-explainer"><ShieldCheck size={17} /><p>当前是内部开发版，只使用上方的专用数据目录。关闭管理窗口会保留本地服务；从应用菜单或托盘选择“退出开发版”后，服务才会停止。</p></div>
          </>}
        {showCreate && <CreateProjectForm name={name} description={description} onNameChange={setName} onDescriptionChange={setDescription}
          onSubmit={createProject} onClose={() => setShowCreate(false)} creating={creating} disabled={workspaceBusy} feedback={feedback} />}
        {overviewProject && overview && <Modal title={overviewProject.name} description="项目详情与维护" wide beforeClose={() => !overviewDismissGuard.current || overviewDismissGuard.current()} onClose={dismissOverview}>
          {feedback}
          {overviewUnavailable && <div className="notice" role="status"><CircleAlert size={18} /><div><strong>项目已不在当前工作区列表中</strong><p>可能已被其他操作移入回收站或工作区已经切换。此处保留你的编辑供复制，项目操作已暂停。</p></div></div>}
          <div className="project-dialog-actions">
            <button className="primary-button" disabled={overviewUnavailable || overviewProject.state !== 'ready' || projectBlocked(overviewProject) || opening !== null} onClick={() => void openProject(overviewProject, 'desktop')}><Monitor size={16} />进入项目</button>
            <button className="refresh-button" disabled={overviewUnavailable || overviewProject.state !== 'ready' || projectBlocked(overviewProject) || opening !== null} onClick={() => void openProject(overviewProject, 'browser')}><Globe size={16} />浏览器打开</button>
            <button className="refresh-button" disabled={overviewUnavailable || workspaceBusy || building.includes(overviewProject.id) || Boolean(tasks[overviewProject.id] && !tasks[overviewProject.id].final) || !maintenance[overviewProject.id] || maintenance[overviewProject.id]?.recoveryRequired || (maintenance[overviewProject.id]?.mode === 'maintenance' && !maintenance[overviewProject.id]?.readyForChanges) || overviewProject.state === 'missing' || overviewProject.state === 'error'} onClick={() => void buildProject(overviewProject)}><Hammer size={16} />{building.includes(overviewProject.id) ? '构建中…' : '构建项目'}</button>
          </div>
          <section className="handoff-card" aria-labelledby="handoff-title"><div className="section-heading"><div><h3 id="handoff-title">让其他 AI 继续开发</h3><p>一键复制软件介绍、当前项目目录和开发操作说明。粘贴给 AI 后，再告诉它你的需求。</p></div><button className="primary-button" disabled={overviewUnavailable || workspaceBusy || handoffCopying} onClick={() => void copyHandoff(overviewProject)}><Copy size={16} />{handoffCopying ? '正在准备说明…' : '复制 AI 接手说明'}</button></div><p className="project-help">接收说明的 AI 需要能访问这台电脑上的项目目录。仅聊天的 AI 也能了解背景，但不能直接修改本机文件。</p>{handoff && <div className="handoff-preview"><div className="section-heading"><label htmlFor="ai-handoff-text">完整说明 · 可核对或手动复制</label><button className="text-button" onClick={clearHandoff}>收起全文</button></div><textarea id="ai-handoff-text" readOnly value={handoff} rows={9} aria-label="给 AI 的完整说明" onFocus={(event) => event.currentTarget.select()} /></div>}</section>
          {tasks[overviewProject.id] && (tasks[overviewProject.id].final && tasks[overviewProject.id].status === 'succeeded'
            ? <details className="project-last-task"><summary>最近任务已完成 · 查看结果与日志</summary><ProjectTask task={tasks[overviewProject.id]} /></details>
            : <ProjectTask task={tasks[overviewProject.id]} />)}
          <ProjectOverview key={overviewProject.id} project={overviewProject} initialTab={overview.tab} status={maintenance[overviewProject.id]} task={tasks[overviewProject.id]} onTask={(task) => { acceptTask(task); if (['export', 'copy', 'delete'].includes(task.type)) acceptLifecycleTask(task); }} onChanged={refresh} onClose={requestCloseOverview} onDismissGuardChange={registerOverviewGuard} disabled={overviewUnavailable || workspaceBusy || Object.values(lifecycleTasks).some((task) => task.projectId === overviewProject.id && !task.final)} />
          <details className="project-technical-details"><summary>运行信息与项目路径</summary><BuildState project={overviewProject} /><ProjectDetails project={overviewProject} />
            {!overviewUnavailable && overviewProject.state === 'ready' && !projectBlocked(overviewProject) && <button className="text-button reload-button" disabled={opening !== null} onClick={() => setReloadProjectId(overviewProject.id)}><RefreshCw size={14} />重新加载项目窗口</button>}
            {reloadProjectId === overviewProject.id && <div className="operation-confirmation"><strong>重新加载会丢弃项目窗口未保存的内容</strong><p>请先复制草稿或保存到别处。</p><div className="confirmation-actions"><button className="refresh-button" disabled={overviewUnavailable || opening !== null || projectBlocked(overviewProject)} onClick={() => { setReloadProjectId(null); void openProject(overviewProject, 'desktop', true); }}>已处理草稿，重新加载</button><button className="text-button" onClick={() => setReloadProjectId(null)}>取消并保留页面</button></div></div>}
          </details>
        </Modal>}
    </ManagerShell>
  );
}

function ProjectDetails({ project }: { project: ProjectInfo }) {
  const location = (relative: string | undefined) => relative ? `${project.projectRoot}/${relative}` : '清单不可用';
  return <div className="project-details">
    {project.state === 'unbuilt' && <p className="project-help">源码模板和数据库已准备好。先构建项目，即可在桌面或浏览器打开；也可以先让 AI 修改源码。</p>}
    {project.state === 'ready' && <p className="project-help">{project.buildInfo?.origin === 'bundledTemplate' ? '这是随平台提供的初始应用，可以直接打开。让 AI 修改源码后，再构建发布新网页。' : '当前网页可以继续使用。构建会发布新网页，已打开的窗口不会自动刷新。'}</p>}
    {project.buildInfo?.reason && <p className="project-help">构建来源：{project.buildInfo.reason}</p>}
    {project.buildInfo?.provenance && <p className="project-help">{project.buildInfo.origin === 'bundledTemplate' ? '初始应用打包时间' : '最近成功构建'}：{new Date(project.buildInfo.provenance.completedAt).toLocaleString('zh-CN', { hour12: false })} · Node {project.buildInfo.provenance.node?.version ?? project.buildInfo.provenance.nodeVersion ?? '版本未记录'}</p>}
    {project.issues.length > 0 && <ul className="project-issues">{project.issues.map((issue, index) => <li key={`${index}:${issue}`}>{issue}</li>)}</ul>}
    <dl><div><dt>项目目录</dt><dd><code>{project.projectRoot}</code></dd></div><div><dt>数据库</dt><dd><code>{location(project.manifest?.database)}</code></dd></div><div><dt>用户素材</dt><dd><code>{location(project.manifest?.attachmentsDir)}</code></dd></div><div><dt>给 AI 的说明</dt><dd><code>{location(project.manifest?.docs?.readme)}</code></dd></div><div><dt>项目体积</dt><dd>请从项目概览的“空间与素材”查看扫描结果</dd></div></dl>
  </div>;
}
const sourceLabels = { unbuilt: '尚未构建', matched: '源码与网页一致', modified: '源码已修改 · 网页仍为旧版', unknown: '构建来源未知' };
function BuildState({ project }: { project: ProjectInfo }) {
  const state = project.buildInfo?.sourceState ?? (project.state === 'unbuilt' ? 'unbuilt' : 'unknown');
  return <p className={`build-source source-${state}`}><Package size={13} />{state === 'matched' && project.buildInfo?.origin === 'bundledTemplate' ? '初始应用 · 可直接打开' : sourceLabels[state]}{project.buildInfo?.stale ? ' · 待重新核对' : ''}</p>;
}


function BuildEnvironment({ enabled }: { enabled: boolean }) {
  const [environment, setEnvironment] = useState<EnvironmentInfo | null>(null);
  const [nodePath, setNodePath] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const request = useRef(0);
  const probe = useCallback(async () => {
    if (!enabled || pending.current) return;
    const id = ++request.current;
    pending.current = true;
    setLoading(true);
    try {
      const value = await invoke<EnvironmentInfo>('environment_doctor');
      if (id !== request.current) return;
      setEnvironment(value);
      setNodePath(value.configuredNodePath ?? '');
      setError(null);
    } catch (cause) { if (id === request.current) setError(messageFrom(cause)); }
    finally { pending.current = false; if (id === request.current) setLoading(false); }
  }, [enabled]);
  useEffect(() => { void probe(); }, [probe]);
  async function configure(path: string | null) {
    if (pending.current) return;
    pending.current = true;
    setLoading(true);
    try {
      const value = await invoke<EnvironmentInfo>('environment_node_set', { nodePath: path });
      setEnvironment(value);
      setNodePath(value.configuredNodePath ?? '');
      setError(null);
    } catch (cause) { setError(messageFrom(cause)); }
    finally { pending.current = false; setLoading(false); }
  }
  return <section className="diagnostics-card build-environment" aria-labelledby="build-environment-title"><div className="section-heading"><div><h2 id="build-environment-title">项目构建环境</h2><p>使用系统 Node。已有项目的网页和数据操作不依赖 Node。</p></div><button className="refresh-button" onClick={() => void probe()} disabled={!enabled || loading}><RefreshCw size={14} />{loading ? '检测中…' : '重新检测'}</button></div><div className="environment-status"><span className={`project-state state-${environment?.ready ? 'ready' : 'unbuilt'}`}><span />{environment ? environment.ready ? '可以构建' : '构建环境需要配置' : loading ? '正在检测' : '尚未检测'}</span><span>最低 22.12 · 支持 22 / 24 · 推荐 24 LTS</span></div><dl className="path-list"><PathRow icon={<Terminal size={16} />} label={`Node ${environment?.node?.version ?? ''}`} value={environment ? environment.node?.path ?? '未找到可用 Node' : undefined} /><PathRow icon={<Terminal size={16} />} label={`npm ${environment?.npm?.version ?? ''}`} value={environment ? environment.npm?.path ?? '未找到配套 npm' : undefined} /></dl>{environment?.issues.map((issue) => <p key={issue.code} className="environment-issue">{issue.message} ({issue.code})</p>)}{environment && !environment.ready && <p className="environment-issue">安装指引：<code>{environment.downloadUrl}</code></p>}<form className="node-path-form" onSubmit={(event) => { event.preventDefault(); void configure(nodePath.trim()); }}><label htmlFor="node-path">指定 Node 可执行文件的绝对路径<span className="optional-label">可选</span></label><div className="node-path-controls"><input id="node-path" value={nodePath} onChange={(event) => setNodePath(event.target.value)} placeholder="例如 /opt/homebrew/bin/node" disabled={loading} autoComplete="off" /><button className="primary-button" type="submit" disabled={!enabled || loading || !nodePath.trim().startsWith('/')}>保存路径</button><button className="refresh-button" type="button" disabled={!enabled || loading} onClick={() => void configure(null)}>自动查找</button></div><p>当前选择：{environment?.selection === 'explicit' ? '手动指定' : environment ? '自动探测' : '尚未读取'}。路径只保存在此开发版的本机设置中。</p></form>{error && <p role="alert" className="environment-issue">{error}</p>}</section>;
}

function StatusCard({ icon, label, value, detail, good }: { icon: ReactNode; label: string; value: string; detail: string; good: boolean }) {
  return <div className="status-card"><div className="status-card-label">{icon}<span>{label}</span>{good && <Check size={14} className="status-check" aria-label="已验证" />}</div><strong>{value}</strong><p>{detail}</p></div>;
}
function PathRow({ icon, label, value }: { icon: ReactNode; label: string; value: string | undefined }) {
  return <div className="path-row"><dt>{icon}{label}</dt><dd><code>{value ?? '尚未读取'}</code></dd></div>;
}
export default App;
