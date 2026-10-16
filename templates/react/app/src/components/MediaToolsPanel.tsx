import { useEffect, useRef, useState } from 'react'
import { platform } from '@local-projects/sdk'
import type { ProjectContext } from '@local-projects/sdk'
import { Button } from './ui'
import { copyTextFromGesture, createMediaTools, downloadBlobFromGesture, droppedFiles, explainMediaError, saveMediaAsset } from '../lib/browser-tools.js'
import type { MediaState, MediaTools, SaveState } from '../lib/browser-tools.js'

function purpose(context: ProjectContext | undefined, device: string) {
  const value = context?.runtimeRequirements?.devices?.[device]
  return value && typeof value === 'object' && 'purpose' in value && typeof value.purpose === 'string' ? value.purpose : ''
}

// This demonstration performs no schema or file writes until the explicit Save button.
export function MediaToolsPanel({ context }: { context?: ProjectContext }) {
  const tools = useRef<MediaTools | null>(null)
  const video = useRef<HTMLVideoElement>(null)
  const [state, setState] = useState<MediaState>({ phase: 'idle', reason: '', asset: null, stream: null })
  const [preview, setPreview] = useState('')
  const [error, setError] = useState('')
  const [saved, setSaved] = useState<SaveState>()
  const [fallbackText, setFallbackText] = useState('')
  const [copyStatus, setCopyStatus] = useState('')
  const [paste, setPaste] = useState('')
  const [saving, setSaving] = useState(false)
  const micPurpose = purpose(context, 'microphone')
  const cameraPurpose = purpose(context, 'camera')
  const active = ['requesting', 'recording', 'camera'].includes(state.phase)
  const projectReady = Boolean(context && ['normal', 'ready'].includes(context.state.mode) && !context.state.busy && !context.state.reloadRequired)

  useEffect(() => {
    const current = createMediaTools({ onChange: setState })
    tools.current = current
    return () => { current.dispose(); tools.current = null }
  }, [])
  useEffect(() => { tools.current?.updateContext(context) }, [context])
  useEffect(() => {
    const current = tools.current
    if (!current || !state.asset) { setPreview(''); return }
    const url = current.previewUrl(state.asset)
    setPreview(url)
    return () => current.revokeUrl(url)
  }, [state.asset])
  useEffect(() => {
    if (!video.current) return
    video.current.srcObject = state.stream
    if (state.stream) void video.current.play().catch(cause => setError(explainMediaError(cause)))
  }, [state.stream])

  async function act(action: () => void | Promise<unknown>) {
    setError('')
    try { await action() } catch (cause) { setError(explainMediaError(cause)) }
  }
  function choose(file?: File) {
    if (!file || saving) return
    void act(() => { tools.current?.selectFile(file); setSaved(undefined) })
  }
  async function save() {
    if (!state.asset || saving || saved) return
    setSaving(true)
    const id = crypto.randomUUID()
    const path = `media-samples/${id}.${state.asset.extension}`
    try {
      await saveMediaAsset({
        asset: state.asset, path,
        upload: (logicalPath, blob) => platform.files.put({ path: logicalPath, content: blob }),
        writeRecord: metadata => platform.db.transaction({ statements: [
          { sql: 'CREATE TABLE IF NOT EXISTS media_samples (id TEXT PRIMARY KEY, attachment_path TEXT NOT NULL, mime_type TEXT NOT NULL, extension TEXT NOT NULL, size_bytes INTEGER NOT NULL, duration_ms INTEGER, original_name TEXT NOT NULL, created_at TEXT NOT NULL)' },
          { sql: 'INSERT INTO media_samples (id, attachment_path, mime_type, extension, size_bytes, duration_ms, original_name, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', params: [id, metadata.path, metadata.mimeType, metadata.extension, metadata.size, metadata.durationMs, metadata.name, new Date().toISOString()] },
        ] }),
        onState: setSaved,
      })
    } finally { setSaving(false) }
  }
  async function copy(text: string) {
    const result = await copyTextFromGesture(text)
    setFallbackText(result.fallbackText)
    setCopyStatus(result.copied ? '已复制。' : '自动复制不可用，请选择下面的文字手动复制。')
  }

  return <details className="media-demo" onToggle={event => { if (!event.currentTarget.open) tools.current?.stop('panel-closed') }}>
    <summary>试用本地媒体与文件组件</summary>
    <div className="media-body">
      <p>录音、拍照和选择文件先留在当前页面。点击“保存素材示例”才上传附件并写入 <code>media_samples</code> 演示表；打开页面不会创建业务数据。</p>
      <p className="muted">录音和摄像头预览最多持续 60 秒，文件最多 10 MiB，并受平台返回的上传限制约束。页面隐藏、离开或项目状态变化会停止设备。刷新会丢失未保存内容。</p>
      <div className="media-actions">
        <Button disabled={!projectReady || !micPurpose || active || saving} onClick={() => void act(async () => { await tools.current?.startRecording(); setSaved(undefined) })}>开始录音</Button>
        <Button disabled={!projectReady || !cameraPurpose || active || saving} onClick={() => void act(async () => { await tools.current?.startCamera(); setSaved(undefined) })}>打开摄像头 / 重拍</Button>
        <Button className="quiet" disabled={!active} onClick={() => tools.current?.stop('user-stopped')}>停止采集</Button>
      </div>
      <p className="muted">麦克风：{micPurpose || '未声明，不启用'}<br />摄像头：{cameraPurpose || '未声明，不启用'}。需要时请让 AI 在项目运行声明中填写具体用途，之后明确重新加载页面。</p>
      {state.phase === 'requesting' && <p role="status">等待设备许可，可随时停止。</p>}
      {state.phase === 'recording' && <p role="status">正在录音…</p>}
      <video ref={video} hidden={state.phase !== 'camera'} muted autoPlay playsInline aria-label="摄像头本地预览" />
      {state.phase === 'camera' && <Button onClick={() => void act(() => video.current ? tools.current?.takePhoto(video.current) : undefined)}>拍下照片并关闭设备</Button>}
      <div className="file-drop" onDragOver={event => event.preventDefault()} onDrop={event => choose(droppedFiles(event)[0])}>
        <label>选择已有文件（或拖入这里）<input type="file" disabled={saving} onChange={event => { choose(event.currentTarget.files?.[0]); event.currentTarget.value = '' }} /></label>
      </div>
      {state.asset && <div className="asset-preview">
        {state.asset.mimeType.startsWith('audio/') ? <audio controls src={preview} /> : state.asset.mimeType.startsWith('image/') ? <img src={preview} alt="所选素材预览" /> : <p>已选文件：{state.asset.name || '本地文件'}</p>}
        <p>{state.asset.mimeType} · {state.asset.size.toLocaleString()} 字节{state.asset.durationMs !== null ? ` · 约 ${(state.asset.durationMs / 1000).toFixed(1)} 秒` : ''}</p>
        <div className="media-actions">
          <Button disabled={!projectReady || active || saving || Boolean(saved)} onClick={() => void save()}>保存素材示例</Button>
          <Button className="quiet" onClick={() => state.asset && downloadBlobFromGesture(state.asset.blob, state.asset.name || `media.${state.asset.extension}`)}>下载本地副本</Button>
          <Button className="quiet" disabled={saving} onClick={() => { tools.current?.discard(); setSaved(undefined) }}>清除页面预览</Button>
        </div>
      </div>}
      {saved && <div role="status" className="save-state">
        {saved.phase === 'uploading' && '正在上传附件，尚未写入记录。'}
        {saved.phase === 'file-saved' && '附件已写入，正在关联业务记录。'}
        {saved.phase === 'saved' && '附件和业务记录均已保存。'}
        {saved.phase === 'upload-failed' && '附件写入未得到成功确认，未请求写入记录。先核查该路径；不要盲目重试。'}
        {saved.phase === 'record-failed' && '附件已保存，业务记录未得到成功确认。文件保留在以下路径，请先核查记录，再决定补写关联或删除孤立附件。'}
        <code>{saved.path}</code><Button className="quiet" onClick={() => void copy(saved.path)}>复制附件路径</Button>
      </div>}
      {(error || state.error) && <p role="alert">{error || state.error}</p>}
      {state.reason && !active && <p className="muted">采集已停止；设备已释放。页面中的预览尚未自动保存。</p>}
      <label className="paste-field">手动粘贴文字（仅处理您的粘贴或输入操作）<textarea value={paste} onChange={event => setPaste(event.target.value)} placeholder="在此按 ⌘V 粘贴；不自动读取剪贴板" /></label>
      <Button className="quiet" onClick={() => void copy(paste)}>复制文字</Button>
      {copyStatus && <p role="status">{copyStatus}</p>}
      {fallbackText && <textarea readOnly aria-label="手动复制文字" value={fallbackText} onFocus={event => event.target.select()} />}
    </div>
  </details>
}
