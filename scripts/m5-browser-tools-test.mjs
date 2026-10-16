import test from 'node:test'
import assert from 'node:assert/strict'
import { createMediaTools, saveMediaAsset, copyTextFromGesture, downloadBlobFromGesture, droppedFiles, extensionForMime } from '../templates/react/app/src/lib/browser-tools.js'

const context = () => ({ runtimeRequirements: { devices: { microphone: { purpose: 'Audio note' }, camera: { purpose: 'Receipt photo' } } }, limits: { fileUploadMaxBytes: 100 }, state: { mode: 'normal', busy: false, reloadRequired: false } })
const tick = () => new Promise(resolve => queueMicrotask(resolve))
function harness(options = {}) {
  const env = new EventTarget()
  const document = new EventTarget()
  document.hidden = false
  const captures = []
  const recorders = []
  const timers = new Map()
  const revoked = []
  const created = []
  const canvasDraws = []
  let clock = 0
  let timerId = 0
  const track = () => Object.assign(new EventTarget(), { stops: 0, stop() { this.stops++ } })
  const stream = () => { const tracks = [track()]; return { getTracks: () => tracks } }
  class Recorder {
    static isTypeSupported(type) { return options.supported?.includes(type) ?? type === 'audio/mp4' }
    constructor(source, opts) {
      if (options.constructorError) throw new DOMException('Cannot record', 'NotSupportedError')
      this.stream = source; this.options = opts; this.mimeType = options.actualMime ?? opts?.mimeType ?? 'audio/mp4'; this.state = 'inactive'; recorders.push(this)
    }
    start(timeslice) { this.timeslice = timeslice; this.state = 'recording' }
    data(value, mime = this.mimeType) { this.ondataavailable?.({ data: new Blob([value], { type: mime }) }) }
    stop() { this.state = 'inactive'; queueMicrotask(() => { if (options.finalChunk) this.data(options.finalChunk); this.onstop?.() }) }
  }
  const anchors = []
  const canvas = { width: 0, height: 0, getContext: () => ({ drawImage: (...args) => canvasDraws.push(args) }), toBlob(callback, mime) { callback(new Blob(['photo'], { type: mime })) } }
  document.createElement = tag => tag === 'canvas' ? canvas : (() => { const anchor = { clickCount: 0, removeCount: 0, click() { this.clickCount++ }, remove() { this.removeCount++ } }; anchors.push(anchor); return anchor })()
  document.body = { append() {} }
  Object.assign(env, {
    document, Blob, isSecureContext: true,
    navigator: { userActivation: { isActive: true }, mediaDevices: { async getUserMedia(constraints) { captures.push(constraints); if (options.getUserMedia) return options.getUserMedia(constraints); return stream() } }, clipboard: { async writeText() {} } },
    MediaRecorder: Recorder,
    URL: { createObjectURL(blob) { const url = `blob:mock/${created.length}`; created.push({ url, blob }); return url }, revokeObjectURL(url) { revoked.push(url) } },
    performance: { now: () => clock },
    setTimeout(fn, ms) { const id = ++timerId; timers.set(id, { fn, ms }); return id },
    clearTimeout(id) { timers.delete(id) },
  })
  const dispatch = state => { const event = new Event('platform:state'); event.detail = state; env.dispatchEvent(event) }
  const advance = ms => { clock += ms; const due = [...timers]; timers.clear(); for (const [,timer] of due) timer.fn() }
  return { env, captures, recorders, timers, revoked, created, anchors, canvas, canvasDraws, stream, dispatch, advance }
}

test('no device request at construction; declarations and user gesture required', async () => {
  const h = harness(); const tools = createMediaTools({ environment: h.env, context: { ...context(), runtimeRequirements: {} } })
  assert.equal(h.captures.length, 0)
  await assert.rejects(tools.startRecording(), /尚未声明/)
  tools.updateContext(context()); h.env.navigator.userActivation.isActive = false
  await assert.rejects(tools.startCamera(), /点击/)
  assert.equal(h.captures.length, 0); tools.dispose()
})

test('recording detects supported MIME and persists the actual type and extension', async () => {
  const h = harness({ supported: ['audio/webm;codecs=opus'], actualMime: 'audio/mp4' })
  const tools = createMediaTools({ environment: h.env, context: context() })
  await tools.startRecording()
  assert.deepEqual(h.captures, [{ audio: true, video: false }])
  assert.equal(h.recorders[0].options.mimeType, 'audio/webm;codecs=opus')
  h.recorders[0].data('hello'); tools.stop(); await tick()
  assert.equal(tools.state.asset.mimeType, 'audio/mp4'); assert.equal(tools.state.asset.extension, 'm4a')
  assert.equal(tools.state.asset.size, 5); assert.ok(h.recorders[0].stream.getTracks()[0].stops > 0)
  const url = tools.previewUrl(); tools.dispose(); assert.deepEqual(h.revoked, [url])
})

