// Local Projects browser tools 0.1.0. Independent of the fixed SDK package.
// Capture is foreground-only; these helpers cannot govern arbitrary project scripts.
export const AUDIO_MIME_TYPES = ['audio/webm;codecs=opus', 'audio/mp4', 'audio/ogg;codecs=opus', 'audio/webm']
export const DEFAULT_MEDIA_LIMITS = Object.freeze({ maxDurationMs: 60_000, maxBytes: 10 * 1024 * 1024 })

export function extensionForMime(mime) {
  return ({ 'audio/webm': 'webm', 'audio/mp4': 'm4a', 'audio/ogg': 'ogg', 'audio/wav': 'wav', 'video/mp4': 'mp4', 'image/jpeg': 'jpg', 'image/png': 'png', 'image/webp': 'webp' })[mime.split(';')[0].trim().toLowerCase()] ?? 'bin'
}

export function explainMediaError(error) {
  return ({ NotAllowedError: '未获得设备权限。请在浏览器或系统设置中允许访问，或选择已有文件。', NotFoundError: '未找到设备，请选择已有文件。', NotReadableError: '设备暂时不可用，可能正被其他应用占用。', OverconstrainedError: '当前设备无法满足采集要求，也可能暂时不可用。请检查设备，或选择已有文件。', SecurityError: '当前页面不允许使用设备，请从平台重新打开。', AbortError: '操作已停止，未保存。', NotSupportedError: '此浏览器不支持该录制格式，请选择已有文件。' })[error?.name] ?? error?.message ?? '媒体操作失败，请选择已有文件。'
}

function failure(message, name = 'Error') { return Object.assign(new Error(message), { name }) }
function activeGesture(env) {
  if (env.navigator?.userActivation?.isActive === false) throw failure('请通过点击或键盘操作开始。', 'NotAllowedError')
}
function stopTracks(stream) { for (const track of stream?.getTracks?.() ?? []) track.stop() }
function blocked(state) { return state && (state.offline || state.expired || state.reloadRequired || state.busy || state.recoveryRequired || state.trashed || state.workspaceMigrating || state.instanceChanged || (state.mode && !['normal', 'ready'].includes(state.mode))) }

