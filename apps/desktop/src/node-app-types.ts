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
}

export interface NodeAppDeleteResult {
  appId: string;
  operationOutcome: 'deleted';
  ownership: 'managed' | 'external';
  sourcePreserved: true;
  evidenceRetained: true;
  metadataCleanupPending?: boolean;
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
  id: string;
  name: string;
  sourcePath: string;
  objectLayout?: ObjectLayout | null;
  dataContract?: NodeDataContract | null;
  installation?: { kind: 'managed'; generation: string; sourceId: string; sourceDigest: string } | null;
  runtime: 'node-service';
  window?: { width: number; height: number; rememberSize?: boolean } | null;
  state: 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';
  restartRequired: boolean;
  runningRevision: number | null;
  lastError: string | null;
  configuration: {
    revision: number;
    values: Record<string, NodeConfigurationValue>;
    secretStatus: Record<string, 'set' | 'missing' | 'endpoint-changed'>;
    missingRequired: string[];
    canStart: boolean;
  };
  fields: NodeConfigurationField[];
}

export interface NodeAppInspection {
  /** Source-slot preview binding; directory onboarding carries its own revision. */
  expectedRevision?: string;
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
}

export type NodeSecretChange = { action: 'keep' | 'unbind' | 'rebind' } | { action: 'replace'; value: string };

export interface NodeConfigurationInput {
  revision: number;
  values: Record<string, NodeConfigurationValue>;
  secrets: Record<string, NodeSecretChange>;
}
