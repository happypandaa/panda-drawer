import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { TriangleAlert, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { DeleteProgress } from './DeleteProgress';
import { Modal } from './Modal';
import type { NodeAppDeletePreview, NodeAppDeleteResult, NodeDeletionTaskSnapshot } from './node-app-types';
import { nodeDeletionActive, nodeDeletionLabel, validNodeDeletionTask } from './node-deletion-task-state';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform } from './frontend-platform';

function validPreview(value: NodeAppDeletePreview, appId: string, isAbsolutePath: (path: string) => boolean): boolean {
  return Boolean(value && value.appId === appId && typeof value.name === 'string' && value.name
    && ['managed', 'external'].includes(value.ownership) && typeof value.expectedRevision === 'string' && value.expectedRevision
    && typeof value.platformRoot === 'string' && isAbsolutePath(value.platformRoot) && typeof value.sourcePath === 'string' && isAbsolutePath(value.sourcePath)
    && Array.isArray(value.deletePaths) && value.deletePaths.length > 0 && value.deletePaths.every(path => typeof path === 'string' && isAbsolutePath(path))
    && Array.isArray(value.preservedPaths) && value.preservedPaths.every(path => typeof path === 'string' && isAbsolutePath(path))
    && (value.blocker == null || typeof value.blocker.code === 'string' && Boolean(value.blocker.code) && typeof value.blocker.message === 'string' && Boolean(value.blocker.message)));
}

/** Lists full paths grouped by parent directory, relative to a base directory shown once above the list. */
function PathGroups({ paths, base }: { paths: string[]; base: string }) {
  const platform = useFrontendPlatform();
  const groups = new Map<string, string[]>();
  for (const path of new Set(paths)) { const dir = platform.parentPath(path); groups.set(dir, [...(groups.get(dir) ?? []), path]); }
  const relative = (dir: string) => platform.relativeDirectory(dir, base);
  return <ul className="node-delete-groups">{[...groups].map(([dir, items]) => <li key={dir}>
    {relative(dir) && <code className="node-delete-dir" title={dir}>{relative(dir)}</code>}
    <ul className="node-delete-names">{items.map(path => <li key={path} data-path={path} title={path}><code>{platform.pathBaseName(path)}</code></li>)}</ul>
  </li>)}</ul>;
}

