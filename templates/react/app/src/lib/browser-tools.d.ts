import type { ProjectContext } from '@local-projects/sdk'
export const AUDIO_MIME_TYPES: readonly string[]
export const DEFAULT_MEDIA_LIMITS: Readonly<{ maxDurationMs: number; maxBytes: number }>
export interface MediaAsset { blob: Blob; mimeType: string; extension: string; size: number; durationMs: number | null; name: string }
export interface MediaState { phase: 'idle' | 'requesting' | 'recording' | 'camera' | 'preview' | 'error'; reason: string; asset: MediaAsset | null; stream: MediaStream | null; error?: string }
export interface SaveState { phase: 'uploading' | 'upload-failed' | 'file-saved' | 'record-failed' | 'saved'; path: string; file?: unknown; metadata?: Omit<MediaAsset, 'blob'> & { path: string }; error?: unknown; outcomeUnknown?: boolean }
export interface MediaTools {
 readonly state: MediaState
 updateContext(context?: ProjectContext): void
 startRecording(): Promise<void>
 startCamera(): Promise<MediaStream>
 takePhoto(video: HTMLVideoElement): Promise<MediaAsset>
 selectFile(file: File): MediaAsset
 stop(reason?: string): void
 previewUrl(asset?: MediaAsset): string
 revokeUrl(url: string): void
 discard(): void
 dispose(): void
}
export function createMediaTools(options?: { context?: ProjectContext; onChange?: (state: MediaState) => void; environment?: typeof globalThis; maxDurationMs?: number; maxBytes?: number }): MediaTools
export function extensionForMime(mime: string): string
export function explainMediaError(error: unknown): string
export function saveMediaAsset(options: { asset: MediaAsset; path: string; upload: (path: string, blob: Blob) => Promise<unknown>; writeRecord: (metadata: Omit<MediaAsset, 'blob'> & { path: string }) => Promise<unknown>; onState?: (state: SaveState) => void }): Promise<SaveState>
export function droppedFiles(event: { preventDefault(): void; dataTransfer: DataTransfer | null }): File[]
export function copyTextFromGesture(text: string, environment?: typeof globalThis): Promise<{ copied: boolean; fallbackText: string }>
export function downloadBlobFromGesture(blob: Blob, filename: string, environment?: typeof globalThis): void
