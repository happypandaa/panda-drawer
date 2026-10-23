import { invoke } from '@tauri-apps/api/core';
import { Activity, FolderOpen, Info, KeyRound, Monitor, RefreshCw, Server, Square, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import { Modal } from './Modal';
import { NodeConfigurationForm } from './NodeConfigurationForm';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { DeletionNotice } from './DeletionNotice';
import { NodeInstallPanel } from './NodeInstallPanel';
import type { SourceInstallCandidate } from './NodeInstallPanel';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import type { NodeAppDetail, NodeAppInspection, NodeConfigurationInput, NodeSecretChange } from './node-app-types';

type Draft = NodeConfigurationDraft;
type SecretDraft = Draft['secrets'][string];
const stateLabels: Record<NodeAppDetail['state'], string> = { stopped: '已停止', starting: '启动中', running: '运行中', stopping: '停止中', failed: '运行异常' };

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
  return { revision: app.configuration.revision, values, secrets };
}

export interface NodeAppRequest { app: NodeAppDetail; intent: 'enter' | 'details' | 'delete'; token: number }
interface DeletePreview { appId: string; name: string; path: string; externalSourcePath?: string | null; canDelete: boolean; blockedReasons?: string[]; previewId: string; git: { state: string; remoteSync: string } }

export function NodeAppsPanel({ enabled, instanceId, request, addRequest = 0, onAppChanged, onAppDeleted, onModalStateChange, onDraftStateChange, onTask }: {
  enabled: boolean; request?: NodeAppRequest | null; addRequest?: number;
  instanceId?: string; onTask?: (task: TaskRecord) => void;
  onAppChanged?: (app: NodeAppDetail) => void; onAppDeleted?: (appId: string) => void; onModalStateChange?: (open: boolean) => void;
  onDraftStateChange: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [modalOpen, setModalOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deletePreview, setDeletePreview] = useState<DeletePreview | null>(null);
  const [deleteAcknowledged, setDeleteAcknowledged] = useState(false);
  const deletedCallback = useRef(onAppDeleted); deletedCallback.current = onAppDeleted;
  const [tab, setTab] = useState<'overview' | 'configuration' | 'runtime'>('overview');
  const [dismissConfirm, setDismissConfirm] = useState(false);
  const [detail, setDetail] = useState<NodeAppDetail | null>(null);
  const [draft, setDraft] = useState<Draft>({ ordinary: {}, secrets: {} });
  const [configurationRead, setConfigurationRead] = useState(0);
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [statusError, setStatusError] = useState('');
  const [notice, setNotice] = useState('');
  const [addOpen, setAddOpen] = useState(false);
  const [sourcePath, setSourcePath] = useState('');
  const [inspection, setInspection] = useState<NodeAppInspection | null>(null);
  const [requestedSelection, setRequestedSelection] = useState<NodeAppDetail | null>(null);
  const [installSources, setInstallSources] = useState<SourceInstallCandidate[] | null>(null);
  const [installPending, setInstallPending] = useState(false);
  const [installTask, setInstallTask] = useState<TaskRecord | null>(null);
  const [installInstance, setInstallInstance] = useState(instanceId);
  const [updateOpen, setUpdateOpen] = useState(false);
  const currentInstance = useRef(instanceId); currentInstance.current = instanceId;
  const deleteRead = useRef(0);
  const sourceRead = useRef(0);
  const statusRead = useRef(0);
  const revealRead = useRef(0);
  const pendingOperation = useRef(pending); pendingOperation.current = pending;
  const seenRequest = useRef<number | null>(null);
  const seenAdd = useRef(0);
  const changedCallback = useRef(onAppChanged); changedCallback.current = onAppChanged;
  const modalCallback = useRef(onModalStateChange); modalCallback.current = onModalStateChange;
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const selected = useRef<string | null>(null);
  const confirmation = useRef<HTMLDivElement>(null);
  const deletionPanel = useRef<HTMLElement>(null);
  const deleteEntry = useRef<HTMLButtonElement>(null);
  const previousDeleteOpen = useRef(false);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const base = useId();
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  const blocked = !enabled || Boolean(pending) || installPending;

  const accept = useCallback((value: NodeAppDetail, resetDraft: boolean) => {
    changedCallback.current?.(value);
    if (selected.current === value.id) {
      setDetail((current) => resetDraft || !current ? value : { ...current, state: value.state, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError });
      if (resetDraft) { setDraft(draftFrom(value)); setConfigurationRead((current) => current + 1); }
    }
  }, []);

  const enterApp = useCallback(async (value: NodeAppDetail) => {
    if (operation.current || !enabled) return;
    if (!value.configuration.canStart) { setModalOpen(true); setTab('configuration'); return; }
    operation.current = true; setPending('enter'); setError(''); setNotice('');
    const appId = value.id;
    try {
      if (value.state === 'running') await invoke('node_app_open', { appId });
      else {
        const opened = await invoke<NodeAppDetail>('node_app_start', { appId });
        if (alive.current && selected.current === appId) accept(opened, false);
      }
    } catch (cause) {
      if (alive.current && selected.current === appId) { setError(messageFrom(cause)); setModalOpen(true); setTab('runtime'); }
    } finally { operation.current = false; if (alive.current) setPending(null); }
  }, [accept, enabled]);

  const select = useCallback(async (app: NodeAppDetail, initialTab: 'overview' | 'configuration' | 'runtime' = 'overview', enter = false) => {
    const current = ++sequence.current;
    selected.current = app.id;
    setInstallSources(null); setInstallTask(null); setUpdateOpen(false); setDeleteOpen(false); setDeletePreview(null); setDeleteAcknowledged(false);
    setDetail(app); setDraft(draftFrom(app)); setError(''); setStatusError(''); setNotice(''); setLoading(true);
    setTab(initialTab); setDismissConfirm(false); setModalOpen(!enter);
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId: app.id });
      if (!alive.current || sequence.current !== current) return;
      accept(value, true);
      if (enter) {
        if (!value.configuration.canStart) { setModalOpen(true); setTab('configuration'); }
        else await enterApp(value);
      }
    } catch (cause) { if (alive.current && sequence.current === current) { setError(messageFrom(cause)); setModalOpen(true); } }
    finally { if (alive.current && sequence.current === current) setLoading(false); }
  }, [accept, enterApp]);

  const previewDeletion = useCallback(async (app: NodeAppDetail) => {
    if (operation.current || !enabled) return;
    operation.current = true; setPending('delete-preview'); setError(''); setNotice('');
    const current = ++sequence.current; const read = ++deleteRead.current; const originInstance = currentInstance.current;
    if (selected.current !== app.id) { setDetail(app); setDraft(draftFrom(app)); }
    selected.current = app.id;
    setModalOpen(true); setDeleteOpen(true); setDeletePreview(null); setDeleteAcknowledged(false);
    setInstallSources(null); setInstallTask(null); setDismissConfirm(false); setLoading(false);
    try {
      const preview = await invoke<DeletePreview>('node_app_delete_preview', { appId: app.id });
      if (alive.current && sequence.current === current && selected.current === app.id && deleteRead.current === read && currentInstance.current === originInstance) {
        if (preview.appId !== app.id || !preview.path || !preview.previewId) throw new Error('删除范围与当前应用不一致，请重新核对。');
        setDeletePreview(preview);
      }
    } catch (cause) { if (alive.current && sequence.current === current && deleteRead.current === read && currentInstance.current === originInstance) setError(messageFrom(cause)); }
    finally { if (deleteRead.current === read) { operation.current = false; if (alive.current && sequence.current === current) setPending(null); } }
  }, [enabled]);

  async function deleteApp() {
    if (blocked || operation.current || !detail || !deletePreview || !deletePreview.canDelete || !deleteAcknowledged || deletePreview.appId !== detail.id) return;
    operation.current = true; setPending('delete'); setError('');
    const appId = detail.id; const originInstance = currentInstance.current;
    try {
      const result = await invoke<{ appId: string; operationOutcome: string }>('node_app_delete', { appId, input: { previewId: deletePreview.previewId } });
      if (!alive.current || selected.current !== appId || currentInstance.current !== originInstance) return;
      if (result.appId !== appId || result.operationOutcome !== 'system_trashed') throw new Error('删除结果尚未确认，请保留当前记录并重新核对，勿重复提交。');
      closeDetails(); deletedCallback.current?.(appId);
    } catch (cause) { if (alive.current && selected.current === appId) { setError(messageFrom(cause)); setDeletePreview(null); setDeleteAcknowledged(false); } }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; selected.current = null; draftCallback.current({ dirty: false, pending: false }); }; }, []);
  useEffect(() => {
    sourceRead.current++; statusRead.current++; deleteRead.current++; revealRead.current++;
    setDeletePreview(null); setDeleteAcknowledged(false);
    if (deleteOpen) setError('平台连接已更新，请重新核对删除范围。');
    if (pendingOperation.current === 'delete-preview') { operation.current = false; setPending(null); }
    setInstallSources(null); setInstallTask(null); setInstallPending(false); setInstallInstance(instanceId);
    if (['read-install-sources', 'node_app_reveal'].includes(pendingOperation.current ?? '')) { operation.current = false; setPending(null); }
  }, [instanceId]);
  useEffect(() => {
    if (!request || request.token === seenRequest.current || operation.current) return;
    seenRequest.current = request.token;
    if (dirty && selected.current !== request.app.id) { setRequestedSelection(request.app); return; }
    if (request.intent === 'delete') { void previewDeletion(request.app); return; }
    void select(request.app, request.intent === 'enter' ? 'configuration' : 'overview', request.intent === 'enter');
  }, [request, select, previewDeletion, pending]);
  useEffect(() => {
    if (!addRequest || addRequest === seenAdd.current || operation.current || !enabled) return;
    seenAdd.current = addRequest; setAddOpen(true); setError(''); setSourcePath(''); setInspection(null);
  }, [addRequest, enabled, pending]);
  useEffect(() => { modalCallback.current?.(modalOpen || addOpen); }, [modalOpen, addOpen]);
  useEffect(() => { if (dismissConfirm || requestedSelection) confirmation.current?.focus(); }, [dismissConfirm, requestedSelection]);
  useEffect(() => {
    if (deleteOpen) deletionPanel.current?.focus({ preventScroll: true });
    else if (previousDeleteOpen.current && modalOpen) deleteEntry.current?.focus({ preventScroll: true });
    previousDeleteOpen.current = deleteOpen;
  }, [deleteOpen, modalOpen]);
  useEffect(() => { draftCallback.current({ dirty, pending: Boolean(pending) || loading || installPending }); }, [dirty, pending, loading, installPending]);
  useEffect(() => {
    if (!enabled || !detail || pending || installPending || loading || !modalOpen || deleteOpen) return;
    let cancelled = false; let polling = false;
    const appId = detail.id;
    const selection = sequence.current;
    const timer = window.setInterval(() => {
      if (polling || operation.current) return;
      polling = true;
      const revision = statusRead.current;
      const current = () => !cancelled && alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId && statusRead.current === revision;
      void invoke<NodeAppDetail>('node_app_status', { appId }).then((value) => {
        if (current() && !operation.current) {
          // Status refresh never replaces configuration declarations, the saved
          // revision used by a draft, or any password text the user is editing.
          setDetail((current) => current?.id === appId ? { ...current, state: value.state, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError } : current);
          changedCallback.current?.(value);
          setStatusError('');
        }
      }).catch(() => { if (current()) setStatusError('运行状态暂时无法更新。请点击“刷新运行状态”核对，应用不会因此自动重启。'); }).finally(() => { polling = false; });
    }, 1500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [enabled, detail?.id, pending, installPending, loading, modalOpen, deleteOpen, instanceId]);

  async function chooseSource() {
    if (blocked || operation.current) return;
    operation.current = true; setPending('choose'); setError(''); setInspection(null);
    try {
      const value = await invoke<string | null>('choose_path', { kind: 'nodeapp' });
      if (!alive.current || !value) return;
      setSourcePath(value);
      const preview = await invoke<NodeAppInspection>('node_app_inspect', { sourcePath: value });
      if (alive.current) setInspection(preview);
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  async function register() {
    if (blocked || operation.current || !inspection || !sourcePath) return;
    operation.current = true; setPending('register'); setError('');
    try {
      const value = await invoke<NodeAppDetail>('node_app_register', { sourcePath });
      if (!alive.current) return;
      selected.current = value.id; sequence.current++;
      accept(value, true); setAddOpen(false); setModalOpen(true); setTab('configuration'); setSourcePath(''); setInspection(null);
      setNotice('应用已登记。请先填写配置并保存，再点击“进入项目”。');
    } catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (blocked || operation.current || !detail || loading) return;
    let input: NodeConfigurationInput;
    try { input = nodeConfigurationInput(detail, draft); }
    catch (cause) { setError(messageFrom(cause)); return; }
    operation.current = true; setPending('save'); setError(''); setNotice('');
    const appId = detail.id;
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration_save', { appId, input });
      if (!alive.current || selected.current !== appId) return;
      accept(value, true);
      setNotice(value.configuration.canStart ? value.restartRequired ? '配置已保存。正在运行的应用仍使用旧配置；点击“重启并应用配置”后才生效。' : '配置已保存。下次启动使用这些设置。' : value.configuration.missingRequired.length ? '已保存，补齐标红项后才能启动。' : '配置已保存，暂时无法启动。请检查运行状态。');
    } catch (cause) { if (alive.current && selected.current === appId) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  async function run(command: 'node_app_start' | 'node_app_stop' | 'node_app_restart' | 'node_app_status') {
    if (blocked || operation.current || !detail || loading || (dirty && command !== 'node_app_stop')) return;
    operation.current = true; setPending(command); setError(''); setNotice('');
    const appId = detail.id;
    try {
      const value = await invoke<NodeAppDetail>(command, { appId });
      if (!alive.current || selected.current !== appId) return;
      accept(value, !dirty);
      setStatusError('');
    } catch (cause) { if (alive.current && selected.current === appId) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  async function revealLocalDirectory() {
    if (blocked || operation.current || !detail || loading) return;
    operation.current = true; setPending('node_app_reveal'); setError('');
    const appId = detail.id; const selection = sequence.current; const originInstance = currentInstance.current;
    const revision = ++revealRead.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === originInstance && revealRead.current === revision;
    try { await invoke('node_app_reveal', { appId }); }
    catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally {
      if (revealRead.current === revision) { operation.current = false; if (current()) setPending(null); }
    }
  }

  async function open(command: 'node_app_open' | 'node_app_open_help', fieldName?: string) {
    if (blocked || operation.current || !detail) return;
    operation.current = true; setPending(command); setError('');
    try { await invoke(command, { appId: detail.id, ...(fieldName ? { fieldName } : {}) }); }
    catch (cause) { if (alive.current) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
  }

  function ordinaryChange(name: string, value: string) {
    setDraft((current) => ({ ...current, ordinary: { ...current.ordinary, [name]: value }, secrets: Object.fromEntries(Object.entries(current.secrets).map(([key, edit]) => [key, detail?.fields.find((field) => field.name === key)?.endpointField === name && edit.action === 'rebind' ? { action: 'keep', value: '' } : edit])) }));
    setNotice('');
  }
  function secretChange(name: string, edit: SecretDraft) { setDraft((current) => ({ ...current, secrets: { ...current.secrets, [name]: edit } })); setNotice(''); }
  function closeDetails() {
    sequence.current++; selected.current = null; setModalOpen(false); setDetail(null);
    setDeleteOpen(false); setDeletePreview(null); setDeleteAcknowledged(false);
    setDraft({ ordinary: {}, secrets: {} }); setDismissConfirm(false); setRequestedSelection(null); setLoading(false);
  }
  async function prepareUpdate() {
    if (blocked || loading || dirty || !detail?.installation || operation.current) return;
    operation.current = true; setPending('read-install-sources'); setError('');
    const appId = detail.id;
    const selection = sequence.current; const request = ++sourceRead.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId && sourceRead.current === request;
    try {
      const sources = await invoke<SourceInstallCandidate[]>('source_list');
      if (current()) { setInstallSources(sources); setInstallInstance(instanceId); }
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally { if (current()) { operation.current = false; setPending(null); } }
  }
  async function refreshInstalled(appId: string) {
    const selection = sequence.current; const revision = ++statusRead.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId && statusRead.current === revision;
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId });
      if (!current()) return;
      statusRead.current++;
      setDetail(value); changedCallback.current?.(value);
      // Keep the installation task and its committed-but-incomplete result visible.
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
  }
  function changeTab(value: typeof tab) {
    if (blocked || loading) return;
    setTab(value); setDismissConfirm(false);
  }
  function tabKeys(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const values = ['overview', 'configuration', 'runtime'] as const;
    const next = event.key === 'ArrowRight' ? (index + 1) % 3 : event.key === 'ArrowLeft' ? (index + 2) % 3 : event.key === 'Home' ? 0 : event.key === 'End' ? 2 : null;
    if (next === null || blocked || loading) return;
    event.preventDefault(); changeTab(values[next]); document.getElementById(`${base}-${values[next]}-tab`)?.focus();
  }

  const updateRequired = installSources !== null || Boolean(installTask) || installPending || pending === 'read-install-sources';

  return <>
    {modalOpen && detail && <Modal title={deleteOpen ? '删除项目？' : detail.name} wide={!deleteOpen} className={deleteOpen ? 'node-delete-modal' : 'software-detail-modal'} closeDisabled={Boolean(pending) || installPending} beforeClose={() => { if (dirty) { setDismissConfirm(true); return false; } return true; }} onClose={closeDetails}>
      <section className="node-app-detail node-app-modal-detail" aria-label={`${detail.name}项目详情`} aria-busy={Boolean(pending) || loading}>
        {dismissConfirm && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="关闭配置确认"><strong>有未保存的应用配置</strong><p>关闭会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="primary-button" onClick={closeDetails}>丢弃草稿并关闭</button></div></div>}
        {requestedSelection && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="重新读取配置确认"><strong>当前配置有未保存的修改</strong><p>继续会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setRequestedSelection(null)}>继续编辑</button><button className="primary-button" onClick={() => { const value = requestedSelection; setRequestedSelection(null); void select(value, 'configuration'); }}>丢弃草稿并重新读取</button></div></div>}
        {error && <p className="task-error node-inline-message" role="alert">{error}</p>}
        {notice && <p className="workspace-inline-notice node-inline-message" role="status">{notice}</p>}
        <div hidden={deleteOpen}>
        <header className="node-app-heading"><span className="node-runtime-label"><Server size={14} />Node 服务</span><span className={`node-process-state node-process-${detail.state}`} role="status">{pending === 'node_app_start' || pending === 'node_app_restart' ? '打开中…' : pending === 'node_app_stop' ? '关闭中…' : stateLabels[detail.state]}</span></header>
        <div className="node-runtime-actions detail-secondary-actions">
          <button className="primary-button" disabled={blocked || loading || deleteOpen || (dirty && detail.state !== 'running') || detail.state === 'starting' || detail.state === 'stopping'} onClick={() => void enterApp(detail)}><Monitor size={15} />进入项目</button>
          <button className="refresh-button" disabled={blocked || loading} onClick={() => void revealLocalDirectory()}><FolderOpen size={15} />打开本地目录</button>
          {tab === 'overview' && ['running', 'starting', 'failed'].includes(detail.state) && <button className="refresh-button" disabled={blocked || loading} onClick={() => void run('node_app_stop')}><Square size={14} />关闭应用</button>}
          <button ref={deleteEntry} className="text-button detail-delete-action" disabled={blocked || loading} onClick={() => void previewDeletion(detail)}><Trash2 size={15} />删除项目</button>
        </div>
        {detail.lastError && <p className="task-error node-inline-message" role="alert">{detail.lastError}</p>}
        {statusError && <p className="task-error node-inline-message" role="alert">{statusError}</p>}
        {detail.restartRequired && <p className="node-config-warning" role="status">已保存配置与正在运行的配置不同。{detail.configuration.canStart ? '请明确重启后应用新配置。' : '先补齐必填项，再重启应用。'}</p>}
        </div>
        {deleteOpen && <section ref={deletionPanel} className="project-delete-confirmation compact-delete-confirmation" tabIndex={-1} role="group" aria-label="删除 Node 应用确认">
          <div className="project-delete-target"><span className="project-delete-icon" aria-hidden="true"><Trash2 size={22} /></span><strong>{detail.name}</strong></div>
          <p className="project-delete-summary">整个应用及其本机数据将移入系统废纸篓。</p>
          <DeletionNotice />
          {deletePreview ? <details className="project-delete-details"><summary>查看删除范围</summary><div>
            <p>包括程序、依赖、数据、附件、配置与密钥、缓存和本机 Git。</p><code className="project-delete-path">{deletePreview.path}</code>
            {deletePreview.externalSourcePath && <p>外部源码保留：<code>{deletePreview.externalSourcePath}</code></p>}<p>独立来源副本、安装材料和目录外的历史资料保留。</p>
            <p>数据库、附件和配置无法从源码版本恢复。找回文件请使用 Finder 废纸篓，再核对登记状态；应用不会自动启动。</p>
          </div></details> : <p role="status" className="project-delete-read-status">{pending === 'delete-preview' ? '正在核对…' : '请重新核对删除范围。'}</p>}
          {dirty && <p className="project-delete-draft">尚有未保存的配置，删除不会保存草稿。</p>}
          {deletePreview?.blockedReasons?.map((reason) => <p className="task-error" key={reason}>{reason}</p>)}
          {!blocked && !deletePreview?.canDelete && <button className="text-button" onClick={() => void previewDeletion(detail)}>重新核对删除范围</button>}
          <label className="checkbox-label"><input type="checkbox" checked={deleteAcknowledged} disabled={blocked || !deletePreview?.canDelete} onChange={(event) => setDeleteAcknowledged(event.target.checked)} />我已确认删除范围及未同步风险</label>
          <div className="confirmation-actions"><button className="refresh-button" disabled={blocked} onClick={() => { setDeleteOpen(false); setDeletePreview(null); setDeleteAcknowledged(false); }}>取消</button><button className="danger-button" disabled={blocked || !deletePreview?.canDelete || !deleteAcknowledged} onClick={() => void deleteApp()}>{pending === 'delete' ? '正在移入…' : '移入废纸篓'}</button></div>
        </section>}
        <div hidden={deleteOpen}>
        <div className="overview-tabs" role="tablist" aria-label="Node 项目详情">{([['overview', '概览', Info], ['configuration', '应用配置', KeyRound], ['runtime', '运行状态', Activity]] as const).map(([value, label, Icon], index) => <button key={value} id={`${base}-${value}-tab`} role="tab" type="button" aria-selected={tab === value} aria-controls={`${base}-${value}-panel`} tabIndex={tab === value ? 0 : -1} disabled={blocked || loading} onClick={() => changeTab(value)} onKeyDown={(event) => tabKeys(event, index)}><Icon size={14} />{label}</button>)}</div>
        <div role="tabpanel" id={`${base}-overview-panel`} aria-labelledby={`${base}-overview-tab`} hidden={tab !== 'overview'}>
          <div className="detail-summary-facts">
            <div><span>应用配置</span><strong>{detail.configuration.canStart ? '配置完整' : '待配置'}</strong><button className="text-button" disabled={blocked || loading} onClick={() => changeTab('configuration')}>{detail.configuration.canStart ? '查看配置' : '填写配置'}</button></div>
            <div><span>安装方式</span><strong>{detail.installation?.kind === 'managed' ? '受管安装' : '依赖来源目录'}</strong><p>{detail.installation?.kind === 'managed' ? '程序、数据与配置保存在本机。' : '运行依赖原目录，请勿移动或删除。'}</p></div>
          </div>
          {detail.installation?.kind === 'managed' && <details className="detail-disclosure node-update-details" open={updateOpen || updateRequired} onToggle={(event) => { if (updateRequired && !event.currentTarget.open) event.currentTarget.open = true; else setUpdateOpen(event.currentTarget.open); }}>
            <summary onClick={(event) => { if (updateRequired) event.preventDefault(); }}>应用更新</summary>
            <div><p>选择新的来源副本，核对后更新应用。</p><button className="refresh-button" disabled={blocked || loading || dirty} onClick={() => void prepareUpdate()}>选择更新来源或查看候选</button>
              {installSources && installInstance === instanceId && <NodeInstallPanel sources={installSources} targetAppId={detail.id} instanceId={instanceId} task={installTask} disabled={!enabled || Boolean(pending) || loading || dirty} onPendingChange={setInstallPending} onTask={value => { setInstallTask(value); onTask?.(value); }} onInstalled={() => void refreshInstalled(detail.id)} />}
            </div>
          </details>}
          <details className="detail-disclosure node-source-details"><summary>详细信息</summary><div>
            <h3>来源目录</h3><code>{detail.sourcePath}</code>
            <p>{detail.installation?.kind === 'managed' ? '当前程序是独立安装副本，运行不依赖原作者目录。' : '此项目运行依赖上方来源目录，尚未核实可编辑源码与构建方式。'}</p>
            <p>“打开本地目录”显示本机实例的程序、数据和配置目录。</p>
            <dl><div><dt>项目 ID</dt><dd><code>{detail.id}</code></dd></div></dl>
            <p>此类应用暂不支持 Git 版本管理。</p>
            <ObjectLayoutPanel layout={detail.objectLayout} />
          </div></details>
        </div>
        <div role="tabpanel" id={`${base}-runtime-panel`} aria-labelledby={`${base}-runtime-tab`} hidden={tab !== 'runtime'}><h3>运行状态</h3><p>{stateLabels[detail.state]}。关闭应用窗口会停止本应用。</p><div className="node-runtime-actions">{tab === 'runtime' && <button className="refresh-button" disabled={blocked || loading || !['running', 'starting', 'failed'].includes(detail.state)} onClick={() => void run('node_app_stop')}><Square size={14} />关闭应用</button>}<button className="refresh-button" disabled={blocked || loading || dirty || !detail.configuration.canStart || detail.state !== 'running'} onClick={() => void run('node_app_restart')}><RefreshCw size={14} />重启并应用配置</button><button className="refresh-button" disabled={blocked || loading || dirty} onClick={() => void run('node_app_status')}>刷新运行状态</button></div></div>
        <div role="tabpanel" id={`${base}-configuration-panel`} aria-labelledby={`${base}-configuration-tab`} hidden={tab !== 'configuration'}>
        <NodeConfigurationForm key={`${detail.id}:${configurationRead}`} app={detail} draft={draft} active={tab === 'configuration' && !deleteOpen} disabled={blocked || loading} dirty={dirty} saving={pending === 'save'} onOrdinaryChange={ordinaryChange} onSecretChange={secretChange} onSave={(event) => void save(event)} onReload={() => { if (dirty) setRequestedSelection(detail); else void select(detail, 'configuration'); }} onOpenHelp={(name) => void open('node_app_open_help', name)} />
        </div>
        </div>
      </section></Modal>}
    {addOpen && <Modal title="添加本地应用" description="选择应用目录，只读核对应用声明后再登记。" closeDisabled={Boolean(pending)} onClose={() => { setAddOpen(false); setSourcePath(''); setInspection(null); setError(''); }}>
      <div className="node-add-dialog"><p>应用目录中需要有平台可识别的 <code>local-app.json</code>。登记后先配置，再打开应用。</p><button className="refresh-button" disabled={blocked} onClick={() => void chooseSource()}><FolderOpen size={16} />{pending === 'choose' ? '正在核对…' : '选择应用目录'}</button>{sourcePath && <code className="node-selected-source">{sourcePath}</code>}{error && <p role="alert" className="task-error">{error}</p>}{inspection && <div className="node-inspection-preview"><h3>{inspection.app.name}</h3><p>{inspection.app.description}</p><span className="node-runtime-label"><Server size={14} />{inspection.runtimeLabel} · {inspection.app.version}</span><p>应用声明了 {inspection.configuration.fields.length} 项配置，其中 {inspection.configuration.fields.filter((field) => field.required).length} 项必填。登记不读取旧的 .env，也不会自动运行应用。</p><ul>{inspection.configuration.fields.map((field) => <li key={field.name}>{field.label}{field.required ? '（必填）' : '（选填）'}{field.secret ? ' · 密钥' : ''}</li>)}</ul><button className="primary-button" disabled={blocked || inspection.runtime !== 'node-service'} onClick={() => void register()}>{pending === 'register' ? '登记中…' : '登记此应用'}</button></div>}</div>
    </Modal>}
  </>;
}
