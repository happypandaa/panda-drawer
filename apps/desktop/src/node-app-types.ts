import type { ObjectLayout } from './object-layout-types';

export type NodeConfigurationValue = string | number | boolean;

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
  runtime: string;
  runtimeLabel: string;
  app: { id: string; name: string; version: string; description?: string };
  configuration: { fields: NodeConfigurationField[]; missingRequired: string[] };
}

export type NodeSecretChange = { action: 'keep' | 'unbind' | 'rebind' } | { action: 'replace'; value: string };

export interface NodeConfigurationInput {
  revision: number;
  values: Record<string, NodeConfigurationValue>;
  secrets: Record<string, NodeSecretChange>;
}
