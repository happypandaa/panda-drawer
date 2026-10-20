import { invoke } from '@tauri-apps/api/core';
import { CircleAlert, ExternalLink, FolderOpen, KeyRound, Monitor, Plus, RefreshCw, Save, Server, Square } from 'lucide-react';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { FormEvent } from 'react';
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

export function NodeAppsPanel({ enabled, onDraftStateChange }: { enabled: boolean; onDraftStateChange: (state: { dirty: boolean; pending: boolean }) => void }) {
  const [apps, setApps] = useState<NodeAppDetail[]>([]);
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
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const selected = useRef<string | null>(null);
  const draftCallback = useRef(onDraftStateChange); draftCallback.current = onDraftStateChange;
  const base = useId();
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  const blocked = !enabled || Boolean(pending);

  const accept = useCallback((value: NodeAppDetail, resetDraft: boolean) => {
    setApps((current) => current.some((app) => app.id === value.id) ? current.map((app) => app.id === value.id ? value : app) : [...current, value]);
    if (selected.current === value.id) {
      setDetail((current) => resetDraft || !current ? value : { ...current, state: value.state, restartRequired: value.restartRequired, runningRevision: value.runningRevision, lastError: value.lastError });
      if (resetDraft) setDraft(draftFrom(value));
    }
  }, []);

  const select = useCallback(async (app: NodeAppDetail) => {
    const request = ++sequence.current;
    selected.current = app.id;
    setDetail(app); setDraft(draftFrom(app)); setError(''); setStatusError(''); setNotice(''); setLoading(true);
    try {
      const value = await invoke<NodeAppDetail>('node_app_configuration', { appId: app.id });
      if (alive.current && sequence.current === request) accept(value, true);
    } catch (cause) { if (alive.current && sequence.current === request) setError(messageFrom(cause)); }
    finally { if (alive.current && sequence.current === request) setLoading(false); }
  }, [accept]);

  const refreshList = useCallback(async () => {
    if (!enabled || operation.current) return;
    const request = ++sequence.current;
    setLoading(true); setError('');
    try {
      const values = await invoke<NodeAppDetail[]>('node_app_list');
      if (!alive.current || sequence.current !== request) return;
      setApps(values);
      if (!selected.current && values.length) await select(values[0]);
    } catch (cause) { if (alive.current && sequence.current === request) setError(messageFrom(cause)); }
    finally { if (alive.current && sequence.current === request) setLoading(false); }
  }, [enabled, select]);

  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; selected.current = null; draftCallback.current({ dirty: false, pending: false }); }; }, []);
  useEffect(() => { void refreshList(); }, [refreshList]);
  useEffect(() => { draftCallback.current({ dirty, pending: Boolean(pending) }); }, [dirty, pending]);
  useEffect(() => {
    if (!enabled || !detail || pending) return;
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
          setApps((current) => current.map((app) => app.id === appId ? value : app));
          setStatusError('');
        }
      }).catch(() => { if (!cancelled && alive.current && selected.current === appId) setStatusError('运行状态暂时无法更新。请点击“刷新运行状态”核对，应用不会因此自动重启。'); }).finally(() => { polling = false; });
    }, 1500);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [enabled, detail?.id, pending]);

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
      accept(value, true); setAddOpen(false); setSourcePath(''); setInspection(null);
      setNotice('应用已登记。请先填写配置并保存，再点击“打开应用”。');
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
  function selectRequested(app: NodeAppDetail) {
    if (blocked || loading || app.id === selected.current) return;
    if (dirty) setRequestedSelection(app); else void select(app);
  }

  return <section className="node-apps-panel" aria-label="Node 应用管理">
    <div className="node-apps-intro"><div><Server size={18} /><p>先完成配置，再打开应用窗口。关闭应用窗口会停止本应用；每个应用只有一个窗口，再次打开会回到已有窗口。</p></div><span className="subtle-tag">源码接入 · 内部验证</span></div>
    <div className="node-apps-toolbar"><button className="primary-button" disabled={blocked || dirty || loading} onClick={() => { setAddOpen(true); setError(''); }}><Plus size={16} />添加 Node 应用</button><button className="refresh-button" disabled={blocked || loading} onClick={() => void refreshList()}><RefreshCw size={15} className={loading ? 'rotating' : undefined} />刷新列表</button></div>
    {error && !addOpen && <p className="task-error node-inline-message" role="alert">{error}</p>}
    {notice && <p className="workspace-inline-notice node-inline-message" role="status">{notice}</p>}
    <div className="node-apps-layout">
      <div className="node-app-list" aria-label="已登记的 Node 应用">
        {apps.map((app) => <button key={app.id} type="button" className={detail?.id === app.id ? 'node-app-list-item selected' : 'node-app-list-item'} aria-pressed={detail?.id === app.id} disabled={blocked || loading} onClick={() => selectRequested(app)}><Server size={20} /><span><strong>{app.name}</strong><small>Node 服务 · {stateLabels[app.state]}</small></span></button>)}
        {!apps.length && <div className="node-app-empty"><Server size={28} /><h2>{loading ? '正在读取应用…' : '还没有 Node 应用'}</h2><p>选择带有应用说明文件的源码目录，核对后登记。不会自动打开应用。</p></div>}
      </div>
      {detail && <article className="node-app-detail" aria-labelledby={`${base}-title`} aria-busy={Boolean(pending) || loading}>
        <header className="node-app-heading"><div><h2 id={`${base}-title`}>{detail.name}</h2><span className="node-runtime-label"><Server size={14} />Node 服务</span></div><span className={`node-process-state node-process-${detail.state}`} role="status">{pending === 'node_app_start' || pending === 'node_app_restart' ? '打开中…' : pending === 'node_app_stop' ? '关闭中…' : stateLabels[detail.state]}</span></header>
        <div className="node-runtime-actions">
          <button className="primary-button" disabled={blocked || loading || (dirty && detail.state !== 'running') || !detail.configuration.canStart || detail.state === 'starting' || detail.state === 'stopping'} onClick={() => { if (detail.state === 'running') void open('node_app_open'); else void run('node_app_start'); }}><Monitor size={15} />{detail.state === 'running' ? '返回应用窗口' : '打开应用'}</button>
          <button className="refresh-button" disabled={blocked || loading || !['running', 'starting', 'failed'].includes(detail.state)} onClick={() => void run('node_app_stop')}><Square size={14} />关闭应用</button>
          <button className="refresh-button" disabled={blocked || loading || dirty || !detail.configuration.canStart || detail.state !== 'running'} onClick={() => void run('node_app_restart')}><RefreshCw size={14} />重启并应用配置</button>
          <button className="text-button" disabled={blocked || loading || dirty} onClick={() => void run('node_app_status')}>刷新运行状态</button>
        </div>
        {detail.lastError && <p className="task-error node-inline-message" role="alert">{detail.lastError}</p>}
        {statusError && <p className="task-error node-inline-message" role="alert">{statusError}</p>}
        {detail.restartRequired && <p className="node-config-warning" role="status">已保存配置与正在运行的配置不同。{detail.configuration.canStart ? '请明确重启后应用新配置。' : '先补齐必填项，再重启应用。'}</p>}
        {detail.configuration.missingRequired.length > 0 && <p className="node-config-warning"><CircleAlert size={16} /><span>请先配置：{detail.configuration.missingRequired.map((name) => detail.fields.find((field) => field.name === name)?.label ?? name).join('、')}。补齐并保存后才能打开应用。</span></p>}
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
        <details className="node-source-details"><summary>来源与当前接入范围</summary><p>此入口登记本机源码目录，使用平台内置 Node 启动。目录需保留；完整外部分发包、迁移、备份与回收流程尚未接入。</p><code>{detail.sourcePath}</code></details>
      </article>}
    </div>
    {addOpen && <Modal title="添加 Node 应用" description="选择源码目录，只读核对应用声明后再登记。" closeDisabled={Boolean(pending)} onClose={() => { setAddOpen(false); setSourcePath(''); setInspection(null); setError(''); }}>
      <div className="node-add-dialog"><p>应用目录中需要有平台可识别的 <code>local-app.json</code>。登记后先配置，再打开应用。</p><button className="refresh-button" disabled={blocked} onClick={() => void chooseSource()}><FolderOpen size={16} />{pending === 'choose' ? '正在核对…' : '选择应用目录'}</button>{sourcePath && <code className="node-selected-source">{sourcePath}</code>}{error && <p role="alert" className="task-error">{error}</p>}{inspection && <div className="node-inspection-preview"><h3>{inspection.app.name}</h3><p>{inspection.app.description}</p><span className="node-runtime-label"><Server size={14} />{inspection.runtimeLabel} · {inspection.app.version}</span><p>应用声明了 {inspection.configuration.fields.length} 项配置，其中 {inspection.configuration.fields.filter((field) => field.required).length} 项必填。登记不读取旧的 .env，也不会自动运行应用。</p><ul>{inspection.configuration.fields.map((field) => <li key={field.name}>{field.label}{field.required ? '（必填）' : '（选填）'}{field.secret ? ' · 密钥' : ''}</li>)}</ul><button className="primary-button" disabled={blocked || inspection.runtime !== 'node-service'} onClick={() => void register()}>{pending === 'register' ? '登记中…' : '登记此应用'}</button></div>}</div>
    </Modal>}
    {requestedSelection && <Modal title={requestedSelection.id === detail?.id ? '重新读取配置' : '切换应用'} description="当前配置有未保存的修改。" onClose={() => setRequestedSelection(null)}><p>继续操作会清空当前草稿，包括尚未保存的密钥。已保存配置不会改变。</p><div className="confirmation-actions"><button className="refresh-button" onClick={() => setRequestedSelection(null)}>继续编辑</button><button className="primary-button" onClick={() => { const value = requestedSelection; setRequestedSelection(null); void select(value); }}>{requestedSelection.id === detail?.id ? '丢弃草稿并重新读取' : '丢弃草稿并切换'}</button></div></Modal>}
  </section>;
}
