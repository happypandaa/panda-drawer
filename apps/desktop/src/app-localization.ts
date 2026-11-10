import type { AppDecl, ConfigurationField, PandaManifest, RuntimePolicy } from './generated/panda-manifest';
import type { Locale } from './i18n';
import type { ProjectInfo } from './types';

export type ManifestLocalization = PandaManifest['localization'];

/** Display projections only. Never write translated metadata back to a manifest or configuration. */
export function localizedApp(app: AppDecl, localization: ManifestLocalization, locale: Locale): AppDecl {
  return { ...app, ...localization?.[locale]?.app };
}

export function localizedFields<T extends ConfigurationField>(fields: T[], localization: ManifestLocalization, locale: Locale): T[] {
  const translations = localization?.[locale]?.configuration;
  if (!translations) return fields;
  return fields.map(field => {
    const translation = translations[field.name];
    if (!translation) return field;
    const { options, ...text } = translation;
    return { ...field, ...text, ...(field.options ? { options: field.options.map(option => ({ ...option, label: options?.[option.value] ?? option.label })) } : {}) };
  });
}

export function localizedRequirements(requirements: RuntimePolicy | undefined, localization: ManifestLocalization, locale: Locale): RuntimePolicy | undefined {
  const translation = localization?.[locale]?.runtimeRequirements;
  if (!requirements || !translation) return requirements;
  const network = { ...requirements.network };
  for (const kind of ['connect', 'images', 'media'] as const) {
    const entries = network[kind];
    if (entries) network[kind] = entries.map(item => ({ ...item, ...translation.network?.[kind]?.[item.origin] }));
  }
  const devices = { ...requirements.devices };
  for (const kind of ['camera', 'microphone'] as const) {
    const requirement = devices[kind];
    if (requirement) devices[kind] = { ...requirement, ...translation.devices?.[kind] };
  }
  return { ...requirements, network, devices };
}

export function localizedProject(project: ProjectInfo, locale: Locale): ProjectInfo {
  const translation = project.manifest?.localization?.[locale]?.app;
  return translation ? { ...project, ...translation } : project;
}
