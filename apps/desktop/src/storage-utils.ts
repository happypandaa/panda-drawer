import type { ByteCount, StoragePart } from './types';
export function bytesText(value: ByteCount | undefined): string {
  if (value === null || value === undefined) return '尚未统计';
  try {
    const bytes = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64);
    if (bytes < 0n) return '未知';
    const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB']; let divisor = 1n; let unit = 0;
    while (bytes >= divisor * 1000n && unit < units.length - 1) { divisor *= 1000n; unit += 1; }
    return unit === 0 ? `${bytes} B` : `${bytes / divisor}.${(bytes % divisor) * 10n / divisor} ${units[unit]}`;
  } catch { return '未知'; }
}
export const partBytes = (part: StoragePart | undefined) => part?.totalBytes ?? part?.bytes ?? part?.sizeBytes;
export function timeText(value: string | number | null | undefined) {
  if (!value) return '未记录'; const date = new Date(value); return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString('zh-CN', { hour12: false });
}
export function percentOf(part: ByteCount | undefined, total: ByteCount | undefined): number | null {
  try {
    const exact = (value: ByteCount | undefined) => value === null || value === undefined ? null : typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64);
    const numerator = exact(part); const denominator = exact(total);
    if (numerator === null || denominator === null || denominator <= 0n || numerator < 0n || numerator > denominator) return null;
    return Number(numerator * 1000n / denominator) / 10;
  } catch { return null; }
}

export function exactBytes(value: ByteCount | undefined): string {
  if (value === null || value === undefined) return '尚未统计';
  try { return `${typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64)} 字节`; } catch { return '未知'; }
}

// Exact non-negative byte counts for sums; null means unknown.
export function bigBytes(value: ByteCount | undefined): bigint | null {
  try { if (value === null || value === undefined) return null; const bytes = typeof value === 'number' ? BigInt(Math.trunc(value)) : BigInt(value.$int64); return bytes >= 0n ? bytes : null; } catch { return null; }
}
export const byteCount = (value: bigint | null): ByteCount => value === null ? null : { $int64: value.toString() };
export const bigText = (value: bigint | null) => bytesText(byteCount(value));