test('pending permission canceled by maintenance releases a late stream', async () => {
  let resolve
  const h = harness({ getUserMedia: () => new Promise(done => { resolve = done }) })
  const tools = createMediaTools({ environment: h.env, context: context() })
  const pending = tools.startCamera(); const rejection = assert.rejects(pending, { name: 'AbortError' })
  h.dispatch({ mode: 'maintenance' }); const late = h.stream(); resolve(late); await rejection
  assert.equal(late.getTracks()[0].stops, 1); assert.equal(tools.state.stream, null); tools.dispose()
})

test('foreground lifecycle and every blocked project state release devices', async () => {
  for (const state of [{ busy: true }, { recoveryRequired: true }, { trashed: true }, { workspaceMigrating: true }, { instanceChanged: true }, { mode: 'maintenance' }, { mode: 'trashed' }, { mode: 'migrating' }, { offline: true }, { expired: true }, { reloadRequired: true }]) {
    const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() })
    const stream = await tools.startCamera(); h.dispatch(state)
    assert.ok(stream.getTracks()[0].stops > 0, JSON.stringify(state)); assert.equal(tools.state.stream, null)
    await assert.rejects(tools.startCamera(), /当前不能/); tools.dispose()
  }
  for (const eventName of ['visibilitychange', 'pagehide', 'beforeunload', 'offline']) {
    const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() }); const stream = await tools.startCamera()
    if (eventName === 'visibilitychange') { h.env.document.hidden = true; h.env.document.dispatchEvent(new Event(eventName)) }
    else h.env.dispatchEvent(new Event(eventName))
    assert.ok(stream.getTracks()[0].stops > 0, eventName); tools.dispose()
  }
})

test('blur leaves foreground recording alone; hidden stops and retains local preview', async () => {
  const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() }); await tools.startRecording()
  h.recorders[0].data('voice'); h.env.dispatchEvent(new Event('blur')); assert.equal(tools.state.phase, 'recording')
  h.env.document.hidden = true; h.env.document.dispatchEvent(new Event('visibilitychange')); await tick()
  assert.equal(tools.state.phase, 'preview'); assert.equal(tools.state.asset.size, 5); tools.dispose()
})

test('time cap stops capture; byte cap includes final chunk and never exposes oversized asset', async () => {
  const h = harness(); const tools = createMediaTools({ environment: h.env, context: context(), maxDurationMs: 500 })
  await tools.startRecording(); h.recorders[0].data('voice'); assert.equal([...h.timers.values()][0].ms, 500)
  h.advance(500); await tick(); assert.equal(tools.state.asset.durationMs, 500); assert.equal(tools.state.reason, 'time-limit'); tools.dispose()
  const overflow = harness({ finalChunk: '0123456789' }); const other = createMediaTools({ environment: overflow.env, context: context(), maxBytes: 8 })
  await other.startRecording(); overflow.recorders[0].data('123'); other.stop(); await tick()
  assert.equal(other.state.phase, 'error'); assert.equal(other.state.asset, null); assert.match(other.state.error, /字节/); other.dispose()
})

test('platform upload limit is applied to recordings and picked files', async () => {
  const h = harness(); const ctx = context(); ctx.limits.fileUploadMaxBytes = 4
  const tools = createMediaTools({ environment: h.env, context: ctx })
  assert.throws(() => tools.selectFile(new File(['12345'], 'large.txt')), /限制/)
  h.env.navigator.userActivation.isActive = false // A completed native file dialog need not retain transient activation.
  assert.equal(tools.selectFile(new File(['1234'], 'small.txt')).name, 'small.txt'); tools.dispose()
})

test('recorder initialization failure releases the granted microphone', async () => {
  let granted
  const h = harness({ constructorError: true, getUserMedia: async () => { granted = h.stream(); return granted } })
  const tools = createMediaTools({ environment: h.env, context: context() })
  await assert.rejects(tools.startRecording(), { name: 'NotSupportedError' }); assert.equal(granted.getTracks()[0].stops, 1); tools.dispose()
})

test('camera uses no microphone, canvas capture uses actual blob MIME and closes camera', async () => {
  const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() })
  const stream = await tools.startCamera(); assert.equal(h.captures[0].audio, false)
  const photo = await tools.takePhoto({ videoWidth: 4000, videoHeight: 2000 })
  assert.equal(h.canvas.width, 1920); assert.equal(h.canvas.height, 960); assert.equal(h.canvasDraws.length, 1)
  assert.equal(photo.mimeType, 'image/jpeg'); assert.equal(photo.extension, 'jpg'); assert.equal(photo.durationMs, null)
  assert.ok(stream.getTracks()[0].stops > 0); tools.dispose()
})

