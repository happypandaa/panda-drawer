import { t } from './i18n';
import type { NodeConfigurationDraft } from './node-configuration-validation';
import type { ConfigurationFormApp, NodeConfigurationInput, NodeSecretChange } from './node-app-types';
type Draft = NodeConfigurationDraft;
export function draftFrom(app: ConfigurationFormApp): Draft {
  return {
    ordinary: Object.fromEntries(app.fields.filter((field) => !field.secret).map((field) => [field.name, Object.hasOwn(app.configuration.values, field.name) ? String(app.configuration.values[field.name]) : ''])),
    secrets: Object.fromEntries(app.fields.filter((field) => field.secret).map((field) => [field.name, { action: 'keep', value: '' }])),
  };
}

export function nodeConfigurationInput(app: ConfigurationFormApp, draft: Draft): NodeConfigurationInput {
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
      if (!/^-?\d+$/.test(value) || !Number.isSafeInteger(Number(value))) throw new Error(t("「{p0}」请填写可精确保存的整数（绝对值不超过 9007199254740991）。", { p0: String(field.label) }));
      values[field.name] = Number(value);
    } else if (field.type === 'boolean') {
      if (value !== 'true' && value !== 'false') throw new Error(t("「{p0}」请选择开启或关闭。", { p0: String(field.label) }));
      values[field.name] = value === 'true';
    } else {
      values[field.name] = value;
    }
  }
  return { revision: app.configuration.revision, ...(app.manifestDigest ? { manifestDigest: app.manifestDigest } : {}), values, secrets };
}
