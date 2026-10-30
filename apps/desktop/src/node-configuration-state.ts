import type { NodeAppDetail } from './node-app-types';
import { validateNodeConfiguration } from './node-configuration-validation';

/** Presentation only. canStart remains the backend's complete execution gate:
 * it also includes runtime/cleanup/source restrictions, not just configuration. */
export function nodeConfigurationState(app: NodeAppDetail) {
  const configuration = app.configuration;
  const draft = {
    ordinary: Object.fromEntries(app.fields.filter(field => !field.secret).map(field => [field.name, Object.hasOwn(configuration.values, field.name) ? String(configuration.values[field.name]) : ''])),
    secrets: {},
  };
  const fieldIssues = validateNodeConfiguration(app, draft);
  const endpointChanged = Object.values(configuration.secretStatus).some(status => status === 'endpoint-changed');
  // The DTO cannot distinguish an unset optional secret from a lost, previously
  // bound optional credential. Preserve that uncertainty while start is blocked.
  const uncertainSecret = !configuration.canStart && app.fields.some(field => field.secret && !field.required && configuration.secretStatus[field.name] === 'missing');
  const kind = configuration.unavailable ? 'unavailable'
    : endpointChanged ? 'secret-target'
    : configuration.missingRequired.length ? 'required'
    : configuration.invalidFields?.length || Object.keys(fieldIssues).length ? 'invalid'
    : uncertainSecret ? 'uncertain-secret' : 'complete';
  const needsAttention = ['required', 'invalid', 'secret-target', 'uncertain-secret'].includes(kind);
  const attentionLabel = kind === 'unavailable' ? '配置暂不可用' : kind === 'uncertain-secret' ? '配置待核对' : needsAttention ? '需要配置' : null;
  const overviewText = kind === 'unavailable' ? '暂不可核对'
    : kind === 'secret-target' ? '密钥地址需确认'
    : kind === 'required' ? `缺少 ${configuration.missingRequired.length} 项必填`
    : kind === 'invalid' ? '需要核对配置'
    : kind === 'uncertain-secret' ? '配置待核对'
    : `已完整${app.fields.length ? `（${app.fields.length} 项）` : ''}`;
  const blockedView: 'configuration' | 'details' = !app.runRecovery && needsAttention && !(kind === 'uncertain-secret' && (app.state === 'failed' || Boolean(app.lastError))) ? 'configuration' : 'details';
  return { kind, needsAttention, attentionLabel, overviewText, blockedView,
    actionLabel: kind === 'required' ? '补齐应用配置' : needsAttention ? '核对应用配置' : '查看或修改应用配置' };
}