export function NodeAppDeleteDialog({ app, enabled, instanceId, initialTask, onClose, onPendingChange, onDeleted, onTaskChanged, onAbandoned }: {
  app: { id: string; name: string; isAgent?: boolean };
  enabled: boolean;
  instanceId?: string;
  initialTask?: NodeDeletionTaskSnapshot;
  onClose: () => void;
  onPendingChange: (pending: boolean) => void;
  onDeleted: (appId: string, result: NodeAppDeleteResult) => void;
  onTaskChanged?: (task: NodeDeletionTaskSnapshot) => void;
  onAbandoned?: (task: NodeDeletionTaskSnapshot) => void;
}) {
  const platform = useFrontendPlatform();
  const [preview, setPreview] = useState<NodeAppDeletePreview | null>(null);
  const [task, setTask] = useState<NodeDeletionTaskSnapshot | null>(null);
  const [verified, setVerified] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [error, setError] = useState('');
  const [observationError, setObservationError] = useState('');
  const [errorTitle, setErrorTitle] = useState(t("无法核对删除范围"));
  // The platform could not confirm the app stopped: the user either stops it
  // and checks again, or deletes directly. Deletion never stays stuck here.
  const [runUncertain, setRunUncertain] = useState('');
  const [request, setRequest] = useState<'preview' | 'submit' | 'retry' | 'abandon' | null>(null);
  const alive = useRef(true);
  const operation = useRef<typeof request>(null);
  const taskRef = useRef<NodeDeletionTaskSnapshot | null>(null);
  const sequence = useRef(0);
  const delivered = useRef(new Set<string>());
  const errorPanel = useRef<HTMLDivElement>(null);
  const refreshStatus = useRef<(() => void) | null>(null);
  const approval = useRef<{ appId: string; instanceId?: string; revision: string; sequence: number } | null>(null);
  const context = useRef({ appId: app.id, instanceId, enabled }); context.current = { appId: app.id, instanceId, enabled };
  const callbacks = useRef({ onClose, onPendingChange, onDeleted, onTaskChanged, onAbandoned }); callbacks.current = { onClose, onPendingChange, onDeleted, onTaskChanged, onAbandoned };
  const active = Boolean(task && nodeDeletionActive(task));
  const busy = active || Boolean(request && request !== 'preview');
  const blocked = !enabled || request !== null || active;
  const pending = request === 'preview' ? 'preview' : busy ? 'delete' : null;

  function isCurrent(origin: typeof context.current, generation: number) {
    return alive.current && sequence.current === generation && context.current.appId === origin.appId && context.current.instanceId === origin.instanceId;
  }
  function acceptTask(value: NodeDeletionTaskSnapshot, origin: typeof context.current, generation: number) {
    if (!isCurrent(origin, generation)) return;
    if (!validNodeDeletionTask(value, origin.appId)) throw new Error(t("删除结果尚不能确认。请保留当前记录并重新核对，不要重复提交。"));
    const previous = taskRef.current;
    const absent = value.phase === 'deleted' && value.result?.alreadyAbsent === true && value.ownership === 'absent';
    if (!absent && previous && (value.taskId !== previous.taskId || value.targetRevision !== previous.targetRevision || value.ownership !== previous.ownership)) throw new Error(t("删除任务身份已变化，请重新读取原任务。"));
    if (!absent && previous && value.journalRevision < previous.journalRevision) return;
    if (!absent && previous && (previous.renamedAny && !value.renamedAny || value.journalRevision === previous.journalRevision && value.phase !== previous.phase)) throw new Error(t("删除任务状态无法核实，请刷新原任务。"));
    if (!absent && !previous && approval.current && (value.targetRevision !== approval.current.revision || value.ownership !== preview?.ownership)) throw new Error(t("删除任务与已确认范围不符，请查看原任务。"));
    if (!previous || value.journalRevision > previous.journalRevision) setError('');
    taskRef.current = value; setTask(value); setVerified(false); setAcknowledged(false); setObservationError('');
    callbacks.current.onTaskChanged?.(value);
    const terminal = `${value.taskId}:${value.phase}`;
    if (value.phase === 'deleted' && !delivered.current.has(terminal)) {
      delivered.current.add(terminal); callbacks.current.onDeleted(value.appId, value.result!);
    } else if (value.phase === 'abandoned' && !delivered.current.has(terminal)) {
      delivered.current.add(terminal); callbacks.current.onAbandoned?.(value);
    }
  }
  function close() {
    if (busy || operation.current && operation.current !== 'preview') return;
    sequence.current++; approval.current = null; operation.current = null;
    callbacks.current.onPendingChange(false); callbacks.current.onClose();
  }
  async function readPreview() {
    if (!context.current.enabled || operation.current || taskRef.current) return;
    operation.current = 'preview'; setRequest('preview');
    approval.current = null; setVerified(false); setAcknowledged(false); setError(''); setErrorTitle(t("无法核对删除范围")); setRunUncertain('');
    const generation = ++sequence.current; const origin = context.current;
    try {
      const value = await invoke<NodeAppDeletePreview>('node_app_delete_preview', { appId: origin.appId });
      if (!isCurrent(origin, generation) || !context.current.enabled) return;
      if (!validPreview(value, origin.appId, platform.isAbsolutePath)) throw new Error(t("删除范围不完整或项目身份不符，请重新核对。"));
      setPreview(value);
      if (value.blocker) setRunUncertain(messageFrom(value.blocker));
      approval.current = { appId: origin.appId, instanceId: origin.instanceId, revision: value.expectedRevision, sequence: generation };
      setVerified(true);
    } catch (cause) { if (isCurrent(origin, generation) && context.current.enabled) setError(messageFrom(cause)); }
    finally { if (isCurrent(origin, generation)) { operation.current = null; setRequest(null); } }
  }
  async function deleteApp() {
    const approved = approval.current;
    if (blocked || operation.current || !preview || !verified || !acknowledged || !approved || approved.appId !== context.current.appId
      || approved.instanceId !== context.current.instanceId || approved.revision !== preview.expectedRevision || approved.sequence !== sequence.current) return;
    const origin = context.current; const generation = sequence.current;
    operation.current = 'submit'; setRequest('submit'); setError(''); setErrorTitle(t("删除未完成"));
    try { acceptTask(await invoke<NodeDeletionTaskSnapshot>('node_app_delete_submit', { appId: approved.appId, input: { expectedRevision: approved.revision, force: Boolean(runUncertain) } }), origin, generation); }
    catch (cause) { if (isCurrent(origin, generation)) { setError(messageFrom(cause)); setVerified(false); setAcknowledged(false); approval.current = null; } }
    finally { if (isCurrent(origin, generation)) { operation.current = null; setRequest(null); } }
  }
  async function actOnTask(action: 'retry' | 'abandon') {
    const current = taskRef.current;
    if (!enabled || operation.current || !current || action === 'retry' && (!current.canRetry || nodeDeletionActive(current)) || action === 'abandon' && (!current.canAbandon || current.renamedAny)) return;
    const origin = context.current; const generation = sequence.current;
    operation.current = action; setRequest(action); setError(''); setErrorTitle(action === 'abandon' || current.abandonRequested ? t("放弃删除未完成") : t("删除未完成"));
    try { acceptTask(await invoke<NodeDeletionTaskSnapshot>(`node_app_delete_${action}`, { taskId: current.taskId, appId: current.appId, observedRevision: current.journalRevision }), origin, generation); }
    catch (cause) { if (isCurrent(origin, generation)) setError(messageFrom(cause)); }
    finally { if (isCurrent(origin, generation)) { operation.current = null; setRequest(null); } }
  }

  useEffect(() => {
    alive.current = true;
    if (initialTask) {
      try { acceptTask(initialTask, context.current, sequence.current); }
      catch (cause) { setError(messageFrom(cause)); }
    } else void readPreview();
    return () => { alive.current = false; sequence.current++; approval.current = null; operation.current = null; callbacks.current.onPendingChange(false); };
  }, []);
  useEffect(() => { callbacks.current.onPendingChange(busy || request === 'preview'); }, [busy, request]);
  useEffect(() => {
    if (!enabled && !busy) {
      sequence.current++; approval.current = null; operation.current = null; setRequest(null); callbacks.current.onPendingChange(false); callbacks.current.onClose();
    }
  }, [enabled, busy]);
  useEffect(() => {
    const observed = taskRef.current;
    if (!observed || ['deleted', 'abandoned'].includes(observed.phase)) return;
    let cancelled = false; let reading = false;
    const origin = context.current; const generation = sequence.current;
    const observe = async () => {
      if (reading || cancelled || !isCurrent(origin, generation)) return;
      reading = true;
      try {
        const value = await invoke<NodeDeletionTaskSnapshot>('node_app_delete_status', { taskId: observed.taskId, appId: observed.appId });
        if (!cancelled) acceptTask(value, origin, generation);
      } catch (cause) { if (!cancelled && isCurrent(origin, generation)) setObservationError(t("删除状态暂未更新：{p0}。任务仍保留，请刷新原任务。", { p0: String(messageFrom(cause)) })); }
      finally { reading = false; }
    };
    const refresh = () => void observe();
    refreshStatus.current = refresh;
    const timer = window.setInterval(refresh, 1000);
    return () => { cancelled = true; if (refreshStatus.current === refresh) refreshStatus.current = null; window.clearInterval(timer); };
  }, [task?.taskId, task?.phase, app.id, instanceId]);
  useEffect(() => { if ((error || task?.phase === 'failed') && !busy && request === null) errorPanel.current?.focus(); }, [error, task?.phase, busy, request]);

  const preserved = preview ? [...new Set(preview.preservedPaths)] : [];
  const source = preview && preserved.includes(preview.sourcePath) ? preview.sourcePath : null;
  const otherPreserved = preserved.filter(path => path !== source);
  const base = preview ? platform.commonDirectory([...preview.deletePaths, ...otherPreserved]) : '/';
  const recheck = <button type="button" className="text-button node-delete-recheck" disabled={blocked} onClick={() => void readPreview()}>{t("重新核对删除范围")}</button>;
  const failure = error || (task?.phase === 'failed' ? messageFrom(task.error ?? { message: t("删除未完成，请核对剩余内容后重试。") }) : '');

  return <Modal title={task ? t("删除任务") : t("删除项目")} titleIcon={<Trash2 size={22} aria-hidden="true" />} className="project-delete-modal node-delete-modal" closeDisabled={busy} onClose={close}>
    <section className="node-delete-confirmation" aria-label={t("永久删除 Node 项目确认")}>
      {!task && <div className="node-delete-body" hidden={pending !== null} aria-busy={pending !== null}>
        <h3>{t("永久删除「{name}」？此操作无法撤销。", { name: preview?.name ?? app.name })}</h3>
        <p className="project-delete-warning"><TriangleAlert size={18} />{t("不进入平台或系统回收站，平台也不会为本次删除保存可恢复副本。")}</p>
        <p>{t("会关闭应用窗口，未保存的内容将丢失。")}</p>
        {app.isAgent && <p className="project-help">{t("如果你在 AI 工具里为它设置过定时任务，记得一并关掉，否则 AI 还会继续尝试更新这个文件夹。")}</p>}
        {preview && <>
          <p>{preview.ownership === 'managed' ? t("将永久删除本项目的全部平台管理内容：可编辑源码（包括 Git 和未提交修改）、data/ 文件数据、配置与密钥、日志、缓存、任务材料及浏览器数据。") : t("这是你的文件夹：项目代码、Git 和 data/ 文件数据保留；本实例的平台登记、配置与密钥、日志、缓存、任务材料和浏览器数据将清除。删除后重新添加也无法读取本实例的浏览器数据。")}</p>
          <div className="node-delete-scope">
            <p className="node-delete-base">{t("平台目录")}<code title={base}>{platform.directoryLabel(base)}</code></p>
            <section className="node-delete-removed"><div className="node-delete-scope-heading"><h4>{t("将删除")}</h4>{verified && recheck}</div><PathGroups paths={preview.deletePaths} base={base} /></section>
            <section className="node-delete-kept"><div className="node-delete-scope-heading"><h4>{t("继续保留")}</h4></div>
              {source && <p className="node-delete-source"><span>{t("原始应用目录")}</span><code data-path={source} title={source}>{source}</code></p>}
              {otherPreserved.length > 0 && <PathGroups paths={otherPreserved} base={base} />}
              <p className="node-delete-footnote">{t("平台自带运行时、工具链和平台全局状态不随项目删除。")}</p>
            </section>
          </div><p className="node-delete-id">{t("项目 ID：")}<code>{preview.appId}</code></p>
        </>}
      </div>}
      {pending !== null && <DeleteProgress name={task?.name ?? preview?.name ?? app.name} checking={pending === 'preview'} title={request === 'abandon' || task?.abandonRequested ? t("正在放弃删除…") : undefined} detail={request === 'abandon' || request === 'retry' && task?.abandonRequested ? t("正在放弃删除，等待安全收尾…") : task ? nodeDeletionLabel(task) : undefined} />}
      {task && !active && request === null && <div className="node-delete-body"><h3>{nodeDeletionLabel(task)} · {task.name}</h3>{task.phase === 'abandoned' && <p>{t("已停止本次删除，不会自动启动应用。运行状态将重新核对。")}</p>}{task.phase === 'deleted' && <p>{t("已按确认范围完成删除。")}</p>}</div>}
      {task && <section className="node-delete-task" aria-label={t("删除任务实际范围")}>
        <p className="node-delete-id">{t("任务 ID：")}<code>{task.taskId}</code></p>
        {task.remaining.length > 0 && <><h4>{t("尚未完成")}</h4>{task.remaining.map(item => <p key={item.path}><code>{item.path}</code>{item.stagedPath && item.state !== 'planned' && <code>{t("待清理位置：")}{item.stagedPath}</code>}{item.reason && <span>{t(item.reason)}</span>}</p>)}</>}
        {task.preservedPaths.length > 0 && <><h4>{t("继续保留")}</h4><PathGroups paths={task.preservedPaths} base={platform.commonDirectory(task.preservedPaths)} /></>}
      </section>}
      {observationError && <p className="task-error" role="status">{t(observationError)}</p>}
      <div className="node-delete-footer" hidden={!task && pending === 'delete'}>
        {failure && <div ref={errorPanel} className="node-delete-error" role="alert" tabIndex={-1}><strong>{error ? t(errorTitle) : task?.abandonRequested ? t("放弃删除未完成") : t("删除未完成")}</strong><p>{t(failure)}</p><p>{task?.abandonRequested ? t("重试只会继续放弃删除的安全收尾，不会开始删除或启动应用。") : task ? t("重试会继续本次已确认的任务，不会重新启动应用。") : t("处理上述问题后，重新核对删除范围；本次不会自动重试。")}</p></div>}
        {task ? <div className="confirmation-actions">
          {task.phase === 'failed' && <button className="primary-button" disabled={!enabled || request !== null || busy || !task.canRetry} onClick={() => void actOnTask('retry')}>{task.abandonRequested ? t("重试放弃删除") : t("重试删除")}</button>}
          {task.phase === 'failed' && !active && !task.canRetry && <button className="refresh-button" disabled={request !== null} onClick={() => refreshStatus.current?.()}>{t("重新读取任务")}</button>}
          {task.canAbandon && !task.renamedAny && <button className="refresh-button" disabled={!enabled || request !== null} onClick={() => void actOnTask('abandon')}>{request === 'abandon' ? t("正在放弃删除…") : t("放弃删除")}</button>}
          {task.phase === 'failed' && !task.canAbandon && !task.abandonRequested && <p className="project-help">{task.renamedAny ? t("已有内容进入待清理位置，不能再放弃删除。") : t("正在核对移动或收尾结果，暂不能放弃删除。")}</p>}
          <button className="refresh-button" disabled={busy} onClick={close}>{t("关闭")}</button>
        </div> : <>
          {runUncertain && verified && pending !== 'preview' && <div className="node-delete-error" role="alert"><strong>{t("应用可能还在运行")}</strong><p>{t(runUncertain)}</p><p>{t("请先在详情里停止运行，或关闭相关程序后点「已停止，重新检查」。也可以直接删除：平台会先尽力停止，再删除平台内的数据。")}</p></div>}
          <label className="checkbox-label" hidden={!verified || pending === 'preview'}><input type="checkbox" checked={acknowledged} disabled={blocked || !verified} onChange={event => setAcknowledged(event.target.checked)} />{t("我已核对上述范围，并理解会关闭应用窗口、丢失未保存内容且永久删除无法撤销")}</label>
          <div className="confirmation-actions"><button className="refresh-button" disabled={pending === 'delete'} onClick={close}>{t("取消，保留项目")}</button>{!verified && pending !== 'preview' && <button className="primary-button" disabled={blocked} onClick={() => void readPreview()}>{t("重新核对删除范围")}</button>}{runUncertain && verified && pending !== 'preview' && <button className="refresh-button" disabled={blocked} onClick={() => void readPreview()}>{t("已停止，重新检查")}</button>}<button className="danger-button" hidden={!verified || pending === 'preview'} disabled={blocked || !verified || !acknowledged} onClick={() => void deleteApp()}>{pending === 'delete' ? t("正在永久删除…") : runUncertain ? t("直接删除") : t("确认永久删除项目")}</button></div>
        </>}
      </div>
    </section>
  </Modal>;
}
