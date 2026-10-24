import { invoke } from '@tauri-apps/api/core';
import { Activity, ChevronRight, Copy, FolderOpen, HardDrive, Info, KeyRound, Monitor, RefreshCw, Server, Square, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import { Modal } from './Modal';
import { NodeAppDeleteDialog } from './NodeAppDeleteDialog';
import { NodeConfigurationForm } from './NodeConfigurationForm';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import { ObjectLayoutPanel } from './ObjectLayoutPanel';
import { NodeInstallPanel } from './NodeInstallPanel';
import { NodeDataContractInfo } from './NodeDataContractInfo';
import { LocalDirectoryPanel } from './LocalDirectoryPanel';
import type { SourceInstallCandidate } from './NodeInstallPanel';
import type { TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import type { NodeAppDeleteResult, NodeAppDetail, NodeConfigurationInput, NodeSecretChange } from './node-app-types';

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
export function NodeAppsPanel({ enabled, instanceId, request, addRequest = 0, onAppChanged, onAppDeleted, onModalStateChange, onDraftStateChange, onTask }: {
  enabled: boolean; request?: NodeAppRequest | null; addRequest?: number;
  instanceId?: string; onTask?: (task: TaskRecord) => void;
  onAppChanged?: (app: NodeAppDetail) => void; onAppDeleted?: (appId: string, result: NodeAppDeleteResult) => void; onModalStateChange?: (open: boolean) => void;
  onDraftStateChange: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [modalOpen, setModalOpen] = useState(false);
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
  const [directoryPending, setDirectoryPending] = useState(false);
  const [requestedSelection, setRequestedSelection] = useState<NodeAppDetail | null>(null);
  const [installSources, setInstallSources] = useState<SourceInstallCandidate[] | null>(null);
  const [installPending, setInstallPending] = useState(false);
  const [installTask, setInstallTask] = useState<TaskRecord | null>(null);
  const [installInstance, setInstallInstance] = useState(instanceId);
  const [deleteTarget, setDeleteTarget] = useState<{ app: NodeAppDetail; token: number; instanceId?: string } | null>(null);
  const [deletionPending, setDeletionPending] = useState(false);
  const deleteTargetRef = useRef(deleteTarget); deleteTargetRef.current = deleteTarget;
  const deletionSequence = useRef(0);
  const currentInstance = useRef(instanceId); currentInstance.current = instanceId;
  const sourceRead = useRef(0);
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
  const base = useId();
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  const blocked = !enabled || Boolean(pending) || installPending || directoryPending || Boolean(deleteTarget);

  function clearDeletion() {
    deleteTargetRef.current = null;
    setDeleteTarget(null); setDeletionPending(false);
  }
  const openDeletion = useCallback((app: NodeAppDetail) => {
    if (!enabled || pendingOperation.current || installPending || directoryPending || loading || operation.current || deleteTargetRef.current || (dirty && selected.current === app.id)) return;
    statusRead.current++;
    const target = { app, token: ++deletionSequence.current, instanceId };
    deleteTargetRef.current = target; setDeleteTarget(target);
  }, [enabled, installPending, directoryPending, loading, dirty, instanceId]);

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
    const selection = ++sequence.current;
    const current = () => alive.current && selected.current === app.id && sequence.current === selection && currentInstance.current === instanceId;
    selected.current = app.id;
    clearDeletion();
    setInstallSources(null); setInstallTask(null);
    setDetail(app); setDraft(draftFrom(app)); setError(''); setStatusError(''); setNotice(''); setLoading(true);
    setTab(initialTab); setDismissConfirm(false); setModalOpen(!enter);
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId: app.id });
      if (!current()) return;
      if (value.id !== app.id) throw new Error('配置返回的应用身份不符，请重新读取。');
      accept(value, true);
      if (enter) {
        if (!value.configuration.canStart) { setModalOpen(true); setTab('configuration'); }
        else await enterApp(value);
      }
    } catch (cause) { if (current()) { setError(messageFrom(cause)); setModalOpen(true); } }
    finally { if (current()) setLoading(false); }
  }, [accept, enterApp, instanceId]);

  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; selected.current = null; draftCallback.current({ dirty: false, pending: false }); }; }, []);
  useEffect(() => {
    sourceRead.current++; statusRead.current++; sequence.current++; setLoading(false);
    clearDeletion();
    setInstallSources(null); setInstallTask(null); setInstallPending(false); setInstallInstance(instanceId);
    if (pendingOperation.current === 'read-install-sources') { operation.current = false; setPending(null); }
  }, [instanceId]);
  useEffect(() => {
    if (!request || request.token === seenRequest.current) return;
    if (deleteTargetRef.current) { seenRequest.current = request.token; return; }
    if (operation.current || directoryPending) return;
    seenRequest.current = request.token;
    if (request.intent === 'delete') { openDeletion(request.app); return; }
    if (dirty && selected.current !== request.app.id) { setRequestedSelection(request.app); return; }
    void select(request.app, request.intent === 'enter' ? 'configuration' : 'overview', request.intent === 'enter');
  }, [request, select, openDeletion, pending, directoryPending]);
  useEffect(() => {
    if (!addRequest || addRequest === seenAdd.current || operation.current || directoryPending || !enabled) return;
    seenAdd.current = addRequest; setAddOpen(true); setError('');
  }, [addRequest, enabled, pending, directoryPending]);
  useEffect(() => { modalCallback.current?.(modalOpen || addOpen || Boolean(deleteTarget)); }, [modalOpen, addOpen, Boolean(deleteTarget)]);
  useEffect(() => { if (dismissConfirm || requestedSelection) confirmation.current?.focus(); }, [dismissConfirm, requestedSelection]);
  useEffect(() => { draftCallback.current({ dirty, pending: Boolean(pending) || loading || installPending || directoryPending || deletionPending }); }, [dirty, pending, loading, installPending, directoryPending, deletionPending]);
  useEffect(() => {
    if (!enabled || !detail || pending || installPending || loading || !modalOpen || deleteTarget) return;
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
  }, [enabled, detail?.id, pending, installPending, loading, modalOpen, instanceId, Boolean(deleteTarget)]);

  function acceptDirectory(value: NodeAppDetail) {
    selected.current = value.id; sequence.current++;
    accept(value, true); setAddOpen(false); setModalOpen(true); setTab('configuration'); setError('');
    setNotice('项目已添加，尚未运行。请核对配置；需要填写时保存，再明确点击“进入项目”。');
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (blocked || operation.current || !detail || loading) return;
    let input: NodeConfigurationInput;
    try { input = nodeConfigurationInput(detail, draft); }
    catch (cause) { setError(messageFrom(cause)); return; }
    operation.current = true; setPending('save'); setError(''); setNotice('');
    const appId = detail.id;
    const selection = sequence.current;
    const current = () => alive.current && selected.current === appId && sequence.current === selection && currentInstance.current === instanceId;
    statusRead.current++;
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration_save', { appId, input });
      if (!current()) return;
      if (value.id !== appId) throw new Error('保存结果的应用身份不符，请重新读取配置后核对。');
      accept(value, true);
      setNotice(value.configuration.canStart ? value.restartRequired ? '配置已保存。正在运行的应用仍使用旧配置；点击“重启并应用配置”后才生效。' : '配置已保存。下次启动使用这些设置。' : value.configuration.missingRequired.length ? '已保存，补齐标红项后才能启动。' : '配置已保存，暂时无法启动。请检查运行状态。');
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally {
      // A workspace change revokes this response, while the pending write still
      // holds the operation gate until it has finished. Never retry that write.
      operation.current = false; if (alive.current) setPending(null);
    }
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
      if (command === 'node_app_stop') setNotice(value.state === 'stopped' ? '应用已关闭，相关服务已停止。已保存的配置保留。' : '关闭结果已更新，请查看应用状态。');
      if (command === 'node_app_start' || command === 'node_app_restart') setNotice(value.state === 'running' ? '应用窗口已打开。关闭窗口会停止本应用。' : '打开结果已更新，请查看应用状态。');
    } catch (cause) { if (alive.current && selected.current === appId) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setPending(null); }
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
    sequence.current++; statusRead.current++; selected.current = null; clearDeletion(); setModalOpen(false); setDetail(null);
    setDraft({ ordinary: {}, secrets: {} }); setDismissConfirm(false); setRequestedSelection(null); setLoading(false);
  }
  function acceptDeletion(appId: string, result: NodeAppDeleteResult) {
    const target = deleteTargetRef.current;
    if (!target || target.app.id !== appId || target.instanceId !== currentInstance.current) return;
    // Revoke all observations for the deleted selection before the launcher is
    // notified. An unrelated detail/draft remains mounted behind its dialog.
    statusRead.current++;
    if (selected.current === appId) { closeDetails(); setInstallSources(null); setInstallTask(null); }
    else clearDeletion();
    deletedCallback.current?.(appId, result);
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
    clearDeletion();
    setTab(value); setDismissConfirm(false);
  }
  function tabKeys(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const values = ['overview', 'configuration', 'runtime'] as const;
    const next = event.key === 'ArrowRight' ? (index + 1) % 3 : event.key === 'ArrowLeft' ? (index + 2) % 3 : event.key === 'Home' ? 0 : event.key === 'End' ? 2 : null;
    if (next === null || blocked || loading) return;
    event.preventDefault(); changeTab(values[next]); document.getElementById(`${base}-${values[next]}-tab`)?.focus();
  }

  const deleteDialog = deleteTarget && <NodeAppDeleteDialog key={deleteTarget.token} app={deleteTarget.app} enabled={enabled && !pending && !installPending && !directoryPending && !loading} instanceId={instanceId} onClose={clearDeletion} onPendingChange={value => { if (deleteTargetRef.current?.token === deleteTarget.token) setDeletionPending(value); }} onDeleted={acceptDeletion} />;

  return <>
    {modalOpen && detail && <Modal title={detail.name} titleIcon={<Server size={22} aria-hidden="true" />} description="Node 服务 · 本机独立数据" headerAside={<span className={`detail-status-badge node-process-state node-process-${detail.state}${detail.state === 'running' ? ' is-ready' : detail.state === 'failed' ? ' is-error' : detail.state === 'stopped' ? ' is-idle' : ''}`} role="status"><span />{pending === 'node_app_start' || pending === 'node_app_restart' ? '打开中…' : pending === 'node_app_stop' ? '关闭中…' : stateLabels[detail.state]}</span>} className="software-detail-modal node-detail-modal" wide closeDisabled={Boolean(pending) || installPending || deletionPending} beforeClose={() => { if (dirty) { setDismissConfirm(true); return false; } return true; }} onClose={closeDetails}>
      <section className="node-app-detail node-app-modal-detail" aria-label={`${detail.name}项目详情`} aria-busy={Boolean(pending) || loading}>
        {dismissConfirm && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="关闭配置确认"><strong>有未保存的应用配置</strong><p>关闭会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="primary-button" onClick={closeDetails}>丢弃草稿并关闭</button></div></div>}
        {requestedSelection && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="重新读取配置确认"><strong>当前配置有未保存的修改</strong><p>继续会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setRequestedSelection(null)}>继续编辑</button><button className="primary-button" onClick={() => { const value = requestedSelection; setRequestedSelection(null); void select(value, 'configuration'); }}>丢弃草稿并重新读取</button></div></div>}
        {error && <p className="task-error node-inline-message" role="alert">{error}</p>}
        {notice && <p className="workspace-inline-notice node-inline-message" role="status">{notice}</p>}
        <div className="detail-actions">
          <div className="detail-action-row">
            <button className="primary-button" disabled={blocked || loading || (dirty && detail.state !== 'running') || detail.state === 'starting' || detail.state === 'stopping'} onClick={() => void enterApp(detail)}><Monitor size={18} />进入项目</button>
            <button className="refresh-button" disabled={blocked || loading} onClick={() => changeTab('configuration')}><KeyRound size={18} />修改配置</button>
            <button className="text-button detail-delete-button" disabled={blocked || loading || dirty} onClick={() => openDeletion(detail)}><Trash2 size={15} />删除项目</button>
          </div>
          <p className="detail-action-note">{dirty ? '有未保存的应用配置。删除前请先保存或丢弃。' : '应用在独立窗口中运行；关闭窗口即停止服务，再次进入会回到已有窗口。'}</p>
        </div>
        {detail.lastError && <p className="task-error node-inline-message" role="alert">{detail.lastError}</p>}
        {statusError && <p className="task-error node-inline-message" role="alert">{statusError}</p>}
        {detail.restartRequired && <p className="node-config-warning" role="status">已保存配置与正在运行的配置不同。{detail.configuration.canStart ? '请明确重启后应用新配置。' : '先补齐必填项，再重启应用。'}</p>}
        <div className="overview-tabs" role="tablist" aria-label="Node 项目详情">{([['overview', '概览', Info], ['configuration', '应用配置', KeyRound], ['runtime', '运行状态', Activity]] as const).map(([value, label, Icon], index) => <button key={value} id={`${base}-${value}-tab`} role="tab" type="button" aria-selected={tab === value} aria-controls={`${base}-${value}-panel`} tabIndex={tab === value ? 0 : -1} disabled={blocked || loading} onClick={() => changeTab(value)} onKeyDown={(event) => tabKeys(event, index)}><Icon size={14} />{label}</button>)}</div>
        <div className="detail-overview-body node-overview-body" role="tabpanel" id={`${base}-overview-panel`} aria-labelledby={`${base}-overview-tab`} hidden={tab !== 'overview'}>
          <dl className="detail-summary-facts"><div><dt>运行状态</dt><dd className={detail.state === 'running' ? 'detail-ready-text' : detail.state === 'failed' ? 'detail-error-text' : ''}>{stateLabels[detail.state]}{detail.restartRequired ? ' · 待重启' : ''}</dd></div><div><dt>应用配置</dt><dd className={detail.configuration.canStart ? '' : 'detail-warning-text'}>{detail.configuration.canStart ? `已完整${detail.fields.length ? `（${detail.fields.length} 项）` : ''}` : `缺少 ${detail.configuration.missingRequired.length} 项必填`}</dd></div><div><dt>程序来源</dt><dd>{detail.installation?.kind === 'managed' ? '平台安装副本' : '本机应用目录'}</dd></div></dl>
          <section className="detail-usage-section"><h3>配置与数据</h3>
            <button className="detail-document-row" disabled={blocked || loading} onClick={() => changeTab('configuration')}><KeyRound size={24} /><span><strong>{detail.configuration.canStart ? '查看或修改应用配置' : '补齐应用配置'}</strong><small>{detail.fields.length ? `${detail.fields.slice(0, 3).map(field => field.label).join('、')}${detail.fields.length > 3 ? ` 等 ${detail.fields.length} 项` : ''}。` : '此应用未声明配置项。'}保存后不会自动打开或重启应用。</small></span><ChevronRight size={18} /></button>
            <div className="detail-data-row"><HardDrive size={24} /><span><strong>数据保存在本机</strong><small>业务数据、配置与密钥保存在本应用实例中，普通更新不会替换。</small></span></div>
          </section>
          <section className="detail-location-section"><h3>应用位置</h3><div className="detail-location-row"><FolderOpen size={22} /><span><strong>{detail.installation?.kind === 'managed' ? '安装副本来源目录' : '本机应用目录'}</strong><code>{detail.sourcePath}</code></span><button className="icon-button" aria-label="复制应用目录" onClick={() => { void navigator.clipboard?.writeText(detail.sourcePath).then(() => setNotice('路径已复制。'), () => setNotice('请选中完整路径后复制。')); }}><Copy size={20} /></button></div></section>
          {detail.installation?.kind === 'managed' && <section className="detail-update-section"><h3>应用更新</h3><p className="detail-section-note">先在“获取项目源码”中准备新的来源副本，再在这里选择更新。配置有草稿时请先保存或处理；应用更新前需关闭窗口并等待服务停止。</p><button className="refresh-button" disabled={blocked || loading || dirty} onClick={() => void prepareUpdate()}>选择更新来源或查看候选</button>{installSources && installInstance === instanceId && <NodeInstallPanel sources={installSources} targetAppId={detail.id} instanceId={instanceId} task={installTask} disabled={!enabled || Boolean(pending) || loading || dirty} onPendingChange={setInstallPending} onTask={value => { setInstallTask(value); onTask?.(value); }} onInstalled={() => void refreshInstalled(detail.id)} />}</section>}
          <details className="detail-technical-information"><summary>详细运行信息</summary><NodeDataContractInfo value={detail.dataContract} unavailable={detail.state === 'failed' && detail.dataContract == null} legacyNotice={detail.installation?.kind === 'managed' ? '后续普通更新无法核对数据契约，将被阻止；现有应用仍可使用。' : '平台未验证实际数据格式。'} /><p>{detail.installation?.kind === 'managed' ? '当前程序是平台的独立安装副本，运行不依赖原作者目录。数据和配置保留在本应用实例中；更新需要重新准备候选，不会自动启动应用。' : '此入口登记本机应用目录，使用平台内置 Node。运行仍依赖该目录，尚未核实可编辑源码与构建方式。版本能力将采用项目标准 Git；旧备份与未完成现场继续保留。'}</p><code className="overview-project-id">{detail.id}</code><ObjectLayoutPanel layout={detail.objectLayout} /></details>
        </div>
        <div className="detail-overview-body" role="tabpanel" id={`${base}-runtime-panel`} aria-labelledby={`${base}-runtime-tab`} hidden={tab !== 'runtime'}><h3>运行状态</h3><p className="detail-section-note">当前{stateLabels[detail.state]}。关闭应用窗口会停止本应用；关闭此详情弹窗不会关闭应用窗口。</p><div className="node-runtime-actions"><button className="refresh-button" disabled={blocked || loading || !['running', 'starting', 'failed'].includes(detail.state)} onClick={() => void run('node_app_stop')}><Square size={14} />关闭应用</button><button className="refresh-button" disabled={blocked || loading || dirty || !detail.configuration.canStart || detail.state !== 'running'} onClick={() => void run('node_app_restart')}><RefreshCw size={14} />重启并应用配置</button><button className="refresh-button" disabled={blocked || loading || dirty} onClick={() => void run('node_app_status')}>刷新运行状态</button></div></div>
        <div className="detail-overview-body node-configuration-body" role="tabpanel" id={`${base}-configuration-panel`} aria-labelledby={`${base}-configuration-tab`} hidden={tab !== 'configuration'}>
        <NodeConfigurationForm key={`${instanceId ?? ''}:${detail.id}:${configurationRead}`} app={detail} draft={draft} active={tab === 'configuration' && !deleteTarget} disabled={blocked || loading} dirty={dirty} saving={pending === 'save'} onOrdinaryChange={ordinaryChange} onSecretChange={secretChange} onSave={(event) => void save(event)} onReload={() => { if (dirty) setRequestedSelection(detail); else void select(detail, 'configuration'); }} onOpenHelp={(name) => void open('node_app_open_help', name)} />
        </div>
        {deleteDialog}
      </section></Modal>}
    {(!modalOpen || !detail) && deleteDialog}
    {addOpen && <Modal title="添加本地应用" description="选择本地项目文件夹。空目录可生成并添加，已有项目先检查预览。" closeDisabled={Boolean(pending) || directoryPending} onClose={() => { setAddOpen(false); setError(''); }}>
      <LocalDirectoryPanel enabled={enabled && !pending && !installPending && !loading} instanceId={instanceId} onPendingChange={setDirectoryPending} onAdded={acceptDirectory} />
    </Modal>}
  </>;
}