/** All callbacks report local state only. No upload or SQL is performed by capture. */
export function createMediaTools({ context, onChange = () => {}, environment = globalThis, maxDurationMs = DEFAULT_MEDIA_LIMITS.maxDurationMs, maxBytes = DEFAULT_MEDIA_LIMITS.maxBytes } = {}) {
  const env = environment
  let currentContext = context
  let runtimeState = context?.state
  let generation = 0
  let stream
  let recorder
  let recordingStoppedAt
  let suppressRecordingResult = false
  let timer
  let pending = false
  let disposed = false
  let status = { phase: 'idle', reason: '', asset: null, stream: null }
  const urls = new Set()
  const listeners = []
  const limit = () => Math.min(maxBytes, currentContext?.limits?.fileUploadMaxBytes ?? maxBytes)
  const emit = patch => { status = { ...status, ...patch }; if (!disposed) onChange(status) }
  const listen = (target, name, callback) => { target?.addEventListener?.(name, callback); listeners.push(() => target?.removeEventListener?.(name, callback)) }
  const clearTimer = () => { if (timer !== undefined) env.clearTimeout(timer); timer = undefined }
  const assertReady = device => {
    activeGesture(env)
    if (env.isSecureContext === false) throw failure('设备采集需要安全上下文，请从平台的本机地址重新打开。', 'SecurityError')
    if (disposed || env.document?.hidden || blocked(runtimeState)) throw failure('项目当前不能使用设备；处理维护、连接或重载提示后再试。', 'NotAllowedError')
    if (!currentContext?.runtimeRequirements?.devices?.[device]?.purpose?.trim()) throw failure(`项目尚未声明${device === 'camera' ? '摄像头' : '麦克风'}用途。`, 'NotAllowedError')
    if (pending || stream || recorder) throw failure('请先停止当前采集。')
    if (!env.navigator?.mediaDevices?.getUserMedia) throw failure('此页面不支持设备采集，请选择已有文件。', 'NotSupportedError')
  }
  const release = (reason = 'stopped') => {
    if (['file-selected', 'discarded', 'disposed'].includes(reason)) suppressRecordingResult = true
    generation++ // Revokes a late getUserMedia resolution as well as active capture.
    pending = false
    clearTimer()
    const activeRecorder = recorder
    if (activeRecorder && recordingStoppedAt === undefined) recordingStoppedAt = env.performance?.now?.() ?? Date.now()
    stopTracks(stream)
    stream = undefined
    emit({ stream: null, reason })
    if (activeRecorder && activeRecorder.state !== 'inactive') {
      try { activeRecorder.stop() } catch (error) { recorder = undefined; emit({ phase: 'error', error: explainMediaError(error) }) }
    } else if (status.phase === 'requesting' || status.phase === 'camera' || status.phase === 'recording') emit({ phase: status.asset ? 'preview' : 'idle' })
  }
  const acquire = async (device, constraints) => {
    assertReady(device)
    const requestGeneration = ++generation
    pending = true
    emit({ phase: 'requesting', reason: '', error: undefined })
    try {
      const acquired = await env.navigator.mediaDevices.getUserMedia(constraints)
      if (disposed || requestGeneration !== generation || env.document?.hidden || blocked(runtimeState)) {
        stopTracks(acquired)
        throw failure('等待权限期间项目状态发生变化，设备已释放。', 'AbortError')
      }
      stream = acquired
      for (const track of acquired.getTracks()) track.addEventListener?.('ended', () => release('device-ended'), { once: true })
      return acquired
    } catch (error) {
      if (requestGeneration === generation) emit({ phase: 'error', error: explainMediaError(error) })
      throw error
    } finally { if (requestGeneration === generation) pending = false }
  }
  const assertAcquired = acquired => {
    if (disposed || stream !== acquired || env.document?.hidden || blocked(runtimeState)) {
      stopTracks(acquired)
      throw failure('项目状态已变化，设备已释放。', 'AbortError')
    }
  }
  const asset = (blob, durationMs = null, name = '') => {
    if (!(blob instanceof env.Blob) || blob.size === 0) throw failure('没有可保存的媒体内容。')
    if (blob.size > limit()) throw failure(`文件超过当前 ${limit()} 字节限制。`)
    const mimeType = blob.type || 'application/octet-stream'
    return { blob, mimeType, extension: extensionForMime(mimeType), size: blob.size, durationMs, name }
  }
  const publish = value => { emit({ phase: 'preview', asset: value, stream: null, error: undefined }); return value }
  const api = {
    get state() { return status },
    updateContext(value) { currentContext = value; runtimeState = { ...runtimeState, ...value?.state }; if (blocked(runtimeState)) release('project-state') },
    async startRecording() {
      if (!env.MediaRecorder || typeof env.MediaRecorder.isTypeSupported !== 'function') throw failure('浏览器不支持录音，请选择已有音频文件。', 'NotSupportedError')
      const acquired = await acquire('microphone', { audio: true, video: false })
      assertAcquired(acquired)
      recordingStoppedAt = undefined
      suppressRecordingResult = false
      let chunks = []
      let bytes = 0
      let oversized = false
      let failed = false
      const started = env.performance?.now?.() ?? Date.now()
      try {
        const preferred = AUDIO_MIME_TYPES.find(type => env.MediaRecorder.isTypeSupported(type))
        const activeRecorder = new env.MediaRecorder(acquired, preferred ? { mimeType: preferred } : undefined)
        recorder = activeRecorder
        activeRecorder.ondataavailable = event => {
          if (!event.data?.size || oversized || failed) return
          bytes += event.data.size
          if (bytes > limit()) { oversized = true; chunks = []; release('byte-limit'); return }
          chunks.push(event.data)
        }
        activeRecorder.onerror = event => { failed = true; chunks = []; release('recorder-error'); emit({ phase: 'error', error: explainMediaError(event.error) }) }
        activeRecorder.onstop = () => {
          clearTimer()
          stopTracks(acquired)
          if (stream === acquired) stream = undefined
          if (recorder === activeRecorder) recorder = undefined
          if (suppressRecordingResult) { chunks = []; return }
          if (oversized) { emit({ phase: 'error', stream: null, error: '录音超过字节限制，未保存；请缩短后重录。' }); return }
          if (failed || disposed) return
          try {
            const mime = activeRecorder.mimeType || chunks.find(chunk => chunk.type)?.type || ''
            const blob = new env.Blob(chunks, { type: mime })
            const elapsed = Math.max(0, (recordingStoppedAt ?? env.performance?.now?.() ?? Date.now()) - started)
            publish(asset(blob, Math.round(elapsed)))
          } catch (error) { emit({ phase: 'error', stream: null, error: explainMediaError(error) }) }
          finally { chunks = [] }
        }
        activeRecorder.start(250)
        emit({ phase: 'recording', stream: acquired, asset: null })
        timer = env.setTimeout(() => release('time-limit'), maxDurationMs)
      } catch (error) { recorder = undefined; stopTracks(acquired); stream = undefined; emit({ phase: 'error', stream: null, error: explainMediaError(error) }); throw error }
    },
    async startCamera() {
      const acquired = await acquire('camera', { video: { facingMode: 'environment' }, audio: false })
      assertAcquired(acquired)
      emit({ phase: 'camera', stream: acquired, asset: null })
      timer = env.setTimeout(() => release('time-limit'), maxDurationMs)
      return acquired
    },
    async takePhoto(video) {
      activeGesture(env)
      if (!stream || status.phase !== 'camera' || blocked(runtimeState) || env.document?.hidden) throw failure('请先打开摄像头预览。')
      if (!video.videoWidth || !video.videoHeight) throw failure('画面还未准备好，请稍后再拍。')
      const requestGeneration = generation
      const canvas = env.document.createElement('canvas')
      const scale = Math.min(1, 1920 / Math.max(video.videoWidth, video.videoHeight))
      canvas.width = Math.max(1, Math.round(video.videoWidth * scale))
      canvas.height = Math.max(1, Math.round(video.videoHeight * scale))
      canvas.getContext('2d').drawImage(video, 0, 0, canvas.width, canvas.height)
      const blob = await new Promise(resolve => canvas.toBlob(resolve, 'image/jpeg', 0.9))
      if (requestGeneration !== generation || disposed) throw failure('采集已经停止。', 'AbortError')
      release('photo-taken')
      return publish(asset(blob))
    },
    selectFile(file) { release('file-selected'); return publish(asset(file, null, file.name ?? '')) },
    stop: release,
    previewUrl(value = status.asset) { if (!value) return ''; const url = env.URL.createObjectURL(value.blob); urls.add(url); return url },
    revokeUrl(url) { if (urls.delete(url)) env.URL.revokeObjectURL(url) },
    discard() { release('discarded'); for (const url of urls) env.URL.revokeObjectURL(url); urls.clear(); emit({ phase: 'idle', asset: null, error: undefined }) },
    dispose() { if (disposed) return; release('disposed'); disposed = true; for (const remove of listeners) remove(); for (const url of urls) env.URL.revokeObjectURL(url); urls.clear(); status = { phase: 'idle', reason: 'disposed', asset: null, stream: null } },
  }
  listen(env.document, 'visibilitychange', () => { if (env.document.hidden) release('hidden') })
  listen(env, 'pagehide', () => release('pagehide'))
  listen(env, 'beforeunload', () => release('leaving'))
  listen(env, 'offline', () => { runtimeState = { ...runtimeState, offline: true }; release('offline') })
  listen(env, 'platform:state', event => { runtimeState = { ...runtimeState, ...event.detail }; if (blocked(runtimeState)) release('project-state') })
  return api
}

