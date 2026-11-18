import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { FolderMinus, FolderSearch, TriangleAlert, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { DeleteProgress } from './DeleteProgress';
import { Modal } from './Modal';
import type { DeletePresentation } from './project-deletion';
import { ProjectTask } from './ProjectTask';
import { projectSource } from './project-source';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform } from './frontend-platform';

interface DeletePreview {
  projectId: string; instanceId: string; projectRoot: string | null;
  expectedDocumentEpoch: string | null; mode: string; maintenanceId: string | null;
  deletePaths: string[]; alreadyAbsent: boolean; cleanupOnly: boolean;
  ownership: 'user' | 'platform' | null; sourcePreserved: boolean;
}
type ReviewedPreview = { scope: string; value: DeletePreview };

function validPreview(value: DeletePreview, project: ProjectInfo, instanceId: string | undefined, isAbsolutePath: (path: string) => boolean) {
  const absent = value?.alreadyAbsent === true || value?.cleanupOnly === true;
  return Boolean(value && value.projectId === project.id && typeof value.instanceId === 'string' && value.instanceId.length > 0 && (!instanceId || value.instanceId === instanceId)
    && typeof value.alreadyAbsent === 'boolean' && typeof value.cleanupOnly === 'boolean' && !(value.alreadyAbsent && value.cleanupOnly)
    && (absent ? value.projectRoot === null && value.expectedDocumentEpoch === null : value.projectRoot === project.projectRoot && typeof value.expectedDocumentEpoch === 'string' && value.expectedDocumentEpoch.length > 0)
    && (absent || value.ownership === project.ownership && value.sourcePreserved === (project.ownership === 'user'))
    && ['ready', 'maintenance', 'unknown'].includes(value.mode) && (value.maintenanceId === null || typeof value.maintenanceId === 'string')
    && (value.mode !== 'maintenance' || Boolean(value.maintenanceId))
    && Array.isArray(value.deletePaths) && value.deletePaths.every(path => typeof path === 'string' && isAbsolutePath(path)) && (!value.alreadyAbsent || value.deletePaths.length === 0));
}

function ScopeColumn({ heading, items, tone, note }: {
  heading: string; items: string[]; tone?: 'keep' | 'delete'; note?: string;
}) {
  return <section className={`project-delete-scope${tone ? ` is-${tone}` : ''}`}>
    <h3>{heading}</h3>
    <ul>{items.map(item => <li key={item}>{item}</li>)}</ul>
    {note && <p className="project-delete-scope-note">{note}</p>}
  </section>;
}

