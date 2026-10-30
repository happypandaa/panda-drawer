import { useSyncExternalStore } from 'react';
import type { ByteCount } from './types';

// The latest history preview for one workspace in this app session. Storage settings
// totals use it without repeating history scans in background polling.
export interface HistoryScanSummary {
  workspaceRoot: string;
  scannedAt: number;
  itemCount: number;
  cleanableCount: number;
  // Sums of measured items; an unmeasured item makes the sum a lower bound.
  totalBytes: bigint;
  totalComplete: boolean;
  cleanableBytes: bigint;
  stale: boolean;
}

let current: HistoryScanSummary | null = null;
const listeners = new Set<() => void>();
const emit = () => { for (const listener of listeners) listener(); };

function exact(value: ByteCount | undefined): bigint | null {
  try { if (value == null) return null; const bytes = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return bytes >= 0n ? bytes : null; } catch { return null; }
}
const sum = (values: (ByteCount | undefined)[]) => values.reduce<bigint>((total, value) => total + (exact(value) ?? 0n), 0n);

export function publishHistoryScan(workspaceRoot: string, items: { kind: string; totalBytes: ByteCount; canClean: boolean; blockedReasons: string[] }[]) {
  const cleanable = items.filter((item) => item.canClean && !item.blockedReasons.length);
  current = {
    workspaceRoot, scannedAt: Date.now(), itemCount: items.length, cleanableCount: cleanable.length,
    totalBytes: sum(items.map((item) => item.totalBytes)), totalComplete: items.every((item) => exact(item.totalBytes) !== null), cleanableBytes: sum(cleanable.map((item) => item.totalBytes)),
    stale: false,
  };
  emit();
}
// After a cleanup the listed sizes no longer describe the disk.
export function markHistoryScanStale() { if (current && !current.stale) { current = { ...current, stale: true }; emit(); } }
export function resetHistoryScanForTests() { current = null; emit(); }
function subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; }
export function useHistoryScan(workspaceRoot: string | null): HistoryScanSummary | null {
  const value = useSyncExternalStore(subscribe, () => current, () => current);
  return value && value.workspaceRoot === workspaceRoot ? value : null;
}
