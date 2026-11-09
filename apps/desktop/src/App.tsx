import { localizePlatformPrompt } from './platform-prompt';
import { t, useLocale, formatDateTime } from './i18n';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getAllWindows } from '@tauri-apps/api/window';
import {
  CircleAlert, Play,
  Download, FolderOpen, Monitor, Plus,
  RefreshCw, Hammer, Settings, ChevronRight,
} from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { ProjectTask } from './ProjectTask';
import { ProjectOverview } from './ProjectOverview';
import { LocalDirectoryPanel } from './LocalDirectoryPanel';
import { ManagementConfirmation } from './ManagementConfirmation';
import { ProjectDeleteDialog } from './ProjectDeleteDialog';
import { SettingsDialog } from './SettingsDialog';
import { FrontendPlatformProvider, PlatformCliPathProvider, PlatformSkillPathProvider, frontendPlatformFromBootstrap, platformCliPathFromBootstrap, platformSkillPathFromBootstrap } from './frontend-platform';
import type { SettingsSection } from './SettingsDialog';
import { TaskCenter } from './TaskCenter';
import { AttentionCenter } from './AttentionCenter';
import type { AttentionItem } from './AttentionCenter';
import { AiProjectHelp } from './AiProjectHelp';
import { checkedProcessDiagnostics } from './process-diagnostics';
import { AiToolsProvider } from './AiToolsProvider';
import { AddApplicationDialog } from './AddApplicationDialog';
import type { ApplicationSource } from './AddApplicationDialog';
import { ApplicationLibraryWelcome } from './ApplicationLibraryWelcome';
import { Modal } from './Modal';
import { iconSlot, launcherMonogram, ProjectCard } from './ProjectCard';
import { messageFrom } from './ui-utils';
import { useAppearance } from './appearance';
import { SourceAcquisitionPanel } from './SourceAcquisitionPanel';
import { ImportProjectCard } from './ImportProjectCard';
import { canDismissGitImport, gitImportLabel, gitImportNeedsAttention } from './git-import-view-state';
import type { GitImportViewState } from './git-import-view-state';
import { GitImportRecoveryNotice } from './GitImportRecoveryNotice';
import { CodexHandoffPanel } from './CodexHandoffPanel';
import { FeedbackNotices, useManagementFeedback } from './ManagementFeedback';
import { deriveWorkspaceTaskState, mergeLifecycleTask } from './workspace-task-state';
import { matchingProjectTask, recoverBuildResult } from './build-result-state';
import type { BuildResultView } from './build-result-state';
import type { HandoffAction, ProjectHandoff } from './CodexHandoffPanel';
import type { BootstrapInfo, OpenProjectResult, ProjectInfo, MaintenanceStatus, TaskRecord, WorkspaceInfo } from './types';
import { acceptProjectDeletion, checkedProjectDeletion, observeDeletionRuntime, onProjectDeleted, projectWasDeleted, taskProjectOwner, taskWasDeleted } from './project-deletion';
import type { ProjectDeletedNotice } from './project-deletion';


function verifyMaintenanceEpoch(status: MaintenanceStatus, project: ProjectInfo) {
  // Only snapshots without the new field use the legacy command contract.
  if (Object.hasOwn(status, 'latestBuild') && (typeof status.documentEpoch !== 'string' || !status.documentEpoch
    || status.documentEpoch !== project.documentEpoch)) throw new Error(t("项目数据代次已变化或无法核对，请刷新项目列表后重新读取构建结果。"));
}


