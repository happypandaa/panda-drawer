import type { TaskRecord, WorkspaceInfo } from './types';

export type LifecycleTasks = Record<string, TaskRecord>;

// A delayed workspace read or queued acknowledgement must never revive a task
// after its durable final result has already arrived.
export function mergeLifecycleTask(current: LifecycleTasks, incoming: TaskRecord): LifecycleTasks {
  if (current[incoming.taskId]?.final && !incoming.final) return current;
  return { ...current, [incoming.taskId]: incoming };
}

function snapshotTask(value: WorkspaceInfo['activeTask']): TaskRecord | null {
  if (value && typeof value === 'object' && 'final' in value && 'type' in value && 'status' in value) return value as TaskRecord;
  return null;
}

function taskId(value: WorkspaceInfo['activeTask']): string | null {
  return typeof value === 'string' ? value : value?.taskId ?? null;
}

function resolveTask(snapshot: TaskRecord | null, known: TaskRecord | undefined, type: string): TaskRecord | null {
  if (known?.type !== type) known = undefined;
  if (snapshot?.type !== type) snapshot = null;
  if (known?.final) return known;
  if (snapshot?.final) return snapshot;
  return known ?? snapshot;
}

function outcome(task: TaskRecord | null): unknown {
  return task?.error?.details?.operationOutcome ?? task?.result?.operationOutcome;
}

export function deriveWorkspaceTaskState(workspace: WorkspaceInfo | null, knownTasks: LifecycleTasks) {
  const known = Object.values(knownTasks);
  const migrationId = taskId(workspace?.activeTask ?? null) ?? workspace?.lastTaskId ?? null;
  const selectedMigration = resolveTask(snapshotTask(workspace?.activeTask ?? null), migrationId ? knownTasks[migrationId] : undefined, 'workspaceMigration');
  const pendingMigrations = known.filter((task) => task.type === 'workspaceMigration' && !task.final && !(
    selectedMigration?.taskId === task.taskId && selectedMigration.final
  ));
  const migrationTask = selectedMigration ?? pendingMigrations.at(-1) ?? null;
  const migrationPhase = workspace?.migrationState ?? '';
  const unresolvedSnapshot = !selectedMigration?.final && (
    Boolean(workspace?.activeTask) || ['running', 'migrating'].includes(migrationPhase)
  );
  const migrating = pendingMigrations.length > 0 || unresolvedSnapshot;
  // "paused" is a real backend access gate. A final task ends the running
  // notice, but committed_needs_attention (or an unknown outcome) cannot prove
  // that a paused workspace is available again.
  const resumed = Boolean(selectedMigration?.final && (
    selectedMigration.status === 'succeeded' || ['not_committed', 'committed'].includes(String(outcome(selectedMigration)))
  ));
  const paused = migrationPhase === 'paused' && !migrating && !resumed;

  const snapshotCleanup = workspace?.cleanupTask ?? null;
  const currentCleanup = resolveTask(snapshotCleanup, snapshotCleanup ? knownTasks[snapshotCleanup.taskId] : undefined, 'workspaceCleanup');
  const pendingCleanups = known.filter((task) => task.type === 'workspaceCleanup' && !task.final && !(
    currentCleanup?.taskId === task.taskId && currentCleanup.final
  ));
  // The snapshot's different active cleanup is authoritative even when a final
  // historical task was inserted into the local map more recently.
  const cleanupTask = currentCleanup && !currentCleanup.final ? currentCleanup
    : pendingCleanups.at(-1) ?? currentCleanup ?? known.filter((task) => task.type === 'workspaceCleanup').at(-1) ?? null;
  const cleanupBusy = pendingCleanups.length > 0 || Boolean(currentCleanup && !currentCleanup.final);
  return { migrating, migrationTask, paused, cleanupBusy, cleanupTask };
}