export function ProjectDeleteDialog({ project, status, task, instanceId, disabled, running = false, onTask, onClose, onPendingChange, feedback }: {
  project: ProjectInfo;
  status?: MaintenanceStatus;
  task?: TaskRecord;
  instanceId?: string;
  disabled: boolean;
  running?: boolean;
  onTask: (task: TaskRecord, context?: DeletePresentation) => void;
  onClose: () => void;
  onPendingChange?: (pending: boolean) => void;
  feedback?: ReactNode;
}) {
  const platform = useFrontendPlatform();
  const [acknowledgedIdentity, setAcknowledgedIdentity] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [accepted, setAccepted] = useState<TaskRecord | null>(null);
  const [error, setError] = useState('');
  const [reviewedPreview, setReviewedPreview] = useState<ReviewedPreview | null>(null);
  const [reading, setReading] = useState(true);
  const [readError, setReadError] = useState('');
  const alive = useRef(true);
  const operation = useRef(false);
  const sequence = useRef(0);
  const identity = JSON.stringify([project.id, project.projectRoot, project.documentEpoch, instanceId]);
  const previewScope = JSON.stringify([identity, project.ownership, status?.mode, status?.maintenanceId]);
  const preview = reviewedPreview?.scope === previewScope ? reviewedPreview.value : null;
  const confirmationIdentity = JSON.stringify([previewScope, project.name, preview?.instanceId, preview?.expectedDocumentEpoch, preview?.mode,
    preview?.maintenanceId, preview?.deletePaths, preview?.ownership, preview?.sourcePreserved, preview?.alreadyAbsent, preview?.cleanupOnly]);
  const currentPreviewScope = useRef(previewScope); currentPreviewScope.current = previewScope;
  const currentConfirmation = useRef(confirmationIdentity); currentConfirmation.current = confirmationIdentity;
  const context = useRef(identity); context.current = identity;
  const acceptedIdentity = useRef<string | null>(null);
  const submitted = useRef<{ identity: string; ownership: DeletePresentation['ownership'] } | null>(null);
  const callbacks = useRef({ onTask, onClose, onPendingChange }); callbacks.current = { onTask, onClose, onPendingChange };
  const ownership = preview && !preview.alreadyAbsent && !preview.cleanupOnly ? preview.ownership : null;
  const requiresAcknowledgement = ownership !== 'user';
  const acknowledged = acknowledgedIdentity === confirmationIdentity;
  const observed = acceptedIdentity.current !== identity ? null : accepted && task?.taskId === accepted.taskId && task.projectId === project.id && task.type === 'delete' && (!accepted.final || task.final) ? task : accepted;
  const busy = submitting || Boolean(observed && !observed.final);
  const unavailable = disabled || reading || !preview || Boolean(status?.activeTask);
  const blocked = unavailable || busy;
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; callbacks.current.onPendingChange?.(false); };
  }, []);
  useEffect(() => {
    sequence.current++; setAcknowledgedIdentity(null); acceptedIdentity.current = null; setAccepted(null); setError(''); setReviewedPreview(null);
  }, [identity]);
  useEffect(() => { setAcknowledgedIdentity(null); }, [confirmationIdentity]);
  useEffect(() => { if (unavailable) setAcknowledgedIdentity(null); }, [unavailable]);
  useEffect(() => {
    if (acceptedIdentity.current === identity && observed?.final && accepted && !accepted.final) setAccepted(observed);
  }, [identity, observed, accepted]);
  useEffect(() => { callbacks.current.onPendingChange?.(busy); }, [busy]);

  async function readPreview() {
    if (operation.current || observed && !observed.final) return;
    const request = ++sequence.current; const scope = previewScope;
    setReading(true); setReviewedPreview(null); setReadError(''); setAcknowledgedIdentity(null);
    const current = () => alive.current && request === sequence.current && currentPreviewScope.current === scope;
    try {
      const value = await invoke<DeletePreview>('app_delete_preview', { appId: project.id });
      if (!current()) return;
      if (!validPreview(value, project, instanceId, platform.isAbsolutePath)) throw new Error(t("删除范围或应用身份不符，请重新核对。"));
      setReviewedPreview({ scope, value });
    } catch (cause) { if (current()) setReadError(messageFrom(cause)); }
    finally { if (current()) setReading(false); }
  }
  useEffect(() => {
    if (!busy && !preview) void readPreview();
  }, [previewScope, busy]);

  async function remove() {
    if (operation.current || blocked || !preview || currentConfirmation.current !== confirmationIdentity
      || requiresAcknowledgement && !acknowledged) return;
    operation.current = true; setSubmitting(true); setAcknowledgedIdentity(null); setError('');
    const request = ++sequence.current; const selected = identity;
    submitted.current = { identity: selected, ownership };
    const current = () => alive.current && request === sequence.current && context.current === selected;
    try {
      const value = await invoke<TaskRecord>('app_delete', { appId: project.id, input: { ...(preview.expectedDocumentEpoch ? { expectedDocumentEpoch: preview.expectedDocumentEpoch } : {}), ...(preview.mode === 'maintenance' ? { maintenanceId: preview.maintenanceId } : {}) } });
      if (!current()) return;
      if (!value?.taskId || value.projectId !== project.id || value.type !== 'delete' || typeof value.final !== 'boolean') throw new Error(t("删除任务的应用身份或结果不符。请核对任务记录，不要重复提交。"));
      acceptedIdentity.current = selected; setAccepted(value);
      if (value.status === 'failed' || value.status === 'interrupted') setError(value.error ? messageFrom(value.error) : t("操作未完成，请核对任务结果后再决定。"));
      callbacks.current.onTask(value, {
        ownership,
        sourceExists: Boolean(preview.projectRoot && ['ready', 'needsBuild', 'entryMissing', 'needsConfiguration'].includes(project.state)),
      });
    } catch (cause) { if (current()) setError(messageFrom(cause)); }
    finally { operation.current = false; if (alive.current) setSubmitting(false); }
  }
  function close() {
    if (!operation.current && !busy) callbacks.current.onClose();
  }
  async function revealFolder() {
    const selected = identity;
    try { await invoke('project_reveal', { projectId: project.id, target: 'root' }); }
    catch (cause) { if (alive.current && context.current === selected) setError(messageFrom(cause)); }
  }

  const source = projectSource(project);
  const sourceTitle = source.kind === 'downloaded' ? t("下载的应用")
    : source.kind === 'created' ? t("新建的应用") : source.kind === 'local' ? t("本机文件夹") : t("来源待核对");
  const taskOwnership = submitted.current?.identity === identity ? submitted.current.ownership : undefined;
  const displayOwnership = busy ? taskOwnership : ownership;
  const removing = displayOwnership === 'user';
  const deleting = displayOwnership === 'platform';
  const title = removing ? t("把「{name}」移出小抽屉？", { name: project.name })
    : deleting ? t("删除「{name}」？", { name: project.name }) : t("核对「{name}」的处理范围", { name: project.name });
  const description = removing ? t("文件夹里的代码和 data/ 会保留。平台配置、密钥和浏览器数据会清除。")
    : deleting ? t("应用会整个删掉，包括你和 AI 改过的代码。删除后无法恢复。") : t("小抽屉会根据核对结果处理这个应用。");
  const sourceReason = removing
    ? source.kind === 'downloaded' ? t("这个应用下载到了你选的文件夹。小抽屉记录它的位置，并在需要时运行应用。")
      : t("这个文件夹由你保管。小抽屉记录它的位置，并在需要时运行应用。")
    : source.kind === 'downloaded'
      ? t("这个应用是小抽屉帮你下载的，文件放在小抽屉的存放位置里，由小抽屉管理。所以删除时，文件也一起删掉。")
      : source.kind === 'created'
        ? t("这个应用是小抽屉帮你新建的，文件放在小抽屉的存放位置里，由小抽屉管理。所以删除时，文件也一起删掉。")
        : t("这个应用的文件夹由小抽屉管理，删除时会一起清除。");
  const sourcePath = removing ? preview?.projectRoot : undefined;
  const repository = source.host && source.repository
    ? t("来自 {host} · {path}", { host: source.host, path: source.repository }) : source.url;
  const reviewLabel = removing ? t("移出应用确认") : deleting ? t("删除应用确认") : t("应用处理确认");
  const progressTitle = removing ? t("正在移出…") : deleting ? t("正在永久删除…") : t("正在处理应用…");
  const progressDetails: Record<string, string> = {
    queued: removing ? t("移出任务已受理，正在等待执行") : deleting ? t("删除任务已受理，正在等待执行") : t("任务已受理，正在等待执行"),
    'delete-freeze': t("正在停止应用写入并关闭数据库"),
    'delete-prepared': removing ? t("正在核对应用身份和移出范围") : deleting ? t("正在核对应用身份和删除范围") : t("正在核对应用身份和处理范围"),
    'delete-intent': removing ? t("正在清除小抽屉里的配置和浏览器数据") : deleting ? t("正在永久删除应用文件") : t("正在处理已核对的材料"),
    finalizing: t("正在清除应用登记与关联材料"),
    'delete-finalizing': t("正在清除应用登记与关联材料"),
  };
  const progressDetail = submitting
    ? removing ? t("正在提交移出请求") : deleting ? t("正在提交删除请求") : t("正在提交处理请求")
    : progressDetails[observed?.phase ?? ''] ?? (removing ? t("正在等待移出任务的最终结果") : deleting ? t("正在等待删除任务的最终结果") : t("正在等待任务的最终结果"));
  const action = removing ? t("移出小抽屉") : deleting ? t("永久删除")
    : preview?.alreadyAbsent ? t("核对完成状态") : preview?.cleanupOnly ? t("清除剩余材料") : t("确认处理");
  const acknowledgement = deleting ? t("我知道删除后无法恢复") : t("我已核对处理范围");
  const runningWarning = removing ? t("{name} 正在运行，移出前会先关掉它的窗口，没保存的内容会丢失。", { name: project.name })
    : deleting ? t("{name} 正在运行，删除前会先关掉它的窗口，没保存的内容会丢失。", { name: project.name })
      : t("{name} 正在运行或仍有窗口，请先保存内容。", { name: project.name });
  return <Modal title={busy ? progressTitle : title}
    titleIcon={removing ? <FolderMinus size={22} aria-hidden="true" /> : deleting ? <Trash2 size={22} aria-hidden="true" /> : <FolderSearch size={22} aria-hidden="true" />}
    description={busy ? undefined : description}
    className={`project-delete-modal app-project-delete-modal${removing ? ' is-removal' : deleting ? ' is-deletion' : ' is-review'}`}
    closeDisabled={busy} beforeClose={() => !operation.current && !busy} onClose={close}>
    <section className="project-delete-confirmation" aria-label={reviewLabel}>
      <div className="project-delete-review" hidden={busy} aria-busy={busy}>
        {(removing || deleting) && <section className="project-delete-source">
          <h3>{removing && <span className="project-source-dot" aria-hidden="true" />}{sourceTitle}</h3>
          {sourcePath && <div className="project-delete-source-location">
            <div>{source.kind === 'downloaded' && <p>{t("放在你选的文件夹")}</p>}<code>{sourcePath}</code></div>
            {project.state !== 'missing' && <button type="button" className="text-button" disabled={blocked} onClick={() => void revealFolder()}>{t("打开")}</button>}
          </div>}
          {deleting && repository && <p className="project-delete-repository">{repository}</p>}
          <p className="project-delete-source-reason"><strong>{removing ? t("为什么是「移出」？") : t("为什么是「删除」？")}</strong>{sourceReason}</p>
        </section>}
        {removing && <div className="project-delete-columns">
          <ScopeColumn heading={t("会保留")} tone="keep" items={[t("文件夹和全部代码"), t("文件夹里的 Git 历史"), t("data/ 下的数据库和附件")]} />
          <ScopeColumn heading={t("会清除")} items={[t("在小抽屉里填的配置和密钥"), t("浏览器里存的数据"), t("应用登记、日志、任务和缓存")]} note={t("重新添加也找不回")} />
        </div>}
        {deleting && <div className="project-delete-columns">
          <ScopeColumn heading={t("会删除")} tone="delete" items={[t("应用文件夹和全部代码"), t("应用里的数据和附件"), t("配置和密钥"), t("浏览器里存的数据"), t("应用登记、日志、任务和缓存")]} />
          <ScopeColumn heading={t("不受影响")} items={[
            ...(source.host ? [t("{host} 上的原仓库", { host: source.host })] : source.kind === 'downloaded' ? [t("原仓库")] : []),
            t("其他应用"), t("小抽屉的设置"), t("已保存到外部文件夹的副本"),
          ]} />
        </div>}
        {deleting && project.state !== 'missing' && <div className="project-delete-copy">
          <p>{t("想留着改过的代码？先打开文件夹复制一份。")}</p>
          <button type="button" className="text-button" disabled={blocked} onClick={() => void revealFolder()}>{t("打开文件夹")}</button>
        </div>}
        {running && <p className="project-delete-running"><TriangleAlert size={18} aria-hidden="true" />{runningWarning}</p>}
        {reading && <p className="project-help" role="status">{t("正在核对删除范围…")}</p>}
        {readError && <p className="task-error" role="alert">{t("无法核对删除范围：")}{t(readError)}</p>}
        {preview?.alreadyAbsent && <p className="project-help">{t("应用已不存在，确认后将核对完成状态。")}</p>}
        {preview?.cleanupOnly && <p className="project-help">{t("本次只清除已核实归属的剩余材料。")}</p>}
        {preview?.mode === 'unknown' && <p className="project-help">{t("应用状态异常，操作会按已核对的范围执行。")}</p>}
        {status?.recoveryRequired && <p className="task-error">{t("当前未完成操作会先停止，再按已核实的范围清除平台材料。")}</p>}
        {!busy && (disabled || Boolean(status?.activeTask)) && <p className="project-help">{t("当前操作进行中或目标已变化，请等待完成并重新核对。")}</p>}
        <div className="project-delete-review-tools">
          {!!preview?.deletePaths.length && <details className="project-delete-paths">
            <summary>{t("查看具体位置")}</summary>
            {preview.deletePaths.map(path => <code className="workspace-scope-path" key={path}>{path}</code>)}
          </details>}
          {!reading && !busy && <button type="button" className="text-button" onClick={() => void readPreview()}>{deleting ? t("重新核对删除范围") : t("重新核对范围")}</button>}
        </div>
      </div>
      {busy && <DeleteProgress name={project.name} title={progressTitle} detail={progressDetail} />}
      {error && <p className="task-error" role="alert">{t(error)}</p>}
      {observed && (observed.final ? <ProjectTask task={observed} deletionOwnership={taskOwnership} />
        : <details className="delete-task-details"><summary>{removing ? t("查看移出任务详情") : deleting ? t("查看删除任务详情") : t("查看任务详情")}</summary><ProjectTask task={observed} deletionOwnership={taskOwnership} /></details>)}
      {feedback}
      <div className="project-delete-footer modal-footer" hidden={busy}>
        {requiresAcknowledgement && <label className="checkbox-label">
          <input type="checkbox" checked={acknowledged} disabled={blocked} onChange={event => setAcknowledgedIdentity(event.target.checked ? confirmationIdentity : null)} />{acknowledgement}
        </label>}
        <div className="confirmation-actions">
          <button type="button" className="refresh-button" disabled={busy} onClick={close}>{t("取消")}</button>
          <button type="button" className={deleting ? 'danger-button' : 'primary-button'}
            disabled={blocked || requiresAcknowledgement && !acknowledged} onClick={() => void remove()}>{busy ? progressTitle : action}</button>
        </div>
      </div>
    </section>
  </Modal>;
}