function App() {
  useLocale();
  const [info, setInfo] = useState<BootstrapInfo | null>(null);
  const frontendPlatform = frontendPlatformFromBootstrap(info?.platform);
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const { theme, setTheme } = useAppearance();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('general');
  const [tasksOpen, setTasksOpen] = useState(false);
  const [attentionOpen, setAttentionOpen] = useState(false);
  const [showDisposable, setShowDisposable] = useState(() => { try { return window.localStorage.getItem('panda-drawer:show-disposable:v1') === '1'; } catch { return false; } });
  useEffect(() => { try { window.localStorage.setItem('panda-drawer:show-disposable:v1', showDisposable ? '1' : '0'); } catch { /* Grouping still works without storage. */ } }, [showDisposable]);
  const [environmentBusy, setEnvironmentBusy] = useState(false);
  const [gitEnvironmentBusy, setGitEnvironmentBusy] = useState(false);
  const [localDataPending, setLocalDataPending] = useState(false);
  const [localDataLocked, setLocalDataLocked] = useState(false);
  const localDataLock = useRef(false);
  const [storagePending, setStoragePending] = useState(false);
  const settingsBusy = environmentBusy || gitEnvironmentBusy || localDataPending || localDataLocked || storagePending;
  const [nodeDraft, setNodeDraft] = useState({ dirty: false, pending: false });
  // The name typed in 新建 travels with "选一个空文件夹"; other ways of opening the folder dialog clear it.
  const [suggestedAppName, setSuggestedAppName] = useState('');
  const [nodeModalOpen, setNodeModalOpen] = useState(false);
  const deletionListenerReady = useRef<Promise<unknown>>(Promise.resolve());
  const [loading, setLoading] = useState(true);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [opening, setOpening] = useState<string | null>(null);
  const [projectFilter, setProjectFilter] = useState<'all' | 'running' | 'attention'>('all');
  const [tasks, setTasks] = useState<Record<string, TaskRecord>>({});
  // Space scans run automatically after builds. Keep the user's build result
  // visible while still polling the scan for its actual busy state.
  const [buildResults, setBuildResults] = useState<Record<string, BuildResultView>>({});
  const [maintenance, setMaintenance] = useState<Record<string, MaintenanceStatus>>({});
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [lifecycleTasks, setLifecycleTasks] = useState<Record<string, TaskRecord>>({});
  const [showSources, setShowSources] = useState(false);
  const [addApplicationOpen, setAddApplicationOpen] = useState(false);
  const [sourceRequest, setSourceRequest] = useState<ApplicationSource | undefined>(undefined);
  const [sourceRequestToken, setSourceRequestToken] = useState(0);
  const [sourcesMounted, setSourcesMounted] = useState(false);
  const [sourceState, setSourceState] = useState<GitImportViewState | null>(null);
  const [webWindows, setWebWindows] = useState<Set<string> | null>(null);
  const listSnapshot = useRef(projects); listSnapshot.current = projects;
  const [sourceRecoveryNeedsAttention, setSourceRecoveryNeedsAttention] = useState(false);
  const sourceRepositories = projects.filter(app => !projectWasDeleted(app.id) && app.gitSource).map(app => ({ ...app.gitSource!, appId: app.id, name: app.name }));
  const sourceRecoveryOpenRef = useRef<(() => void) | null>(null);
  const sourceTaskIds = useRef(new Set<string>());
  const sourceTaskOwners = useRef(new Map<string, string>());
  function closeSourceFlow(options?: { keepSession: boolean }) {
    setShowSources(false);
    if (options?.keepSession || sourceState && !projectWasDeleted(sourceState.snapshot?.appId ?? '') && !(sourceState.task && taskWasDeleted(sourceState.task)) && (sourceState.busy || sourceState.view === 'unknown' || sourceState.snapshot?.cleanupPending)) return;
    setSourcesMounted(false);
    setSourceState(null);
    sourceTaskIds.current.clear();
    sourceTaskOwners.current.clear();
  }
  const [overview, setOverview] = useState<{ project: ProjectInfo; configureRequest?: number } | null>(null);
  const configurationRequest = useRef(0);
  const [overviewDraft, setOverviewDraft] = useState({ dirty: false, pending: false });
  const [deleteProject, setDeleteProject] = useState<ProjectInfo | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [requestedDelete, setRequestedDelete] = useState<{ projectId: string; projectRoot: string; documentEpoch: string; instanceId?: string; taskId: string; origin: { scope: string; version: number } } | null>(null);
  const [reloadProjectId, setReloadProjectId] = useState<string | null>(null);
  // A just-created app is handed to the AI with the create prompt until another is created.
  const [startProjectId, setStartProjectId] = useState<string | null>(null);
  const [building, setBuilding] = useState<string[]>([]);
  const [handoff, setHandoff] = useState<ProjectHandoff | null>(null);
  const [handoffAction, setHandoffAction] = useState<HandoffAction | null>(null);
  const buildPending = useRef(new Set<string>());
  const buildRevisions = useRef<Record<string, number>>({});
  const projectScopes = useRef<Record<string, string>>({});
  const requestId = useRef(0);
  const createPending = useRef(false);
  const openPending = useRef(false);
  const openRequest = useRef(0);
  const openTarget = useRef<string | null>(null);
  const inDesktop = isTauri();
  const instanceId = useRef<string | null>(null);
  const serviceWorkspaceRoot = useRef<string | null>(null);
  const handoffRequest = useRef(0);
  const handoffInFlight = useRef<number | null>(null);
  const clipboardQueue = useRef<Promise<void>>(Promise.resolve());
  const selectedOverviewId = useRef<string | null>(null);
  const selectedOverviewRoot = useRef<string | null>(null);
  const overviewDismissGuard = useRef<(() => boolean) | null>(null);
  const feedbackScope = `projects:${settingsOpen ? 'settings' : showCreate ? 'create' : nodeModalOpen ? 'node' : showSources ? 'sources' : deleteProject ? `delete:${deleteProject.id}` : overview ? `project:${overview.project.id}` : 'page'}`;
  const { capture, isCurrent, error: showError, success: showSuccess, recover, reset, dismiss, forgetProject: forgetFeedback, messages } = useManagementFeedback(feedbackScope);
  function forgetDeletedProject(notice: ProjectDeletedNotice) {
    const id = notice.id;
    requestId.current++;
    setLoading(false);
    setProjects(current => current.filter(project => project.id !== id));
    if (openTarget.current === id) { openRequest.current++; openTarget.current = null; openPending.current = false; setOpening(null); }
    setTasks(current => Object.fromEntries(Object.entries(current).filter(([key, task]) => key !== id && !taskWasDeleted(task))));
    setLifecycleTasks(current => Object.fromEntries(Object.entries(current).filter(([, task]) => !taskWasDeleted(task))));
    for (const taskId of sourceTaskIds.current) if (notice.taskIds.includes(taskId) || sourceTaskOwners.current.get(taskId) === id) { sourceTaskIds.current.delete(taskId); sourceTaskOwners.current.delete(taskId); }
    setBuildResults(current => Object.fromEntries(Object.entries(current).filter(([key]) => key !== id)));
    setMaintenance(current => Object.fromEntries(Object.entries(current).filter(([key]) => key !== id)));
    setBuilding(current => current.filter(key => key !== id)); buildPending.current.delete(id);
    delete buildRevisions.current[id]; delete projectScopes.current[id];
    setDeleteProject(current => current?.id === id ? null : current);
    setRequestedDelete(current => current?.projectId === id ? null : current);
    if (selectedOverviewId.current === id) dismissOverview();
    forgetFeedback(id, notice.taskIds);
  }
  const forgetDeletedProjectRef = useRef(forgetDeletedProject); forgetDeletedProjectRef.current = forgetDeletedProject;
  const registerOverviewGuard = useCallback((guard: (() => boolean) | null) => { overviewDismissGuard.current = guard; }, []);
  const reportOverviewDevelopment = useCallback((state: { dirty: boolean; pending: boolean }) => { setOverviewDraft(state); }, []);
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
  function openOverview(project: ProjectInfo, configure = false) {
    if (nodeDraft.pending || nodeModalOpen || deleteProject) return;
    clearHandoff();
    selectedOverviewId.current = project.id;
    selectedOverviewRoot.current = project.projectRoot;
    setReloadProjectId(null);
    setOverview({ project, configureRequest: configure ? ++configurationRequest.current : undefined });
  }
  function openDelete(project: ProjectInfo) {
    if (nodeDraft.pending || nodeModalOpen || deleteProject || deletePending) return;
    clearHandoff();
    setDeleteProject(project);
  }
  const { migrating, paused } = deriveWorkspaceTaskState(workspace, lifecycleTasks);
  const workspaceBusy = migrating || paused || localDataLocked;
  useEffect(() => { if (workspaceBusy) clearHandoff(); }, [workspaceBusy, clearHandoff]);
  const acceptLifecycleTask = useCallback((task: TaskRecord) => {
    if (taskWasDeleted(task) || task.projectId && projectWasDeleted(task.projectId)) return;
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
      await deletionListenerReady.current;
      if (current !== requestId.current) return;
      // Read bootstrap first so an unavailable committed workspace reports its
      // concrete startup error, rather than a missing native command state.
      let runtime = await invoke<BootstrapInfo>('bootstrap_info');
      if (localDataLock.current || current !== requestId.current) return;
      observeDeletionRuntime(runtime.instanceId);
      if (current !== requestId.current) return;
      const [rawList, currentWorkspace] = await Promise.all([
        invoke<ProjectInfo[]>('app_list'), invoke<WorkspaceInfo>('workspace_info'),
      ]);
      if (currentWorkspace.workspaceRoot !== runtime.workspaceRoot) runtime = await invoke<BootstrapInfo>('bootstrap_info');
      if (current !== requestId.current) return;
      observeDeletionRuntime(runtime.instanceId);
      if (current !== requestId.current) return;
      recover('workspace-read');
      if (instanceId.current && (instanceId.current !== runtime.instanceId || serviceWorkspaceRoot.current !== runtime.workspaceRoot)) {
        origin = reset();
        clearHandoff();
        setBuildResults({});
        // Old service tasks cannot gate writes in the newly observed workspace.
        // Current workspace tasks are accepted below from its own snapshot.
        setLifecycleTasks({});
        sourceTaskIds.current.clear(); sourceTaskOwners.current.clear();
        setSourceState(null); setSourcesMounted(false); setShowSources(false);
        setSourceRecoveryNeedsAttention(false);
        showError(origin, t("工作区服务已切换。旧窗口的草稿仍保留；处理草稿后请明确重新加载项目，并从新目录重新复制 AI 说明。"));
      }
      const list = rawList.filter(project => !projectWasDeleted(project.id));
      const selectedProject = list.find((project) => project.id === selectedOverviewId.current);
      if (selectedOverviewId.current && (!selectedProject || selectedProject.projectRoot !== selectedOverviewRoot.current)) clearHandoff();
      selectedOverviewRoot.current = selectedProject?.projectRoot ?? null;
      instanceId.current = runtime.instanceId;
      serviceWorkspaceRoot.current = runtime.workspaceRoot;
      const previousScopes = projectScopes.current;
      const scopes = Object.fromEntries(list.map(project => [project.id, JSON.stringify([runtime.instanceId, runtime.workspaceRoot, project.projectRoot, project.documentEpoch])]));
      projectScopes.current = scopes;
      const kept = <T,>(values: Record<string, T>) => Object.fromEntries(Object.entries(values).filter(([id]) => scopes[id] && scopes[id] === previousScopes[id]));
      setBuildResults(kept);
      setTasks(kept);
      setMaintenance(kept);
      for (const id of buildPending.current) if (scopes[id] !== previousScopes[id]) buildPending.current.delete(id);
      setBuilding(values => values.filter(id => scopes[id] === previousScopes[id]));
      setWorkspace(currentWorkspace);
      if (currentWorkspace.migrationCleanupTask) acceptLifecycleTask(currentWorkspace.migrationCleanupTask);
      const workspaceTaskId = (typeof currentWorkspace.activeTask === 'string' ? currentWorkspace.activeTask : currentWorkspace.activeTask?.taskId) ?? currentWorkspace.lastTaskId;
      if (workspaceTaskId) { const task = await invoke<TaskRecord>('task_get', { taskId: workspaceTaskId }); if (current === requestId.current) acceptLifecycleTask(task); }
      if (current !== requestId.current) return;
      setInfo(runtime);
      setProjects(list);
      const revisions = Object.fromEntries(list.map(project => [project.id, buildRevisions.current[project.id] ?? 0]));
      const statuses = await Promise.allSettled(list.map(async (project) => {
        const revision = revisions[project.id];
        const status = await invoke<MaintenanceStatus>('maintenance_status', { projectId: project.id });
        if (status.projectId !== undefined && status.projectId !== project.id) throw new Error(t("维护状态项目身份不符。"));
        verifyMaintenanceEpoch(status, project);
        const activeReference = typeof status.activeTask === 'string' ? status.activeTask : status.activeTask?.taskId;
        const activeId = activeReference ?? status.lastTaskId;
        const task = activeId ? await invoke<TaskRecord>('task_get', { taskId: activeId }) : null;
        if (task && !matchingProjectTask(task, project.id, activeId ?? undefined)) throw new Error(t("项目任务身份或格式不符。"));
        return { projectId: project.id, status, task, activeReference, revision,
          build: Object.hasOwn(status, 'latestBuild') ? recoverBuildResult(status.latestBuild ?? null, project.id) : null };
      }));
      if (current !== requestId.current) return;
      const confirmedRuntime = await invoke<BootstrapInfo>('bootstrap_info');
      if (current !== requestId.current) return;
      if (confirmedRuntime.instanceId !== runtime.instanceId || confirmedRuntime.workspaceRoot !== runtime.workspaceRoot) {
        projectScopes.current = {}; setBuildResults({}); setTasks({}); setMaintenance({});
        throw new Error(t("读取构建结果时工作区服务已切换，请刷新后重新核对。"));
      }
      const next: Record<string, MaintenanceStatus> = {};
      const activeTasks: Record<string, TaskRecord> = {};
      const recovered: Record<string, BuildResultView> = {};
      for (const project of list) { const scanning = project.storage?.activeTask; if (matchingProjectTask(scanning, project.id) && scanning.type === 'storageScan') activeTasks[project.id] = scanning; }
      for (const [index, result] of statuses.entries()) {
        const projectId = list[index].id;
        if (result.status === 'fulfilled') {
          recover(`maintenance:${projectId}`);
          next[result.value.projectId] = result.value.status;
          if (result.value.revision === (buildRevisions.current[projectId] ?? 0) && scopes[projectId] === projectScopes.current[projectId]) {
            if (result.value.task) activeTasks[projectId] = result.value.task;
            if (result.value.build) recovered[projectId] = result.value.build;
            if (result.value.task?.type === 'build') {
              if (result.value.build?.task?.taskId === result.value.task.taskId || result.value.activeReference && !result.value.task.final || result.value.build === null) recovered[projectId] = { task: result.value.task, issues: result.value.build?.issues ?? [] };
              else if (result.value.build.task) activeTasks[projectId] = result.value.build.task;
              else delete activeTasks[projectId]; // A bad pointer never falls back to another historical build.
            } else if (!result.value.task && result.value.build?.task && !result.value.build.task.final) activeTasks[projectId] = result.value.build.task;
          }
        } else {
          const issue = t("维护状态读取失败：{p0}。请刷新状态后再操作。", { p0: String(messageFrom(result.reason)) });
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
        for (const id of Object.keys(recovered)) if (currentResult(id) && next[id]?.type === 'build') delete next[id];
        return { ...next, ...Object.fromEntries(Object.entries(activeTasks).filter(([id]) => currentResult(id))) };
      });
      setRefreshVersion((version) => version + 1);
      setCheckedAt(new Date().toISOString());
    } catch (cause) {
      if (current === requestId.current) showError(origin, messageFrom(cause), 'main', { backgroundKey: 'workspace-read' });
    } finally {
      if (current === requestId.current) setLoading(false);
    }
  }, [acceptLifecycleTask, clearHandoff, capture, showError, recover, reset]);

  useEffect(() => {
    let disposed = false; let unlisten: (() => void) | undefined;
    const unsubscribe = onProjectDeleted(notice => { forgetDeletedProjectRef.current(notice); void refresh(); });
    deletionListenerReady.current = isTauri() ? listen<unknown>('panda-project-deleted', event => {
      if (checkedProjectDeletion(event.payload) && !acceptProjectDeletion(event.payload)) void refresh();
    })
      .then(stop => { if (disposed) stop(); else unlisten = stop; })
      .catch(() => { /* Native deletion receipts still invalidate local UI operations. */ }) : Promise.resolve();
    return () => { disposed = true; unlisten?.(); unsubscribe(); };
  }, [refresh]);

  useEffect(() => {
    void refresh();
    return () => { requestId.current += 1; handoffRequest.current++; selectedOverviewId.current = null; };
  }, [refresh]);

  useEffect(() => {
    if (localDataLocked || !info || workspaceBusy) return;
    let cancelled = false; let polling = false;
    const read = async () => {
      if (cancelled || localDataLock.current || polling) return;
      polling = true;
      const generation = requestId.current;
      const origin = capture();
      const current = () => !cancelled && !localDataLock.current && generation === requestId.current;
      try {
        const runtime = await invoke<BootstrapInfo>('bootstrap_info');
        if (!current()) return;
        if (runtime.instanceId !== instanceId.current || runtime.workspaceRoot !== serviceWorkspaceRoot.current) { void refresh(); return; }
        const results = await Promise.allSettled([
          invoke<ProjectInfo[]>('app_list'),
          getAllWindows(),
        ]);
        if (!current()) return;
        const [web, windows] = results;
        if (web.status === 'fulfilled') {
          const values = web.value.filter(project => !projectWasDeleted(project.id));
          const identity = (project: ProjectInfo) => JSON.stringify([project.id, project.projectRoot, project.documentEpoch, project.state, project.name, project.description, project.iconDataUrl, project.manifest]);
          if (values.length !== listSnapshot.current.length || values.some(value => !listSnapshot.current.some(previous => identity(previous) === identity(value)))) void refresh();
          else setProjects(values);
          recover('project-list');
        } else showError(origin, t("应用列表暂时无法更新，正在保留上次读取的内容。"), 'main', { backgroundKey: 'project-list' });
        if (windows.status === 'fulfilled') { setWebWindows(new Set(windows.value.map(value => value.label))); recover('window-status'); }
        else showError(origin, t("应用窗口状态暂时无法核对，正在保留上次读取的状态。"), 'main', { backgroundKey: 'window-status' });
      } catch (cause) { if (current()) showError(origin, messageFrom(cause), 'main', { backgroundKey: 'workspace-read' }); }
      finally { polling = false; }
    };
    void read();
    const timer = window.setInterval(() => void read(), 2000);
    const onFocus = () => { void refresh().then(read); };
    window.addEventListener('focus', onFocus);
    return () => { cancelled = true; window.clearInterval(timer); window.removeEventListener('focus', onFocus); };
  }, [Boolean(info), workspaceBusy, refresh, capture, showError, recover, localDataLocked]);

  async function createProject(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPending.current || workspaceBusy || !name.trim()) return;
    createPending.current = true;
    setCreating(true);
    const origin = capture();
    try {
      const project = await invoke<ProjectInfo>('app_create', {
        input: { name: name.trim(), description: description.trim() || null },
      });
      if (projectWasDeleted(project.id)) return;
      setProjects((previous) => [...previous.filter((item) => item.id !== project.id), project]);
      if (isCurrent(origin)) {
        setName('');
        setDescription('');
        setShowCreate(false);
        setStartProjectId(project.id);
        openOverview(project);
        showSuccess(origin, t("已新建「{p0}」。点下面的「开始做」，和你的 AI 一起把它做出来。", { p0: String(project.name) }), 'main', `projects:project:${project.id}`);
      }
      await refresh();
    } catch (cause) {
      showError(origin, messageFrom(cause));
    } finally {
      createPending.current = false;
      setCreating(false);
    }
  }

  // Every titlebar uses the same reload operation, irrespective of backend capability.
  const reloadFromTitlebar = useRef<(request: { appId?: string; projectId?: string; windowGeneration: number }) => void>(() => {});
  reloadFromTitlebar.current = request => {
    const project = projects.find(item => item.id === (request.appId ?? request.projectId));
    if (project && !projectWasDeleted(project.id)) void openProject(project, 'desktop', true, request.windowGeneration);
  };
  useEffect(() => {
    if (!inDesktop) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<unknown>('app-reload-request', event => {
      const payload = event.payload as { projectId?: unknown; appId?: unknown; windowGeneration?: unknown } | null;
      const projectId = typeof payload?.projectId === 'string' ? payload.projectId : undefined;
      const appId = typeof payload?.appId === 'string' ? payload.appId : undefined;
      const windowGeneration = payload?.windowGeneration;
      if ((projectId || appId) && typeof windowGeneration === 'number' && Number.isSafeInteger(windowGeneration) && windowGeneration >= 0) reloadFromTitlebar.current({ projectId, appId, windowGeneration });
    }).then(unlisten => { if (disposed) unlisten(); else stop = unlisten; }).catch(() => {});
    return () => { disposed = true; stop?.(); };
  }, [inDesktop]);

  const externalReload = useRef<(value: {appId: string; state?: string; phase?: string; message?: string; error?: {message?: string}}) => void>(() => {});
  externalReload.current = value => {
    const project = projects.find(app => app.id === value.appId);
    if (!project || projectWasDeleted(project.id)) return;
    const state = value.state ?? value.phase;
    if (state === 'needsConfiguration') openOverview(project, true);
    else if (state === 'invalidManifest' || state === 'failed') showError(capture(), value.message ?? value.error?.message ?? t("应用暂时无法打开，请检查清单与运行日志。"));
    else if (['opened', 'reloaded', 'ready'].includes(state ?? '') && value.message?.trim()) showSuccess(capture(), value.message);
    void refresh();
  };
  useEffect(() => {
    if (!inDesktop) return;
    let disposed = false; let stop: (() => void) | undefined;
    void listen<{appId: string; state?: string; phase?: string; message?: string; error?: {message?: string}}>('panda-app-reload', event => externalReload.current(event.payload)).then(unlisten => { if (disposed) unlisten(); else stop = unlisten; }).catch(() => {});
    return () => { disposed = true; stop?.(); };
  }, [inDesktop]);

  async function openProject(project: ProjectInfo, target: 'desktop', reload = false, expectedWindowGeneration?: number) {
    if (localDataLock.current || openPending.current || projectWasDeleted(project.id)) return;
    if (project.state === 'needsConfiguration' && !webWindows?.has(`app-${project.id}`)) { openOverview(project, true); return; }
    const request = ++openRequest.current; openTarget.current = project.id;
    openPending.current = true;
    setOpening(`${project.id}:${target}`);
    const origin = capture();
    try {
      const result = await invoke<OpenProjectResult>(reload ? 'app_reload' : 'app_open', { appId: project.id, ...(expectedWindowGeneration === undefined ? {} : {expectedWindowGeneration}) });
      if (request !== openRequest.current || projectWasDeleted(project.id)) return;
      void getAllWindows().then(values => { if (request === openRequest.current && !localDataLock.current) setWebWindows(new Set(values.map(value => value.label))); }).catch(() => {});
      if (result.state === 'needsConfiguration') { openOverview(project, true); void refresh(); return; }
      if (result.state === 'invalidManifest' || result.state === 'failed') { showError(origin, result.message ?? t("应用暂时无法打开，请检查清单与运行日志。")); void refresh(); return; }
      // Meaningful reload notices also serve CLI callers and arrive through the native event.
      // The window appearing is the feedback; only a platform message (for example removed secrets) is shown.
      if (!reload && result.message?.trim()) showSuccess(origin, result.message);
      if (reload) void refresh();
    } catch (cause) {
      if (cause && typeof cause === 'object' && 'code' in cause && cause.code === 'APP_WINDOW_ACTION_EXPIRED') return;
      if (request === openRequest.current && !projectWasDeleted(project.id)) {
        if (cause && typeof cause === 'object' && 'code' in cause && ['WEB_CONFIGURATION_REQUIRED', 'APP_CONFIGURATION_REQUIRED'].includes(String(cause.code))) { openOverview(project, true); void refresh(); }
        else showError(origin, messageFrom(cause));
      }
    } finally {
      if (request === openRequest.current) { openPending.current = false; openTarget.current = null; setOpening(null); }
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
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId, ...(previous.type === 'delete' && previous.projectId ? { projectId: previous.projectId, taskType: 'delete' } : {}) });
          if (cancelled || taskWasDeleted(previous) || taskWasDeleted(task) || !previous.projectId || scopes[previous.projectId] !== projectScopes.current[previous.projectId]) return;
          if (previous.type === 'build' && revisions[previous.projectId] !== buildRevisions.current[previous.projectId]) return;
          if (!matchingProjectTask(task, previous.projectId, previous.taskId) || task.type !== previous.type) throw new Error(t("任务身份或类型不符，请刷新核对。"));
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
          if (!cancelled && !taskWasDeleted(previous)) showError(origin, t("任务状态暂时无法读取：{p0}。读取失败本身不会取消任务，请以最终记录为准。", { p0: String(messageFrom(cause)) }), `task:${previous.projectId}`, { backgroundKey: `task:${previous.taskId}` });
        }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [tasks, acceptLifecycleTask, refresh, capture, showError, recover, localDataLocked]);

  useEffect(() => {
    if (localDataLocked) return;
    const pending = Object.values(lifecycleTasks).filter(task => !task.final && !(task.type === 'nodeGitImport' && sourceTaskIds.current.has(task.taskId)));
    if (!pending.length) return;
    let cancelled = false; let polling = false;
    const timer = window.setInterval(() => {
      if (localDataLock.current || polling) return;
      polling = true;
      void Promise.all(pending.map(async (previous) => {
        const origin = capture();
        try {
          const task = await invoke<TaskRecord>('task_get', { taskId: previous.taskId, ...(previous.type === 'delete' && previous.projectId ? { projectId: previous.projectId, taskType: 'delete' } : {}) });
          if (cancelled || taskWasDeleted(previous) || taskWasDeleted(task)) return;
          recover(`lifecycle:${previous.taskId}`);
          acceptLifecycleTask(task);
          if (task.final) void refresh();
        } catch (cause) { if (!cancelled && !taskWasDeleted(previous)) showError(origin, t("任务状态暂时无法读取：{p0}。读取失败本身不会取消任务，请以最终记录为准；不要重复提交操作。", { p0: String(messageFrom(cause)) }), 'main', { backgroundKey: `lifecycle:${previous.taskId}` }); }
      })).finally(() => { polling = false; });
    }, 1000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [lifecycleTasks, acceptLifecycleTask, refresh, capture, showError, recover, localDataLocked]);

  async function buildProject(project: ProjectInfo) {
    if (buildPending.current.has(project.id)) return;
    if (webRunning(project)) { showError(capture(), t("请先关闭应用窗口，再构建。"), `task:${project.id}`); return; }
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
      const task = await invoke<TaskRecord>('app_build', { appId: project.id, input: status.mode === 'maintenance' ? { maintenanceId: status.maintenanceId } : {} });
      if (!current()) return;
      if (!matchingProjectTask(task, project.id) || task.type !== 'build') throw new Error(t("构建任务身份或类型不符，请刷新核对。"));
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
      const data = await invoke<ProjectHandoff>('app_handoff', { appId: project.id, purpose: startProjectId === project.id ? 'create' : null });
      if (!current()) return;
      setHandoff(data);
      if ((data.projectRoot && data.projectRoot !== initialRoot) || (data.instanceId && data.instanceId !== initialInstance)) {
        showError(origin, t("项目目录或服务信息已变化，请刷新项目列表后重新准备说明。"), 'handoff');
        return;
      }
      if (!copy) return;
      let copied = false;
      try {
        // A system clipboard write cannot be cancelled after closing a detail.
        // Keep writes ordered across projects so an older write cannot finish last.
        const write = clipboardQueue.current.then(async () => {
          if (current()) await navigator.clipboard.writeText(localizePlatformPrompt(data.text));
        });
        clipboardQueue.current = write.catch(() => {});
        await write;
        copied = true;
      } catch { /* The complete handoff remains available for manual copying. */ }
      if (!current()) return;
      showSuccess(origin, copied ? t("已复制「{p0}」的 AI 接手说明。粘贴给其他 AI，再补充你想做的功能即可。", { p0: String(project.name) }) : t("说明已准备好，可以选中下面的文字并复制给 AI。"), 'handoff');
    } catch (cause) { if (current()) showError(origin, messageFrom(cause), 'handoff'); }
    finally {
      if (handoffInFlight.current === sequence) handoffInFlight.current = null;
      if (current()) setHandoffAction(null);
    }
  }

  function acceptTask(task: TaskRecord, deletionTarget?: ProjectInfo) {
    if (taskWasDeleted(task)) return;
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
      const deletedId = requestedDelete.projectId;
      const replacement = projects.some(project => project.id === deletedId && !matchesTarget(project));
      if (!replacement) acceptProjectDeletion({ version: 1, kind: 'app', id: deletedId, instanceId: requestedDelete.instanceId,
        taskIds: [...Object.values(lifecycleTasks), ...Object.values(tasks)].filter(value => value.projectId === deletedId).map(value => value.taskId),
        sourceIds: [], sourceRoots: [requestedDelete.projectRoot] });
      setProjects((current) => current.filter((project) => !matchesTarget(project)));
      if (!replacement) {
        setTasks(current => Object.fromEntries(Object.entries(current).filter(([id, task]) => id !== deletedId && task.projectId !== deletedId)));
        setLifecycleTasks(current => Object.fromEntries(Object.entries(current).filter(([, task]) => task.projectId !== deletedId)));
        setBuildResults(current => Object.fromEntries(Object.entries(current).filter(([id]) => id !== deletedId)));
        setMaintenance(current => Object.fromEntries(Object.entries(current).filter(([id]) => id !== deletedId)));
        setBuilding(current => current.filter(id => id !== deletedId));
        buildPending.current.delete(deletedId);
        delete buildRevisions.current[deletedId];
        delete projectScopes.current[deletedId];
      } else {
        setTasks(current => Object.fromEntries(Object.entries(current).filter(([, value]) => value.taskId !== task.taskId)));
        setLifecycleTasks(current => Object.fromEntries(Object.entries(current).filter(([id]) => id !== task.taskId)));
      }
      void refresh();
      if (isCurrent(requestedDelete.origin)) {
        const matchesDialog = Boolean(deleteProject && matchesTarget(projects.find(project => project.id === deleteProject.id) ?? deleteProject));
        const matchesOverview = Boolean(selectedOverviewId.current === requestedDelete.projectId && overview && matchesTarget(projects.find(project => project.id === overview.project.id) ?? overview.project));
        if (matchesDialog) setDeleteProject(null);
        if (matchesOverview) dismissOverview();
        const deletedName = (matchesDialog ? deleteProject?.name : overview?.project.name) ?? '';
        if (matchesDialog || matchesOverview) showSuccess(requestedDelete.origin, deletedName ? t("已删除「{p0}」。", { p0: deletedName }) : t("应用已删除。"), 'main', 'projects:page');
      }
    }
    setRequestedDelete(null);
  }, [requestedDelete, tasks, lifecycleTasks, dismissOverview, refresh, isCurrent, showSuccess, info?.instanceId, deleteProject, overview, projects]);

  function projectBlocked(project: ProjectInfo) {
    const status = maintenance[project.id];
    return workspaceBusy || !status || status.mode === 'maintenance' || status.recoveryRequired;
  }

  const currentOverviewProject = projects.find((project) => project.id === overview?.project.id);
  const overviewProject = currentOverviewProject ?? overview?.project;
  const overviewUnavailable = Boolean(overview && !currentOverviewProject);
  const currentDeleteProject = projects.find(project => project.id === deleteProject?.id);
  const deletionTarget = currentDeleteProject ?? deleteProject;
  const webRunning = (project: ProjectInfo) => project.webRuntime ? ['starting', 'running', 'closing'].includes(project.webRuntime.state) : webWindows?.has(`app-${project.id}`) ?? false;
  const webRuntimeLabel = (project: ProjectInfo) => project.webRuntime ? ({ starting: t("启动中"), running: t("运行中"), closing: t("正在关闭"), closed: t("应用已关闭"), failed: t("关闭或运行异常") })[project.webRuntime.state] : webWindows === null ? t("状态待核对") : webRunning(project) ? t("运行中") : t("已停止");
  const showImportCard = Boolean(sourceState && !(sourceState.view === 'success' && !sourceState.snapshot?.cleanupPending && sourceState.snapshot?.commitState === 'committed' && projects.some(project => project.id === sourceState.snapshot?.appId)));
  const projectIssues = (project: ProjectInfo) => messages.filter(item => item.kind === 'error' && item.slot === `task:${project.id}`);
  const attentionFor = (project: ProjectInfo): AttentionItem | null => {
    const status = maintenance[project.id];
    const state = status?.recoveryRequired ? 'recoveryRequired' : status?.mode === 'maintenance' ? 'maintenance' : project.state;
    const base = { key: project.id, name: project.name, slot: iconSlot(project.id), iconDataUrl: project.iconDataUrl };
    const view = { label: t("查看"), run: () => openOverview(project) };
    const name = { p0: project.name };
    const firstIssue = project.issues[0] ? t(project.issues[0]) : undefined;
    if (project.webRuntime?.state === 'failed') return { ...base, title: t("「{p0}」关闭或运行异常", name), detail: project.webRuntime.error?.message, primary: view };
    if (state === 'needsConfiguration') return { ...base, title: t("「{p0}」需要填写配置", name), detail: t("填好后就能打开。"), primary: { label: t("填写配置"), run: () => openOverview(project, true) } };
    if (state === 'needsBuild') return { ...base, title: t("「{p0}」需要先准备", name), detail: t("在详情里点「准备应用」。"), primary: view };
    if (state === 'entryMissing') return { ...base, title: t("「{p0}」缺少网页入口文件", name), detail: t("让你的 AI 按 panda.json 补齐入口。"), primary: view };
    if (state === 'error') return { ...base, title: t("「{p0}」出错了", name), detail: firstIssue, primary: view };
    if (state === 'missing') return { ...base, title: t("「{p0}」的文件找不到了", name), detail: t("项目文件夹可能被移动或删除了。"), primary: view };
    if (state === 'maintenance') return { ...base, title: t("「{p0}」正在维护，操作暂停", name), primary: view };
    if (state === 'recoveryRequired') return { ...base, title: t("「{p0}」需要恢复", name), detail: firstIssue, primary: view };
    const issues = projectIssues(project);
    if (issues.length) return { ...base, title: t("「{p0}」需要检查", name), detail: t(issues[issues.length - 1].text), primary: view, dismiss: () => issues.forEach(item => dismiss(item.id)) };
    return null;
  };
  const attentionItems: AttentionItem[] = [];
  if (showImportCard && sourceState && gitImportNeedsAttention(sourceState)) {
    const importName = sourceState.snapshot?.app?.name || sourceState.name || t("正在添加的应用");
    attentionItems.push({ key: 'import', name: importName, slot: 0, title: sourceState.view === 'failed' ? t("「{p0}」没加进来", { p0: importName }) : t("「{p0}」添加需要处理", { p0: importName }), detail: gitImportLabel(sourceState),
      primary: { label: t("查看"), run: () => setShowSources(true) }, ...(canDismissGitImport(sourceState) ? { secondary: { label: t("移除"), run: () => removeImportCard() } } : {}) });
  }
  const attentionProjects = new Set<string>();
  if (!workspaceBusy) for (const project of projects) { const item = attentionFor(project); if (item) { attentionItems.push(item); attentionProjects.add(project.id); } }
  const filteredProjects = projects.filter(project => projectFilter === 'all' || (projectFilter === 'running' ? webRunning(project) : attentionProjects.has(project.id)));
  const attentionCount = attentionItems.length;
  const runningCount = projects.filter(webRunning).length;
  const filterChoices = [['all', t("全部")], ['running', t("运行中")], ['attention', t("需要处理")]] as const;
  const isDisposable = (name: string) => name.includes('可丢弃');
  const mainProjects = filteredProjects.filter(project => !isDisposable(project.name));
  const disposableProjects = filteredProjects.filter(project => isDisposable(project.name));
  const disposableCount = disposableProjects.length;
  const totalProjects = projects.length;
  const importVisible = showImportCard && (projectFilter === 'all' || projectFilter === 'attention' && sourceState && gitImportNeedsAttention(sourceState));
  const filteredCount = filteredProjects.length + (importVisible ? 1 : 0);
  const activeLifecycleTasks = Object.values(lifecycleTasks).filter(task => !task.final);
  const modalOpen = Boolean(overviewProject && overview) || Boolean(deleteProject) || showCreate || nodeModalOpen || settingsOpen || showSources || addApplicationOpen;
  const requestCloseOverview = () => { if (!overviewDismissGuard.current || overviewDismissGuard.current()) dismissOverview(); };
  const feedback = <FeedbackNotices messages={messages} dismiss={dismiss} />;
  const pageFeedback = <FeedbackNotices messages={messages} dismiss={dismiss} toast />;
  const sourceActive = Boolean(sourceState && (sourceState.busy || sourceState.view === 'placement' || sourceState.view === 'waiting' || sourceState.view === 'unknown' || sourceState.task?.final === false || sourceState.snapshot?.cleanupPending || sourceState.snapshot?.remainingPaths.length));
  const addBlocked = !info || workspaceBusy || nodeDraft.pending || nodeModalOpen || creating || opening !== null || sourceActive;
  const existingSourceUrls = [...sourceRepositories.map(source => source.url), ...(sourceState && (sourceActive || sourceState.snapshot?.commitState === 'committed') ? [sourceState.url] : [])];
  const existingCatalogIds = sourceRepositories.flatMap(source => source.catalogId ? [source.catalogId] : []);
  function startSource(source?: ApplicationSource) {
    if (addBlocked) return;
    setAddApplicationOpen(false); setSourceRequest(source); setSourceRequestToken(value => value + 1);
    setSourceState(null); setSourcesMounted(true); setShowSources(true);
  }
  function addLocal(suggested?: unknown) { if (!addBlocked) { setSuggestedAppName(typeof suggested === 'string' ? suggested : ''); setAddApplicationOpen(false); setNodeModalOpen(true); } }
  function addBlank() { if (!addBlocked) { setAddApplicationOpen(false); setShowCreate(true); } }
  const addActions = (className: string) => <div className={className} role="group" aria-label={t("添加应用")}>
    <button className="primary-button" disabled={addBlocked} onClick={() => { if (!addBlocked) { setSourcesMounted(true); setShowSources(true); } }}><Download size={15} aria-hidden="true" />{t("从 Git 添加")}</button>
    <button className="refresh-button" disabled={addBlocked} onClick={() => { if (!addBlocked) { setSuggestedAppName(''); setNodeModalOpen(true); } }}><FolderOpen size={15} aria-hidden="true" />{t("本机文件夹")}</button>
    <button className="refresh-button" disabled={addBlocked || creating} onClick={() => { if (!addBlocked) setShowCreate(true); }}><Plus size={15} aria-hidden="true" />{t("新建")}</button>
  </div>;
  const openSettings = (section?: SettingsSection) => { if (localDataLock.current || nodeDraft.pending || nodeModalOpen) return; if (section) setSettingsSection(section); setTasksOpen(false); setSettingsOpen(true); };
  const recentTasks = [...Object.values(lifecycleTasks), ...Object.values(tasks).filter(task => !lifecycleTasks[task.taskId])].filter(task => !taskWasDeleted(task)).reverse();
  const projectName = (projectId: string | null) => projectId ? projects.find(project => project.id === projectId)?.name ?? (sourceState?.snapshot?.appId === projectId ? sourceState.name : null) : null;
  function removeImportCard() {
    if (!sourceState || !canDismissGitImport(sourceState)) return;
    const taskId = sourceState.task?.taskId;
    if (taskId) setLifecycleTasks(current => Object.fromEntries(Object.entries(current).filter(([id]) => id !== taskId)));
    closeSourceFlow();
  }
  const lockLocalData = useCallback(() => {
    localDataLock.current = true;
    requestId.current++; handoffRequest.current++;
    setLocalDataLocked(true); setLoading(false); clearHandoff();
  }, [clearHandoff]);
  const localDataUnavailable = !inDesktop || !info || workspaceBusy || environmentBusy || gitEnvironmentBusy
    || nodeDraft.pending || nodeDraft.dirty || nodeModalOpen || Boolean(overview) || Boolean(deleteProject) || deletePending
    || creating || Boolean(opening) || building.length > 0 || activeLifecycleTasks.length > 0 || Object.values(tasks).some(task => !task.final);
  const renderProjectCard = (project: ProjectInfo) => <ProjectCard key={project.id} project={project} running={webRunning(project)} status={maintenance[project.id]} task={tasks[project.id]} blocked={projectBlocked(project) || opening !== null || nodeDraft.pending || deletePending} opening={opening === `${project.id}:desktop`} deletingDisabled={workspaceBusy || nodeDraft.pending || deletePending} hasIssue={projectIssues(project).length > 0} detailsDisabled={nodeDraft.pending || deletePending} onEnter={() => void openProject(project, 'desktop')} onDetails={() => openOverview(project)} onDelete={() => openDelete(project)} />;

  return (
    <FrontendPlatformProvider value={frontendPlatform}><PlatformSkillPathProvider value={platformSkillPathFromBootstrap(info?.platform)}><PlatformCliPathProvider value={platformCliPathFromBootstrap(info?.platform)}><AiToolsProvider instanceId={info?.instanceId} scopeKey={info ? `${info.instanceId}:${info.workspaceRoot}` : undefined} enabled={Boolean(info) && !workspaceBusy && !localDataLocked}>
    <div className="radio-shell" data-theme={theme}>
      <header className="radio-header">
        <div className="radio-brand">
          <img src="/panda-app-drawer.png" alt="" />
          <strong className="radio-brand-name">{t("熊猫小抽屉")}</strong>
        </div>
        <div className="radio-header-actions">
          {!localDataLocked && <AttentionCenter items={attentionItems} open={attentionOpen} disabled={nodeDraft.pending} onOpenChange={setAttentionOpen} />}
          {!localDataLocked && <TaskCenter tasks={recentTasks} projectName={projectName} open={tasksOpen} onOpenChange={setTasksOpen} importFlow={sourceState ? { taskId: sourceState.task?.taskId, active: sourceState.busy, needsAttention: gitImportNeedsAttention(sourceState), onOpen: () => { setTasksOpen(false); setShowSources(true); } } : undefined} recoveryFlow={sourceRecoveryNeedsAttention ? { needsAttention: true, onOpen: () => { setTasksOpen(false); sourceRecoveryOpenRef.current?.(); } } : undefined} onViewTask={task => { if (sourceTaskIds.current.has(task.taskId)) { setTasksOpen(false); setShowSources(true); } }} canViewTask={task => sourceTaskIds.current.has(task.taskId)} />}
          <button className="radio-settings-button" aria-label={t("设置")} title={t("设置")} disabled={nodeDraft.pending || localDataLocked} onClick={() => openSettings()}><Settings size={17} /></button>
        </div>
      </header>
      <main className="radio-main">
        {!localDataLocked && <div className="application-heading">
          <div className="application-heading-main"><h1>{t("我的应用")}</h1><span className="application-count" role="status">{t("{count} 个 · {running} 个运行中", { count: totalProjects, running: runningCount })}</span><div className="project-filter-chips" role="group" aria-label={t("筛选项目状态")}>{filterChoices.map(([value, label]) => <button key={value} type="button" aria-pressed={projectFilter === value} onClick={() => setProjectFilter(value)}>{label}{value === 'attention' && attentionCount > 0 && <span className="filter-count">{attentionCount}</span>}</button>)}</div><div className="application-heading-actions">{frontendPlatform.appLibraryAvailable ? <button className="primary-button" disabled={addBlocked} onClick={() => setAddApplicationOpen(true)}><Plus size={15} aria-hidden="true" />{t("添加应用")}</button> : addActions('radio-add-actions')}</div></div>
        </div>}
        <div className="radio-notices">
          {!inDesktop && <div className="notice" role="status"><Monitor size={20} /><div><strong>{t("项目管理在桌面应用中进行")}</strong><p>{t("这里是前端开发页面。启动桌面应用后创建项目，再从项目卡片选择“进入项目”打开原生应用窗口。")}</p></div></div>}
          {!modalOpen && pageFeedback}
          {migrating && <div className="notice" role="status"><RefreshCw size={18} /><div><strong>{t("工作区正在迁移，新操作暂时暂停")}</strong><p>{t("各项目窗口保留未保存内容。请等待任务最终结果，不要重复提交迁移。")}</p></div></div>}
          {paused && <div className="notice" role="status"><CircleAlert size={18} /><div><strong>{t("工作区已暂停，需要查看处理")}</strong><p>{t("请查看迁移任务结果并刷新状态；已有窗口的草稿保留。")}</p></div></div>}
        </div>
          {localDataLocked ? <p className="workspace-operation-status" role="status">{t("本机内容清理已提交，项目管理已停止。请在设置中核对清理结果并退出软件。")}</p> : <>
            {info && !localDataLocked && <GitImportRecoveryNotice reportErrors={sourcesMounted} onFound={() => setSourceRecoveryNeedsAttention(true)} onOpenReady={open => { sourceRecoveryOpenRef.current = open; }} onAttentionChange={setSourceRecoveryNeedsAttention} instanceId={info?.instanceId} onOpenExisting={appId => { setShowSources(false); const project = projects.find(project => project.id === appId); if (project) openOverview(project); else void refresh(); }} />}
      {frontendPlatform.appLibraryAvailable && addApplicationOpen && <AddApplicationDialog disabled={addBlocked} existingUrls={existingSourceUrls} existingIds={existingCatalogIds} onAdd={startSource} onLocal={addLocal} onCreate={addBlank} onClose={() => setAddApplicationOpen(false)} />}
      {sourcesMounted && <SourceAcquisitionPanel initialSource={sourceRequest} requestToken={sourceRequestToken} waitForCompletion={frontendPlatform.appLibraryAvailable} onWebEnter={project => { setShowSources(false); void openProject(project, 'desktop'); }} open={showSources} projectDirectory={info ? `${frontendPlatform.directoryLabel(info.workspaceRoot)}project-sources` : undefined} platformVersion={info?.version} existingRepositories={sourceRepositories} onOpenExisting={appId => { setShowSources(false); const project = projects.find(project => project.id === appId); if (project) void openProject(project, 'desktop'); }} onOpenLocal={() => { setShowSources(false); setNodeModalOpen(true); }} onOpenSettings={() => { setShowSources(false); openSettings('storage'); }} onBackground={() => setShowSources(false)} onStateChange={setSourceState} instanceId={info?.instanceId} onInstalled={() => void refresh()} onTask={task => { if (taskWasDeleted(task)) return; sourceTaskIds.current.add(task.taskId); const owner = taskProjectOwner(task); if (owner) sourceTaskOwners.current.set(task.taskId, owner); acceptLifecycleTask(task); }} onClose={closeSourceFlow} disabled={!info || workspaceBusy} />}
            <div className="sr-only" role="status">{t(loading ? "{count} 个应用，正在读取" : "{count} 个应用", { count: filteredCount })}</div>
            {filteredCount > 0 ? <section className="project-grid radio-project-grid" aria-label={t("项目列表")}>
              {importVisible && sourceState && <ImportProjectCard state={sourceState} onView={() => setShowSources(true)} onRemove={removeImportCard} />}
              {mainProjects.map(renderProjectCard)}
              {disposableCount > 0 && <div className="project-group-divider"><button type="button" className="project-group-toggle" aria-expanded={showDisposable} onClick={() => setShowDisposable((value) => !value)}><ChevronRight size={14} aria-hidden="true" />{t("测试项目")}<span>{disposableCount}</span></button></div>}
              {disposableCount > 0 && showDisposable && <>{disposableProjects.map(renderProjectCard)}</>}
            </section> : totalProjects > 0 ? <section className="project-filter-empty"><FolderOpen size={28} /><h2>{t("没有符合筛选的应用")}</h2><button className="refresh-button" onClick={() => setProjectFilter('all')}>{t("显示全部应用")}</button></section> : frontendPlatform.appLibraryAvailable && info && !loading ? <ApplicationLibraryWelcome disabled={addBlocked} existingUrls={existingSourceUrls} existingIds={existingCatalogIds} onAdd={startSource} onLink={() => startSource()} onLocal={addLocal} onCreate={addBlank} /> : <section className="workspace-card empty-projects">
              <div className="workspace-art" aria-hidden="true"><div className="art-sheet art-sheet-back" /><div className="art-sheet art-sheet-front"><FolderOpen size={38} strokeWidth={1.2} /><span /><span /></div><span className="art-small-square" /></div>
              <div className="workspace-message"><h2>{loading ? t("正在读取你的应用…") : info ? t("从第一个应用开始") : t("连接桌面应用，查看应用")}</h2><p>{info ? t("从 Git 导入别人做好的应用改成自己用，或添加本机已有的应用文件夹，也可以新建一个空项目。每个项目的数据和文件都保存在本机。") : t("实际项目列表由桌面应用读取。")}</p>{info && addActions('empty-add-actions')}</div>
            </section>}
          </>}
      </main>
      <footer className="radio-footer"><span title={info?.workspaceRoot}>{t("项目文件夹")} {info?.workspaceRoot ?? t("等待读取")}</span><button className="text-button" disabled={!info || localDataLocked} onClick={() => { if (info) { const origin = capture(); void invoke('open_project_folder', { path: info.workspaceRoot }).catch(cause => showError(origin, messageFrom(cause))); } }}>{t(frontendPlatform.openFolderLabel)}</button></footer>
      {settingsOpen && <SettingsDialog info={info} loading={loading} checkedAt={checkedAt ? formatDateTime(checkedAt) : null} theme={theme} onTheme={setTheme} section={settingsSection} onSection={setSettingsSection} workspace={workspace} workspaceBlocked={migrating || paused} closeDisabled={settingsBusy} localDataLocked={localDataLocked} localDataUnavailable={localDataUnavailable} guideEnabled={Boolean(info) && !workspaceBusy && !nodeDraft.pending && !settingsBusy} feedback={feedback} onClose={() => setSettingsOpen(false)} onTask={acceptLifecycleTask} onRefresh={refresh} onCreateProject={() => { if (settingsBusy) return; setSettingsOpen(false); setShowCreate(true); }} onEnvironmentPending={setEnvironmentBusy} onGitPending={setGitEnvironmentBusy} onLocalDataPending={setLocalDataPending} onLocalDataLock={lockLocalData} onStoragePending={setStoragePending} revision={refreshVersion} onProjectTask={(task, project) => acceptTask(task, project)} isProjectBusy={project => projectBlocked(project) || Boolean(tasks[project.id] && !tasks[project.id].final) || Object.values(lifecycleTasks).some(task => task.projectId === project.id && !task.final)} projects={projects} />}

        {!localDataLocked && nodeModalOpen && <Modal title={t("添加本机文件夹")} closeDisabled={nodeDraft.pending} onClose={() => setNodeModalOpen(false)}>
          <LocalDirectoryPanel enabled={Boolean(info) && inDesktop && !workspaceBusy} instanceId={info?.instanceId} workspacePath={info?.workspaceRoot} platformVersion={info?.version} apps={projects} suggestedName={suggestedAppName}
            onPendingChange={pending => setNodeDraft({dirty: false, pending})} onAdded={() => void refresh()} onEnter={project => { setNodeModalOpen(false); void openProject(project, 'desktop'); }} onViewDetails={project => { setNodeModalOpen(false); openOverview(project); }} onClose={() => setNodeModalOpen(false)} />
        </Modal>}
        <ManagementConfirmation instanceId={info?.instanceId} />
        {deletionTarget && <ProjectDeleteDialog project={deletionTarget} status={maintenance[deletionTarget.id]} task={requestedDelete?.projectId === deletionTarget.id ? lifecycleTasks[requestedDelete.taskId] ?? tasks[deletionTarget.id] : tasks[deletionTarget.id]} instanceId={info?.instanceId} disabled={!currentDeleteProject || workspaceBusy || Object.values(lifecycleTasks).some(task => task.projectId === deletionTarget.id && !task.final)} onTask={task => { acceptTask(task, deletionTarget); acceptLifecycleTask(task); }} onClose={() => setDeleteProject(null)} onPendingChange={setDeletePending} feedback={<FeedbackNotices messages={messages} dismiss={dismiss} slot={`task:${deletionTarget.id}`} />} />}
        {showCreate && <Modal title={t("新建应用")} titleIcon={<Plus size={20} />} description={t("起一个名字，平台生成起始项目，再交给你的 AI 一起做出来。")} closeDisabled={creating} onClose={() => setShowCreate(false)}>
          {feedback}
          <form className="create-project-form" onSubmit={(event) => void createProject(event)}>
            <label htmlFor="project-name">{t("应用名称")}<span className="required-label">{t("必填")}</span></label>
            <input id="project-name" required value={name} onChange={(event) => setName(event.target.value)} placeholder={t("例如：孩子的英语学习")} readOnly={creating || workspaceBusy} autoComplete="off" />
            <label htmlFor="project-description">{t("简介")}<span className="optional-label">{t("选填")}</span></label>
            <textarea id="project-description" value={description} onChange={(event) => setDescription(event.target.value)} placeholder={t("这个应用准备帮你做什么？")} rows={3} readOnly={creating || workspaceBusy} />
            <p className="create-project-location"><FolderOpen size={14} aria-hidden="true" />{t("保存在熊猫小抽屉的项目文件夹，你和 AI 都可以修改。")} <button type="button" className="text-button" disabled={creating} onClick={() => { setShowCreate(false); addLocal(name.trim()); }}>{t("想放在别的位置？选一个空文件夹")}</button></p>
            <div className="confirmation-actions"><button className="primary-button" type="submit" disabled={creating || workspaceBusy || !name.trim()}>{creating ? t("正在创建…") : t("创建应用")}</button><button type="button" className="refresh-button" disabled={creating} onClick={() => setShowCreate(false)}>{t("取消")}</button></div>
          </form>
        </Modal>}
        {overviewProject && overview && <Modal title={overviewProject.name} description={t("应用详情")} titleIcon={overviewProject.iconDataUrl ? <img src={overviewProject.iconDataUrl} alt="" /> : <span className={`detail-monogram launcher-color-${iconSlot(overviewProject.id)}`} aria-hidden="true">{launcherMonogram(overviewProject.name)}</span>} headerAside={<span className={`detail-status-badge${overviewProject.state === 'ready' ? webRunning(overviewProject) ? ' is-ready' : webWindows === null ? '' : ' is-idle' : ['error', 'missing'].includes(overviewProject.state) ? ' is-error' : ''}`}><span />{overviewProject.state === 'ready' ? webRuntimeLabel(overviewProject) : ({ needsBuild: t("需要准备"), entryMissing: t("入口文件缺失"), needsConfiguration: t("需要配置"), error: t("出错"), missing: t("文件缺失"), maintenance: t("操作暂停"), recoveryRequired: t("需要处理") })[overviewProject.state]}</span>} className="software-detail-modal" beforeClose={() => !overviewDismissGuard.current || overviewDismissGuard.current()} onClose={dismissOverview}>
          {feedback}
          {overviewUnavailable && <div className="notice" role="status"><CircleAlert size={18} /><div><strong>{t("项目已不在当前工作区列表中")}</strong><p>{t("可能已被其他操作删除或工作区已经切换。此处保留你的编辑供复制，项目操作已暂停。")}</p></div></div>}
          <ProjectOverview key={overviewProject.id} configureRequest={overview.configureRequest} project={overviewProject} instanceId={info?.instanceId} running={webWindows === null ? undefined : webRunning(overviewProject)} actions={<><button className="primary-button" disabled={overviewUnavailable || overviewDraft.dirty || overviewDraft.pending || (!webRunning(overviewProject) && !['ready', 'needsConfiguration'].includes(overviewProject.state)) || overviewProject.webRuntime?.state === 'closing' || projectBlocked(overviewProject) || opening !== null} onClick={() => void openProject(overviewProject, 'desktop')}><Play size={20} />{t("打开")}</button>{overviewProject.manifest?.development && <button className="refresh-button" disabled={overviewUnavailable || workspaceBusy || overviewDraft.dirty || overviewDraft.pending || building.includes(overviewProject.id) || Boolean(tasks[overviewProject.id] && !tasks[overviewProject.id].final) || !maintenance[overviewProject.id] || maintenance[overviewProject.id]?.recoveryRequired || (maintenance[overviewProject.id]?.mode === 'maintenance' && !maintenance[overviewProject.id]?.readyForChanges) || overviewProject.state === 'missing' || overviewProject.state === 'error'} onClick={() => void buildProject(overviewProject)}><Hammer size={20} />{building.includes(overviewProject.id) ? t("准备中…") : t("准备应用")}</button>}</>} handoff={<CodexHandoffPanel target={{ appId: overviewProject.id, appName: overviewProject.name, ...(startProjectId === overviewProject.id ? { purpose: 'create' as const } : {}) }} key={`${info?.instanceId}:${overviewProject.id}`} handoff={handoff} action={handoffAction} disabled={overviewUnavailable || workspaceBusy || overviewDraft.dirty || overviewDraft.pending} onOpenFolder={() => { const origin = capture(); void invoke('project_reveal', { projectId: overviewProject.id, target: 'root' }).catch(cause => showError(origin, messageFrom(cause), 'handoff')); }} onPrepare={action => void copyHandoff(overviewProject, action)} onShowHelp={() => void copyHandoff(overviewProject, 'copy', false)} onClear={clearHandoff} feedback={<FeedbackNotices messages={messages} dismiss={dismiss} slot="handoff" />} />} notices={<>          <FeedbackNotices messages={messages} dismiss={dismiss} slot={`task:${overviewProject.id}`} />
          {overviewProject.webRuntime?.state === 'failed' && <div role="alert"><p>{overviewProject.webRuntime.error?.message ?? t("应用关闭或运行未完成，请核对后重试关闭。")}</p><button className="refresh-button" onClick={() => { void invoke('web_app_close', { projectId: overviewProject.id }).then(() => refresh()).catch(cause => showError(capture(), messageFrom(cause), `task:${overviewProject.id}`)); }}>{t("重试关闭应用")}</button></div>}
          {buildResults[overviewProject.id]?.issues.map(issue => <p className="task-error" role="alert" key={issue}>{t(issue)}</p>)}
          <ProjectTaskResult task={tasks[overviewProject.id]?.type === 'storageScan' ? buildResults[overviewProject.id]?.task ?? tasks[overviewProject.id] : tasks[overviewProject.id] ?? buildResults[overviewProject.id]?.task} />
          <BuildFailureHelp project={overviewProject} task={tasks[overviewProject.id] ?? buildResults[overviewProject.id]?.task ?? undefined} />
</>} onDelete={() => openDelete(overviewProject)} technicalDetails={<> <ProjectDetails project={overviewProject} />
            {!overviewUnavailable && overviewProject.state === 'ready' && !projectBlocked(overviewProject) && <button className="text-button reload-button" disabled={opening !== null} onClick={() => setReloadProjectId(overviewProject.id)}><RefreshCw size={14} />{t("重新加载项目窗口")}</button>}
            {reloadProjectId === overviewProject.id && <div className="operation-confirmation"><strong>{t("重新加载会丢弃项目窗口未保存的内容")}</strong><p>{t("请先复制草稿或保存到别处。")}</p><div className="confirmation-actions"><button className="refresh-button" disabled={overviewUnavailable || opening !== null || projectBlocked(overviewProject)} onClick={() => { setReloadProjectId(null); void openProject(overviewProject, 'desktop', true); }}>{t("已处理草稿，重新加载")}</button><button className="text-button" onClick={() => setReloadProjectId(null)}>{t("取消并保留页面")}</button></div></div>} </>} status={maintenance[overviewProject.id]} task={requestedDelete?.projectId === overviewProject.id ? lifecycleTasks[requestedDelete.taskId] ?? tasks[overviewProject.id] : tasks[overviewProject.id]} onTask={(task) => { acceptTask(task, overviewProject); if (task.type === 'delete') acceptLifecycleTask(task); }} onChanged={refresh} onClose={requestCloseOverview} onDismissGuardChange={registerOverviewGuard} onDevelopmentStateChange={reportOverviewDevelopment} disabled={overviewUnavailable || workspaceBusy || Object.values(lifecycleTasks).some((task) => task.projectId === overviewProject.id && !task.final)} />
        </Modal>}
    </div>
    </AiToolsProvider></PlatformCliPathProvider></PlatformSkillPathProvider></FrontendPlatformProvider>
  );
}

