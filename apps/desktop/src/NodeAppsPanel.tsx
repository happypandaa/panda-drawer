import { invoke } from '@tauri-apps/api/core';
import { Activity, CircleAlert, ExternalLink, FolderOpen, Info, KeyRound, Monitor, RefreshCw, Save, Server, Square } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import { Modal } from './Modal';
import { messageFrom } from './ui-utils';
import type { NodeAppDetail, NodeAppInspection, NodeConfigurationField, NodeConfigurationInput, NodeSecretChange } from './node-app-types';

type SecretDraft = { action: 'keep' | 'replace' | 'unbind' | 'rebind'; value: string };
type Draft = { ordinary: Record<string, string>; secrets: Record<string, SecretDraft> };
const stateLabels: Record<NodeAppDetail['state'], string> = { stopped: '已停止', starting: '启动中', running: '运行中', stopping: '停止中', failed: '运行异常' };

function draftFrom(app: NodeAppDetail): Draft {
  return {
    ordinary: Object.fromEntries(app.fields.filter((field) => !field.secret).map((field) => [field.name, Object.hasOwn(app.configuration.values, field.name) ? String(app.configuration.values[field.name]) : ''])),
    secrets: Object.fromEntries(app.fields.filter((field) => field.secret).map((field) => [field.name, { action: 'keep', value: '' }])),
  };
}

function valueLabel(value: string | number | boolean) { return typeof value === 'boolean' ? value ? '开启' : '关闭' : String(value); }

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

