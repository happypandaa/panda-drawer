import { CircleAlert, ExternalLink, Info, Save, Trash2, Undo2 } from 'lucide-react';
import { useEffect, useId, useState } from 'react';
import type { FormEvent } from 'react';
import { Modal } from './Modal';
import type { NodeAppDetail, NodeConfigurationField } from './node-app-types';
import { validateNodeConfiguration } from './node-configuration-validation';
import type { NodeConfigurationDraft } from './node-configuration-validation';

function valueLabel(field: NodeConfigurationField, value: string | number | boolean) {
  if (typeof value === 'boolean') return value ? '开启' : '关闭';
  return field.options?.find((option) => option.value === value)?.label ?? String(value);
}

function hasHelp(field: NodeConfigurationField) {
  return Boolean(field.description.trim() || field.howToGet.some((step) => step.trim()) || field.helpUrl || field.default !== undefined);
}

export function NodeConfigurationForm({ app, draft, active, disabled, dirty, saving, onOrdinaryChange, onSecretChange, onSave, savedNotice, onRestart, restartDisabled, onOpenHelp }: {
  app: NodeAppDetail;
  draft: NodeConfigurationDraft;
  active: boolean;
  disabled: boolean;
  dirty: boolean;
  saving: boolean;
  onOrdinaryChange: (name: string, value: string) => void;
  onSecretChange: (name: string, edit: NodeConfigurationDraft['secrets'][string]) => void;
  onSave: (event: FormEvent<HTMLFormElement>) => void;
  savedNotice?: string;
  onRestart?: () => void;
  restartDisabled?: boolean;
  onOpenHelp: (name: string) => void;
}) {
  const base = useId();
  const [touched, setTouched] = useState<Set<string>>(() => new Set());
  const [submitted, setSubmitted] = useState(false);
  const [browserIssues, setBrowserIssues] = useState<Record<string, string>>({});
  const [help, setHelp] = useState<{ kind: 'general' } | { kind: 'field'; name: string } | null>(null);
  const issues = validateNodeConfiguration(app, draft);
  const helpField = help?.kind === 'field' ? app.fields.find((field) => field.name === help.name) : undefined;
  useEffect(() => { if (!active) setHelp(null); }, [active]);

  function syncBrowserIssue(name: string, control: HTMLInputElement | HTMLSelectElement) {
    const message = control.validity.badInput ? '请填写有效数值' : undefined;
    setBrowserIssues((current) => {
      if (current[name] === message) return current;
      const next = { ...current };
      if (message) next[name] = message;
      else delete next[name];
      return next;
    });
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (disabled) return;
    setSubmitted(true);
    const nativeIssues: Record<string, string> = {};
    app.fields.forEach((field, index) => {
      const control = document.getElementById(`${base}-field-${index}`) as HTMLInputElement | HTMLSelectElement | null;
      if (control?.validity.badInput) nativeIssues[field.name] = '请填写有效数值';
      control?.setCustomValidity(issues[field.name]?.blocking ? issues[field.name].message : nativeIssues[field.name] ?? '');
    });
    setBrowserIssues(nativeIssues);
    // Missing required values may be saved as an incomplete configuration. Bad
    // formats and unconfirmed credential targets must be corrected before save.
    const invalid = app.fields.findIndex((field) => issues[field.name]?.blocking || nativeIssues[field.name]);
    if (invalid !== -1) {
      const control = document.getElementById(`${base}-field-${invalid}`) as HTMLInputElement | HTMLSelectElement | null;
      control?.focus();
      control?.reportValidity();
      return;
    }
    // Number inputs can retain an unfinished exponent/sign while exposing an
    // empty value. Let the browser reject that input rather than save it empty.
    if (!event.currentTarget.reportValidity()) return;
    onSave(event);
  }

  return <>
    <div className="node-configuration-intro"><p>保存后，下次启动生效。</p><button type="button" className="icon-button node-field-info" aria-label="配置与密钥说明" aria-haspopup="dialog" onClick={() => setHelp({ kind: 'general' })}><Info size={16} /></button></div>
    <form className="node-configuration-form" onSubmit={submit} noValidate>
      {app.fields.map((field, index) => {
        const inputId = `${base}-field-${index}`;
        const errorId = `${inputId}-error`;
        const ordinary = draft.ordinary[field.name] ?? '';
        const edit = draft.secrets[field.name] ?? { action: 'keep', value: '' };
        const hasSecret = ['set', 'endpoint-changed'].includes(app.configuration.secretStatus[field.name]);
        const endpoint = app.fields.find((value) => value.name === field.endpointField);
        const target = field.endpointField ? draft.ordinary[field.endpointField]?.trim() ? draft.ordinary[field.endpointField] : endpoint?.default : undefined;
        const previousTarget = field.endpointField ? app.configuration.values[field.endpointField] ?? endpoint?.default : undefined;
        const changedEndpoint = field.secret && hasSecret && Boolean(field.endpointField) && (target !== previousTarget || app.configuration.secretStatus[field.name] === 'endpoint-changed');
        const issue = issues[field.name] ?? (browserIssues[field.name] ? { message: browserIssues[field.name], blocking: true } : undefined);
        const showIssue = Boolean(issue && (submitted || touched.has(field.name) || app.configuration.missingRequired.includes(field.name) || app.configuration.secretStatus[field.name] === 'endpoint-changed'));
        const controlProps = {
          id: inputId,
          'aria-required': field.required,
          'aria-invalid': showIssue || undefined,
          'aria-describedby': showIssue ? errorId : undefined,
          disabled,
          onInput: (event: FormEvent<HTMLInputElement | HTMLSelectElement>) => syncBrowserIssue(field.name, event.currentTarget),
          onBlur: (event: FormEvent<HTMLInputElement | HTMLSelectElement>) => { syncBrowserIssue(field.name, event.currentTarget); setTouched((current) => new Set(current).add(field.name)); },
          onInvalid: (event: FormEvent<HTMLInputElement | HTMLSelectElement>) => {
            event.preventDefault();
            syncBrowserIssue(field.name, event.currentTarget);
          },
          ref: (control: HTMLInputElement | HTMLSelectElement | null) => { control?.setCustomValidity(issue?.blocking ? issue.message : ''); },
        };
        return <div className={`node-config-field${showIssue ? ' node-config-field-invalid' : ''}`} key={field.name}>
          <div className="node-config-row">
            <label className="node-field-label" htmlFor={inputId} title={field.label}>{field.label}</label>
            <div className="node-field-control">
              {field.secret ? <input {...controlProps} type="password" value={edit.value} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} disabled={disabled || edit.action === 'unbind'} placeholder={edit.action === 'unbind' ? '保存后移除' : hasSecret ? '已设置，留空保留' : field.placeholder ?? '输入密钥'} onChange={(event) => onSecretChange(field.name, { action: event.target.value ? 'replace' : 'keep', value: event.target.value })} />
                : field.type === 'select' || field.type === 'boolean' ? <select {...controlProps} value={ordinary} onChange={(event) => onOrdinaryChange(field.name, event.target.value)}><option value="">{field.default === undefined ? '请选择' : `默认：${valueLabel(field, field.default)}`}</option>{field.type === 'boolean' ? <><option value="true">开启</option><option value="false">关闭</option></> : field.options?.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select>
                  : <input {...controlProps} type={field.type === 'integer' ? 'number' : field.type === 'url' ? 'url' : 'text'} step={field.type === 'integer' ? 1 : undefined} value={ordinary} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} placeholder={field.default !== undefined ? valueLabel(field, field.default) : field.placeholder} onChange={(event) => onOrdinaryChange(field.name, event.target.value)} />}
              {showIssue && <span className="node-field-validation" title={issue.message}><CircleAlert size={16} aria-hidden="true" /><span id={errorId} className="node-field-error-message">{issue.message}</span></span>}
              {field.secret && (hasSecret || edit.action === 'unbind') && <button type="button" className="icon-button node-secret-action" disabled={disabled} aria-label={edit.action === 'unbind' ? '取消移除' : '移除此应用的密钥'} title={edit.action === 'unbind' ? '取消移除' : '移除此应用的密钥'} onClick={() => onSecretChange(field.name, { action: edit.action === 'unbind' ? 'keep' : 'unbind', value: '' })}>{edit.action === 'unbind' ? <Undo2 size={15} /> : <Trash2 size={15} />}</button>}
            </div>
            {hasHelp(field) ? <button type="button" className="icon-button node-field-info" aria-label={`${field.label}说明`} aria-haspopup="dialog" onClick={() => setHelp({ kind: 'field', name: field.name })}><Info size={16} /></button> : <span />}
          </div>
          {changedEndpoint && edit.action !== 'unbind' && edit.action !== 'replace' && <div className="node-endpoint-confirmation"><p>服务地址已更改，请确认密钥的使用目标。</p><code>{String(target ?? '尚未填写有效服务地址')}</code><label className="checkbox-label"><input type="checkbox" checked={edit.action === 'rebind'} disabled={disabled} onChange={(event) => onSecretChange(field.name, { action: event.target.checked ? 'rebind' : 'keep', value: '' })} />将已保存的密钥用于此地址</label></div>}
        </div>;
      })}
      {!app.fields.length && <p className="project-help">此应用无需配置。</p>}
      <div className="node-config-save"><span role="status">{dirty ? '有未保存的修改' : savedNotice ?? '已保存'}</span><div>{onRestart && <button className="refresh-button" type="button" disabled={disabled || restartDisabled} onClick={onRestart}>立即重启</button>}<button className="primary-button" type="submit" disabled={disabled || !dirty}><Save size={15} />{saving ? '保存中…' : '保存配置'}</button></div></div>
    </form>
    {active && help?.kind === 'general' && <Modal title="配置与密钥说明" className="node-field-help-modal" onClose={() => setHelp(null)}>
      <div className="node-field-help"><p>留空使用默认值。缺少必要配置时可先保存，补齐后才能启动。</p><p>每个应用的密钥分别保存在本机私有配置文件中，仅当前用户可读写。已保存的密钥不会显示；留空保留，点击移除图标后保存才会解除配置。</p><p>保存配置不会自动启动或重启应用。</p></div>
    </Modal>}
    {active && helpField && <Modal title={`${helpField.label}说明`} className="node-field-help-modal" onClose={() => setHelp(null)}>
      <div className="node-field-help">
        {helpField.description.trim() && <p>{helpField.description}</p>}
        {helpField.default !== undefined && <p className="node-field-help-default">留空使用默认值：<strong>{valueLabel(helpField, helpField.default)}</strong></p>}
        {helpField.howToGet.some((step) => step.trim()) && <><h3>填写方法</h3><ol>{helpField.howToGet.filter((step) => step.trim()).map((step, index) => <li key={index}>{step}</li>)}</ol></>}
        {helpField.helpUrl && <button className="refresh-button" type="button" disabled={disabled} onClick={() => onOpenHelp(helpField.name)}><ExternalLink size={14} />打开官方说明</button>}
        <details className="node-field-help-technical"><summary>技术信息</summary><p>变量：<code>{helpField.name}</code>{helpField.endpointField && <> · 关联地址：<code>{helpField.endpointField}</code></>}</p></details>
      </div>
    </Modal>}
  </>;
}
