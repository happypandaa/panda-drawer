import type { NodeAppDetail, NodeConfigurationField, NodeConfigurationValue } from './node-app-types';

export type NodeConfigurationDraft = {
  ordinary: Record<string, string>;
  secrets: Record<string, { action: 'keep' | 'replace' | 'unbind' | 'rebind'; value: string }>;
};

type ConfigurationIssue = { message: string; blocking: boolean };
const controlCharacter = /[\u0000-\u001f\u007f-\u009f]/;
const byteLength = (value: string) => new TextEncoder().encode(value).length;

function validUrl(value: string): boolean {
  if (byteLength(value) > 4000 || value.trim() !== value || controlCharacter.test(value) || value.includes('?') || value.includes('#')) return false;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && Boolean(url.hostname) && !url.username && !url.password;
  } catch { return false; }
}

function ordinaryIssue(field: NodeConfigurationField, value: string): string | undefined {
  if (field.type === 'integer') return /^-?\d+$/.test(value) && Number.isSafeInteger(Number(value)) ? undefined : '请填写可精确保存的整数';
  if (field.type === 'boolean') return value === 'true' || value === 'false' ? undefined : '请选择开启或关闭';
  if (field.type === 'select') return field.options?.some((option) => option.value === value) ? undefined : '请选择列表中的选项';
  if (field.type === 'url') return validUrl(value) ? undefined : '请填写不含账号、查询或片段的 HTTPS 地址';
  return byteLength(value) <= 4000 && !controlCharacter.test(value) ? undefined : '内容过长或含有不可用字符';
}

/** Missing required values may be saved as incomplete configuration; invalid
 * values and unconfirmed credential targets may not. The backend remains the
 * authority for saving and starting, including unreadable credential files. */
export function validateNodeConfiguration(app: NodeAppDetail, draft: NodeConfigurationDraft): Record<string, ConfigurationIssue> {
  const issues: Record<string, ConfigurationIssue> = {};
  const effective = new Map<string, NodeConfigurationValue>();
  for (const field of app.fields) {
    if (field.secret) continue;
    const value = draft.ordinary[field.name] ?? '';
    if (!value.trim()) {
      if (field.default !== undefined) effective.set(field.name, field.default);
      else if (field.required) issues[field.name] = { message: field.type === 'select' || field.type === 'boolean' ? '请选择此项' : '请填写此项', blocking: false };
      continue;
    }
    const message = ordinaryIssue(field, value);
    if (message) issues[field.name] = { message, blocking: true };
    effective.set(field.name, field.type === 'integer' ? Number(value) : field.type === 'boolean' ? value === 'true' : value);
  }
  for (const field of app.fields) {
    if (!field.secret) continue;
    const edit = draft.secrets[field.name] ?? { action: 'keep', value: '' };
    const status = app.configuration.secretStatus[field.name];
    const hasSecret = status === 'set' || status === 'endpoint-changed';
    if (edit.action === 'unbind') {
      if (field.required) issues[field.name] = { message: '请填写此项', blocking: false };
      continue;
    }
    if (edit.action === 'replace' && (!edit.value.trim() || byteLength(edit.value) > 8192 || controlCharacter.test(edit.value))) {
      issues[field.name] = { message: '密钥为空、过长或含有不可用字符', blocking: true };
      continue;
    }
    if (edit.action === 'rebind' && !hasSecret) {
      issues[field.name] = { message: '请先填写密钥', blocking: true };
      continue;
    }
    const endpoint = field.endpointField ? effective.get(field.endpointField) : undefined;
    if (field.endpointField && (edit.action === 'replace' || edit.action === 'rebind') && (typeof endpoint !== 'string' || !validUrl(endpoint))) {
      issues[field.name] = { message: '请先填写有效的服务地址', blocking: true };
      continue;
    }
    const endpointField = app.fields.find((candidate) => candidate.name === field.endpointField);
    const previousEndpoint = field.endpointField ? app.configuration.values[field.endpointField] ?? endpointField?.default : undefined;
    if (edit.action === 'keep' && hasSecret && field.endpointField && (status === 'endpoint-changed' || endpoint !== previousEndpoint)) {
      issues[field.name] = { message: '请确认新地址或更换、移除密钥', blocking: true };
    } else if (field.required && edit.action === 'keep' && !hasSecret) {
      issues[field.name] = { message: '请填写此项', blocking: false };
    }
  }
  return issues;
}