test('file then SQL states distinguish successful persistence and both uncertain failures, without replay', async () => {
  const asset = { blob: new Blob(['hello']), mimeType: 'text/plain', extension: 'bin', size: 5, durationMs: null, name: 'note.txt' }
  const phases = []; const order = []
  const result = await saveMediaAsset({ asset, path: 'test/one.bin', upload: async path => { order.push(`file:${path}`); return { path } }, writeRecord: async metadata => { order.push(`sql:${metadata.path}`) }, onState: state => phases.push(state.phase) })
  assert.deepEqual(order, ['file:test/one.bin', 'sql:test/one.bin']); assert.deepEqual(phases, ['uploading', 'file-saved', 'saved']); assert.equal(result.phase, 'saved')
  let sqlCalls = 0; let uploads = 0
  const failedUpload = await saveMediaAsset({ asset, path: 'test/two.bin', upload: async () => { uploads++; throw new Error('response lost') }, writeRecord: async () => { sqlCalls++ } })
  assert.equal(failedUpload.phase, 'upload-failed'); assert.equal(failedUpload.outcomeUnknown, true); assert.equal(sqlCalls, 0); assert.equal(uploads, 1)
  const failedSql = await saveMediaAsset({ asset, path: 'test/three.bin', upload: async () => ({ path: 'test/three.bin' }), writeRecord: async () => { sqlCalls++; throw new Error('write acknowledgement lost') } })
  assert.equal(failedSql.phase, 'record-failed'); assert.equal(failedSql.file.path, 'test/three.bin'); assert.equal(sqlCalls, 1)
})

test('clipboard fallback is selectable text; download uses ordinary anchor and releases blob URL', async () => {
  const h = harness(); let copies = 0; h.env.navigator.clipboard.writeText = async () => { copies++ }
  assert.deepEqual(await copyTextFromGesture('hello', h.env), { copied: true, fallbackText: '' }); assert.equal(copies, 1)
  h.env.navigator.userActivation.isActive = false
  assert.deepEqual(await copyTextFromGesture('manual', h.env), { copied: false, fallbackText: 'manual' }); assert.equal(copies, 1)
  assert.throws(() => downloadBlobFromGesture(new Blob(['x']), 'x.txt', h.env), /点击/)
  h.env.navigator.userActivation.isActive = true; downloadBlobFromGesture(new Blob(['x']), 'x.txt', h.env)
  assert.equal(h.anchors[0].download, 'x.txt'); assert.equal(h.anchors[0].clickCount, 1); assert.equal(h.anchors[0].removeCount, 1)
  h.advance(1000); assert.deepEqual(h.revoked, ['blob:mock/0'])
})

test('drop reads only explicitly delivered files and MIME extensions are conservative', () => {
  let prevented = false; const file = new File(['x'], 'x.txt')
  assert.deepEqual(droppedFiles({ preventDefault() { prevented = true }, dataTransfer: { files: [file] } }), [file]); assert.equal(prevented, true)
  assert.equal(extensionForMime('audio/mp4; codecs=mp4a.40.2'), 'm4a'); assert.equal(extensionForMime('application/unknown'), 'bin')
})

test('cancellation between permission resolution and recorder/camera continuation cannot restart capture', async () => {
  for (const method of ['startRecording', 'startCamera']) {
    const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() })
    const pending = tools[method](); const rejection = assert.rejects(pending, { name: 'AbortError' })
    await Promise.resolve(); tools.stop('user-canceled'); await rejection
    assert.equal(h.recorders.length, 0); assert.equal(tools.state.stream, null); tools.dispose()
  }
})


test('real normal mode allows capture; insecure context is rejected before device request', async () => {
  const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() })
  assert.equal(context().state.mode, 'normal'); await tools.startCamera(); assert.equal(h.captures.length, 1); tools.stop()
  h.env.isSecureContext = false; await assert.rejects(tools.startCamera(), { name: 'SecurityError' }); assert.equal(h.captures.length, 1); tools.dispose()
})

test('a late recorder stop cannot replace a newly selected file or restore a discarded draft', async () => {
  for (const action of ['file', 'discard']) {
    const h = harness(); const tools = createMediaTools({ environment: h.env, context: context() }); await tools.startRecording(); h.recorders[0].data('old audio')
    if (action === 'file') tools.selectFile(new File(['new file'], 'chosen.txt'))
    else tools.discard()
    await tick()
    if (action === 'file') { assert.equal(tools.state.asset.name, 'chosen.txt'); assert.equal(await tools.state.asset.blob.text(), 'new file') }
    else { assert.equal(tools.state.asset, null); assert.equal(tools.state.phase, 'idle') }
    tools.dispose()
  }
})