/** Never retries writes. A response-lost upload/SQL may already have committed. */
export async function saveMediaAsset({ asset, path, upload, writeRecord, onState = () => {} }) {
  onState({ phase: 'uploading', path })
  let file
  try { file = await upload(path, asset.blob) }
  catch (error) { const state = { phase: 'upload-failed', path, error, outcomeUnknown: true }; onState(state); return state }
  onState({ phase: 'file-saved', path, file })
  const metadata = { path, mimeType: asset.mimeType, extension: asset.extension, size: asset.size, durationMs: asset.durationMs, name: asset.name }
  try { await writeRecord(metadata); const state = { phase: 'saved', path, file, metadata }; onState(state); return state }
  catch (error) { const state = { phase: 'record-failed', path, file, metadata, error, outcomeUnknown: true }; onState(state); return state }
}

export function droppedFiles(event) { event.preventDefault(); return Array.from(event.dataTransfer?.files ?? []) }

/** Call synchronously from a click. On denial, display/select fallbackText. */
export async function copyTextFromGesture(text, environment = globalThis) {
  try { activeGesture(environment); if (!environment.navigator?.clipboard?.writeText) throw failure('Clipboard unavailable'); await environment.navigator.clipboard.writeText(text); return { copied: true, fallbackText: '' } }
  catch { return { copied: false, fallbackText: text } }
}

/** Ordinary anchor download, with no File System Access API dependency. */
export function downloadBlobFromGesture(blob, filename, environment = globalThis) {
  activeGesture(environment)
  const url = environment.URL.createObjectURL(blob)
  const anchor = environment.document.createElement('a')
  anchor.href = url; anchor.download = filename; anchor.rel = 'noopener'
  environment.document.body.append(anchor)
  try { anchor.click() } finally { anchor.remove(); environment.setTimeout(() => environment.URL.revokeObjectURL(url), 1000) }
}
