import type { ObjectLayout } from './object-layout-types';

export type NodeConfigurationValue = string | number | boolean;

export interface NodeDataContract {
  id: string;
  version: number;
}

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

export interface NodeConfigurationField {
  name: string;
  label: string;
  type: 'string' | 'url' | 'integer' | 'boolean' | 'select';
  required: boolean;
  secret: boolean;
  description: string;
  howToGet: string[];
  placeholder?: string;
  default?: NodeConfigurationValue;
  endpointField?: string;
  helpUrl?: string;
  options?: { value: string; label: string }[];
}

export interface NodeAppDetail {
  version?: string;
  manifestDigest?: string;
  gitSource?: { url: string; branch: string | null } | null;
  ownership?: 'managed' | 'external';
  unavailableReason?: string | null;
  development?: { sourcePath: string; runtimeIsSource: boolean; manifestId: string } | null;
  id: string;
  name: string;
  sourcePath: string;
  objectLayout?: ObjectLayout | null;
  dataContract?: NodeDataContract | null;
  installation?: { kind: 'managed'; generation: string; sourceId: string; sourceDigest: string } | null;
  runtime: 'node-service';
  window?: { width: number; height: number; rememberSize?: boolean } | null;
  state: 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';
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
  runtime: string;
  runtimeLabel: string;
  app: { id: string; name: string; version: string; description?: string };
  dataContract?: NodeDataContract | null;
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
  projectFormat?: 'project-v1';
  existingApp?: { id: string; name: string };
}

export type NodeSecretChange = { action: 'keep' | 'unbind' | 'rebind' } | { action: 'replace'; value: string };

export interface NodeConfigurationInput {
  revision: number;
  manifestDigest?: string;
  values: Record<string, NodeConfigurationValue>;
  secrets: Record<string, NodeSecretChange>;
}
