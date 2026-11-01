import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { NodeConfigurationForm } from './NodeConfigurationForm';
import { draftFrom, nodeConfigurationInput } from './NodeAppsPanel';
import type { WebConfigurationDetail } from './node-app-types';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import { projectWasDeleted } from './project-deletion';
import { messageFrom } from './ui-utils';

export function WebAppConfiguration({ projectId, instanceId, disabled, onBack, onSaved, onState }: {
  projectId: string; instanceId?: string; disabled: boolean;
  onBack: () => void; onSaved: () => Promise<void>;
  onState: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [detail, setDetail] = useState<WebConfigurationDetail | null>(null);
  const [draft, setDraft] = useState<NodeConfigurationDraft>({ ordinary: {}, secrets: {} });
  const [loading, setLoading] = useState(true); const [saving, setSaving] = useState(false);
  const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  const operation = useRef(false); const sequence = useRef(0); const alive = useRef(true);
  const currentScope = useRef(instanceId); currentScope.current = instanceId;
  const boundScope = useRef(instanceId);
  const scopeChanged = boundScope.current !== instanceId;
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; }; }, []);
  useEffect(() => { onState({ dirty, pending: loading || saving }); }, [dirty, loading, saving, onState]);
  useEffect(() => () => onState({ dirty: false, pending: false }), [onState]);
  function checked(value: WebConfigurationDetail) {
    if (value?.id !== projectId || value.runtime !== 'web' || !value.configuration || !Array.isArray(value.fields) || value.fields.some(field => field.secret) || typeof value.manifestDigest !== 'string') throw new Error('应用配置身份不符，请重新读取。');
    return value;
  }
  async function read() {
    if (operation.current) return;
    operation.current = true; setLoading(true); setError('');
    const scope = currentScope.current;
    const revision = ++sequence.current;
    try {
      const value = checked(await invoke<WebConfigurationDetail>('web_app_configuration', { projectId }));
      if (!alive.current || revision !== sequence.current || projectWasDeleted(projectId)) return;
      if (currentScope.current !== scope) { setError('平台服务已切换，未替换当前草稿。请复制需要的内容，再重新读取配置。'); return; }
      boundScope.current = scope;
      setDetail(value); setDraft(draftFrom(value));
    } catch (cause) { if (alive.current && revision === sequence.current) setError(messageFrom(cause)); }
    finally { if (alive.current && revision === sequence.current) { operation.current = false; setLoading(false); } }
  }
  useEffect(() => { void read(); }, [projectId]);
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (!detail || operation.current || disabled || scopeChanged) return;
    operation.current = true; setSaving(true); setError(''); setNotice('');
    const revision = ++sequence.current;
    const scope = boundScope.current;
    try {
      const value = checked(await invoke<WebConfigurationDetail>('web_app_configuration_save', { projectId, input: nodeConfigurationInput(detail, draft) }));
      if (!alive.current || revision !== sequence.current || projectWasDeleted(projectId)) return;
      if (currentScope.current !== scope) { setError('平台服务已切换，保存结果需要重新核对。草稿仍保留，请复制后再重新读取配置。'); return; }
      setDetail(value); setDraft(draftFrom(value));
      setNotice('已保存，下次打开或刷新页面生效。');
      await onSaved();
      if (alive.current && revision === sequence.current && value.configuration.canStart && currentScope.current === scope) onBack();
    } catch (cause) { if (alive.current && revision === sequence.current) setError(messageFrom(cause)); }
    finally { if (alive.current && revision === sequence.current) { operation.current = false; setSaving(false); } }
  }
  return <section className="web-app-configuration" aria-label="网页应用配置">
    <button type="button" className="text-button" disabled={loading || saving} onClick={onBack}>{dirty ? '放弃修改并返回详情' : '返回详情'}</button>
    <h3>应用配置</h3>
    {loading && <p role="status">正在读取配置…</p>}
    {scopeChanged && <div role="status"><p>平台服务已切换，当前配置草稿仍保留。复制需要的内容后，再放弃草稿并读取当前配置。</p><button className="refresh-button" disabled={loading || saving} onClick={() => { if (!navigator.clipboard) { setError('剪贴板不可用，请手动复制需要的文字。'); return; } void navigator.clipboard.writeText(JSON.stringify(draft.ordinary, null, 2)).then(() => setNotice('配置草稿已复制。')).catch(() => setError('复制未完成，请手动复制输入框中的文字。')); }}>复制配置草稿</button><button className="refresh-button" disabled={loading || saving} onClick={() => void read()}>放弃草稿并重新读取</button></div>}
    {error && <p className="task-error" role="alert">{error}</p>}
    {!loading && !detail && <button className="refresh-button" onClick={() => void read()}>重新读取配置</button>}
    {detail && <NodeConfigurationForm runtime="web" app={detail} draft={draft} active disabled={disabled || scopeChanged || loading || saving} dirty={dirty} saving={saving} savedNotice={notice}
      onOrdinaryChange={(name, value) => setDraft(current => ({ ...current, ordinary: { ...current.ordinary, [name]: value } }))}
      onSecretChange={() => {}} onSave={event => void save(event)}
      onOpenHelp={fieldName => { void invoke('web_app_open_help', { projectId, fieldName }).catch(cause => { if (alive.current) setError(messageFrom(cause)); }); }} />}
  </section>;
}
