export type SqlValue = null | string | number | bigint | boolean | Uint8Array;
export type SqlOutput = null | string | number | bigint | Uint8Array;
export interface SqlOptions { maxRows?: number; timeoutMs?: number }
export interface SqlStatement { sql: string; params?: readonly SqlValue[] }
export interface SqlRequest extends SqlStatement { options?: SqlOptions }
export interface TransactionRequest { statements: readonly SqlStatement[]; options?: SqlOptions }
export interface SqlResult<Row extends Record<string, SqlOutput> = Record<string, SqlOutput>> {
  columns: Array<{ name: string }>;
  rows: Row[];
  changes: number;
  lastInsertRowid?: number | bigint;
  changed: boolean;
}
export interface SchemaObject {
  type: string; name: string; tbl_name: string; sql: string | null;
  columns?: Array<Record<string, string | number | null>>;
  foreignKeys?: Array<Record<string, string | number | null>>;
  indexes?: Array<Record<string, unknown>>;
}
export interface SchemaInfo { tables: SchemaObject[]; indexes: SchemaObject[]; views: SchemaObject[]; triggers: SchemaObject[] }
export interface FileInfo { path: string; size: number; mediaType: string; modifiedAt: number }
export interface NetworkRequirement { origin: string; purpose: string; dataSent: string }
export interface ProjectContext {
  projectId: string;
  name: string;
  sdkApiVersion: 1;
  instanceId: string;
  capabilities: string[];
  runtimeRequirements: {
    network?: { connect?: NetworkRequirement[]; images?: NetworkRequirement[]; media?: NetworkRequirement[] };
    devices?: Record<string, unknown>;
  };
  /** Values are reported by the service, not frozen SDK defaults. */
  limits: {
    limitsStatus?: string;
    fileUploadMaxBytes: number;
    jsonBodyMaxBytes?: number;
    sqlDefaultMaxRows?: number;
    sqlMaxRows?: number;
    sqlResultMaxBytes?: number;
    sqlTimeoutMs?: number;
    sqlMaxTimeoutMs?: number;
    transactionMaxStatements?: number;
  };
  state: { mode: string; busy: boolean; reloadRequired: boolean };
}
export class PlatformError extends Error {
  readonly name: 'PlatformError';
  readonly code: string;
  readonly details: Record<string, unknown>;
  readonly retryable: boolean;
  readonly requestId?: string;
  constructor(code: string, message: string, details?: Record<string, unknown>, retryable?: boolean);
}
export type PlatformEvent =
  | { type: 'data.changed'; projectId: string; source: string; requestId?: string }
  | { type: 'project.updated'; projectId: string; source: 'build'; taskId: string }
  | { type: 'error'; error: PlatformError };
export interface Platform {
  context: { get(): Promise<ProjectContext> };
  db: {
    execute<Row extends Record<string, SqlOutput> = Record<string, SqlOutput>>(request: SqlRequest): Promise<SqlResult<Row>>;
    transaction(request: TransactionRequest): Promise<SqlResult[]>;
    schema(): Promise<SchemaInfo>;
    /** Invalidation events require a fresh query. Error and project.updated events never replay writes/reload the page. */
    subscribe(listener: (event: PlatformEvent) => void): () => void;
  };
  files: {
    put(request: { path: string; content: Blob | Uint8Array; overwrite?: boolean }): Promise<FileInfo>;
    get(path: string): Promise<Blob>;
    list(prefix?: string): Promise<FileInfo[]>;
    remove(path: string): Promise<{ removed: boolean; path: string }>;
    url(path: string): Promise<string>;
  };
}
/** The default instance snapshots this document's injected epoch and project URL. */
export const platform: Platform;
/** Optional browser globals are useful for controlled integration tests; never supply credentials. */
export function createPlatform(environment?: {
  document?: Document;
  location?: Pick<Location, 'origin' | 'pathname'>;
  window?: Window;
  fetch?: typeof fetch;
  EventSource?: typeof EventSource;
}): Platform;
