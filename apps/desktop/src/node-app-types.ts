import type { AppDecl, ConfigurationField } from './generated/panda-manifest';
import type { ObjectLayout } from './object-layout-types';

export type NodeConfigurationValue = string | number | boolean;

export interface NodeAppDeletePreview {
  appId: string;
  name: string;
  ownership: 'managed' | 'external';
  platformRoot: string;
  sourcePath: string;
  deletePaths: string[];
  preservedPaths: string[];
  expectedRevision: string;
  blocker?: { code: string; message: string } | null;
}

export interface NodeAppDeleteResult {
  appId: string;
  operationOutcome: 'deleted';
  ownership: 'managed' | 'external' | 'absent';
  sourcePreserved: boolean;
  evidenceRetained: false;
  alreadyAbsent?: boolean;
  metadataCleanupPending?: boolean;
  durabilityConfirmed?: boolean;
  warning?: string | null;
}

export type NodeDeletionPhase = 'waiting' | 'stopping' | 'staging' | 'removing' | 'failed' | 'deleted' | 'abandoned';
export interface NodeDeletionTarget { path: string; stagedPath?: string; state: string; reason?: string; }
export interface NodeDeletionTaskSnapshot {
  taskId: string;
  appId: string;
  name: string;
  targetRevision: string;
  journalRevision: number;
  phase: NodeDeletionPhase;
  active: boolean;
  ownership: 'managed' | 'external' | 'absent';
  sourcePath: string;
  waitingFor: string[];
  renamedAny: boolean;
  canAbandon: boolean;
  canRetry: boolean;
  abandonRequested?: boolean;
  targets: NodeDeletionTarget[];
  remaining: NodeDeletionTarget[];
  preservedPaths: string[];
  error: { code: string; message: string; details?: Record<string, unknown> } | null;
  result: NodeAppDeleteResult | null;
}

export type NodeConfigurationField = ConfigurationField;
export type ConfigurationFormApp = Pick<NodeAppDetail, 'fields' | 'configuration' | 'manifestDigest'>;
export interface WebConfigurationDetail extends ConfigurationFormApp {
  id: string; runtime: 'web'; name: string;
}

export interface NodeStartupProgress {
  phase: 'preparing' | 'initializing' | 'waiting-ready' | 'checking-health' | 'preparing-page' | 'cleanup';
  elapsedMs: number;
  phaseElapsedMs: number;
  timeoutMs: number;
  healthTimeoutMs: number;
}

export interface NodeAppDetail {
  dependencyWarnings?: string[];
  version?: string | null;
  description?: string | null;
  iconDataUrl?: string | null;
  manifestDigest?: string;
  gitSource?: { url: string; branch: string | null } | null;
  ownership?: 'managed' | 'external';
  unavailableReason?: string | null;
  id: string;
  name: string;
  sourcePath: string;
  objectLayout?: ObjectLayout | null;
  runtime: 'node';
  window?: { width: number; height: number; rememberSize?: boolean } | null;
  state: 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';
  startupProgress?: NodeStartupProgress | null;
  runRecovery?: { state: 'orphaned' | 'unconfirmed'; canStop: boolean; message: string } | null;
  restartRequired: boolean;
  runningRevision: number | null;
  lastError: string | null;
  configuration: {
    unavailable?: boolean;
    revision: number;
    values: Record<string, NodeConfigurationValue>;
    secretStatus: Record<string, 'set' | 'missing' | 'endpoint-changed'>;
    missingRequired: string[];
    invalidFields?: string[];
    canStart: boolean;
  };
  fields: NodeConfigurationField[];
}

export interface NodeAppInspection {
  dependencyWarnings?: string[];
  needsBuild?: boolean;
  entryMissing?: boolean;
  runtime: string;
  runtimeLabel: string;
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
  existingApp?: { id: string; name: string; runtime?: 'node' | 'web' };
}

export type NodeSecretChange = { action: 'keep' | 'unbind' | 'rebind' } | { action: 'replace'; value: string };

export interface NodeConfigurationInput {
  revision: number;
  manifestDigest?: string;
  values: Record<string, NodeConfigurationValue>;
  secrets: Record<string, NodeSecretChange>;
}
