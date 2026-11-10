import type { AppDecl, ConfigurationField } from './generated/panda-manifest';
import type { ManifestLocalization } from './app-localization';

export type NodeConfigurationValue = string | number | boolean;

export type NodeConfigurationField = ConfigurationField;
export interface ConfigurationFormApp {
  localization?: ManifestLocalization;
  fields: NodeConfigurationField[];
  manifestDigest?: string;
  configuration: {
    unavailable?: boolean;
    revision: number;
    values: Record<string, NodeConfigurationValue>;
    secretStatus: Record<string, 'set' | 'missing' | 'endpoint-changed'>;
    missingRequired: string[];
    invalidFields?: string[];
    canStart: boolean;
  };
}
export interface WebConfigurationDetail extends ConfigurationFormApp {
  id: string; name: string; hasBackend?: boolean;
}

export interface NodeStartupProgress {
  phase: 'preparing' | 'initializing' | 'waiting-ready' | 'checking-health' | 'preparing-page' | 'cleanup';
  elapsedMs: number;
  phaseElapsedMs: number;
  timeoutMs: number;
  healthTimeoutMs: number;
}

export interface NodeAppInspection {
  localization?: ManifestLocalization;
  dependencyWarnings?: string[];
  needsBuild?: boolean;
  entryMissing?: boolean;
  hasBackend?: boolean;
  app: AppDecl;
  configuration: { fields: NodeConfigurationField[]; missingRequired: string[] };
}

export interface NodeDirectoryReview {
  sourcePath: string;
  state: 'empty' | 'ready' | 'needs-review';
  expectedRevision: string;
  inspection?: NodeAppInspection;
  issues: string[];
  reviewPrompt: string;
  category?: 'needs-changes' | 'unsupported' | 'insufficient';
  reasonCodes?: string[];
  existingApp?: { id: string; name: string; hasBackend?: boolean };
}

export type NodeSecretChange = { action: 'keep' | 'unbind' | 'rebind' } | { action: 'replace'; value: string };

export interface NodeConfigurationInput {
  revision: number;
  manifestDigest?: string;
  values: Record<string, NodeConfigurationValue>;
  secrets: Record<string, NodeSecretChange>;
}
