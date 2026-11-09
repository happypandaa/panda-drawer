import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { AppConfigurationForm } from './AppConfigurationForm';
import { draftFrom, nodeConfigurationInput } from './app-configuration';
import type { WebConfigurationDetail } from './node-app-types';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import { projectWasDeleted } from './project-deletion';
import { messageFrom } from './ui-utils';

export function AppConfiguration({ projectId, instanceId, disabled, onBack, onSaved, onState, returnToAdded = false }: {
  projectId: string; instanceId?: string; disabled: boolean;
  returnToAdded?: boolean;
  onBack: () => void; onSaved: () => Promise<void>;
  onState: (state: { dirty: boolean; pending: boolean }) => void;
}) {
  const [detail, setDetail] = useState<WebConfigurationDetail | null>(null);
  const [draft, setDraft] = useState<NodeConfigurationDraft>({ ordinary: {}, secrets: {} });
  const [loading, setLoading] = useState(true); const [saving, setSaving] = useState(false);
  const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  const operation = useRef(false); const sequence = useRef(0); const alive = useRef(true);
  const heading = useRef<HTMLHeadingElement>(null);
  const currentScope = useRef(instanceId); currentScope.current = instanceId;
  const boundScope = useRef(instanceId);
  const scopeChanged = boundScope.current !== instanceId;
  const dirty = Boolean(detail && JSON.stringify(draft) !== JSON.stringify(draftFrom(detail)));
  useEffect(() => { alive.current = true; return () => { alive.current = false; sequence.current++; }; }, []);
  useEffect(() => { heading.current?.focus(); }, []);
  useEffect(() => { onState({ dirty, pending: loading || saving }); }, [dirty, loading, saving, onState]);
  useEffect(() => () => onState({ dirty: false, pending: false }), [onState]);
  function checked(value: WebConfigurationDetail) {
    if (value?.id !== projectId || !value.configuration || !Array.isArray(value.fields) || typeof value.manifestDigest !== 'string') throw new Error(t("应用配置身份不符，请重新读取。"));
    return value;
  }
  async function read() {
    if (operation.current) return;
    operation.current = true; setLoading(true); setError('');
    const scope = currentScope.current;
    const revision = ++sequence.current;
    try {
      const value = checked(await invoke<WebConfigurationDetail>('app_configuration', { appId: projectId }));
      if (!alive.current || revision !== sequence.current || projectWasDeleted(projectId)) return;
      if (currentScope.current !== scope) { setError(t("平台服务已切换，未替换当前草稿。请复制需要的内容，再重新读取配置。")); return; }
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
      const value = checked(await invoke<WebConfigurationDetail>('app_configuration_save', { appId: projectId, input: nodeConfigurationInput(detail, draft) }));
      if (!alive.current || revision !== sequence.current || projectWasDeleted(projectId)) return;
      if (currentScope.current !== scope) { setError(t("平台服务已切换，保存结果需要重新核对。草稿仍保留，请复制后再重新读取配置。")); return; }
      setDetail(value); setDraft(draftFrom(value));
      setNotice(t("已保存，下次重新加载生效。"));
      await onSaved();
      if (alive.current && revision === sequence.current && value.configuration.canStart && currentScope.current === scope) onBack();
    } catch (cause) { if (alive.current && revision === sequence.current) setError(messageFrom(cause)); }
    finally { if (alive.current && revision === sequence.current) { operation.current = false; setSaving(false); } }
  }
  return <section className="web-app-configuration" aria-label={t("应用配置")}>
    <button type="button" className="text-button" disabled={loading || saving} onClick={onBack}>{returnToAdded ? dirty ? t("放弃修改并返回添加结果") : t("返回添加结果") : dirty ? t("放弃修改并返回详情") : t("返回详情")}</button>
    <h3 ref={heading} tabIndex={-1}>{t("应用配置")}</h3>
    {loading && <p role="status">{t("正在读取配置…")}</p>}
    {scopeChanged && <div role="status"><p>{t("平台服务已切换，当前配置草稿仍保留。复制需要的内容后，再放弃草稿并读取当前配置。")}</p><button className="refresh-button" disabled={loading || saving} onClick={() => { if (!navigator.clipboard) { setError(t("剪贴板不可用，请手动复制需要的文字。")); return; } void navigator.clipboard.writeText(JSON.stringify(draft.ordinary, null, 2)).then(() => setNotice(t("配置草稿已复制。"))).catch(() => setError(t("复制未完成，请手动复制输入框中的文字。"))); }}>{t("复制配置草稿")}</button><button className="refresh-button" disabled={loading || saving} onClick={() => void read()}>{t("放弃草稿并重新读取")}</button></div>}
    {error && <p className="task-error" role="alert">{t(error)}</p>}
    {!loading && !detail && <button className="refresh-button" onClick={() => void read()}>{t("重新读取配置")}</button>}
    {detail && <AppConfigurationForm app={detail} draft={draft} active disabled={disabled || scopeChanged || loading || saving} dirty={dirty} saving={saving} savedNotice={t(notice)}
      onOrdinaryChange={(name, value) => setDraft(current => ({ ...current, ordinary: { ...current.ordinary, [name]: value } }))}
      onSecretChange={(name, edit) => setDraft(current => ({ ...current, secrets: { ...current.secrets, [name]: edit } }))} onSave={event => void save(event)}
      onOpenHelp={fieldName => { void invoke('app_open_help', { appId: projectId, fieldName }).catch(cause => { if (alive.current) setError(messageFrom(cause)); }); }} />}
  </section>;
}
