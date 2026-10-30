import { invoke } from '@tauri-apps/api/core';
import { FolderOpen, Monitor, Square, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { Modal } from './Modal';
import { ApplicationDetails, ApplicationDetailRow } from './ApplicationDetails';
import { bytesText, partBytes } from './storage-utils';
import type { WorkspaceStorage } from './workspace-storage-data';
import { nodeRowFor } from './workspace-storage-data';
import { iconSlot, launcherMonogram } from './ProjectCard';
import type { StoragePart } from './types';
import { NodeAppDeleteDialog } from './NodeAppDeleteDialog';
import { NodeConfigurationForm } from './NodeConfigurationForm';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import { nodeConfigurationState } from './node-configuration-state';
import { CodexHandoffPanel } from './CodexHandoffPanel';
import type { ProjectHandoff } from './CodexHandoffPanel';
import { LocalDirectoryPanel } from './LocalDirectoryPanel';
import { AiProjectHelp } from './AiProjectHelp';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { onProjectDeleted, projectWasDeleted } from './project-deletion';
import type { NodeAppDeleteResult, NodeAppDetail, NodeConfigurationInput, NodeDeletionTaskSnapshot, NodeSecretChange } from './node-app-types';

let handoffClipboard = Promise.resolve();
type Draft = NodeConfigurationDraft;
type SecretDraft = Draft['secrets'][string];
const stateLabels: Record<NodeAppDetail['state'], string> = { stopped: '已停止', starting: '启动中', running: '运行中', stopping: '停止中', failed: '运行异常' };
function runtimeStateLabel(app: NodeAppDetail): string {
  return app.runRecovery?.state === 'orphaned' ? '遗留运行' : app.runRecovery?.state === 'unconfirmed' ? '运行待核对' : stateLabels[app.state];
}

function draftFrom(app: NodeAppDetail): Draft {
  return {
    ordinary: Object.fromEntries(app.fields.filter((field) => !field.secret).map((field) => [field.name, Object.hasOwn(app.configuration.values, field.name) ? String(app.configuration.values[field.name]) : ''])),
    secrets: Object.fromEntries(app.fields.filter((field) => field.secret).map((field) => [field.name, { action: 'keep', value: '' }])),
  };
}

export function nodeConfigurationInput(app: NodeAppDetail, draft: Draft): NodeConfigurationInput {
  const values: NodeConfigurationInput['values'] = {};
  const secrets: Record<string, NodeSecretChange> = {};
  for (const field of app.fields) {
    if (field.secret) {
      const edit = draft.secrets[field.name] ?? { action: 'keep', value: '' };
      secrets[field.name] = edit.action === 'replace' ? { action: 'replace', value: edit.value } : { action: edit.action };
      continue;
    }
    const value = draft.ordinary[field.name] ?? '';
    if (!value.trim()) continue;
    if (field.type === 'integer') {
      if (!/^-?\d+$/.test(value) || !Number.isSafeInteger(Number(value))) throw new Error(`「${field.label}」请填写可精确保存的整数（绝对值不超过 9007199254740991）。`);
      values[field.name] = Number(value);
    } else if (field.type === 'boolean') {
      if (value !== 'true' && value !== 'false') throw new Error(`「${field.label}」请选择开启或关闭。`);
      values[field.name] = value === 'true';
    } else {
      values[field.name] = value;
    }
  }
  return { revision: app.configuration.revision, ...(app.manifestDigest ? { manifestDigest: app.manifestDigest } : {}), values, secrets };
}

export type NodeAppRequest = { app: NodeAppDetail; intent: 'enter' | 'configure' | 'details' | 'delete'; token: number }
  | { app: Pick<NodeAppDetail, 'id' | 'name'>; intent: 'delete-task'; task: NodeDeletionTaskSnapshot; token: number };
export function NodeAppsPanel({ apps = [], workspacePath, platformVersion, onImportArchive, enabled, instanceId, request, addRequest = 0, onAppChanged, onAppDeleted, onDeletionTaskChanged, onDeletionAbandoned, onModalStateChange, onDraftStateChange }: {
  apps?: NodeAppDetail[]; workspacePath?: string; platformVersion?: string; onImportArchive?: () => void;
  enabled: boolean; request?: NodeAppRequest | null; addRequest?: number;
  instanceId?: string; onTask?: (task: TaskRecord) => void;
  onAppChanged?: (app: NodeAppDetail) => void; onAppDeleted?: (appId: string, result: NodeAppDeleteResult) => void; onModalStateChange?: (open: boolean) => void;
  onDeletionTaskChanged?: (task: NodeDeletionTaskSnapshot) => void; onDeletionAbandoned?: (task: NodeDeletionTaskSnapshot) => void;
  onDraftStateChange: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [modalOpen, setModalOpen] = useState(false);
  const [handoff, setHandoff] = useState<ProjectHandoff | null>(null);
  const [view, setView] = useState<'details' | 'configuration'>('details');
  const [storage, setStorage] = useState<StoragePart | null>(null);
  const [dismissConfirm, setDismissConfirm] = useState(false);
  const [detail, setDetail] = useState<NodeAppDetail | null>(null);
  const [draft, setDraft] = useState<Draft>({ ordinary: {}, secrets: {} });
  const [configurationRead, setConfigurationRead] = useState(0);
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [openingFolder, setOpeningFolder] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [launchError, setLaunchError] = useState('');
  const [statusError, setStatusError] = useState('');
  const [notice, setNotice] = useState('');
  const [addOpen, setAddOpen] = useState(false);
  const [directoryPending, setDirectoryPending] = useState(false);
  const [requestedSelection, setRequestedSelection] = useState<NodeAppDetail | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ app: Pick<NodeAppDetail, 'id' | 'name'>; task?: NodeDeletionTaskSnapshot; token: number; instanceId?: string } | null>(null);
  const [deletionPending, setDeletionPending] = useState(false);
  const deleteTargetRef = useRef(deleteTarget); deleteTargetRef.current = deleteTarget;
  const deletionSequence = useRef(0);
  const currentInstance = useRef(instanceId); currentInstance.current = instanceId;
  const statusRead = useRef(0);
  const pendingOperation = useRef(pending); pendingOperation.current = pending;
  const seenRequest = useRef<number | null>(null);
  const seenAdd = useRef(0);
  const changedCallback = useRef(onAppChanged); changedCallback.current = onAppChanged;
  const deletedCallback = useRef(onAppDeleted); deletedCallback.current = onAppDeleted;
  const modalCallback = useRef(onModalStateChange); modalCallback.current = onModalStateChange;
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const selected = useRef<string | null>(null);
  const confirmation = useRef<HTMLDivElement>(null);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const configurationDirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  const configurationState = detail ? nodeConfigurationState(detail) : null;
  const launchFailureText = launchError || (detail?.state === 'failed' ? detail.lastError ?? '' : '');
  const canStop = detail ? detail.runRecovery?.canStop ?? ['running', 'starting', 'failed'].includes(detail.state) : false;
  const dirty = configurationDirty;
  const blocked = !enabled || Boolean(pending) || directoryPending || Boolean(deleteTarget);
  const projectPath = detail?.sourcePath;
  const retired = Boolean(detail?.installation);
  const runtimeLogPaths = detail?.objectLayout?.controlPath ? { current: `${detail.objectLayout.controlPath}/logs/current.log`, previous: `${detail.objectLayout.controlPath}/logs/previous.log` } : undefined;

  function clearDeletion() {
    deleteTargetRef.current = null;
    setDeleteTarget(null); setDeletionPending(false);
  }
  const openDeletion = useCallback((app: NodeAppDetail) => {
    if (!enabled || pendingOperation.current || directoryPending || loading || operation.current || deleteTargetRef.current || (dirty && selected.current === app.id)) return;
    statusRead.current++;
    const target = { app, token: ++deletionSequence.current, instanceId };
    deleteTargetRef.current = target; setDeleteTarget(target);
  }, [enabled, directoryPending, loading, dirty, instanceId]);

  const accept = useCallback((value: NodeAppDetail, resetDraft: boolean) => {
    if (projectWasDeleted(value.id)) return;
    changedCallback.current?.(value);
    if (selected.current === value.id) {
      setDetail((current) => resetDraft || !current ? value : { ...current, state: value.state, runRecovery: value.runRecovery, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError, configuration: { ...current.configuration, canStart: value.configuration.canStart } });
      if (resetDraft) { setDraft(draftFrom(value)); setConfigurationRead((current) => current + 1); }
    }
  }, []);

  const refreshMissingConfiguration = useCallback(async (cause: unknown, appId: string) => {
    if (!cause || typeof cause !== 'object' || !('code' in cause) || cause.code !== 'NODE_CONFIGURATION_REQUIRED') return false;
    const selection = sequence.current; const runtime = currentInstance.current;
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId });
      if (!alive.current || selected.current !== appId || sequence.current !== selection || currentInstance.current !== runtime || value.id !== appId) return true;
      accept(value, true); setModalOpen(true); setView('configuration'); setError(''); setLaunchError('');
      return true;
    } catch { return false; }
  }, [accept]);

  const enterApp = useCallback(async (value: NodeAppDetail) => {
    if (operation.current || !enabled) return;
    if (value.runRecovery || !value.configuration.canStart) { setModalOpen(true); setView(nodeConfigurationState(value).blockedView); return; }
    operation.current = true; setPending('enter'); setError(''); setLaunchError(''); setNotice('');
    const appId = value.id;
    try {
      if (value.state === 'running') await invoke('node_app_open', { appId });
      else {

        const opened = await invoke<NodeAppDetail>('node_app_start', { appId });
        if (alive.current && selected.current === appId) accept(opened, false);
      }
    } catch (cause) {
      if (alive.current && selected.current === appId && !await refreshMissingConfiguration(cause, appId)) { setError(messageFrom(cause)); setLaunchError(messageFrom(cause)); setModalOpen(true); setView('details'); }
    } finally { if (alive.current && selected.current === appId) { operation.current = false; setPending(null); } }
  }, [accept, enabled, refreshMissingConfiguration]);

  const select = useCallback(async (app: NodeAppDetail, initialView: 'details' | 'configuration' = 'details', enter = false) => {
    const selection = ++sequence.current;
    const current = () => alive.current && selected.current === app.id && sequence.current === selection && currentInstance.current === instanceId;
    selected.current = app.id;
    clearDeletion();
    setHandoff(null);
    setDetail(app); setDraft(draftFrom(app)); setError(''); setLaunchError(''); setStatusError(''); setNotice(''); setLoading(true);
    setView(initialView === 'configuration' && !app.fields.length ? 'details' : initialView); setDismissConfirm(false); setModalOpen(!enter);
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId: app.id });
      if (!current()) return;
      if (value.id !== app.id) throw new Error('配置返回的应用身份不符，请重新读取。');
      accept(value, true);
      if (initialView === 'configuration') setView(value.fields.length ? 'configuration' : 'details');
      if (enter) {
        if (!value.configuration.canStart) { setModalOpen(true); setView(nodeConfigurationState(value).blockedView); }
        else await enterApp(value);
      }
    } catch (cause) { if (current()) { setError(messageFrom(cause)); setModalOpen(true); } }
    finally { if (current()) setLoading(false); }
  }, [accept, enterApp, instanceId]);

  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; selected.current = null; draftCallback.current({ dirty: false, pending: false }); }; }, []);
  useEffect(() => onProjectDeleted(notice => {
    if (notice.kind !== 'node' || notice.instanceId !== currentInstance.current) return;
    if (selected.current === notice.id) {
      closeDetails(); operation.current = false; setPending(null);
      setError(''); setLaunchError(''); setStatusError(''); setNotice(''); draftCallback.current({ dirty: false, pending: false });
    } else if (deleteTargetRef.current?.app.id === notice.id) clearDeletion();
    setRequestedSelection(current => current?.id === notice.id ? null : current);
  }), []);
  useEffect(() => {
    statusRead.current++; sequence.current++; setLoading(false); setHandoff(null);
    clearDeletion();
  }, [instanceId]);
  useEffect(() => {
    if (!request || request.token === seenRequest.current || projectWasDeleted(request.app.id)) return;
    if (deleteTargetRef.current) { seenRequest.current = request.token; return; }
    if (operation.current || directoryPending) return;
    seenRequest.current = request.token;
    if (request.intent === 'delete-task') {
      if (!enabled || pendingOperation.current || loading || (dirty && selected.current === request.app.id)) return;
      statusRead.current++;
      const target = { app: request.app, task: request.task, token: ++deletionSequence.current, instanceId };
      deleteTargetRef.current = target; setDeleteTarget(target); return;
    }
    if (request.intent === 'delete') { openDeletion(request.app); return; }
    if (dirty && selected.current !== request.app.id) { setRequestedSelection(request.app); return; }
    void select(request.app, request.intent === 'enter' || request.intent === 'configure' ? 'configuration' : 'details', request.intent === 'enter');
  }, [request, select, openDeletion, pending, directoryPending]);
  useEffect(() => {
    if (!addRequest || addRequest === seenAdd.current || operation.current || directoryPending || !enabled) return;
    seenAdd.current = addRequest; setAddOpen(true); setError(''); setLaunchError('');
  }, [addRequest, enabled, pending, directoryPending]);
  useEffect(() => { modalCallback.current?.(modalOpen || addOpen || Boolean(deleteTarget)); }, [modalOpen, addOpen, Boolean(deleteTarget)]);
  useEffect(() => { if (dismissConfirm || requestedSelection) confirmation.current?.focus(); }, [dismissConfirm, requestedSelection]);
  useEffect(() => { draftCallback.current({ dirty, pending: Boolean(pending) || loading || directoryPending || deletionPending }); }, [dirty, pending, loading, directoryPending, deletionPending]);
  useEffect(() => {
    if (!enabled || !detail || pending || loading || !modalOpen || deleteTarget) return;
    let cancelled = false; let polling = false;
    const appId = detail.id;
    const selection = sequence.current;
    const poll = () => {
      if (polling || operation.current) return;
      polling = true;
      const revision = statusRead.current;
      const current = () => !cancelled && alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId && statusRead.current === revision;
      void invoke<NodeAppDetail>('node_app_status', { appId }).then((value) => {
        if (value.id !== appId) throw new Error('运行状态身份不符。');
        if (current() && !operation.current) {
          // Status refresh never replaces configuration declarations, the saved
          // revision used by a draft, or any password text the user is editing.
          setDetail((current) => current?.id === appId ? { ...current, state: value.state, runRecovery: value.runRecovery, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError, configuration: { ...current.configuration, canStart: value.configuration.canStart } } : current);
          changedCallback.current?.(value);
          setStatusError('');
          if (value.state !== 'running') setNotice(current => current === '应用窗口已打开。关闭窗口会停止本应用。' ? '' : current);
        }
      }).catch(() => { if (current()) setStatusError('运行状态暂时无法更新，正在自动重试。应用不会因此自动重启。'); }).finally(() => { polling = false; });
    };
    // Returning from the business window must also refresh overview actions.
    // Observe the shared details view without replacing configuration drafts.
    const visible = () => { if (document.visibilityState === 'visible') poll(); };
    window.addEventListener('focus', poll); document.addEventListener('visibilitychange', visible);
    const timer = window.setInterval(poll, 1500);
    return () => { cancelled = true; if (timer !== null) window.clearInterval(timer); window.removeEventListener('focus', poll); document.removeEventListener('visibilitychange', visible); };
  }, [enabled, detail?.id, pending, loading, modalOpen, view, instanceId, Boolean(deleteTarget)]);

  function acceptDirectory(value: NodeAppDetail) {
    // Keep the add result visible until the user chooses its next action.
    changedCallback.current?.(value);
  }
  function enterDirectory(value: NodeAppDetail) {
    if (directoryPending || operation.current || !enabled) return;
    setAddOpen(false); setError(''); setLaunchError('');
    void select(value, 'configuration', value.configuration.canStart);
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (blocked || operation.current || !detail || loading) return;
    let input: NodeConfigurationInput;
    try { input = nodeConfigurationInput(detail, draft); }
    catch (cause) { setError(messageFrom(cause)); return; }
    operation.current = true; setPending('save'); setError(''); setLaunchError(''); setNotice('');
    const appId = detail.id;
    const selection = sequence.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId;
    statusRead.current++;
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration_save', { appId, input });
      if (!current()) return;
      if (value.id !== appId) throw new Error('保存结果的应用身份不符，请重新读取配置后核对。');
      accept(value, true);
      setView('details');
      setNotice(value.state === 'running' ? '已保存，重启后生效' : '已保存，下次进入时生效');
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally {
      // A workspace change revokes this response, while the pending write still
      // holds the operation gate until it has finished. Never retry that write.
      if (alive.current && selected.current === appId) { operation.current = false; setPending(null); }
    }
  }

  async function run(command: 'node_app_start' | 'node_app_stop' | 'node_app_restart') {
    if (blocked || operation.current || !detail || loading || (dirty && command !== 'node_app_stop')) return;
    operation.current = true; setPending(command); setError(''); setLaunchError(''); setNotice('');
    const appId = detail.id;
    try {
      const value = await invoke<NodeAppDetail>(command, { appId });
      if (!alive.current || selected.current !== appId) return;
      accept(value, !dirty);
      setStatusError('');
      if (command === 'node_app_stop') setNotice(value.state === 'stopped' ? '应用已关闭，相关服务已停止。已保存的配置保留。' : '关闭结果已更新，请查看应用状态。');
      if (command === 'node_app_start' || command === 'node_app_restart') setNotice(value.state === 'running' ? '应用窗口已打开。关闭窗口会停止本应用。' : '打开结果已更新，请查看应用状态。');
    } catch (cause) { if (alive.current && selected.current === appId && !await refreshMissingConfiguration(cause, appId)) { setError(messageFrom(cause)); if (command === 'node_app_start' || command === 'node_app_restart') setLaunchError(messageFrom(cause)); } }
    finally { if (alive.current && selected.current === appId) { operation.current = false; setPending(null); } }
  }

  async function open(command: 'node_app_open' | 'node_app_open_help', fieldName?: string) {
    if (blocked || operation.current || !detail) return;
    operation.current = true; setPending(command); setError(''); setLaunchError('');
    const appId = detail.id;
    try { await invoke(command, { appId: detail.id, ...(fieldName ? { fieldName } : {}) }); }
    catch (cause) { if (alive.current && selected.current === appId) setError(messageFrom(cause)); }
    finally { if (alive.current && selected.current === appId) { operation.current = false; setPending(null); } }
  }

  function ordinaryChange(name: string, value: string) {
    setDraft((current) => ({ ...current, ordinary: { ...current.ordinary, [name]: value }, secrets: Object.fromEntries(Object.entries(current.secrets).map(([key, edit]) => [key, detail?.fields.find((field) => field.name === key)?.endpointField === name && edit.action === 'rebind' ? { action: 'keep', value: '' } : edit])) }));
    setNotice('');
  }
  function secretChange(name: string, edit: SecretDraft) { setDraft((current) => ({ ...current, secrets: { ...current.secrets, [name]: edit } })); setNotice('');  }
  function closeDetails() {
    sequence.current++; statusRead.current++; selected.current = null; clearDeletion(); setModalOpen(false); setDetail(null);
    setHandoff(null);
    setDraft({ ordinary: {}, secrets: {} }); setDismissConfirm(false); setRequestedSelection(null); setLoading(false);
  }
  function acceptDeletion(appId: string, result: NodeAppDeleteResult) {
    const target = deleteTargetRef.current;
    if (!target || target.app.id !== appId || target.instanceId !== currentInstance.current) return;
    // Revoke all observations for the deleted selection before the launcher is
    // notified. An unrelated detail/draft remains mounted behind its dialog.
    statusRead.current++;
    if (selected.current === appId) { closeDetails(); }
    else clearDeletion();
    deletedCallback.current?.(appId, result);
  }
  async function prepareHandoff(copy: boolean) {
    if (blocked || loading || !detail || operation.current) return;
    operation.current = true; setPending(copy ? 'copy-handoff' : 'view-handoff'); setError('');
    const appId = detail.id; const selection = sequence.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId;
    try {
      const value = await invoke<ProjectHandoff>('node_app_handoff', { appId });
      if (!current()) return;
      if (!value.projectRoot || !value.instanceId || value.instanceId !== instanceId) throw new Error('平台已变化，请重新打开详情。');
      setHandoff(value);
      if (copy) {
        let copied = false;
        const write = handoffClipboard.then(async () => { if (current()) { await navigator.clipboard.writeText(value.text); copied = true; } });
        handoffClipboard = write.catch(() => {});
        await write.catch(() => {});
        if (current()) setNotice(copied ? '提示词已复制。' : '请手动复制下方完整说明。');
      }
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }
  async function revealSource() {
    if (!enabled || !detail || !projectPath || openingFolder === projectPath) return;
    const path = projectPath;
    setOpeningFolder(path); setError('');
    const appId = detail.id; const selection = sequence.current;
    try { await invoke('open_project_folder', { path }); }
    catch (cause) { if (alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId) setError(messageFrom(cause)); }
    finally { if (alive.current) setOpeningFolder(current => current === path ? null : current); }
  }
  function changeView(value: typeof view) {
    if (blocked || loading) return;
    clearDeletion(); setView(value); setDismissConfirm(false);
  }
  useEffect(() => {
    if (!enabled || !modalOpen || !detail) return;
    let cancelled = false;
    setStorage(null);
    void invoke<WorkspaceStorage>('workspace_storage').then(value => {
      if (!cancelled) setStorage(nodeRowFor(value.nodeApps, detail.id) ?? null);
    }).catch(() => { /* An unknown measurement stays unknown. */ });
    return () => { cancelled = true; };
  }, [enabled, modalOpen, detail?.id, instanceId]);

  const deleteDialog = deleteTarget && <NodeAppDeleteDialog key={deleteTarget.token} app={deleteTarget.app} initialTask={deleteTarget.task} enabled={enabled && !pending && !directoryPending && !loading} instanceId={instanceId} onClose={clearDeletion} onPendingChange={value => { if (deleteTargetRef.current?.token === deleteTarget.token) setDeletionPending(value); }} onDeleted={acceptDeletion} onTaskChanged={onDeletionTaskChanged} onAbandoned={task => { if (deleteTargetRef.current?.token !== deleteTarget.token || currentInstance.current !== deleteTarget.instanceId) return; clearDeletion(); onDeletionAbandoned?.(task); }} />;

  return <>
    {modalOpen && detail && <Modal title={detail.name} titleIcon={<span className={`detail-monogram launcher-color-${iconSlot(detail.id)}`} aria-hidden="true">{launcherMonogram(detail.name)}</span>} description="Node 服务 · 本机独立数据" headerAside={<span className={`detail-status-badge node-process-state node-process-${detail.state}${!detail.runRecovery && configurationState?.needsAttention ? ' is-attention' : detail.state === 'running' ? ' is-ready' : detail.state === 'failed' ? ' is-error' : detail.state === 'stopped' ? ' is-idle' : ''}`} role="status"><span />{pending === 'node_app_start' || pending === 'node_app_restart' ? '打开中…' : pending === 'node_app_stop' ? '关闭中…' : detail.runRecovery ? runtimeStateLabel(detail) : configurationState?.needsAttention ? '需要配置' : detail.state === 'failed' ? '出错' : runtimeStateLabel(detail)}</span>} className="software-detail-modal node-detail-modal" closeDisabled={Boolean(pending) || deletionPending} beforeClose={() => { if (dirty) { setDismissConfirm(true); return false; } return true; }} onClose={closeDetails}>
      <section className="node-app-detail node-app-modal-detail" aria-label={`${detail.name}项目详情`} aria-busy={Boolean(pending) || loading}>
        {dismissConfirm && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="关闭配置确认"><strong>有未保存的修改</strong><p>关闭会清空当前配置草稿，包括尚未保存的密钥。已保存内容不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="primary-button" onClick={closeDetails}>丢弃草稿并关闭</button></div></div>}
        {requestedSelection && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="切换项目确认"><strong>当前有未保存的修改</strong><p>继续会清空当前配置草稿，包括尚未保存的密钥。已保存内容不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setRequestedSelection(null)}>继续编辑</button><button className="primary-button" onClick={() => { const value = requestedSelection; setRequestedSelection(null); void select(value, 'configuration'); }}>丢弃草稿并切换</button></div></div>}
        {error && <p className="task-error node-inline-message" role="alert">{error}</p>}
        {launchFailureText && !retired && <div className="node-launch-error" role="alert"><h3>「{detail.name}」没能打开</h3><p className="task-error node-inline-message">{launchFailureText}</p><p>文件在哪里：<code>{projectPath ?? '源码目录暂不可用'}</code></p><p>应用直接运行项目文件夹里的代码，最近的修改可能影响启动。 启动过程中程序可能已经执行，请以实际记录核对应用数据。</p><AiProjectHelp allowCodex={false} context={{ runtimeLogPaths, mode: 'start-failed', sourceKind: detail.gitSource ? 'git' : 'local', sourcePath: projectPath, appName: detail.name, appVersion: detail.version, platformVersion, issues: [launchFailureText] }} label="复制说明给 AI" className="primary-button" disabled={Boolean(pending)} /></div>}
        {notice && <p className="workspace-inline-notice node-inline-message" role="status">{notice}</p>}
        <ApplicationDetails
          children={view === 'configuration' ? <div className="detail-overview-body node-configuration-body">
          <div className="detail-section-heading"><h3>应用配置</h3><button className="text-button" disabled={blocked || loading} onClick={() => changeView('details')}>返回详情</button></div>
          {detail.configuration.unavailable ? <p className="task-error" role="alert">配置暂不可用，请先检查运行状态。</p> : <NodeConfigurationForm key={`${instanceId ?? ''}:${detail.id}:${configurationRead}`} app={detail} draft={draft} active={!deleteTarget} disabled={blocked || loading} dirty={configurationDirty} saving={pending === 'save'} onOrdinaryChange={ordinaryChange} onSecretChange={secretChange} onSave={(event) => void save(event)} onOpenHelp={(name) => void open('node_app_open_help', name)} />}
        </div> : undefined}
          notices={<>{detail.runRecovery && <p className={detail.runRecovery.canStop ? 'project-help' : 'task-error'} role="status">{detail.runRecovery.message}</p>}{detail.lastError && <p className="task-error" role="alert">{detail.lastError}</p>}{statusError && <p className="task-error" role="alert">{statusError}</p>}{detail.restartRequired && <p className="node-config-warning" role="status">已保存配置与正在运行的配置不同。{detail.configuration.canStart ? '请明确重启后应用新配置。' : configurationState?.needsAttention ? '请先核对应用配置，再重启应用。' : '当前暂不能重启，请先核对运行状态。'}</p>}{detail.state === 'running' && detail.restartRequired && detail.configuration.canStart && <button className="refresh-button" disabled={blocked || loading || dirty || !detail.configuration.canStart} onClick={() => void run('node_app_restart')}>立即重启</button>}</>}
          actions={<><button className="primary-button" disabled={blocked || loading || retired || (dirty && detail.state !== 'running') || detail.state === 'starting' || detail.state === 'stopping'} onClick={() => void enterApp(detail)}><Monitor size={18} />打开</button>{canStop && <button className="refresh-button" disabled={blocked || loading} onClick={() => void run('node_app_stop')}><Square size={14} />{detail.runRecovery?.state === 'orphaned' ? '停止遗留运行' : '停止运行'}</button>}</>}
          handoff={view === 'details' ? <CodexHandoffPanel handoff={handoff} action={pending === 'copy-handoff' ? 'copy' : null} disabled={blocked || loading || retired} onPrepare={() => void prepareHandoff(true)} onShowHelp={() => void prepareHandoff(false)} onClear={() => setHandoff(null)} hint={retired ? '此安装方式已退役，请删除后重新添加。' : '复制提示词发给 AI，改完后关闭窗口再打开即可生效。'} /> : undefined}
          information={view === 'details' ? <>{detail.fields.length > 0 && <ApplicationDetailRow label="应用配置" action={<button className="text-button" disabled={blocked || loading} onClick={() => changeView('configuration')}>修改</button>}><span>{configurationState?.needsAttention || configurationState?.kind === 'unavailable' ? configurationState.overviewText : `已填写 ${detail.fields.filter(field => field.secret ? detail.configuration.secretStatus[field.name] === 'set' : Object.hasOwn(detail.configuration.values, field.name)).length} 项`}</span></ApplicationDetailRow>}<ApplicationDetailRow label="项目文件夹" action={projectPath ? <button className="text-button" disabled={!enabled || openingFolder === projectPath} onClick={() => void revealSource()}>打开</button> : undefined}><code>{projectPath ?? '源码目录暂不可用，请核对项目来源。'}</code></ApplicationDetailRow><ApplicationDetailRow label="占用空间">{bytesText(partBytes(storage ?? undefined))}{storage && (!storage.complete || storage.stale) ? ' · 部分统计或待更新' : ''}</ApplicationDetailRow>{<ApplicationDetailRow label="运行日志" action={runtimeLogPaths ? <button className="text-button" onClick={() => void navigator.clipboard?.writeText(`${runtimeLogPaths.current}\n${runtimeLogPaths.previous}`).then(() => setNotice('日志路径已复制。'), () => setNotice('请选中完整路径后复制。'))}>复制路径</button> : undefined}><code>{runtimeLogPaths?.current ?? '路径暂不可用'}</code></ApplicationDetailRow>}</> : undefined}
          more={view === 'details' ? <details className="detail-overview-body application-detail-more"><summary>更多</summary><dl className="path-list node-technical-paths"><div><dt>项目 ID</dt><dd><code>{detail.id}</code></dd></div><div><dt>数据目录</dt><dd><code>{detail.objectLayout?.data.root ?? '路径暂不可用'}</code></dd></div><div><dt>数据库</dt><dd><code>{detail.objectLayout?.data.database ?? '未声明或路径暂不可用'}</code></dd></div><div><dt>配置文件路径</dt><dd><code>{detail.objectLayout?.configurationPath ?? '路径暂不可用'}</code></dd></div></dl></details> : undefined}
          deleteAction={<button className="danger-button" disabled={blocked || loading || dirty} onClick={() => openDeletion(detail)}><Trash2 size={15} />停止并删除…</button>}
        />
        {deleteDialog}
      </section></Modal>}
    {(!modalOpen || !detail) && deleteDialog}
    {addOpen && <Modal title="添加本地项目" titleIcon={<FolderOpen size={20} />} description="选择你的文件夹，检查后添加。原文件夹保持原位。" className="import-dialog" closeDisabled={Boolean(pending) || directoryPending} onClose={() => { setAddOpen(false); setError(''); setLaunchError(''); }}>
      <LocalDirectoryPanel apps={apps} workspacePath={workspacePath} platformVersion={platformVersion} onViewDetails={app => { setAddOpen(false); void select(app); }} onImportArchive={() => { setAddOpen(false); onImportArchive?.(); }} enabled={enabled && !pending && !loading} instanceId={instanceId} onPendingChange={setDirectoryPending} onAdded={acceptDirectory} onEnter={enterDirectory} onClose={() => { setAddOpen(false); setError(''); setLaunchError(''); }} />
    </Modal>}
  </>;
}
