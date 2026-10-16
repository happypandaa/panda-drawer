/* M1 controlled verification client. The distributable SDK is an M2 deliverable. */
const projectId = /^\/p\/([a-z][a-z0-9_-]*)\//.exec(location.pathname)?.[1];
const documentEpoch = document.querySelector('meta[name="platform-document-epoch"]')?.content;
const api = `/p/${projectId}/api/v1`;
const status = document.querySelector('#status');
const wordForm = document.querySelector('#word-form');
const noteForm = document.querySelector('#note-form');
let liveEvents;
let sessionReady = false;
let loadCounter = 0;

function showStatus(message, error = false) {
  status.textContent = message;
  status.className = error ? 'error' : 'ready';
}
async function request(path, options = {}) {
  const response = await fetch(`${api}${path}`, {
    ...options,
    headers: { 'X-Platform-Document-Epoch': documentEpoch, ...options.headers },
    credentials: 'same-origin',
  });
  const result = await response.json();
  if (!result.ok) throw new Error(`${result.error?.code || response.status}: ${result.error?.message || '请求失败'}`);
  return result.data;
}
function sql(sql, params = []) {
  return request('/db/execute', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ sql, params }) });
}
async function refresh() {
  const generation = ++loadCounter;
  const [result, files] = await Promise.all([
    sql('SELECT id, word, meaning FROM words ORDER BY created_at, id'),
    request('/files'),
  ]);
  if (generation !== loadCounter) return;
  const words = document.querySelector('#words');
  words.replaceChildren();
  for (const row of result.rows) {
    const item = document.createElement('li');
    const title = document.createElement('strong');
    title.textContent = row.word;
    const meaning = document.createElement('span');
    meaning.textContent = row.meaning;
    item.append(title, meaning);
    words.append(item);
  }
  if (!result.rows.length) words.textContent = '还没有单词。添加后会保存在本项目数据库。';
  const fileList = document.querySelector('#files');
  fileList.replaceChildren();
  for (const file of Array.isArray(files) ? files : files.files || []) {
    const item = document.createElement('li');
    const link = document.createElement('a');
    link.textContent = file.path || file.name;
    link.href = `${api}/file?${new URLSearchParams({ path: file.path || file.name, documentEpoch })}`;
    link.download = (file.path || file.name).split('/').at(-1);
    const size = document.createElement('span');
    size.textContent = `${file.size ?? file.bytes ?? file.sizeBytes ?? '?'} 字节`;
    item.append(link, size);
    fileList.append(item);
  }
  if (!fileList.childElementCount) fileList.textContent = '还没有附件。';
}
async function action(button, task, message) {
  button.disabled = true;
  try {
    await task();
    await refresh();
    showStatus(message);
  } catch (error) {
    showStatus(`${error.message}。若连接中断，请先重新查询确认保存结果，不要直接重复提交。`, true);
  } finally { button.disabled = false; }
}
wordForm.addEventListener('submit', (event) => {
  event.preventDefault();
  const word = wordForm.elements.word.value.trim();
  const meaning = wordForm.elements.meaning.value.trim();
  if (!word || !meaning) return;
  action(wordForm.querySelector('button'), async () => {
    await sql('INSERT INTO words (id, word, meaning, created_at) VALUES (?, ?, ?, ?)', [crypto.randomUUID(), word, meaning, new Date().toISOString()]);
    wordForm.reset();
  }, '单词已保存到当前项目。');
});
noteForm.addEventListener('submit', (event) => {
  event.preventDefault();
  action(noteForm.querySelector('button'), async () => {
    const name = noteForm.elements.filename.value.trim();
    if (!name || name.includes('/') || name.includes('\\')) throw new Error('请使用单个文件名');
    await request(`/file?${new URLSearchParams({ path: name })}`, { method: 'PUT', headers: { 'Content-Type': 'text/plain; charset=utf-8' }, body: noteForm.elements.note.value });
  }, '笔记已保存为附件。同名文件不会自动覆盖。');
});
document.querySelector('#upload').addEventListener('change', (event) => {
  const file = event.target.files[0];
  if (!file) return;
  action(event.target, () => request(`/file?${new URLSearchParams({ path: file.name })}`, { method: 'PUT', headers: { 'Content-Type': file.type || 'application/octet-stream' }, body: file }), '文件已保存。');
});
document.querySelector('#refresh').addEventListener('click', (event) => action(event.target, refresh, '已重新查询数据库与附件。'));
document.querySelector('#native-check').addEventListener('click', async () => {
  const result = document.querySelector('#native-result');
  if (!window.__TAURI_INTERNALS__?.invoke) { result.textContent = '通过：此页面没有可用的 Tauri 原生命令入口。'; return; }
  try { await window.__TAURI_INTERNALS__.invoke('bootstrap_info'); result.textContent = '失败：项目页面取得了管理窗口信息。'; }
  catch { result.textContent = '通过：平台拒绝项目页面调用管理原生命令。'; }
});
document.querySelector('#project-id').textContent = projectId || '未知';
document.querySelector('#epoch').textContent = documentEpoch || '缺少入口代次';
document.querySelector('#client-kind').textContent = window.__TAURI_INTERNALS__ ? '桌面项目窗口' : '浏览器或受限桌面页面';
(async () => {
  try {
    if (!projectId || !documentEpoch) throw new Error('请从平台的项目入口打开本页');
    await request('/context');
    sessionReady = true;
    await refresh();
    showStatus('已连接本机数据库。数据与附件只属于当前项目。');
    liveEvents = new EventSource(`${api}/events?${new URLSearchParams({ documentEpoch })}`);
    liveEvents.addEventListener('data.changed', () => refresh().catch((error) => showStatus(error.message, true)));
    liveEvents.addEventListener('project.state.changed', () => request('/context').catch((error) => showStatus(error.message, true)));
  } catch (error) { showStatus(error.message, true); }
})();
window.addEventListener('focus', () => {
  if (sessionReady) refresh().catch((error) => showStatus(error.message, true));
});
window.addEventListener('pagehide', () => liveEvents?.close());
