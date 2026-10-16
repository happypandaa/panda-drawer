# Browser tools 0.1.0

This helper is copied with template `react-ts-vite-0.1.3`. It is independent of the unchanged `@local-projects/sdk 0.1.0` package. `src/lib/browser-tools.js` contains browser behavior; its `.d.ts` describes the API. `src/components/MediaToolsPanel.tsx` demonstrates the controls. A platform update does not copy these files into existing projects.

The initial page does not request devices or write a schema. All network lists and device declarations are empty by default. Standard file picking, drag/drop, local previews, anchor downloads, manual paste and user-gesture clipboard writes work without device declarations. Clipboard denial displays selectable text; the helper never reads the clipboard silently. Browser downloads use the browser's normal download behavior. The native host uses a private temporary download, then a separate save-location dialog; cancelling that dialog removes only the temporary download.

To add a device, document a concrete purpose in `project.json`, then obtain a fresh document or explicitly reload after handling drafts:

```json
{
  "runtimeRequirements": {
    "network": { "connect": [], "images": [], "media": [] },
    "devices": {
      "microphone": { "purpose": "Record short spoken notes kept with this project's entries" },
      "camera": { "purpose": "Photograph receipts for this project's expense records" }
    }
  }
}
```

Declare only devices the application actually uses. There is no screen capture, system audio, background recording or automatic transcription. `startRecording` / `startCamera` must be called directly from a user action. The helper checks the current declaration, secure context and project state before asking the browser; the operating system and browser can still refuse permission. Show `explainMediaError` and retain the file picker fallback. A persisted browser permission is not a new per-project OS permission.

Recording probes `MediaRecorder.isTypeSupported` and records the actual resulting MIME type, extension, bytes and measured foreground duration. Never assume WebM support or silently rename MP4 audio to WebM. Photo capture uses a video preview and canvas JPEG capture rather than requiring the newer ImageCapture API. Images are scaled to a maximum 1920-pixel long edge. These are internal template defaults, not finalized platform product limits.

Default capture duration is 60 seconds, including camera preview. Default maximum media/file size is 10 MiB; the effective size limit is the smaller of that value and `context.limits.fileUploadMaxBytes`. Stop device tracks and revoke preview URLs on disposal. The helper also releases devices on `visibilitychange` to hidden, `pagehide`, `beforeunload`, browser offline, or `platform:state` changes indicating maintenance, busy, recovery, trash, migration, instance change, disconnect, expired session or required reload. `normal` is the runtime's ready mode. A late permission response after cancellation is immediately released. A window merely losing focus does not stop capture; a hidden document does. Connection state is detected by the platform runtime notice, so this does not promise instant detection of a disconnected service.

The platform's `platform:state` event is a supplemental template hook, not a change to SDK 0.1.0. Custom code that opens devices outside this helper is responsible for its own release behavior. Browser policies, declarations and the app's native permission callback are not a hostile-code isolation boundary between same-origin projects. Declaration edits require a new document policy; they cannot instantly revoke every API use in an already-open arbitrary page.

Template 0.1.3 also gates the example panel's capture and persistence controls on the latest runtime notice. Maintenance, disconnect, expired documents and required reload keep local file selection, previews, downloads and manual copying available. A late context response cannot clear that gate; only a fresh authoritative runtime state can. Save rechecks the gate synchronously before uploading and again before linking the file in SQL. If the file completed before the gate changed, its path remains visible and no automatic SQL or upload replay occurs.

Saving uses two separate commits: `files.put`, then an explicit parameterized SQL operation. `saveMediaAsset` reports `uploading`, `file-saved`, `saved`, `upload-failed` or `record-failed`. It never retries writes. A lost response may mean the write committed; inspect the reported attachment path and business row before retrying. If only the file succeeded, keep its path visible, then explicitly decide whether to link it or delete an orphan. A blob URL is only a preview and must never be stored as the persistent attachment reference.

The optional panel's “保存素材示例” button is the sole trigger for creating `media_samples` and inserting a row. The table contains `id` (text primary key), `attachment_path`, `mime_type`, `extension`, `size_bytes`, nullable `duration_ms`, `original_name` and ISO-8601 `created_at`. No table or row is created by opening, building or merely selecting a file. Deleting a business row does not implicitly delete its file; a business application must define and document attachment retention/deletion explicitly. “清除页面预览” only discards the local draft and never deletes uploaded files.

Public browser requests need both the correct CSP group and server CORS support. For example, `network.connect` can contain `{ "origin": "https://api.example.com", "purpose": "Fetch public reference information", "dataSent": "The search text entered by the user" }`. Use canonical exact HTTPS origins: no trailing slash, path, credentials, wildcard, query or fragment, IP address or local hostname. `connect`, `images` and `media` grant only their own resource types. Dependencies/scripts stay in the local build. Do not include credentials or keys in the manifest, browser source, logs or templates; M5 has no secret proxy. Describe which features work offline and which requests transmit user data before using remote services.

Validation here consists of automated mock behavior tests and a TypeScript/Vite build. It does not establish real camera/microphone, OS permission, browser CORS, Safari/Chrome playback, native save-dialog or clipboard behavior. Those require the signed application and real browsers/devices. Keep actual tested versions and outcomes in the project's verification notes.
