//! Platform-owned notice injected at response time, including into existing SDK
//! 0.1.0 pages. It never changes project files, SDK code, or the document epoch.

pub(crate) fn script() -> &'static str {
    r#"(() => {
  'use strict';
  const epoch = document.querySelector('meta[name="platform-document-epoch"]')?.content;
  const projectId = location.pathname.match(/^\/p\/([a-z][a-z0-9_-]{0,63})(?:\/|$)/)?.[1];
  const nonce = document.currentScript?.nonce;
  if (!epoch || !projectId || !/^http:\/\/127\.0\.0\.1:\d+$/.test(location.origin)) return;
  const endpoint = new URL(`/p/${projectId}/api/v1/context`, location.origin);
  const eventsUrl = new URL(`/p/${projectId}/api/v1/events`, location.origin);
  eventsUrl.searchParams.set('documentEpoch', epoch);
  let state = null;
  let offline = false;
  let expired = false;
  let reloadRequired = false;
  let loading = false;
  let queryAgain = false;
  let stopped = false;
  let stream;
  let host;
  let title;
  let detail;
  let reload;
  let retry;
  let confirmation;
  let confirmReload;
  let cancelReload;
  let confirmingReload = false;
  let lastConnectionCheck = 0;

  function canReloadDocument() {
    return reloadRequired && state?.mode !== 'maintenance' && !state?.recoveryRequired && !state?.trashed && !state?.workspaceMigrating && !state?.instanceChanged && !expired;
  }

  function render() {
    if (!host) return;
    let heading = '';
    let explanation = '';
    if (state?.trashed) {
      heading = '项目已移入回收站';
      explanation = '当前页面不能继续保存。请先复制草稿或保存到别处；如需继续使用，请在平台回收站恢复项目，再从平台重新进入。';
    } else if (state?.instanceChanged) {
      heading = '工作区服务已切换，需要重新进入';
      explanation = '迁移后旧页面身份已失效，当前草稿仍保留。请先复制未保存内容或保存到别处，再从平台重新加载项目或获取新的浏览器入口。';
    } else if (state?.workspaceMigrating && !expired) {
      heading = '工作区正在迁移，操作暂时暂停';
      explanation = '当前页面和草稿会保留。请等待平台迁移任务完成，处理草稿后再从平台重新进入；平台不会自动重新提交失败的保存。';
    } else if (expired) {
      heading = '本地项目连接已失效';
      explanation = '平台重启或工作区迁移后，需要从平台重新进入此项目。当前页面保留未提交内容，请先复制草稿或保存到别处。';
    } else if (reloadRequired) {
      heading = '项目已更新，需要重新加载';
      explanation = '维护或恢复已完成，当前页面不能继续保存。请先复制未保存内容或保存到别处，再决定是否重新加载。';
    } else if (state?.recoveryRequired) {
      heading = '项目恢复结果需要处理';
      explanation = '项目暂时不能保存数据。请在平台的备份与维护中查看状态；当前页面和未提交内容会保留。';
    } else if (state?.mode === 'maintenance') {
      heading = '项目正在维护，暂时无法保存';
      explanation = '当前页面和未提交内容会保留。维护不会在重启后自动结束，请等待平台完成检查；失败的保存不会自动重试。';
    } else if (state?.busy) {
      heading = '正在备份，暂时无法保存';
      explanation = '当前页面和未提交内容会保留。请等待备份完成后自行检查并重新提交；平台不会自动重试失败的保存。';
    } else if (offline) {
      heading = '暂时无法连接本地平台';
      explanation = '请确认桌面平台仍在运行。未提交内容仍在当前页面中，平台不会自动重试保存。';
    }
    host.hidden = !heading;
    if (title.textContent !== heading) title.textContent = heading;
    if (detail.textContent !== explanation) detail.textContent = explanation;
    const canReload = canReloadDocument();
    if (!canReload) confirmingReload = false;
    reload.hidden = !canReload || confirmingReload;
    confirmation.hidden = !confirmingReload;
    confirmReload.disabled = !canReload;
    retry.disabled = loading;
    retry.textContent = loading ? '正在检查…' : '刷新状态';
  }

  async function check() {
    if (stopped) return;
    if (loading) { queryAgain = true; return; }
    loading = true;
    render();
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 8000);
    try {
      const response = await fetch(endpoint, {
        method: 'GET', credentials: 'same-origin', cache: 'no-store', redirect: 'error',
        headers: { 'X-Platform-Document-Epoch': epoch }, signal: controller.signal,
      });
      const value = await response.json();
      if (!response.ok || value.ok !== true) {
        if (value.error?.code === 'PROJECT_RELOAD_REQUIRED') reloadRequired = true;
        else if (value.error?.code === 'PROJECT_TRASHED') state = { ...state, trashed: true };
        else if (value.error?.code === 'WORKSPACE_MIGRATING') state = { ...state, workspaceMigrating: true };
        else if (value.error?.code === 'SERVICE_INSTANCE_CHANGED') { state = { ...state, instanceChanged: true, workspaceMigrating: false, trashed: false }; expired = true; }
        else if (value.error?.code === 'INVALID_SESSION' || response.status === 401) { expired = true; state = { ...state, workspaceMigrating: false, trashed: false }; }
        else throw new Error('Status unavailable');
      } else if (value.apiVersion === 1 && value.data?.projectId === projectId && value.data?.state) {
        state = value.data.state;
        reloadRequired = reloadRequired || state.reloadRequired === true;
        expired = false;
      } else throw new Error('Invalid project status');
      offline = false;
    } catch {
      offline = true;
    } finally {
      clearTimeout(timeout);
      loading = false;
      render();
      if (queryAgain) { queryAgain = false; void check(); }
    }
  }

  function createNotice() {
    if (stopped || host) return;
    host = document.createElement('div');
    host.id = 'local-projects-platform-status';
    host.hidden = true;
    const shadow = host.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    if (nonce) style.nonce = nonce;
    style.textContent = ':host{position:fixed!important;inset:12px 12px auto!important;display:block!important;z-index:2147483647!important;color-scheme:light!important}:host([hidden]){display:none!important}.notice{box-sizing:border-box;max-width:860px;margin:auto;border:1px solid #d4c7a9;border-radius:10px;background:#fffaf0;color:#514936;box-shadow:0 3px 18px #0002;padding:15px 17px;font:13px/1.65 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;max-height:40vh;overflow:auto}.heading{font-weight:650;font-size:14px}.detail{margin:5px 0 10px}.actions{display:flex;flex-wrap:wrap;gap:8px}button{font:inherit;font-size:12px;border:1px solid #cbc0a7;border-radius:6px;background:#fffdf8;color:#514936;padding:6px 10px;cursor:pointer}button:focus-visible{outline:2px solid #6d5b35;outline-offset:2px}button:disabled{opacity:.6;cursor:wait}[hidden]{display:none!important}.confirmation{border-top:1px solid #d4c7a9;margin-top:12px;padding-top:12px}.confirmation p{margin:0 0 10px}.reload{background:#514936;color:white;border-color:#514936}';
    const notice = document.createElement('section');
    notice.className = 'notice';
    notice.setAttribute('role', 'status');
    notice.setAttribute('aria-live', 'polite');
    notice.setAttribute('aria-atomic', 'true');
    title = document.createElement('div');
    title.className = 'heading';
    detail = document.createElement('p');
    detail.className = 'detail';
    const actions = document.createElement('div');
    actions.className = 'actions';
    retry = document.createElement('button');
    retry.type = 'button';
    retry.textContent = '刷新状态';
    retry.addEventListener('click', () => { void check(); });
    reload = document.createElement('button');
    reload.type = 'button';
    reload.className = 'reload';
    reload.textContent = '处理草稿后重新加载';
    reload.hidden = true;
    reload.addEventListener('click', () => {
      if (!canReloadDocument()) return;
      confirmingReload = true;
      render();
      cancelReload.focus();
    });
    confirmation = document.createElement('div');
    confirmation.className = 'confirmation';
    confirmation.hidden = true;
    confirmation.setAttribute('role', 'group');
    confirmation.setAttribute('aria-label', '重新加载前确认草稿处理');
    const warning = document.createElement('p');
    warning.textContent = '重新加载会丢弃当前页面未保存的内容。请先复制草稿或保存到别处；取消会保留当前页面。';
    const confirmationActions = document.createElement('div');
    confirmationActions.className = 'actions';
    cancelReload = document.createElement('button');
    cancelReload.type = 'button';
    cancelReload.textContent = '取消，保留当前页面';
    function cancelConfirmation() {
      confirmingReload = false;
      render();
      reload.focus();
    }
    cancelReload.addEventListener('click', cancelConfirmation);
    confirmation.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); cancelConfirmation(); }
    });
    confirmReload = document.createElement('button');
    confirmReload.type = 'button';
    confirmReload.className = 'reload';
    confirmReload.textContent = '已处理草稿，确认重新加载';
    confirmReload.addEventListener('click', () => {
      if (confirmingReload && canReloadDocument()) location.reload();
    });
    confirmationActions.append(cancelReload, confirmReload);
    confirmation.append(warning, confirmationActions);
    actions.append(retry, reload);
    notice.append(title, detail, actions, confirmation);
    shadow.append(style, notice);
    document.body.append(host);
    render();
    void check();
    connectEvents();
  }
  function connectEvents() {
    if (typeof EventSource === 'function') {
      stream?.close();
      stream = new EventSource(eventsUrl, { withCredentials: true });
      stream.addEventListener('open', () => { void check(); });
      stream.addEventListener('project.state.changed', (event) => {
        try {
          const update = JSON.parse(event.data);
          if (update.projectId !== projectId) return;
          reloadRequired = reloadRequired || update.reloadRequired === true;
          state = { ...state, ...update };
          render();
        } catch { /* Re-read the authoritative status instead of trusting an invalid event. */ }
        void check();
      });
      stream.addEventListener('error', () => {
        if (Date.now() - lastConnectionCheck > 2000) {
          lastConnectionCheck = Date.now();
          void check();
        }
      });
    }
  }
  window.addEventListener('focus', () => { void check(); });
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') void check(); });
  window.addEventListener('pagehide', () => { stopped = true; stream?.close(); });
  window.addEventListener('pageshow', (event) => { if (event.persisted) { stopped = false; void check(); connectEvents(); } });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', createNotice, { once: true });
  else createNotice();
})();"#
}
