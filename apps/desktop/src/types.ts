import type { ObjectLayout } from './object-layout-types';
import type { PandaManifest } from './generated/panda-manifest';

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
  platform?: Record<string, unknown>;
}

export interface ProjectInfo {
  id: string;
  name: string;
  description: string;
  projectRoot: string;
  documentEpoch: string;
  runtime: 'web';
  ownership: 'user' | 'platform';
  gitSource?: { url: string; branch: string | null } | null;
  state: 'ready' | 'needsBuild' | 'entryMissing' | 'needsConfiguration' | 'missing' | 'error' | 'maintenance' | 'recoveryRequired';
  issues: string[];
  objectLayout?: ObjectLayout | null;
  buildInfo?: BuildInfo;
  webRuntime?: { state: 'starting' | 'running' | 'closing' | 'closed' | 'failed'; error?: { code: string; message: string; details?: Record<string, unknown> } | null };
  iconDataUrl?: string | null;
  storage?: ProjectStorage;
  manifest: Extract<PandaManifest, { runtime: 'web' }> | null;
}

export interface OpenProjectResult {
  projectId: string;
  target: 'desktop';
  reused: boolean;
}

export interface BuildInfo {
  declared: boolean;
  script?: string;
  entryExists?: boolean;
  latest?: { task: TaskRecord | null; issues: unknown[] };
}

export interface TaskRecord {
  taskId: string;
  projectId: string | null;
  type: string;
  status: 'queued' | 'running' | 'waitingConfirmation' | 'succeeded' | 'failed' | 'interrupted';
  final: boolean;
  phase: string;
  result: Record<string, unknown> | null;
  error: { code: string; message: string; details?: Record<string, unknown> } | null;
}



export interface EnvironmentInfo {
  ready: boolean;
  node: { path: string; version: string } | null;
  npm: { path: string; version: string } | null;
  selection: 'explicit' | 'managed' | 'detected';
  configuredNodePath: string | null;
  managedNodePath?: string | null;
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
  readyForChanges: boolean;
  recoveryRequired: boolean;
  documentEpoch: string | null;
  lastTaskId?: string | null;
  activeTask: TaskRecord | { taskId: string } | string | null;
  latestBuild?: { task: TaskRecord | null; selection: 'recorded-submission' | 'legacy-modified-time' | 'none' | 'unavailable'; issues: { code: string; message: string }[] };
  pendingBuild?: boolean;
  pendingBuildReason?: string | null;
  lastError?: { code?: string; message?: string } | string | null;
}



export interface RetainedWorkspaceRoot {
  path?: string;
  sourcePath?: string;
  taskId?: string;
  available?: boolean;
  identityMatches?: boolean;
  absent?: boolean;
  reason?: string;
  cleanup?: {
    completed?: boolean;
    status?: string;
  } | null;
  sourceCleanup?: {
    phase: 'awaitingReopen' | 'pending' | 'removed' | 'absent';
    cleanupPending: boolean;
    completed: boolean;
    sourcePath: string;
    error?: TaskRecord['error'];
  } | null;
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
  migrationCleanupTask?: TaskRecord | null;
  retainedRoots: (string | RetainedWorkspaceRoot)[];
}

export type ByteCount = number | { $int64: string } | null;
export interface StoragePart {
  bytes?: ByteCount; sizeBytes?: ByteCount; totalBytes?: ByteCount; complete?: boolean;
  issues?: unknown[]; [key: string]: unknown;
}
export interface ProjectStorage {
  currentProject: StoragePart & { status?: string; scanId?: string; startedAt?: string | number; completedAt?: string | number; stale?: boolean; categories?: Record<string, StoragePart> };
  relatedHistory?: Record<string, StoragePart>;
  activeTask?: string | { taskId: string } | null;
}
export interface StoredFile {
  path: string; sizeBytes?: ByteCount; size?: ByteCount; modifiedAt?: string | number;
  mimeType?: string; mediaType?: string; contentType?: string; displayType?: string;
}
export interface StorageFiles { files: StoredFile[]; nextCursor: string | null; complete: boolean; issues: unknown[]; scope?: unknown; startedAt?: string | number; completedAt?: string | number }