export interface NodeAppRequest { app: NodeAppDetail; intent: 'enter' | 'details'; token: number }
export function NodeAppsPanel({ enabled, request, addRequest = 0, onAppChanged, onModalStateChange, onDraftStateChange }: {
  enabled: boolean; request?: NodeAppRequest | null; addRequest?: number;
  onAppChanged?: (app: NodeAppDetail) => void; onModalStateChange?: (open: boolean) => void;
  onDraftStateChange: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [modalOpen, setModalOpen] = useState(false);
  const [tab, setTab] = useState<'overview' | 'configuration' | 'runtime'>('overview');
  const [dismissConfirm, setDismissConfirm] = useState(false);
  const [detail, setDetail] = useState<NodeAppDetail | null>(null);
  const [draft, setDraft] = useState<Draft>({ ordinary: {}, secrets: {} });
  const [loading, setLoading] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [statusError, setStatusError] = useState('');
  const [notice, setNotice] = useState('');
  const [addOpen, setAddOpen] = useState(false);
  const [sourcePath, setSourcePath] = useState('');
  const [inspection, setInspection] = useState<NodeAppInspection | null>(null);
  const [requestedSelection, setRequestedSelection] = useState<NodeAppDetail | null>(null);
  const seenRequest = useRef<number | null>(null);
  const seenAdd = useRef(0);
  const changedCallback = useRef(onAppChanged); changedCallback.current = onAppChanged;
  const modalCallback = useRef(onModalStateChange); modalCallback.current = onModalStateChange;
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const selected = useRef<string | null>(null);
  const confirmation = useRef<HTMLDivElement>(null);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const base = useId();
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  const blocked = !enabled || Boolean(pending);

  const accept = useCallback((value: NodeAppDetail, resetDraft: boolean) => {
    changedCallback.current?.(value);
    if (selected.current === value.id) {
      setDetail((current) => resetDraft || !current ? value : { ...current, state: value.state, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError });
      if (resetDraft) setDraft(draftFrom(value));
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

  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; selected.current = null; draftCallback.current({ dirty: false, pending: false }); }; }, []);
  useEffect(() => {
    if (!request || request.token === seenRequest.current || operation.current) return;
    seenRequest.current = request.token;
    if (dirty && selected.current !== request.app.id) { setRequestedSelection(request.app); return; }
    void select(request.app, request.intent === 'enter' ? 'configuration' : 'overview', request.intent === 'enter');
  }, [request, select, pending]);
  useEffect(() => {
    if (!addRequest || addRequest === seenAdd.current || operation.current || !enabled) return;
    seenAdd.current = addRequest; setAddOpen(true); setError(''); setSourcePath(''); setInspection(null);
  }, [addRequest, enabled, pending]);
  useEffect(() => { modalCallback.current?.(modalOpen || addOpen); }, [modalOpen, addOpen]);
  useEffect(() => { if (dismissConfirm || requestedSelection) confirmation.current?.focus(); }, [dismissConfirm, requestedSelection]);
  useEffect(() => { draftCallback.current({ dirty, pending: Boolean(pending) || loading }); }, [dirty, pending, loading]);
  useEffect(() => {
    if (!enabled || !detail || pending || !modalOpen) return;
    let cancelled = false; let polling = false;
    const appId = detail.id;
    const timer = window.setInterval(() => {
      if (polling || operation.current) return;
      polling = true;
      void invoke<NodeAppDetail>('node_app_status', { appId }).then((value) => {
        if (!cancelled && alive.current && selected.current === appId && !operation.current) {
          // Status refresh never replaces configuration declarations, the saved
          // revision used by a draft, or any password text the user is editing.
          setDetail((current) => current?.id === appId ? { ...current, state: value.state, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError } : current);
          changedCallback.current?.(value);
          setStatusError('');
        }
      }).catch(() => { if (!cancelled && alive.current && selected.current === appId) setStatusError('运行状态暂时无法更新。请点击“刷新运行状态”核对，应用不会因此自动重启。'); }).finally(() => { polling = false; });
    }, 1500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [enabled, detail?.id, pending, modalOpen]);

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
      setNotice(value.configuration.canStart ? value.restartRequired ? '配置已保存。正在运行的应用仍使用旧配置；点击“重启并应用配置”后才生效。' : '配置已保存。下次启动使用这些设置。' : '配置已保存，但仍有必填项未完成。补齐后才能启动；已运行的进程不会因此重启。');
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
  function effectiveDraft(name: string) {
    const field = detail?.fields.find((value) => value.name === name);
    return draft.ordinary[name]?.trim() ? draft.ordinary[name] : field?.default;
  }
  function endpointChanged(field: NodeConfigurationField) {
    if (!detail || !field.endpointField) return false;
    const endpoint = detail.fields.find((value) => value.name === field.endpointField);
    const before = detail.configuration.values[field.endpointField] ?? endpoint?.default;
    return effectiveDraft(field.endpointField) !== before || detail.configuration.secretStatus[field.name] === 'endpoint-changed';
  }
  function closeDetails() {
    sequence.current++; selected.current = null; setModalOpen(false); setDetail(null);
    setDraft({ ordinary: {}, secrets: {} }); setDismissConfirm(false); setRequestedSelection(null); setLoading(false);
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

  return <>
    {modalOpen && detail && <Modal title={detail.name} description="项目详情与应用配置" wide closeDisabled={Boolean(pending)} beforeClose={() => { if (dirty) { setDismissConfirm(true); return false; } return true; }} onClose={closeDetails}>
      <section className="node-app-detail node-app-modal-detail" aria-label={`${detail.name}项目详情`} aria-busy={Boolean(pending) || loading}>
        {dismissConfirm && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="关闭配置确认"><strong>有未保存的应用配置</strong><p>关闭会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setDismissConfirm(false)}>继续编辑</button><button className="primary-button" onClick={closeDetails}>丢弃草稿并关闭</button></div></div>}
        {requestedSelection && <div ref={confirmation} tabIndex={-1} className="operation-confirmation" role="group" aria-label="重新读取配置确认"><strong>当前配置有未保存的修改</strong><p>继续会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setRequestedSelection(null)}>继续编辑</button><button className="primary-button" onClick={() => { const value = requestedSelection; setRequestedSelection(null); void select(value, 'configuration'); }}>丢弃草稿并重新读取</button></div></div>}
        {error && <p className="task-error node-inline-message" role="alert">{error}</p>}
        {notice && <p className="workspace-inline-notice node-inline-message" role="status">{notice}</p>}
        <header className="node-app-heading"><div><h2 id={`${base}-title`}>{detail.name}</h2><span className="node-runtime-label"><Server size={14} />Node 服务</span></div><span className={`node-process-state node-process-${detail.state}`} role="status">{pending === 'node_app_start' || pending === 'node_app_restart' ? '打开中…' : pending === 'node_app_stop' ? '关闭中…' : stateLabels[detail.state]}</span></header>
        <div className="node-runtime-actions">
          <button className="primary-button" disabled={blocked || loading || (dirty && detail.state !== 'running') || detail.state === 'starting' || detail.state === 'stopping'} onClick={() => void enterApp(detail)}><Monitor size={15} />进入项目</button>
        </div>
        {detail.lastError && <p className="task-error node-inline-message" role="alert">{detail.lastError}</p>}
        {statusError && <p className="task-error node-inline-message" role="alert">{statusError}</p>}
        {detail.restartRequired && <p className="node-config-warning" role="status">已保存配置与正在运行的配置不同。{detail.configuration.canStart ? '请明确重启后应用新配置。' : '先补齐必填项，再重启应用。'}</p>}
        {detail.configuration.missingRequired.length > 0 && <p className="node-config-warning"><CircleAlert size={16} /><span>请先配置：{detail.configuration.missingRequired.map((name) => detail.fields.find((field) => field.name === name)?.label ?? name).join('、')}。补齐并保存后，再明确点击“进入项目”。</span></p>}
        <div className="overview-tabs" role="tablist" aria-label="Node 项目详情">{([['overview', '概览', Info], ['configuration', '应用配置', KeyRound], ['runtime', '运行状态', Activity]] as const).map(([value, label, Icon], index) => <button key={value} id={`${base}-${value}-tab`} role="tab" type="button" aria-selected={tab === value} aria-controls={`${base}-${value}-panel`} tabIndex={tab === value ? 0 : -1} disabled={blocked || loading} onClick={() => changeTab(value)} onKeyDown={(event) => tabKeys(event, index)}><Icon size={14} />{label}</button>)}</div>
        <div role="tabpanel" id={`${base}-overview-panel`} aria-labelledby={`${base}-overview-tab`} hidden={tab !== 'overview'}><h3>项目信息</h3><p>此项目在独立应用窗口中使用。关闭应用窗口会停止相关服务；再次进入会返回已有窗口。</p><p>应用类型：Node 服务项目</p><p>配置状态：{detail.configuration.canStart ? '配置完整' : '需要配置'}</p><p>保存配置不会自动打开应用，也不会自动重启。</p><button className="refresh-button" disabled={blocked || loading} onClick={() => changeTab('configuration')}>查看应用配置</button><details className="node-source-details"><summary>来源与当前接入范围</summary><p>此入口登记本机源码目录，使用平台内置 Node。目录需保留；完整外部分发包、迁移、备份与回收流程尚未接入。</p><code>{detail.sourcePath}</code><code>{detail.id}</code></details></div>
        <div role="tabpanel" id={`${base}-runtime-panel`} aria-labelledby={`${base}-runtime-tab`} hidden={tab !== 'runtime'}><h3>运行状态</h3><p>{stateLabels[detail.state]}。关闭应用窗口会停止本应用；关闭此详情弹窗不会关闭应用窗口。</p><div className="node-runtime-actions"><button className="refresh-button" disabled={blocked || loading || !['running', 'starting', 'failed'].includes(detail.state)} onClick={() => void run('node_app_stop')}><Square size={14} />关闭应用</button><button className="refresh-button" disabled={blocked || loading || dirty || !detail.configuration.canStart || detail.state !== 'running'} onClick={() => void run('node_app_restart')}><RefreshCw size={14} />重启并应用配置</button><button className="refresh-button" disabled={blocked || loading || dirty} onClick={() => void run('node_app_status')}>刷新运行状态</button></div></div>
        <div role="tabpanel" id={`${base}-configuration-panel`} aria-labelledby={`${base}-configuration-tab`} hidden={tab !== 'configuration'}>
        <div className="node-configuration-heading"><h3>应用配置</h3><p>每个应用的密钥分别保存在本机私有配置文件中，仅当前用户可读写，打开时交给本应用。这里不会读回或显示已保存的密钥；保存配置不会自动打开应用。</p></div>
        <form className="node-configuration-form" onSubmit={(event) => void save(event)} noValidate>
          {detail.fields.map((field, index) => {
            const inputId = `${base}-field-${index}`;
            const helpId = `${inputId}-help`;
            const ordinary = draft.ordinary[field.name] ?? '';
            const edit = draft.secrets[field.name] ?? { action: 'keep', value: '' };
            const hasSecret = ['set', 'endpoint-changed'].includes(detail.configuration.secretStatus[field.name]);
            const changedEndpoint = field.secret && hasSecret && endpointChanged(field);
            const defaultText = field.default === undefined ? '未设置' : `使用默认：${valueLabel(field.default)}`;
            return <div className="node-config-field" key={field.name}>
              <label className="node-field-label" htmlFor={inputId}>{field.secret && <KeyRound size={15} />}{field.label}<span className={field.required ? 'required-label' : 'optional-label'}>{field.required ? '必填' : '选填'}</span></label>
              <p id={helpId} className="node-field-description">{field.description}</p>
              {field.secret ? <>
                <span className="node-secret-status">{edit.action === 'unbind' ? '待移除：点击保存后解除本应用的密钥配置。' : hasSecret ? '已设置 · 保存值不会显示' : '尚未设置'}</span>
                <input id={inputId} type="password" value={edit.value} aria-describedby={helpId} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} disabled={blocked || loading || edit.action === 'unbind'} placeholder={hasSecret ? '留空保持原密钥；填写新值则替换' : field.placeholder ?? '粘贴你自己的密钥'} onChange={(event) => secretChange(field.name, { action: event.target.value ? 'replace' : 'keep', value: event.target.value })} />
                <div className="node-secret-controls">{edit.action === 'unbind' ? <button type="button" className="text-button" disabled={blocked || loading} onClick={() => secretChange(field.name, { action: 'keep', value: '' })}>取消移除</button> : hasSecret && <button type="button" className="text-button" disabled={blocked || loading} onClick={() => secretChange(field.name, { action: 'unbind', value: '' })}>移除此应用的密钥</button>}</div>
                {changedEndpoint && edit.action !== 'unbind' && edit.action !== 'replace' && <div className="node-endpoint-confirmation"><p>服务地址已改变。已保存的密钥不会自动交给新地址；可以重新填写密钥，或核对以下目标后明确确认。</p><code>{String(effectiveDraft(field.endpointField!) ?? '尚未填写有效服务地址')}</code><label className="checkbox-label"><input type="checkbox" checked={edit.action === 'rebind'} disabled={blocked || loading} onChange={(event) => secretChange(field.name, { action: event.target.checked ? 'rebind' : 'keep', value: '' })} />我确认将已保存的密钥用于上方服务地址</label></div>}
              </> : field.type === 'select' || field.type === 'boolean' ? <select id={inputId} value={ordinary} aria-describedby={helpId} disabled={blocked || loading} onChange={(event) => ordinaryChange(field.name, event.target.value)}><option value="">{defaultText}</option>{field.type === 'boolean' ? <><option value="true">开启</option><option value="false">关闭</option></> : field.options?.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select> : <input id={inputId} type={field.type === 'integer' ? 'number' : field.type === 'url' ? 'url' : 'text'} step={field.type === 'integer' ? 1 : undefined} value={ordinary} aria-describedby={helpId} disabled={blocked || loading} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} placeholder={field.default !== undefined ? valueLabel(field.default) : field.placeholder} onChange={(event) => ordinaryChange(field.name, event.target.value)} />}
              {!field.secret && field.default !== undefined && <p className="node-field-default">留空使用作者默认值：<strong>{valueLabel(field.default)}</strong></p>}
              <details className="node-field-guide"><summary>怎样获取和填写</summary><ol>{field.howToGet.map((step, stepIndex) => <li key={stepIndex}>{step}</li>)}</ol>{field.helpUrl && <button className="text-button" type="button" disabled={blocked || loading} onClick={() => void open('node_app_open_help', field.name)}><ExternalLink size={13} />打开官方说明</button>}<p className="node-field-variable">开发者变量：<code>{field.name}</code>{field.endpointField && <> · 关联地址：<code>{field.endpointField}</code></>}</p></details>
            </div>;
          })}
          {!detail.fields.length && <p className="project-help">此应用没有声明需要用户填写的配置。</p>}
          <div className="node-config-save"><span role="status">{dirty ? '有未保存的修改。保存后不会自动重启或请求外部服务。' : `已保存配置 · 修订 ${detail.configuration.revision}`}</span><div><button className="refresh-button" type="button" disabled={blocked || loading} onClick={() => { if (dirty) setRequestedSelection(detail); else void select(detail); }}>重新读取配置</button><button className="primary-button" type="submit" disabled={blocked || loading || !dirty}><Save size={15} />{pending === 'save' ? '保存中…' : '保存配置'}</button></div></div>
        </form>
        </div>
      </section></Modal>}
    {addOpen && <Modal title="添加本地应用" description="选择源码目录，只读核对应用声明后再登记。" closeDisabled={Boolean(pending)} onClose={() => { setAddOpen(false); setSourcePath(''); setInspection(null); setError(''); }}>
      <div className="node-add-dialog"><p>应用目录中需要有平台可识别的 <code>local-app.json</code>。登记后先配置，再打开应用。</p><button className="refresh-button" disabled={blocked} onClick={() => void chooseSource()}><FolderOpen size={16} />{pending === 'choose' ? '正在核对…' : '选择应用目录'}</button>{sourcePath && <code className="node-selected-source">{sourcePath}</code>}{error && <p role="alert" className="task-error">{error}</p>}{inspection && <div className="node-inspection-preview"><h3>{inspection.app.name}</h3><p>{inspection.app.description}</p><span className="node-runtime-label"><Server size={14} />{inspection.runtimeLabel} · {inspection.app.version}</span><p>应用声明了 {inspection.configuration.fields.length} 项配置，其中 {inspection.configuration.fields.filter((field) => field.required).length} 项必填。登记不读取旧的 .env，也不会自动运行应用。</p><ul>{inspection.configuration.fields.map((field) => <li key={field.name}>{field.label}{field.required ? '（必填）' : '（选填）'}{field.secret ? ' · 密钥' : ''}</li>)}</ul><button className="primary-button" disabled={blocked || inspection.runtime !== 'node-service'} onClick={() => void register()}>{pending === 'register' ? '登记中…' : '登记此应用'}</button></div>}</div>
    </Modal>}
  </>;
}