function ProjectTaskResult({ task }: { task?: TaskRecord }) {
  if (!task) return null;
  return task.final && task.status === 'succeeded'
    ? <details className="project-last-task"><summary>{t("最近任务已完成 · 查看结果与日志")}</summary><ProjectTask task={task} /></details>
    : <ProjectTask task={task} />;
}

function BuildFailureHelp({ project, task }: { project: ProjectInfo; task?: TaskRecord }) {
  if (task?.type !== 'build' || !task.final || !['failed', 'interrupted'].includes(task.status)) return null;
  return <AiProjectHelp context={{ mode: 'start-failed', appId: project.id, hasBackend: Boolean(project.manifest?.backend), sourceKind: project.ownership === 'user' ? 'local' : 'managed', sourcePath: project.projectRoot, appName: project.name, issues: task.error ? [task.error.message] : [], processDiagnostics: checkedProcessDiagnostics(task.error?.details?.processDiagnostics) }} label={t("复制说明给 AI")} />;
}

function ProjectDetails({ project }: { project: ProjectInfo }) {
  const location = (relative: string | undefined) => relative ? `${project.projectRoot}/${relative}` : t("未声明");
  return <div className="project-details">
    {project.state === 'needsBuild' && <p className="project-help">{t("网页入口尚未生成，请先构建。平台会在项目文件夹里运行声明的脚本。")}</p>}
    {project.state === 'entryMissing' && <p className="project-help">{t("网页入口文件缺失，请让 AI 补齐 panda.json 声明的入口。")}</p>}
    {project.state === 'ready' && <p className="project-help">{t("直接使用项目文件夹里的运行文件。修改完成后，请按 AI 的说明操作。")}</p>}
    {project.issues.length > 0 && <ul className="project-issues">{project.issues.map((issue, index) => <li key={`${index}:${t(issue)}`}>{t(issue)}</li>)}</ul>}
    <dl><div><dt>{t("项目目录")}</dt><dd><code>{project.projectRoot}</code></dd></div><div><dt>{t("网页入口")}</dt><dd><code>{location(project.manifest ? `${project.manifest.web.root}/${project.manifest.web.entry}` : undefined)}</code></dd></div>{project.manifest?.development && <div><dt>{t("构建脚本")}</dt><dd><code>npm run {project.manifest.development.build.script}</code></dd></div>}{project.manifest?.platform && <><div><dt>{t("数据库")}</dt><dd><code>{location('data/main.sqlite')}</code> {t("· 首次使用时创建")}</dd></div><div><dt>{t("附件")}</dt><dd><code>{location('data/attachments')}</code></dd></div></>}<div><dt>{t("开发说明")}</dt><dd><code>{location(project.manifest?.documentation?.development)}</code></dd></div></dl>
  </div>;
}


export default App;
