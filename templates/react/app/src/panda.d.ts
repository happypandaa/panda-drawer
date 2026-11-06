/** Panda browser SDK, API 1. Shared by the served SDK, templates and optional package. */
declare namespace Panda {
  type SqlValue = null | string | number | bigint | boolean | Uint8Array;
  type SqlOutput = null | string | number | bigint | Uint8Array;
  interface SqlOptions { maxRows?: number; timeoutMs?: number }
  interface SqlStatement { sql: string; params?: readonly SqlValue[] }
  interface SqlRequest extends SqlStatement { options?: SqlOptions }
  interface TransactionRequest { statements: readonly SqlStatement[]; options?: SqlOptions }
  interface SqlResult<Row extends Record<string, SqlOutput> = Record<string, SqlOutput>> {
    columns: Array<{ name: string }>;
    rows: Row[];
    changes: number;
    lastInsertRowid?: number | bigint;
    changed: boolean;
  }
  interface SchemaObject {
    type: string; name: string; tbl_name: string; sql: string | null;
    columns?: Array<Record<string, string | number | null>>;
    foreignKeys?: Array<Record<string, string | number | null>>;
    indexes?: Array<Record<string, unknown>>;
  }
  interface SchemaInfo { tables: SchemaObject[]; indexes: SchemaObject[]; views: SchemaObject[]; triggers: SchemaObject[] }
  interface FileInfo { path: string; size: number; mediaType: string; modifiedAt: number }
  interface NetworkRequirement { origin: string; purpose: string; dataSent: string }
  interface ProjectContext {
    projectId: string;
    /** This application's absolute root directory, in the system's native path format. */
    projectRoot: string;
    documentEpoch: string;
    runtimePolicyFingerprint?: string;
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
  class PlatformError extends Error {
    readonly name: 'PlatformError';
    readonly code: string;
    readonly details: Record<string, unknown>;
    readonly retryable: boolean;
    readonly requestId?: string;
    constructor(code: string, message: string, details?: Record<string, unknown>, retryable?: boolean);
  }
  type PlatformEvent =
    | { type: 'data.changed'; projectId: string; source: string; requestId?: string }
    | { type: 'project.updated'; projectId: string; source: 'build'; taskId: string }
    | { type: 'error'; error: PlatformError };
  interface Platform {
    readonly PlatformError: typeof PlatformError;
    /** Public typed values only. Works without platform.sdkApiVersion. */
    config<Values extends Record<string, unknown> = Record<string, unknown>>(): Promise<Values>;
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
}
interface Window { readonly panda: Panda.Platform }
declare const panda: Panda.Platform;
