import { invoke, isTauri } from '@tauri-apps/api/core';
import {
  Activity, ArrowUpRight, Boxes, Check, ChevronDown, ChevronRight, CircleAlert,
  Database, Folder, FolderOpen, Globe, LayoutGrid, Monitor, Package, Plus,
  RefreshCw, ShieldCheck, Terminal, X, Hammer, Copy, Archive, Trash2, Upload, Settings2,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { ProjectTask } from './ProjectTask';
import { ImportPanel, StoragePage } from './LifecyclePanels';
import { ProjectOverview } from './ProjectOverview';
import type { OverviewTab } from './ProjectOverview';
import { WorkspaceStorageSummary, ProjectSpaceRanking } from './StoragePanels';
import { AboutPlatform, FirstStartGuide } from './PlatformPanels';
import { bytesText, timeText } from './storage-utils';
import { messageFrom } from './ui-utils';
import type { BootstrapInfo, EnvironmentInfo, OpenProjectResult, ProjectInfo, MaintenanceStatus, TaskRecord, WorkspaceInfo } from './types';

const resourceNames: Record<keyof BootstrapInfo['resources'], string> = {
  sdk: 'SDK 0.1.0', template: 'React 项目模板', readme: '开发说明', cli: '原生 CLI',
};
const projectStates: Record<ProjectInfo['state'], string> = {
  ready: '可打开', unbuilt: '等待构建', missing: '文件缺失', error: '需要检查', maintenance: '维护中', recoveryRequired: '恢复待处理',
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
  const [expanded, setExpanded] = useState<string | null>(null);
  const [tasks, setTasks] = useState<Record<string, TaskRecord>>({});
  const [maintenance, setMaintenance] = useState<Record<string, MaintenanceStatus>>({});
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [lifecycleTasks, setLifecycleTasks] = useState<Record<string, TaskRecord>>({});
  const [showImport, setShowImport] = useState(false);
  const [overview, setOverview] = useState<{ projectId: string; tab: OverviewTab } | null>(null);
  const [reloadProjectId, setReloadProjectId] = useState<string | null>(null);
  const [building, setBuilding] = useState<string[]>([]);
  const [handoff, setHandoff] = useState<string | null>(null);
  const buildPending = useRef(new Set<string>());
  const requestId = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const inDesktop = isTauri();
  const instanceId = useRef<string | null>(null);
  const migrating = Boolean(workspace?.activeTask) || ['running', 'paused', 'migrating'].includes(workspace?.migrationState ?? '') || Object.values(lifecycleTasks).some((task) => task.type === 'workspaceMigration' && !task.final);
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
        setHandoff(null);
        setNotice('工作区服务已切换。旧窗口的草稿仍保留；处理草稿后请明确重新加载项目，并从新目录重新复制 AI 说明。');
      }
      instanceId.current = runtime.instanceId;
      setWorkspace(currentWorkspace);
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
  }, [acceptLifecycleTask]);

  useEffect(() => {
    void refresh();
    return () => { requestId.current += 1; };
  }, [refresh]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPending.current || migrating || !name.trim()) return;
    createPending.current = true;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      const project = await invoke<ProjectInfo>('project_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      setProjects((previous) => [...previous.filter((item) => item.id !== project.id), project]);
      setExpanded(project.id);
      setName('');
      setDescription('');
      setShowCreate(false);
      setHandoff(null);
      setNotice(`已创建「${project.name}」。初始应用已准备好，可以直接打开，也可以交给 AI 继续修改。`);
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
    setExpanded(project.id);
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
    setError(null);
    try {
      const data = await invoke<{ text: string }>('project_handoff', { projectId: project.id });
      setHandoff(data.text);
      try {
        await navigator.clipboard.writeText(data.text);
        setNotice(`已复制「${project.name}」的项目路径、接管 skill 和操作说明，可直接交给外部 AI。`);
      } catch {
        setNotice('说明已准备好，可以选中下面的文字并复制给 AI。');
      }
    } catch (cause) { setError(messageFrom(cause)); }
  }

  function acceptTask(task: TaskRecord) {
    if (!task.projectId) { acceptLifecycleTask(task); return; }
    setTasks((current) => ({ ...current, [task.projectId!]: task }));
    setExpanded(task.projectId);
  }

  function projectBlocked(project: ProjectInfo) {
    const status = maintenance[project.id];
    return migrating || !status || status.mode === 'maintenance' || status.recoveryRequired;
  }

  const overviewProject = projects.find((project) => project.id === overview?.projectId);
  const resourcesReady = info && Object.values(info.resources).every(Boolean);
  const title = page === 'projects' ? '我的项目' : page === 'storage' ? '空间与工作区' : page === 'about' ? '关于平台' : '运行环境';

  return (
    <div className="app-shell">
      <aside className="sidebar" aria-label="工作台导航">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true"><Boxes size={22} strokeWidth={1.6} /></span>
          <div><strong>本地项目</strong><span>一个属于你的工作空间</span></div>
        </div>
        <div className="sidebar-section-label">工作台</div>
        <nav className="side-nav">
          <button className={page === 'projects' ? 'nav-current' : 'nav-item'} aria-current={page === 'projects' ? 'page' : undefined} onClick={() => setPage('projects')}><LayoutGrid size={17} /><span>我的项目</span><small>{projects.length}</small></button>
          <button className={page === 'runtime' ? 'nav-current' : 'nav-item'} aria-current={page === 'runtime' ? 'page' : undefined} onClick={() => setPage('runtime')}><Activity size={17} /><span>运行环境</span></button>
          <button className={page === 'storage' ? 'nav-current' : 'nav-item'} aria-current={page === 'storage' ? 'page' : undefined} onClick={() => setPage('storage')}><Trash2 size={17} /><span>空间与工作区</span></button>
          <button className={page === 'about' ? 'nav-current' : 'nav-item'} aria-current={page === 'about' ? 'page' : undefined} onClick={() => setPage('about')}><Package size={17} /><span>关于平台</span></button>
        </nav>
        <div className="sidebar-stage">
          <span className="eyebrow">LOCAL FIRST</span>
          <strong>项目各自独立，数据留在本机。</strong>
          <p>桌面窗口和浏览器连接同一个本地服务，实时读取项目数据。</p>
          <span className="subtle-tag">M5 · 开发验证</span>
        </div>
        <div className="sidebar-bottom"><span className="environment-dot" />内部开发版<span className="version">{info ? `v${info.version}` : 'M5'}</span></div>
      </aside>

      <div className="content-shell">
        <header className="topbar">
          <span>工作台 <span className="breadcrumb-slash">/</span> <strong>{title}</strong></span>
          <span className="development-badge"><ShieldCheck size={13} />独立开发环境</span>
        </header>
        <main>
          <div className="page-heading">
            <div>
              <div className="eyebrow">YOUR LOCAL WORKSPACE</div>
              <h1>{title}</h1>
              <p>{page === 'projects' ? '把每个想法，放进自己的项目里。' : '查看当前程序、服务和开发数据的位置。'}</p>
            </div>
            <div className="heading-actions">
              <button className="refresh-button" onClick={() => void refresh()} disabled={loading || !inDesktop} aria-label="刷新项目和运行状态"><RefreshCw size={15} className={loading ? 'rotating' : undefined} />{loading ? '读取中' : '刷新'}</button>
              {page === 'projects' && <button className="refresh-button" disabled={!info || migrating} onClick={() => setShowImport(true)}><Upload size={15} />导入项目</button>}
              {page === 'projects' && <button className="primary-button" onClick={() => { setShowCreate(true); setNotice(null); }} disabled={!info || creating || migrating}><Plus size={16} />新建项目</button>}
            </div>
          </div>

          {!inDesktop && <div className="notice" role="status"><Monitor size={20} /><div><strong>项目管理在桌面应用中进行</strong><p>这里是前端开发页面。启动桌面应用后创建项目，再从项目卡片选择“浏览器打开”访问真实项目。</p></div></div>}
          {error && <div className="notice notice-error" role="alert"><CircleAlert size={20} /><div><strong>操作未完成</strong><p>{error}</p><p>请检查项目文件或刷新状态后重试。</p></div><button className="icon-button" aria-label="关闭错误提示" onClick={() => setError(null)}><X size={15} /></button></div>}
          {notice && <div className="notice notice-success" role="status"><Check size={18} /><p>{notice}</p><button className="icon-button" aria-label="关闭操作提示" onClick={() => setNotice(null)}><X size={15} /></button></div>}

          {migrating && <div className="notice" role="status"><RefreshCw size={18} /><div><strong>工作区正在迁移，新操作暂时暂停</strong><p>各项目窗口保留未保存内容。请等待任务最终结果，不要重复提交迁移。</p></div></div>}
          {Object.keys(lifecycleTasks).length > 0 && <div className="lifecycle-tasks" aria-label="项目管理任务">{Object.values(lifecycleTasks).slice(-6).reverse().map((task) => <ProjectTask key={task.taskId} task={task} />)}</div>}
          {handoff && <section className="handoff-card" aria-labelledby="handoff-title"><div className="section-heading"><div><h2 id="handoff-title">交给外部 AI</h2><p>说明中包含接管 skill 的位置。允许 AI 访问这个项目目录，再告诉它你想增加的功能。</p></div><button className="icon-button" onClick={() => setHandoff(null)} aria-label="关闭 AI 说明"><X size={18} /></button></div><textarea readOnly value={handoff} rows={9} aria-label="给 AI 的完整说明" onFocus={(event) => event.currentTarget.select()} /></section>}
          {page === 'projects' ? <>
            {showImport && <ImportPanel onTask={acceptLifecycleTask} onClose={() => setShowImport(false)} disabled={!info || migrating} />}
            {overviewProject && overview && <ProjectOverview key={overviewProject.id} project={overviewProject} initialTab={overview.tab} status={maintenance[overviewProject.id]} task={tasks[overviewProject.id]} onTask={(task) => { acceptTask(task); if (['export', 'copy', 'delete'].includes(task.type)) acceptLifecycleTask(task); }} onChanged={refresh} onHandoff={() => void copyHandoff(overviewProject)} onClose={() => setOverview(null)} disabled={migrating || Object.values(lifecycleTasks).some((task) => task.projectId === overviewProject.id && !task.final)} />}
            {showCreate && <section className="create-card" aria-labelledby="create-title">
              <div className="section-heading"><div><h2 id="create-title">新建项目</h2><p>创建后即可打开初始应用，随时交给 AI 添加功能。</p></div><button className="icon-button" aria-label="取消新建项目" onClick={() => setShowCreate(false)} disabled={creating}><X size={18} /></button></div>
              <form onSubmit={(event) => void createProject(event)}>
                <label htmlFor="project-name">项目名称<span className="required-label">必填</span></label>
                <input id="project-name" autoFocus required value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：孩子的英语学习" readOnly={creating || migrating} autoComplete="off" />
                <label htmlFor="project-description">简介<span className="optional-label">选填</span></label>
                <textarea id="project-description" value={description} onChange={(event) => setDescription(event.target.value)} placeholder="这个项目准备帮你做什么？" rows={2} readOnly={creating || migrating} />
                <div className="form-footer"><span><ShieldCheck size={14} />保存到独立开发工作区</span><button className="primary-button" type="submit" disabled={creating || migrating || !name.trim()}>{creating ? '正在创建…' : '创建项目'}<ArrowUpRight size={14} /></button></div>
              </form>
            </section>}

            <FirstStartGuide enabled={Boolean(info) && !migrating} onCreate={() => setShowCreate(true)} onEnvironment={() => setPage('runtime')} onStorage={() => setPage('storage')} />
            <div className="project-list-heading"><span>{info ? `${projects.length} 个项目` : '项目列表'}</span><span>{loading ? '正在读取…' : checkedAt ? `读取于 ${checkedAt}` : '等待桌面连接'}</span></div>
            {projects.length > 0 ? <section className="project-grid" aria-label="项目列表">
              {projects.map((project) => <article className={`project-card ${expanded === project.id ? 'project-card-expanded' : ''}`} key={project.id}>
                <div className="project-card-top"><span className="project-icon">{project.iconDataUrl ? <img src={project.iconDataUrl} alt="" /> : <FolderOpen size={24} strokeWidth={1.4} />}</span><span className={`project-state state-${maintenance[project.id]?.recoveryRequired ? 'error' : maintenance[project.id]?.mode === 'maintenance' ? 'maintenance' : project.state}`}><span />{maintenance[project.id]?.recoveryRequired ? '恢复待处理' : maintenance[project.id]?.mode === 'maintenance' ? '维护中' : projectStates[project.state]}</span></div>
                <h2>{project.name}</h2>
                <p className="project-description">{project.description || '尚未填写简介'}</p>
                <code className="project-id">{project.id}</code><BuildState project={project} />
                <div className="project-space"><Database size={13} /><span>独立 SQLite</span><button className="text-button" onClick={() => setOverview({ projectId: project.id, tab: 'storage' })}>{bytesText(project.storage?.currentProject.totalBytes)}{project.storage?.currentProject.stale ? ' · 待重新扫描' : project.storage?.currentProject.complete === false ? ' · 部分统计' : ''}</button></div>
                <p className="storage-timestamp">{project.storage?.currentProject.completedAt ? `扫描于 ${timeText(project.storage.currentProject.completedAt)}` : '尚无空间扫描结果'}</p>{project.iconStatus?.state === 'invalid' && <p className="environment-issue">图标不可用，已使用默认图标。</p>}<div className="project-actions">
                  <button className="primary-button" disabled={project.state !== 'ready' || projectBlocked(project) || opening !== null} onClick={() => void openProject(project, 'desktop')}><Monitor size={14} />{opening === `${project.id}:desktop` ? '打开中…' : '打开项目'}</button>
                  <button className="refresh-button" disabled={project.state !== 'ready' || projectBlocked(project) || opening !== null} onClick={() => void openProject(project, 'browser')} aria-label={`在浏览器打开${project.name}`}><Globe size={14} />{opening === `${project.id}:browser` ? '打开中…' : '浏览器打开'}</button>
                </div>
                <div className="project-secondary-actions"><button className="refresh-button" disabled={migrating || building.includes(project.id) || Boolean(tasks[project.id] && !tasks[project.id].final) || !maintenance[project.id] || maintenance[project.id]?.recoveryRequired || (maintenance[project.id]?.mode === 'maintenance' && !maintenance[project.id]?.readyForChanges) || project.state === 'missing' || project.state === 'error'} onClick={() => void buildProject(project)}><Hammer size={14} />{building.includes(project.id) ? '构建中…' : '构建项目'}</button><button className="refresh-button" onClick={() => void copyHandoff(project)}><Copy size={14} />复制给 AI</button><button className="refresh-button" disabled={migrating} onClick={() => setOverview({ projectId: project.id, tab: 'backup' })}><Archive size={14} />备份与维护</button><button className="refresh-button" onClick={() => setOverview({ projectId: project.id, tab: 'overview' })}><Settings2 size={14} />项目概览</button></div>
                <button className="details-toggle" aria-expanded={expanded === project.id} onClick={() => setExpanded(expanded === project.id ? null : project.id)}>{expanded === project.id ? <ChevronDown size={14} /> : <ChevronRight size={14} />}查看详情</button>
                {expanded === project.id && <><ProjectDetails project={project} />{project.state === 'ready' && !projectBlocked(project) && <button className="text-button reload-button" disabled={opening !== null} onClick={() => setReloadProjectId(project.id)}><RefreshCw size={13} />重新加载项目</button>}{reloadProjectId === project.id && <div className="operation-confirmation"><strong>重新加载会丢弃当前窗口未保存的内容</strong><p>请先复制草稿或保存到别处。取消会保留当前页面，旧页面在维护或恢复后仍不能继续保存。</p><div className="confirmation-actions"><button className="refresh-button" disabled={opening !== null || projectBlocked(project)} onClick={() => { setReloadProjectId(null); void openProject(project, 'desktop', true); }}>已处理草稿，重新加载</button><button className="text-button" onClick={() => setReloadProjectId(null)}>取消并保留页面</button></div></div>}{tasks[project.id] && <ProjectTask key={tasks[project.id].taskId} task={tasks[project.id]} />}</>}
              </article>)}
            </section> : <section className="workspace-card empty-projects">
              <div className="workspace-art" aria-hidden="true"><div className="art-sheet art-sheet-back" /><div className="art-sheet art-sheet-front"><FolderOpen size={38} strokeWidth={1.2} /><span /><span /></div><span className="art-small-square" /></div>
              <div className="workspace-message"><span className="subtle-tag">独立测试工作区</span><h2>{loading ? '正在读取你的项目…' : info ? '从第一个项目开始。' : '连接桌面应用，查看项目。'}</h2><p>{info ? '给项目起一个名字。平台保存数据和文件，项目网页可以由外部 AI 编辑，再在桌面或浏览器中使用。' : '实际项目列表由桌面应用读取；此页面不会生成演示数据。'}</p><div className="workspace-footnote"><ShieldCheck size={15} />开发数据与正式数据保持分离</div></div>
            </section>}
            <div className="workspace-location"><Folder size={14} /><span>工作区</span><code>{info?.workspaceRoot ?? '尚未读取'}</code></div>
            <footer className="page-footer"><span>M5 · 项目概览与空间管理</span><span>源码修改后重新构建；已打开的页面由你决定何时重新加载。</span></footer>
          </> : page === 'storage' ? <><WorkspaceStorageSummary revision={refreshVersion} enabled={Boolean(info) && !migrating} /><ProjectSpaceRanking projects={projects} onSelect={(projectId) => { setOverview({ projectId, tab: 'storage' }); setPage('projects'); }} /><StoragePage workspace={workspace} revision={refreshVersion} enabled={Boolean(info)} onTask={acceptLifecycleTask} onRefresh={refresh} /></> : page === 'about' ? <AboutPlatform info={info} /> : <>
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
        </main>
      </div>
    </div>
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
