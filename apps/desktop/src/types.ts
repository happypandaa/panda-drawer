export interface BootstrapInfo {
  displayName: string;
  identifier: string;
  environment: 'development';
  apiVersion: number;
  instanceId: string;
  baseUrl: string;
  workspaceRoot: string;
  dataRoot: string;
  sqliteVersion: string;
  version: string;
  resources: {
    sdk: boolean;
    template: boolean;
    readme: boolean;
    cli: boolean;
  };
  cliPath: string | null;
}

export interface ProjectInfo {
  id: string;
  name: string;
  description: string;
  projectRoot: string;
  documentEpoch: string;
  state: 'ready' | 'unbuilt' | 'missing' | 'error' | 'maintenance' | 'recoveryRequired';
  issues: string[];
  buildInfo?: BuildInfo;
  manifest: {
    database: string;
    attachmentsDir: string;
    sourceDir: string;
    outputDir: string;
    docs: { readme: string; database: string; platform: string };
  } | null;
}

export interface OpenProjectResult {
  projectId: string;
  target: 'desktop' | 'browser';
  reused: boolean;
}

export interface BuildInfo {
  origin?: 'bundledTemplate' | 'projectBuild';
  templateVersion?: string;
  sourceState: 'unbuilt' | 'matched' | 'modified' | 'unknown';
  checkedAt?: string | number | null;
  stale?: boolean;
  reason?: string | null;
  provenance?: { buildId?: string; taskId?: string; completedAt: number; node?: { path: string; version: string }; npm?: { path: string; version: string }; nodeVersion?: string; npmVersion?: string; sdkVersion?: string | null } | null;
}

export interface TaskRecord {
  taskId: string;
  projectId: string | null;
  type: string;
  status: 'queued' | 'running' | 'succeeded' | 'failed' | 'interrupted';
  final: boolean;
  phase: string;
  result: Record<string, unknown> | null;
  error: { code: string; message: string; details?: Record<string, unknown> } | null;
}

export interface TaskLogs {
  taskId: string;
  cursor: number;
  nextCursor: number;
  text: string;
  truncated: boolean;
  gaps: { from: number; to: number }[];
  eof: boolean;
}

export interface EnvironmentInfo {
  ready: boolean;
  node: { path: string; version: string } | null;
  npm: { path: string; version: string } | null;
  selection: 'explicit' | 'detected';
  configuredNodePath: string | null;
  supportedNodeRange: string;
  recommendedNode: string;
  downloadUrl: string;
  issues: { code: string; message: string }[];
}

export interface MaintenanceStatus {
  projectId?: string;
  mode: 'ready' | 'maintenance';
  maintenanceId: string | null;
  reason: string | null;
  startedAt: string | number | null;
  baselineBackupId: string | null;
  readyForChanges: boolean;
  recoveryRequired: boolean;
  documentEpoch: string;
  lastTaskId?: string | null;
  activeTask: TaskRecord | { taskId: string } | string | null;
  pendingBuild?: boolean;
  pendingBuildReason?: string | null;
  lastError?: { code?: string; message?: string } | string | null;
}

export interface BackupSummary {
  backupId: string;
  projectId?: string;
  createdAt: string | number | null;
  kind: string;
  sizeBytes: number;
  verified: boolean;
  protectionReasons: string[];
  note?: string | null;
  issues?: string[];
}

export interface BackupDetails extends BackupSummary {
  formatVersion?: number;
  platformVersion?: string;
  sqliteVersion?: string;
  sdkApiVersion?: number;
  snapshotState?: string;
  hasArtifact?: boolean;
  pendingBuild?: { required: boolean; reason?: string | null };
  buildProvenance?: Record<string, unknown> | null;
  files?: { path: string; sizeBytes?: number; bytes?: number; sha256: string }[];
  excludedPaths?: string[];
}

export interface TrashEntry {
  trashId: string;
  projectId: string;
  name: string;
  deletedAt: string | number | null;
  sizeBytes: number | null;
  mode: string;
  state: 'trashed' | 'purgeIncomplete' | 'needsAttention';
  path?: string;
  issues?: string[];
  lastTaskId?: string | null;
  activeTask?: string | { taskId: string } | null;
}
export interface WorkspaceInfo {
  workspaceRoot: string;
  formatVersion: number;
  filesystem: string | null;
  sizeBytes?: number | null;
  availableBytes?: number | null;
  migrationState: string;
  activeTask: string | { taskId: string } | null;
  lastTaskId?: string | null;
  retainedRoots: (string | { path?: string; sourcePath?: string })[];
}
