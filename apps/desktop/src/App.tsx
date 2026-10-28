import { invoke, isTauri } from '@tauri-apps/api/core';
import {
  BookOpen, CircleAlert, Play,
  Download, FolderOpen, Monitor, Package, Plus,
  RefreshCw, ShieldCheck, Hammer, Trash2, Search, Settings, ChevronRight,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { ProjectTask } from './ProjectTask';
import { ProjectOverview } from './ProjectOverview';
import { ProjectDeleteDialog } from './ProjectDeleteDialog';
import type { OverviewTab } from './ProjectOverview';
import { WorkspaceHub } from './WorkspaceHub';
import type { SpaceTab } from './WorkspaceHub';
import { SettingsDialog } from './SettingsDialog';
import type { SettingsSection } from './SettingsDialog';
import { TaskCenter } from './TaskCenter';
import { Modal } from './Modal';
import { NodeProjectCard, ProjectCard } from './ProjectCard';
import { NodeAppsPanel } from './NodeAppsPanel';
import { NodeDeletionTasksNotice } from './NodeDeletionTasksNotice';
import type { NodeAppRequest } from './NodeAppsPanel';
import type { NodeAppDeleteResult, NodeAppDetail, NodeDeletionTaskSnapshot } from './node-app-types';
import { messageFrom } from './ui-utils';
import { useAppearance } from './appearance';
import { SourceAcquisitionPanel } from './SourceAcquisitionPanel';
import { ImportPanel } from './ArchiveImportPanel';
import { SourceRecoveryNotice } from './SourceRecoveryNotice';
import { CodexHandoffPanel } from './CodexHandoffPanel';
import { FeedbackNotices, useManagementFeedback } from './ManagementFeedback';
import { deriveWorkspaceTaskState, mergeLifecycleTask } from './workspace-task-state';
import { matchingProjectTask, recoverBuildResult } from './build-result-state';
import type { BuildResultView } from './build-result-state';
import type { HandoffAction, ProjectHandoff } from './CodexHandoffPanel';
import type { BootstrapInfo, OpenProjectResult, ProjectInfo, MaintenanceStatus, TaskRecord, WorkspaceInfo } from './types';

type AppPage = 'projects' | 'storage';

function verifyMaintenanceEpoch(status: MaintenanceStatus, project: ProjectInfo) {
  // Only snapshots without the new field use the legacy command contract.
  if (Object.hasOwn(status, 'latestBuild') && (typeof status.documentEpoch !== 'string' || !status.documentEpoch
    || status.documentEpoch !== project.documentEpoch)) throw new Error('项目数据代次已变化或无法核对，请刷新项目列表后重新读取构建结果。');
}

// Pending data exchange intentionally makes store.project unavailable. This
// diagnostic maintenance snapshot cannot establish an epoch for build results.
function hasDataRecoveryGate(project: ProjectInfo) {
  return project.state === 'error' && typeof project.documentEpoch === 'string' && Boolean(project.documentEpoch)
    && project.buildInfo?.dataRecoveryRequired === true
    && ['pending', 'blocked'].includes(project.buildInfo.dataRecoveryState ?? '');
}

function isDataRecoveryDiagnostic(status: MaintenanceStatus, project: ProjectInfo) {
  return Object.hasOwn(status, 'latestBuild') && status.documentEpoch === null && hasDataRecoveryGate(project);
}

function App() {
  const [info, setInfo] = useState<BootstrapInfo | null>(null);
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [page, setPage] = useState<AppPage>('projects');
  const { theme, setTheme } = useAppearance();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('general');
  const [spaceTab, setSpaceTab] = useState<SpaceTab>('overview');
  const [tasksOpen, setTasksOpen] = useState(false);
  const [showDisposable, setShowDisposable] = useState(() => { try { return window.localStorage.getItem('panda-drawer:show-disposable:v1') === '1'; } catch { return false; } });
  useEffect(() => { try { window.localStorage.setItem('panda-drawer:show-disposable:v1', showDisposable ? '1' : '0'); } catch { /* Grouping still works without storage. */ } }, [showDisposable]);
  const [environmentBusy, setEnvironmentBusy] = useState(false);
  const [gitEnvironmentBusy, setGitEnvironmentBusy] = useState(false);
  const [localDataPending, setLocalDataPending] = useState(false);
  const [localDataLocked, setLocalDataLocked] = useState(false);
  const localDataLock = useRef(false);
  const settingsBusy = environmentBusy || gitEnvironmentBusy || localDataPending || localDataLocked;
  const [nodeDraft, setNodeDraft] = useState({ dirty: false, pending: false });
  const [nodeApps, setNodeApps] = useState<NodeAppDetail[]>([]);
  const [nodeDeletionTasks, setNodeDeletionTasks] = useState<NodeDeletionTaskSnapshot[]>([]);
  const [nodeRequest, setNodeRequest] = useState<NodeAppRequest | null>(null);
  const [nodeAddRequest, setNodeAddRequest] = useState(0);
  const [nodeModalOpen, setNodeModalOpen] = useState(false);
  const nodeRequestToken = useRef(0);
  const nodeListRevision = useRef(0);
  const nodeInteraction = useRef({ pending: false, modal: false });
  nodeInteraction.current = { pending: nodeDraft.pending, modal: nodeModalOpen };
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
  // Space scans run automatically after builds. Keep the user's build result
  // visible while still polling the scan for its actual busy state.
  const [buildResults, setBuildResults] = useState<Record<string, BuildResultView>>({});
  const [maintenance, setMaintenance] = useState<Record<string, MaintenanceStatus>>({});
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [lifecycleTasks, setLifecycleTasks] = useState<Record<string, TaskRecord>>({});
  const [showSources, setShowSources] = useState(false);
  const [sourcesMounted, setSourcesMounted] = useState(false);
  const [sourceNeedsAttention, setSourceNeedsAttention] = useState(false);
  const [archiveImportOpen, setArchiveImportOpen] = useState(false);
  const [sourceRepositories, setSourceRepositories] = useState<{ url: string; appId: string; name: string }[]>([]);
  const sourceTaskIds = useRef(new Set<string>());
  function closeSourceFlow(options?: { keepSession: boolean }) {
    setShowSources(false);
    if (options?.keepSession) return;
    setSourcesMounted(false);
    setSourceNeedsAttention(false);
    sourceTaskIds.current.clear();
  }
  useEffect(() => {
    if (!showSources || !info) return;
    let cancelled = false;
    void invoke<{ sourceId: string; url: string }[]>('source_list').then(sources => {
      if (!cancelled && Array.isArray(sources)) setSourceRepositories(nodeApps.flatMap(app => { const source = sources.find(source => source.sourceId === app.installation?.sourceId); return source?.url ? [{ url: source.url, appId: app.id, name: app.name }] : []; }));
    }).catch(() => { if (!cancelled) setSourceRepositories([]); });
    return () => { cancelled = true; };
  }, [showSources, info?.instanceId, nodeApps]);
  const [overview, setOverview] = useState<{ project: ProjectInfo; tab: OverviewTab } | null>(null);
  const [overviewVersionDraft, setOverviewVersionDraft] = useState({ dirty: false, pending: false });
  const [deleteProject, setDeleteProject] = useState<ProjectInfo | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [requestedDelete, setRequestedDelete] = useState<{ projectId: string; projectRoot: string; documentEpoch: string; instanceId?: string; taskId: string; origin: { scope: string; version: number } } | null>(null);
  const [reloadProjectId, setReloadProjectId] = useState<string | null>(null);
  const [building, setBuilding] = useState<string[]>([]);
  const [handoff, setHandoff] = useState<ProjectHandoff | null>(null);
  const [handoffAction, setHandoffAction] = useState<HandoffAction | null>(null);
  const buildPending = useRef(new Set<string>());
  const buildRevisions = useRef<Record<string, number>>({});
  const projectScopes = useRef<Record<string, string>>({});
  const dataRecoveryGates = useRef<Record<string, string>>({});
  const requestId = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const inDesktop = isTauri();
  const instanceId = useRef<string | null>(null);
  const serviceWorkspaceRoot = useRef<string | null>(null);
  const handoffRequest = useRef(0);
  const handoffInFlight = useRef<number | null>(null);
  const clipboardQueue = useRef<Promise<void>>(Promise.resolve());
  const selectedOverviewId = useRef<string | null>(null);
  const selectedOverviewRoot = useRef<string | null>(null);
  const overviewDismissGuard = useRef<(() => boolean) | null>(null);
  const feedbackScope = `${page}:${settingsOpen ? 'settings' : showCreate ? 'create' : nodeModalOpen ? 'node' : showSources ? 'sources' : deleteProject ? `delete:${deleteProject.id}` : overview ? `project:${overview.project.id}` : 'page'}`;
  const { capture, isCurrent, error: showError, success: showSuccess, recover, reset, dismiss, messages } = useManagementFeedback(feedbackScope);
  const registerOverviewGuard = useCallback((guard: (() => boolean) | null) => { overviewDismissGuard.current = guard; }, []);
  const reportNodeDraft = useCallback((state: { dirty: boolean; pending: boolean }) => { setNodeDraft(state); }, []);
  const reportOverviewDevelopment = useCallback((state: { dirty: boolean; pending: boolean }) => { setOverviewVersionDraft(state); }, []);
  function navigatePage(value: AppPage) {
    if (localDataLock.current || value === page || nodeDraft.pending || nodeModalOpen || deleteProject) return;
    setPage(value);
  }
  const acceptNodeApp = useCallback((app: NodeAppDetail) => {
    nodeListRevision.current++;
    setNodeApps((current) => current.some((value) => value.id === app.id) ? current.map((value) => value.id === app.id ? app : value) : [...current, app]);
  }, []);
  const acceptNodeDeletion = useCallback((appId: string, result: NodeAppDeleteResult) => {
    nodeListRevision.current++;
    setNodeApps((current) => current.filter((value) => value.id !== appId));
    setNodeRequest(null);
    const origin = capture();
    const outcome = result.ownership === 'external' ? '项目已从平台移除。原目录和文件未改动。' : '项目文件已永久删除。原始来源目录未删除。';
    if (result.metadataCleanupPending || result.durabilityConfirmed === false) showError(origin, `${outcome} 但删除后的磁盘同步收尾尚未完成。${result.warning ?? '请刷新并核对当前目录。'} 不要再次提交删除。`, 'main', { nextScope: 'projects:page' });
    else showSuccess(origin, outcome, 'main', 'projects:page');
  }, [capture, showError, showSuccess]);
  const acceptNodeDeletionTasks = useCallback((values: NodeDeletionTaskSnapshot[]) => {
    setNodeDeletionTasks(previous => {
      const tasks = new Map(previous.map(task => [task.taskId, task]));
      for (const task of values) {
        const current = tasks.get(task.taskId);
        if (!current || task.appId === current.appId && task.targetRevision === current.targetRevision && task.journalRevision >= current.journalRevision
          && (task.journalRevision > current.journalRevision || task.phase === current.phase) && (!current.renamedAny || task.renamedAny)
          && (!['deleted', 'abandoned'].includes(current.phase) || task.phase === current.phase)) tasks.set(task.taskId, task);
      }
      return [...tasks.values()];
    });
  }, []);
  function requestNodeApp(app: NodeAppDetail, intent: 'enter' | 'configure' | 'details' | 'delete') {
    if (nodeDraft.pending || nodeModalOpen || deleteProject) return;
    setNodeRequest({ app, intent, token: ++nodeRequestToken.current });
  }
  const clearHandoff = useCallback(() => {
    handoffRequest.current++;
    handoffInFlight.current = null;
    setHandoff(null); setHandoffAction(null);
  }, []);
  const dismissOverview = useCallback(() => {
    clearHandoff();
    selectedOverviewId.current = null;
    selectedOverviewRoot.current = null;
    setOverview(null); setReloadProjectId(null);
  }, [clearHandoff]);
  function openOverview(project: ProjectInfo, tab: OverviewTab) {
    if (nodeDraft.pending || nodeModalOpen || deleteProject) return;
    clearHandoff();
    selectedOverviewId.current = project.id;
    selectedOverviewRoot.current = project.projectRoot;
    setReloadProjectId(null);
    setOverview({ project, tab });
  }
  function openDelete(project: ProjectInfo) {
    if (nodeDraft.pending || nodeModalOpen || deleteProject || deletePending) return;
    clearHandoff();
    setDeleteProject(project);
  }
  const { migrating, paused, cleanupTask, cleanupBusy } = deriveWorkspaceTaskState(workspace, lifecycleTasks);
  const workspaceBusy = migrating || paused || cleanupBusy || localDataLocked;
  useEffect(() => { if (workspaceBusy) clearHandoff(); }, [workspaceBusy, clearHandoff]);
  const acceptLifecycleTask = useCallback((task: TaskRecord) => {
    setLifecycleTasks((current) => mergeLifecycleTask(current, task));
  }, []);

  const refresh = useCallback(async () => {
    if (localDataLock.current) return;
    const current = ++requestId.current;
    let origin = capture();
    setLoading(true);
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      // Read bootstrap first so an unavailable committed workspace reports its
      // concrete startup error, rather than a missing native command state.
      let runtime = await invoke<BootstrapInfo>('bootstrap_info');
      if (localDataLock.current || current !== requestId.current) return;
      const [list, currentWorkspace, nodeResult] = await Promise.all([
        invoke<ProjectInfo[]>('project_list'), invoke<WorkspaceInfo>('workspace_info'),
        invoke<NodeAppDetail[]>('node_app_list').then((value) => ({ value, error: '' })).catch(() => ({ value: null, error: '带本地服务的项目暂时无法读取，请刷新后核对。已有项目保留。' })),
      ]);
      if (currentWorkspace.workspaceRoot !== runtime.workspaceRoot) runtime = await invoke<BootstrapInfo>('bootstrap_info');
      if (current !== requestId.current) return;
      recover('workspace-read');
      if (instanceId.current && (instanceId.current !== runtime.instanceId || serviceWorkspaceRoot.current !== runtime.workspaceRoot)) {
        origin = reset();
        clearHandoff();
        setBuildResults({});
        setNodeDeletionTasks([]);
        showError(origin, '工作区服务已切换。旧窗口的草稿仍保留；处理草稿后请明确重新加载项目，并从新目录重新复制 AI 说明。');
      }
      const selectedProject = list.find((project) => project.id === selectedOverviewId.current);
      if (selectedOverviewId.current && (!selectedProject || selectedProject.projectRoot !== selectedOverviewRoot.current)) clearHandoff();
      selectedOverviewRoot.current = selectedProject?.projectRoot ?? null;
      instanceId.current = runtime.instanceId;
      serviceWorkspaceRoot.current = runtime.workspaceRoot;
      const previousScopes = projectScopes.current;
      const scopes = Object.fromEntries(list.map(project => [project.id, JSON.stringify([runtime.instanceId, runtime.workspaceRoot, project.projectRoot, project.documentEpoch])]));
      projectScopes.current = scopes;
      const kept = <T,>(values: Record<string, T>) => Object.fromEntries(Object.entries(values).filter(([id]) => scopes[id] && scopes[id] === previousScopes[id]));
      const recoveryGates = Object.fromEntries(list.filter(hasDataRecoveryGate).map(project => [project.id, JSON.stringify([scopes[project.id], project.buildInfo?.dataRecoveryState])]));
      // Invalidate unverified build/scan work as soon as the list reports the
      // gate. Accepted recovery tasks keep their ordinary project identity.
      for (const [id, gate] of Object.entries(recoveryGates)) if (gate !== dataRecoveryGates.current[id]) buildRevisions.current[id] = (buildRevisions.current[id] ?? 0) + 1;
      dataRecoveryGates.current = recoveryGates;
      setBuildResults(values => Object.fromEntries(Object.entries(kept(values)).filter(([id]) => !recoveryGates[id])));
      setTasks(values => {
        const next = kept(values);
        for (const id of Object.keys(recoveryGates)) if (next[id] && ['build', 'storageScan'].includes(next[id].type)) delete next[id];
        return next;
      });
      setMaintenance(values => Object.fromEntries(Object.entries(kept(values)).filter(([id]) => !recoveryGates[id])));
      for (const id of buildPending.current) if (scopes[id] !== previousScopes[id] || recoveryGates[id]) buildPending.current.delete(id);
      setBuilding(values => values.filter(id => scopes[id] === previousScopes[id] && !recoveryGates[id]));
      setWorkspace(currentWorkspace);
      if (currentWorkspace.cleanupTask) acceptLifecycleTask(currentWorkspace.cleanupTask);
      const workspaceTaskId = (typeof currentWorkspace.activeTask === 'string' ? currentWorkspace.activeTask : currentWorkspace.activeTask?.taskId) ?? currentWorkspace.lastTaskId;
      if (workspaceTaskId) { const task = await invoke<TaskRecord>('task_get', { taskId: workspaceTaskId }); if (current === requestId.current) acceptLifecycleTask(task); }
      if (current !== requestId.current) return;
      setInfo(runtime);
      setProjects(list);
      if (nodeResult.value) setNodeApps(nodeResult.value);
      nodeListRevision.current++;
      if (nodeResult.error) showError(origin, nodeResult.error, 'node-list', { backgroundKey: 'node-list' });
      else recover('node-list');
      const revisions = Object.fromEntries(list.map(project => [project.id, buildRevisions.current[project.id] ?? 0]));
      const statuses = await Promise.allSettled(list.map(async (project) => {
        const revision = revisions[project.id];
        const status = await invoke<MaintenanceStatus>('maintenance_status', { projectId: project.id });
        if (status.projectId !== undefined && status.projectId !== project.id) throw new Error('维护状态项目身份不符。');
        if (isDataRecoveryDiagnostic(status, project)) return { projectId: project.id, status, task: null, activeReference: null,
          revision, build: { issues: [] } as BuildResultView, dataRecoveryDiagnostic: true };
        verifyMaintenanceEpoch(status, project);
        const activeReference = typeof status.activeTask === 'string' ? status.activeTask : status.activeTask?.taskId;
        const activeId = activeReference ?? status.lastTaskId;
        const task = activeId ? await invoke<TaskRecord>('task_get', { taskId: activeId }) : null;
        if (task && !matchingProjectTask(task, project.id, activeId ?? undefined)) throw new Error('项目任务身份或格式不符。');
        return { projectId: project.id, status, task, activeReference, revision,
          build: Object.hasOwn(status, 'latestBuild') ? recoverBuildResult(status.latestBuild ?? null, project.id) : null,
          dataRecoveryDiagnostic: false };
      }));
      if (current !== requestId.current) return;
      const confirmedRuntime = await invoke<BootstrapInfo>('bootstrap_info');
      if (current !== requestId.current) return;
      if (confirmedRuntime.instanceId !== runtime.instanceId || confirmedRuntime.workspaceRoot !== runtime.workspaceRoot) {
        projectScopes.current = {}; setBuildResults({}); setTasks({}); setMaintenance({});
        throw new Error('读取构建结果时工作区服务已切换，请刷新后重新核对。');
      }
      const next: Record<string, MaintenanceStatus> = {};
      const activeTasks: Record<string, TaskRecord> = {};
      const recovered: Record<string, BuildResultView> = {};
      const dataRecoveryDiagnostics = new Set<string>();
      for (const project of list) { const scanning = project.storage?.activeTask; if (matchingProjectTask(scanning, project.id) && scanning.type === 'storageScan') activeTasks[project.id] = scanning; }
      for (const [index, result] of statuses.entries()) {
        const projectId = list[index].id;
        if (result.status === 'fulfilled') {
          recover(`maintenance:${projectId}`);
          // Null-epoch diagnostics must not enable ordinary management actions.
          if (!result.value.dataRecoveryDiagnostic) next[result.value.projectId] = result.value.status;
          if (result.value.revision === (buildRevisions.current[projectId] ?? 0) && scopes[projectId] === projectScopes.current[projectId]) {
            if (result.value.dataRecoveryDiagnostic) {
              dataRecoveryDiagnostics.add(projectId);
              recovered[projectId] = { issues: [] };
              delete activeTasks[projectId];
              continue;
            }
            if (result.value.task) activeTasks[projectId] = result.value.task;
            if (result.value.build) recovered[projectId] = result.value.build;
            if (result.value.task?.type === 'build') {
              if (result.value.build?.task?.taskId === result.value.task.taskId || result.value.activeReference && !result.value.task.final || result.value.build === null) recovered[projectId] = { task: result.value.task, issues: result.value.build?.issues ?? [] };
              else if (result.value.build.task) activeTasks[projectId] = result.value.build.task;
              else delete activeTasks[projectId]; // A bad pointer never falls back to another historical build.
            } else if (!result.value.task && result.value.build?.task && !result.value.build.task.final) activeTasks[projectId] = result.value.build.task;
          }
        } else {
          const issue = `维护状态读取失败：${messageFrom(result.reason)}。请刷新状态后再操作。`;
          recovered[projectId] = { issues: [issue] };
          delete activeTasks[projectId];
          showError(origin, issue, `task:${projectId}`, { backgroundKey: `maintenance:${projectId}` });
        }
      }
      setMaintenance(next);
      const currentResult = (id: string) => scopes[id] === projectScopes.current[id] && revisions[id] === (buildRevisions.current[id] ?? 0) && !buildPending.current.has(id);
      setBuildResults((previous) => ({ ...previous, ...Object.fromEntries(Object.entries(recovered).filter(([id]) => currentResult(id))) }));
      setTasks((previous) => {
        const next = { ...previous };
        for (const id of Object.keys(recovered)) if (currentResult(id) && (next[id]?.type === 'build' || dataRecoveryDiagnostics.has(id) && next[id]?.type === 'storageScan')) delete next[id];
        return { ...next, ...Object.fromEntries(Object.entries(activeTasks).filter(([id]) => currentResult(id))) };
      });
      setRefreshVersion((version) => version + 1);
      setCheckedAt(new Date().toLocaleTimeString('zh-CN', { hour12: false }));
    } catch (cause) {
      if (current === requestId.current) showError(origin, messageFrom(cause), 'main', { backgroundKey: 'workspace-read' });
    } finally {
      if (current === requestId.current) setLoading(false);
    }
  }, [acceptLifecycleTask, clearHandoff, capture, showError, recover, reset]);

  useEffect(() => {
    void refresh();
    return () => { requestId.current += 1; handoffRequest.current++; selectedOverviewId.current = null; };
  }, [refresh]);

  useEffect(() => {
    if (localDataLocked || !info || page !== 'projects' || workspaceBusy || !nodeApps.length) return;
    let cancelled = false; let polling = false;
    const timer = window.setInterval(() => {
      if (localDataLock.current || polling || nodeInteraction.current.pending || nodeInteraction.current.modal) return;
      polling = true;
      const revision = nodeListRevision.current;
      const generation = requestId.current;
      const origin = capture();
      void invoke<NodeAppDetail[]>('node_app_list').then((values) => {
        if (!cancelled && revision === nodeListRevision.current && generation === requestId.current && !nodeInteraction.current.pending && !nodeInteraction.current.modal) { setNodeApps(values); recover('node-list'); }
      }).catch(() => { if (!cancelled && revision === nodeListRevision.current && generation === requestId.current && !nodeInteraction.current.pending && !nodeInteraction.current.modal) showError(origin, '带本地服务的项目状态暂未更新，请刷新后核对。', 'node-list', { backgroundKey: 'node-list' }); }).finally(() => { polling = false; });
    }, 2000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [Boolean(info), page, workspaceBusy, nodeApps.length, capture, showError, recover, localDataLocked]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPending.current || workspaceBusy || !name.trim()) return;
    createPending.current = true;
    setCreating(true);
    const origin = capture();
    try {
      const project = await invoke<ProjectInfo>('project_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      setProjects((previous) => [...previous.filter((item) => item.id !== project.id), project]);
      if (isCurrent(origin)) {
        setName('');
        setDescription('');
        setShowCreate(false);
        openOverview(project, 'overview');
        showSuccess(origin, `已创建「${project.name}」。可以直接进入项目；点击“复制 AI 接手说明”，粘贴给其他 AI 并补充你的需求。`, 'main', `${page}:project:${project.id}`);
      }
      await refresh();
    } catch (cause) {
      showError(origin, messageFrom(cause));
    } finally {
      createPending.current = false;
      setCreating(false);
    }
  }

  async function openProject(project: ProjectInfo, target: 'desktop' | 'browser', reload = false) {
    if (localDataLock.current || openPending.current) return;
    openPending.current = true;
    setOpening(`${project.id}:${target}`);
    const origin = capture();
    try {
      const result = await invoke<OpenProjectResult>('project_open', { projectId: project.id, target, reload });
      showSuccess(origin, target === 'browser'
        ? `已将「${project.name}」交给默认浏览器打开。`
        : reload ? `已重新加载「${project.name}」的当前网页产物。` : result.reused ? `已切换到「${project.name}」的现有窗口。如提示需要重载，请先处理未保存内容，再明确重新加载。` : `已在独立窗口打开「${project.name}」。`);
    } catch (cause) {
      showError(origin, messageFrom(cause));
    } finally {
      openPending.current = false;
      setOpening(null);
    }
  }

  useEffect(() => {
    if (localDataLocked) return;
    const pending = Object.values(tasks).filter((task) => !task.final);
    if (!pending.length) return;
    let cancelled = false;
    let polling = false;
    const scopes = { ...projectScopes.current };
    const revisions = { ...buildRevisions.current };
    const timer = window.setInterval(() => {
      if (localDataLock.current || polling) return;
      polling = true;
      void Promise.all(pending.map(async (previous) => {
        const origin = capture();
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId });
          if (cancelled || !previous.projectId || scopes[previous.projectId] !== projectScopes.current[previous.projectId]) return;
          if (['build', 'storageScan'].includes(previous.type) && dataRecoveryGates.current[previous.projectId]) return;
          if (previous.type === 'build' && revisions[previous.projectId] !== buildRevisions.current[previous.projectId]) return;
          if (!matchingProjectTask(task, previous.projectId, previous.taskId) || task.type !== previous.type) throw new Error('任务身份或类型不符，请刷新核对。');
          recover(`task:${previous.taskId}`);
          if (task.projectId) setTasks((current) => ({ ...current, [task.projectId!]: task }));
          // Both pollers can observe this deletion. A confirmed final result
          // must also release the dialog if the lifecycle poll is still waiting.
          if (task.type === 'delete' && task.final) acceptLifecycleTask(task);
          if (task.projectId && task.type === 'build') {
            buildRevisions.current[task.projectId] = (buildRevisions.current[task.projectId] ?? 0) + 1;
            setBuildResults((current) => ({ ...current, [task.projectId!]: { task, issues: current[task.projectId!]?.issues ?? [] } }));
          }
          if (task.final) {
            setBuilding((current) => current.filter((id) => id !== task.projectId));
            if (task.projectId) buildPending.current.delete(task.projectId);
            void refresh();
          }
        } catch (cause) {
          if (!cancelled) showError(origin, `任务状态暂时无法读取：${messageFrom(cause)}。读取失败本身不会取消任务，请以最终记录为准。`, `task:${previous.projectId}`, { backgroundKey: `task:${previous.taskId}` });
        }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [tasks, acceptLifecycleTask, refresh, capture, showError, recover, localDataLocked]);

  useEffect(() => {
    if (localDataLocked) return;
    const pending = Object.values(lifecycleTasks).filter((task) => !task.final);
    if (!pending.length) return;
    let cancelled = false; let polling = false;
    const timer = window.setInterval(() => {
      if (localDataLock.current || polling) return;
      polling = true;
      void Promise.all(pending.map(async (previous) => {
        const origin = capture();
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId });
          if (cancelled) return;
          recover(`lifecycle:${previous.taskId}`);
          acceptLifecycleTask(task);
          if (task.final) void refresh();
        } catch (cause) { if (!cancelled) showError(origin, `任务状态暂时无法读取：${messageFrom(cause)}。读取失败本身不会取消任务，请以最终记录为准；不要重复提交操作。`, 'main', { backgroundKey: `lifecycle:${previous.taskId}` }); }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [lifecycleTasks, acceptLifecycleTask, refresh, capture, showError, recover, localDataLocked]);

  async function buildProject(project: ProjectInfo) {
    if (buildPending.current.has(project.id)) return;
    buildPending.current.add(project.id);
    let revision = (buildRevisions.current[project.id] ?? 0) + 1;
    buildRevisions.current[project.id] = revision;
    const scope = projectScopes.current[project.id];
    const current = () => scope !== undefined && projectScopes.current[project.id] === scope && buildRevisions.current[project.id] === revision;
    setBuilding((current) => [...current, project.id]);
    const origin = capture();
    try {
      const status = await invoke<MaintenanceStatus>('maintenance_status', { projectId: project.id });
      if (!current()) return;
      verifyMaintenanceEpoch(status, project);
      const task = await invoke<TaskRecord>('project_build', { projectId: project.id, input: status.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {} });
      if (!current()) return;
      if (!matchingProjectTask(task, project.id) || task.type !== 'build') throw new Error('构建任务身份或类型不符，请刷新核对。');
      buildRevisions.current[project.id] = ++revision;
      setTasks((current) => ({ ...current, [project.id]: task }));
      setBuildResults((current) => ({ ...current, [project.id]: { task, issues: [] } }));
      if (task.final) { buildPending.current.delete(project.id); setBuilding((current) => current.filter((id) => id !== project.id)); }
    } catch (cause) {
      if (!current()) return;
      buildPending.current.delete(project.id);
      setBuilding((current) => current.filter((id) => id !== project.id));
      showError(origin, messageFrom(cause), `task:${project.id}`);
    }
  }

  async function copyHandoff(project: ProjectInfo, action: HandoffAction, copy = true) {
    if (selectedOverviewId.current !== project.id || workspaceBusy || !projects.some((value) => value.id === project.id) || handoffInFlight.current !== null) return;
    const sequence = ++handoffRequest.current;
    handoffInFlight.current = sequence;
    const initialInstance = instanceId.current;
    const initialRoot = project.projectRoot;
    const origin = capture();
    const current = () => isCurrent(origin) && sequence === handoffRequest.current && selectedOverviewId.current === project.id && initialInstance === instanceId.current && initialRoot === selectedOverviewRoot.current;
    setHandoff(null); setHandoffAction(action);
    try {
      const data = await invoke<ProjectHandoff>('project_handoff', { projectId: project.id });
      if (!current()) return;
      setHandoff(data);
      if ((data.projectRoot && data.projectRoot !== initialRoot) || (data.instanceId && data.instanceId !== initialInstance)) {
        showError(origin, '项目目录或服务信息已变化，请刷新项目列表后重新准备说明。', 'handoff');
        return;
      }
      if (!copy) return;
      let copied = false;
      try {
        // A system clipboard write cannot be cancelled after closing a detail.
        // Keep writes ordered across projects so an older write cannot finish last.
        const write = clipboardQueue.current.then(async () => {
          if (current()) await navigator.clipboard.writeText(data.text);
        });
        clipboardQueue.current = write.catch(() => {});
        await write;
        copied = true;
      } catch { /* The complete handoff remains available for manual copying. */ }
      if (!current()) return;
      showSuccess(origin, copied ? `已复制「${project.name}」的 AI 接手说明。粘贴给其他 AI，再补充你想做的功能即可。` : '说明已准备好，可以选中下面的文字并复制给 AI。', 'handoff');
    } catch (cause) { if (current()) showError(origin, messageFrom(cause), 'handoff'); }
    finally {
      if (handoffInFlight.current === sequence) handoffInFlight.current = null;
      if (current()) setHandoffAction(null);
    }
  }

  function acceptTask(task: TaskRecord, deletionTarget?: ProjectInfo) {
    if (!task.projectId) { acceptLifecycleTask(task); return; }
    if (task.type === 'delete') {
      const target = deletionTarget ?? projects.find(project => project.id === task.projectId);
      if (!target) return;
      setRequestedDelete({ projectId: target.id, projectRoot: target.projectRoot, documentEpoch: target.documentEpoch, instanceId: info?.instanceId, taskId: task.taskId, origin: capture() });
    }
    setTasks((current) => ({ ...current, [task.projectId!]: task }));
  }

  useEffect(() => {
    if (!requestedDelete) return;
    const task = lifecycleTasks[requestedDelete.taskId] ?? tasks[requestedDelete.projectId];
    if (task?.taskId !== requestedDelete.taskId || task.projectId !== requestedDelete.projectId || task.type !== 'delete' || !task.final) return;
    if (task.result?.operationOutcome === 'deleted' && requestedDelete.instanceId === info?.instanceId) {
      const matchesTarget = (project: ProjectInfo) => project.id === requestedDelete.projectId && project.projectRoot === requestedDelete.projectRoot && project.documentEpoch === requestedDelete.documentEpoch;
      setProjects((current) => current.filter((project) => !matchesTarget(project)));
      void refresh();
      if (isCurrent(requestedDelete.origin)) {
        const matchesDialog = Boolean(deleteProject && matchesTarget(projects.find(project => project.id === deleteProject.id) ?? deleteProject));
        const matchesOverview = Boolean(selectedOverviewId.current === requestedDelete.projectId && overview && matchesTarget(projects.find(project => project.id === overview.project.id) ?? overview.project));
        if (matchesDialog) setDeleteProject(null);
        if (matchesOverview) dismissOverview();
        if (matchesDialog || matchesOverview) showSuccess(requestedDelete.origin, '项目已永久删除，本次删除不可撤销。平台未为本次删除保存可恢复副本；操作记录和外部导出文件另行保留。', 'main', `${page}:page`);
      }
    }
    setRequestedDelete(null);
  }, [requestedDelete, tasks, lifecycleTasks, dismissOverview, refresh, isCurrent, showSuccess, page, info?.instanceId, deleteProject, overview, projects]);

  function projectBlocked(project: ProjectInfo) {
    const status = maintenance[project.id];
    return workspaceBusy || !status || status.mode === 'maintenance' || status.recoveryRequired;
  }

  const currentOverviewProject = projects.find((project) => project.id === overview?.project.id);
  const overviewProject = currentOverviewProject ?? overview?.project;
  const overviewUnavailable = Boolean(overview && !currentOverviewProject);
  const currentDeleteProject = projects.find(project => project.id === deleteProject?.id);
  const deletionTarget = currentDeleteProject ?? deleteProject;
  const projectReady = (project: ProjectInfo) => project.state === 'ready' && !projectBlocked(project);
  const nodeReady = (app: NodeAppDetail) => app.configuration.canStart && !['failed', 'starting', 'stopping'].includes(app.state) && !workspaceBusy;
  const filteredProjects = projects.filter((project) => {
    const matchesQuery = `${project.name} ${project.description ?? ''} ${project.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const ready = projectReady(project);
    return matchesQuery && (projectFilter === 'all' || (projectFilter === 'ready' ? ready : !ready));
  });
  const deletingNodeIds = new Set(nodeDeletionTasks.filter(task => !['deleted', 'abandoned'].includes(task.phase)).map(task => task.appId));
  const availableNodeApps = nodeApps.filter(app => !deletingNodeIds.has(app.id));
  const filteredNodeApps = availableNodeApps.filter((app) => {
    const matches = `${app.name} ${app.id} ${app.sourcePath} Node 服务项目`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const ready = nodeReady(app);
    return matches && (projectFilter === 'all' || (projectFilter === 'ready' ? ready : !ready));
  });
  const attentionCount = projects.filter((project) => !projectReady(project)).length + availableNodeApps.filter((app) => !nodeReady(app)).length;
  const runningCount = availableNodeApps.filter((app) => app.state === 'running').length;
  const filterChoices = [['all', '全部'], ['ready', '可使用'], ['attention', '需要处理']] as const;
  // Disposable validation projects follow the "可丢弃" naming convention; keep them out of the way unless searched for.
  const isDisposable = (name: string) => name.includes('可丢弃');
  const groupDisposable = !query.trim();
  const mainProjects = groupDisposable ? filteredProjects.filter((project) => !isDisposable(project.name)) : filteredProjects;
  const mainNodeApps = groupDisposable ? filteredNodeApps.filter((app) => !isDisposable(app.name)) : filteredNodeApps;
  const disposableProjects = groupDisposable ? filteredProjects.filter((project) => isDisposable(project.name)) : [];
  const disposableNodeApps = groupDisposable ? filteredNodeApps.filter((app) => isDisposable(app.name)) : [];
  const disposableCount = disposableProjects.length + disposableNodeApps.length;
  const totalProjects = projects.length + availableNodeApps.length;
  const filteredCount = filteredProjects.length + filteredNodeApps.length;
  const activeLifecycleTasks = Object.values(lifecycleTasks).filter((task) => !task.final);
  const completedLifecycleTasks = Object.values(lifecycleTasks).filter((task) => task.final).slice(-6).reverse();
  const failedLifecycleTasks = completedLifecycleTasks.filter((task) => task.status === 'failed' || task.status === 'interrupted');
  const modalOpen = Boolean(overviewProject && overview) || Boolean(deleteProject) || showCreate || nodeModalOpen || settingsOpen || showSources;
  const requestCloseOverview = () => { if (!overviewDismissGuard.current || overviewDismissGuard.current()) dismissOverview(); };
  const feedback = <FeedbackNotices messages={messages} dismiss={dismiss} />;
  const addBlocked = !info || workspaceBusy || nodeDraft.pending || nodeModalOpen;
  const addActions = (className: string) => <div className={className} role="group" aria-label="添加项目">
    <button className="refresh-button" disabled={addBlocked || creating} onClick={() => { if (!addBlocked) setShowCreate(true); }}><Plus size={15} aria-hidden="true" />新建项目</button>
    <button className="refresh-button" disabled={addBlocked} onClick={() => { if (!addBlocked) setNodeAddRequest((value) => value + 1); }}><FolderOpen size={15} aria-hidden="true" />添加本地项目</button>
    <button className="primary-button" disabled={addBlocked} onClick={() => { if (!addBlocked) { setSourcesMounted(true); setShowSources(true); setSourceNeedsAttention(false); } }}><Download size={15} aria-hidden="true" />从 Git 添加</button>
  </div>;
  const openSettings = (section?: SettingsSection) => { if (localDataLock.current || nodeDraft.pending || nodeModalOpen) return; if (section) setSettingsSection(section); setTasksOpen(false); setSettingsOpen(true); };
  const openSpace = (tab?: SpaceTab) => { if (tab) setSpaceTab(tab); navigatePage('storage'); };
  const recentTasks = [...Object.values(lifecycleTasks), ...Object.values(tasks).filter((task) => !lifecycleTasks[task.taskId])].reverse().slice(0, 20);
  const projectName = (projectId: string | null) => projectId ? projects.find((project) => project.id === projectId)?.name ?? null : null;
  const lockLocalData = useCallback(() => {
    localDataLock.current = true;
    requestId.current++; nodeListRevision.current++; handoffRequest.current++;
    setLocalDataLocked(true); setLoading(false); clearHandoff();
  }, [clearHandoff]);
  const localDataUnavailable = !inDesktop || !info || workspaceBusy || environmentBusy || gitEnvironmentBusy
    || nodeDraft.pending || nodeDraft.dirty || nodeModalOpen || Boolean(overview) || Boolean(deleteProject) || deletePending
    || creating || Boolean(opening) || building.length > 0 || activeLifecycleTasks.length > 0 || Object.values(tasks).some(task => !task.final)
    || nodeDeletionTasks.some(task => !['deleted', 'abandoned'].includes(task.phase));
  const renderProjectCard = (project: ProjectInfo) => <ProjectCard key={`web:${project.id}`} project={project} status={maintenance[project.id]} task={tasks[project.id]} blocked={projectBlocked(project) || opening !== null || nodeDraft.pending || deletePending} opening={opening === `${project.id}:desktop`} deletingDisabled={workspaceBusy || nodeDraft.pending || deletePending} detailsDisabled={nodeDraft.pending || deletePending} onEnter={() => void openProject(project, 'desktop')} onDetails={() => openOverview(project, 'overview')} onDelete={() => openDelete(project)} />;
  const renderNodeCard = (app: NodeAppDetail) => <NodeProjectCard key={`node:${app.id}`} app={app} blocked={!info || workspaceBusy || nodeDraft.pending || opening !== null} opening={nodeRequest?.app.id === app.id && nodeDraft.pending} onEnter={() => requestNodeApp(app, 'enter')} onDetails={() => requestNodeApp(app, 'details')} onDelete={() => requestNodeApp(app, 'delete')} />;

  return (
    <div className="radio-shell" data-theme={theme}>
      <header className="radio-header">
        <div className="radio-brand">
          <img src="/panda-app-drawer.png" alt="" />
          <strong className="radio-brand-name">熊猫小抽屉</strong>
        </div>
        <nav className="radio-primary-nav" aria-label="工作台导航">
          <button aria-current={page === 'projects' ? 'page' : undefined} aria-pressed={page === 'projects'} disabled={nodeDraft.pending || localDataLocked} onClick={() => navigatePage('projects')}>我的项目</button>
          <button aria-current={page === 'storage' ? 'page' : undefined} aria-pressed={page === 'storage'} disabled={nodeDraft.pending || localDataLocked} onClick={() => navigatePage('storage')}>空间</button>
        </nav>
        <div className="radio-header-actions">
          {!localDataLocked && <TaskCenter tasks={recentTasks} projectName={projectName} open={tasksOpen} onOpenChange={setTasksOpen} importFlow={sourcesMounted ? { needsAttention: sourceNeedsAttention, onOpen: () => { setTasksOpen(false); setShowSources(true); setSourceNeedsAttention(false); } } : undefined} onViewTask={task => { if (sourceTaskIds.current.has(task.taskId)) { setTasksOpen(false); setShowSources(true); setSourceNeedsAttention(false); } }} canViewTask={task => sourceTaskIds.current.has(task.taskId)} />}
          <button className="radio-settings-button" aria-label="设置" title="设置" disabled={nodeDraft.pending || localDataLocked} onClick={() => openSettings()}><Settings size={17} /></button>
        </div>
      </header>
      {page === 'projects' && !localDataLocked && <div className="radio-toolbar">
        <label className="project-search"><Search size={16} /><span className="sr-only">搜索项目</span><input type="search" data-modal-return-focus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索项目…" /></label>
        <div className="project-filter-chips" role="group" aria-label="筛选项目状态">{filterChoices.map(([value, label]) => <button key={value} type="button" aria-pressed={projectFilter === value} onClick={() => setProjectFilter(value)}>{label}{value === 'attention' && attentionCount > 0 && <span className="filter-count">{attentionCount}</span>}</button>)}</div>
        <div className="radio-toolbar-spacer" />
        <button className="radio-refresh-button" aria-label="刷新项目和运行状态" aria-busy={loading} title={loading ? '正在刷新' : checkedAt ? `刷新 · 上次更新 ${checkedAt}` : '刷新项目和运行状态'} disabled={loading || !inDesktop || nodeDraft.pending} onClick={() => void refresh()}><RefreshCw size={15} className={loading ? 'rotating' : undefined} /></button>
        {addActions('radio-add-actions')}
      </div>}
      <main className={`radio-main${page === 'storage' ? ' radio-workspace-view' : ''}`}>
        <h1 className="sr-only">{page === 'projects' ? '我的项目' : '空间'}</h1>
        <div className="radio-notices">
          {!inDesktop && <div className="notice" role="status"><Monitor size={20} /><div><strong>项目管理在桌面应用中进行</strong><p>这里是前端开发页面。启动桌面应用后创建项目，再从项目卡片选择“进入项目”打开原生应用窗口。</p></div></div>}
          {!modalOpen && feedback}
          {info && inDesktop && !localDataLocked && <NodeDeletionTasksNotice instanceId={info.instanceId} revision={refreshVersion} tasks={nodeDeletionTasks} enabled={!workspaceBusy && !nodeDraft.pending && !nodeModalOpen && !deleteProject} onTasks={acceptNodeDeletionTasks} onOpen={task => { if (nodeDraft.pending || nodeModalOpen || deleteProject) return; setNodeRequest({ app: { id: task.appId, name: task.name }, intent: 'delete-task', task, token: ++nodeRequestToken.current }); }} />}
          {cleanupBusy && <div className="notice" role="status"><Trash2 size={18} /><div><strong>历史清理正在进行，管理写操作暂时停用</strong><p>当前项目及其草稿保留。请等待清理任务的最终结果，不要重复提交。</p></div></div>}
          {migrating && <div className="notice" role="status"><RefreshCw size={18} /><div><strong>工作区正在迁移，新操作暂时暂停</strong><p>各项目窗口保留未保存内容。请等待任务最终结果，不要重复提交迁移。</p></div></div>}
          {paused && <div className="notice" role="status"><CircleAlert size={18} /><div><strong>工作区已暂停，需要查看处理</strong><p>请查看迁移任务结果并刷新状态；已有窗口的草稿保留。</p></div></div>}
          {(activeLifecycleTasks.length > 0 || failedLifecycleTasks.length > 0) && <div className="radio-task-indicator" role="status"><span>{activeLifecycleTasks.length > 0 ? `${activeLifecycleTasks.length} 项管理任务正在进行` : `${failedLifecycleTasks.length} 项管理任务需要查看`}</span><button className="text-button" onClick={() => setTasksOpen(true)}>查看任务</button></div>}
        </div>
          {localDataLocked ? <p className="workspace-operation-status" role="status">本机内容清理已提交，项目管理已停止。请在设置中核对清理结果并退出软件。</p> : page === 'projects' ? <>
            {info && !localDataLocked && <SourceRecoveryNotice reportErrors={sourcesMounted} onFound={() => { setSourcesMounted(true); setSourceNeedsAttention(true); }} instanceId={info?.instanceId} onOpenExisting={appId => { setShowSources(false); const app = nodeApps.find(app => app.id === appId); if (app) requestNodeApp(app, 'details'); else void refresh(); }} />}
      {sourcesMounted && <SourceAcquisitionPanel open={showSources} projectDirectory={info ? `${info.workspaceRoot}/project-sources` : undefined} platformVersion={info?.version} existingRepositories={sourceRepositories.filter(source => nodeApps.some(app => app.id === source.appId))} onOpenExisting={appId => { setShowSources(false); const app = nodeApps.find(app => app.id === appId); if (app) requestNodeApp(app, 'enter'); }} onOpenLocal={() => { setShowSources(false); setNodeAddRequest(value => value + 1); }} onOpenSettings={() => { setShowSources(false); openSettings('storage'); }} onBackground={() => { setShowSources(false); setTasksOpen(true); }} onAttention={() => { setSourceNeedsAttention(true); }} instanceId={info?.instanceId} onInstalled={() => void refresh()} onEnter={(app) => { acceptNodeApp(app); requestNodeApp(app, app.configuration.canStart ? 'enter' : 'configure'); }} onTask={task => { sourceTaskIds.current.add(task.taskId); acceptLifecycleTask(task); }} onClose={closeSourceFlow} disabled={!info || workspaceBusy} />}
            {!modalOpen && <div className="project-list-feedback" aria-label="本地服务项目读取提示"><FeedbackNotices messages={messages} dismiss={dismiss} slot="node-list" /></div>}
            {!modalOpen && filteredProjects.filter((project) => messages.some((item) => item.slot === `task:${project.id}`)).map((project) => <section key={project.id} className="project-list-feedback" aria-label={`项目「${project.name}」状态提示`}><strong>{project.name} · 状态提示</strong><FeedbackNotices messages={messages} dismiss={dismiss} slot={`task:${project.id}`} /></section>)}
            <div className="sr-only" role="status">{query.trim() || projectFilter !== 'all' ? `${filteredCount} / ${totalProjects} 个项目` : `${totalProjects} 个项目`}{loading ? '，正在读取' : ''}</div>
            {filteredCount > 0 ? <section className="project-grid radio-project-grid" aria-label="项目列表">
              {mainProjects.map(renderProjectCard)}
              {mainNodeApps.map(renderNodeCard)}
              {disposableCount > 0 && <div className="project-group-divider"><button type="button" className="project-group-toggle" aria-expanded={showDisposable} onClick={() => setShowDisposable((value) => !value)}><ChevronRight size={14} aria-hidden="true" />测试项目<span>{disposableCount}</span></button></div>}
              {disposableCount > 0 && showDisposable && <>{disposableProjects.map(renderProjectCard)}{disposableNodeApps.map(renderNodeCard)}</>}
            </section> : totalProjects > 0 ? <section className="project-search-empty"><Search size={28} /><h2>没有找到匹配的项目</h2><p>试试其他关键词，或清除状态筛选。</p><button className="refresh-button" onClick={() => { setQuery(''); setProjectFilter('all'); }}>显示全部项目</button></section> : <section className="workspace-card empty-projects">
              <div className="workspace-art" aria-hidden="true"><div className="art-sheet art-sheet-back" /><div className="art-sheet art-sheet-front"><FolderOpen size={38} strokeWidth={1.2} /><span /><span /></div><span className="art-small-square" /></div>
              <div className="workspace-message"><h2>{loading ? '正在读取你的项目…' : info ? '从第一个项目开始' : '连接桌面应用，查看项目'}</h2><p>{info ? '从 Git 导入别人做好的应用改成自己用，或添加本机已有的应用文件夹，也可以新建一个空项目。每个项目的数据和文件都保存在本机。' : '实际项目列表由桌面应用读取。'}</p>{info && addActions('empty-add-actions')}</div>
            </section>}
          </> : <WorkspaceHub workspace={workspace} revision={refreshVersion} enabled={Boolean(info) && !nodeDraft.pending} migrating={migrating} paused={paused} cleanupTask={cleanupTask} projects={projects} nodeApps={availableNodeApps} tab={spaceTab} onTabChange={setSpaceTab} isProjectBusy={(project) => projectBlocked(project) || Boolean(tasks[project.id] && !tasks[project.id].final) || Object.values(lifecycleTasks).some((task) => task.projectId === project.id && !task.final)} onTask={acceptLifecycleTask} onProjectTask={(task, project) => acceptTask(task, project)} onRefresh={refresh} onProjectDetails={(project) => openOverview(project, 'storage')} onProjectDelete={openDelete} onNodeDetails={(app) => requestNodeApp(app, 'details')} onNodeDelete={(app) => requestNodeApp(app, 'delete')} onOpenSettings={(section) => openSettings(section)} />}
      </main>
      <footer className="radio-footer"><span>{query.trim() || projectFilter !== 'all' ? `显示 ${filteredCount} / 共 ${totalProjects} 个应用` : `共 ${totalProjects} 个应用`}{runningCount > 0 ? ` · ${runningCount} 个运行中` : ''}</span><span title={info?.workspaceRoot}><ShieldCheck size={12} aria-hidden="true" />数据保存在本机</span></footer>
      {settingsOpen && <SettingsDialog info={info} loading={loading} checkedAt={checkedAt} theme={theme} onTheme={setTheme} section={settingsSection} onSection={setSettingsSection} workspace={workspace} workspaceBlocked={migrating || paused} cleanupBusy={cleanupBusy} closeDisabled={settingsBusy} localDataLocked={localDataLocked} localDataUnavailable={localDataUnavailable} guideEnabled={Boolean(info) && !workspaceBusy && !nodeDraft.pending && !settingsBusy} feedback={feedback} onClose={() => setSettingsOpen(false)} onTask={acceptLifecycleTask} onRefresh={refresh} onOpenHistory={() => { if (settingsBusy) return; setSettingsOpen(false); openSpace('history'); }} onCreateProject={() => { if (settingsBusy) return; setSettingsOpen(false); setShowCreate(true); }} onOpenSpace={() => { if (settingsBusy) return; setSettingsOpen(false); openSpace('overview'); }} onEnvironmentPending={setEnvironmentBusy} onGitPending={setGitEnvironmentBusy} onLocalDataPending={setLocalDataPending} onLocalDataLock={lockLocalData} projects={projects} nodeApps={nodeApps} />}
        {!localDataLocked && <NodeAppsPanel apps={availableNodeApps} workspacePath={info?.workspaceRoot} platformVersion={info?.version} onImportArchive={() => setArchiveImportOpen(true)} instanceId={info?.instanceId} onTask={acceptLifecycleTask} enabled={Boolean(info) && inDesktop && !workspaceBusy} request={nodeRequest} addRequest={nodeAddRequest} onAppChanged={acceptNodeApp} onAppDeleted={acceptNodeDeletion} onDeletionTaskChanged={task => acceptNodeDeletionTasks([task])} onDeletionAbandoned={() => { setNodeRequest(null); void refresh(); }} onModalStateChange={setNodeModalOpen} onDraftStateChange={reportNodeDraft} />}
        {deletionTarget && <ProjectDeleteDialog project={deletionTarget} status={maintenance[deletionTarget.id]} task={requestedDelete?.projectId === deletionTarget.id ? lifecycleTasks[requestedDelete.taskId] ?? tasks[deletionTarget.id] : tasks[deletionTarget.id]} instanceId={info?.instanceId} disabled={!currentDeleteProject || workspaceBusy || Object.values(lifecycleTasks).some(task => task.projectId === deletionTarget.id && !task.final)} onTask={task => { acceptTask(task, deletionTarget); acceptLifecycleTask(task); }} onClose={() => setDeleteProject(null)} onPendingChange={setDeletePending} feedback={<FeedbackNotices messages={messages} dismiss={dismiss} slot={`task:${deletionTarget.id}`} />} />}
        {archiveImportOpen && <ImportPanel disabled={workspaceBusy} instanceId={info?.instanceId} onTask={acceptLifecycleTask} onCompleted={() => void refresh()} onClose={() => setArchiveImportOpen(false)} />}
        {showCreate && <Modal title="新建项目" titleIcon={<Plus size={20} />} description="起一个名字，即可开始使用。" closeDisabled={creating} onClose={() => setShowCreate(false)}>
          {feedback}
          <form className="create-project-form" onSubmit={(event) => void createProject(event)}>
            <label htmlFor="project-name">项目名称 <span className="required-label">必填</span></label>
            <input id="project-name" required value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：孩子的英语学习" readOnly={creating || workspaceBusy} autoComplete="off" />
            <label htmlFor="project-description">简介 <span className="optional-label">选填</span></label>
            <textarea id="project-description" value={description} onChange={(event) => setDescription(event.target.value)} placeholder="这个项目准备帮你做什么？" rows={3} readOnly={creating || workspaceBusy} />
            <p className="create-project-location"><FolderOpen size={14} aria-hidden="true" />保存在熊猫小抽屉的项目目录，你和 AI 都可以修改。</p>
            <div className="confirmation-actions"><button className="primary-button" type="submit" disabled={creating || workspaceBusy || !name.trim()}>{creating ? '正在创建…' : '创建项目'}</button><button type="button" className="refresh-button" disabled={creating} onClick={() => setShowCreate(false)}>取消</button></div>
          </form>
        </Modal>}
        {overviewProject && overview && <Modal title={overviewProject.name} description="网页应用 · 本机独立数据" titleIcon={overviewProject.iconDataUrl ? <img src={overviewProject.iconDataUrl} alt="" /> : <BookOpen size={20} />} headerAside={<span className={`detail-status-badge${overviewProject.state === 'ready' ? ' is-ready' : ''}`}><span />{({ ready: '可使用', unbuilt: '等待构建', error: '需要检查', missing: '文件缺失', maintenance: '操作暂停', recoveryRequired: '有操作待处理' })[overviewProject.state]}</span>} className="software-detail-modal" wide beforeClose={() => !overviewDismissGuard.current || overviewDismissGuard.current()} onClose={dismissOverview}>
          {feedback}
          {overviewUnavailable && <div className="notice" role="status"><CircleAlert size={18} /><div><strong>项目已不在当前工作区列表中</strong><p>可能已被其他操作删除或工作区已经切换。此处保留你的编辑供复制，项目操作已暂停。</p></div></div>}
          <CodexHandoffPanel key={`${info?.instanceId}:${overviewProject.id}`} handoff={handoff} action={handoffAction} disabled={overviewUnavailable || workspaceBusy || overviewVersionDraft.dirty || overviewVersionDraft.pending} onPrepare={action => void copyHandoff(overviewProject, action)} onShowHelp={() => void copyHandoff(overviewProject, 'copy', false)} onClear={clearHandoff} feedback={<FeedbackNotices messages={messages} dismiss={dismiss} slot="handoff" />} buildAction={<button className="refresh-button" disabled={overviewUnavailable || workspaceBusy || overviewVersionDraft.dirty || overviewVersionDraft.pending || building.includes(overviewProject.id) || Boolean(tasks[overviewProject.id] && !tasks[overviewProject.id].final) || !maintenance[overviewProject.id] || maintenance[overviewProject.id]?.recoveryRequired || (maintenance[overviewProject.id]?.mode === 'maintenance' && !maintenance[overviewProject.id]?.readyForChanges) || overviewProject.state === 'missing' || overviewProject.state === 'error'} onClick={() => void buildProject(overviewProject)}><Hammer size={20} />{building.includes(overviewProject.id) ? '构建中…' : '构建更新'}</button>}>
            <button className="primary-button" disabled={overviewUnavailable || overviewVersionDraft.dirty || overviewVersionDraft.pending || overviewProject.state !== 'ready' || projectBlocked(overviewProject) || opening !== null} onClick={() => void openProject(overviewProject, 'desktop')}><Play size={20} />进入应用</button>
            <button className="refresh-button" disabled={overviewUnavailable || workspaceBusy || overviewVersionDraft.dirty || overviewVersionDraft.pending} onClick={() => { const origin = capture(); void invoke('project_reveal', { projectId: overviewProject.id, target: 'root' }).catch(cause => showError(origin, messageFrom(cause))); }}><FolderOpen size={20} />打开本地目录</button>
          </CodexHandoffPanel>
          <FeedbackNotices messages={messages} dismiss={dismiss} slot={`task:${overviewProject.id}`} />
          {buildResults[overviewProject.id]?.issues.map(issue => <p className="task-error" role="alert" key={issue}>{issue}</p>)}
          <ProjectTaskResult task={tasks[overviewProject.id]?.type === 'storageScan' ? buildResults[overviewProject.id]?.task ?? tasks[overviewProject.id] : tasks[overviewProject.id] ?? buildResults[overviewProject.id]?.task} />
          <ProjectOverview key={overviewProject.id} project={overviewProject} instanceId={info?.instanceId} technicalDetails={<> <BuildState project={overviewProject} /><ProjectDetails project={overviewProject} />
            {!overviewUnavailable && overviewProject.state === 'ready' && !projectBlocked(overviewProject) && <button className="text-button reload-button" disabled={opening !== null} onClick={() => setReloadProjectId(overviewProject.id)}><RefreshCw size={14} />重新加载项目窗口</button>}
            {reloadProjectId === overviewProject.id && <div className="operation-confirmation"><strong>重新加载会丢弃项目窗口未保存的内容</strong><p>请先复制草稿或保存到别处。</p><div className="confirmation-actions"><button className="refresh-button" disabled={overviewUnavailable || opening !== null || projectBlocked(overviewProject)} onClick={() => { setReloadProjectId(null); void openProject(overviewProject, 'desktop', true); }}>已处理草稿，重新加载</button><button className="text-button" onClick={() => setReloadProjectId(null)}>取消并保留页面</button></div></div>} </>} initialTab={overview.tab} status={maintenance[overviewProject.id]} task={requestedDelete?.projectId === overviewProject.id ? lifecycleTasks[requestedDelete.taskId] ?? tasks[overviewProject.id] : tasks[overviewProject.id]} onTask={(task) => { acceptTask(task, overviewProject); if (['export', 'copy', 'delete'].includes(task.type)) acceptLifecycleTask(task); }} onChanged={refresh} onClose={requestCloseOverview} onDismissGuardChange={registerOverviewGuard} onDevelopmentStateChange={reportOverviewDevelopment} disabled={overviewUnavailable || workspaceBusy || Object.values(lifecycleTasks).some((task) => task.projectId === overviewProject.id && !task.final)} />
        </Modal>}
    </div>
  );
}

function ProjectTaskResult({ task }: { task?: TaskRecord }) {
  if (!task) return null;
  return task.final && task.status === 'succeeded'
    ? <details className="project-last-task"><summary>最近任务已完成 · 查看结果与日志</summary><ProjectTask task={task} /></details>
    : <ProjectTask task={task} />;
}

function ProjectDetails({ project }: { project: ProjectInfo }) {
  const location = (relative: string | undefined) => relative ? `${project.projectRoot}/${relative}` : '清单不可用';
  return <div className="project-details">
    {project.state === 'unbuilt' && <p className="project-help">源码模板和数据库已准备好。先构建项目，即可在原生应用窗口中打开；也可以先让 AI 修改源码。</p>}
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


export default App;
