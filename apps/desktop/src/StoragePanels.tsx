import { t } from './i18n';
import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, Download, Eye, File, FileAudio, FileImage, FileVideo, Search, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ProjectInfo, StorageFiles, StoredFile } from './types';
import { bytesText, timeText, exactBytes } from './storage-utils';
import { messageFrom } from './ui-utils';

function issueText(issue: unknown) { if (typeof issue === 'object' && issue) { const data = issue as Record<string, unknown>; return [data.path, t(String(data.message ?? data.reason ?? data.code ?? ''))].filter(Boolean).join(' · '); } return String(issue); }
function Issues({ items }: { items?: unknown[] }) { return items?.length ? <details className="storage-issues"><summary>{t("{count} 项未能完整统计 / 需要检查", { count: items.length })}</summary>{items.map((issue, index) => <p key={index}>{t(issueText(issue))}</p>)}</details> : null; }
const defaultFileFilters = { type: 'all', prefix: '', query: '', sort: 'name', order: 'asc' };
function fileType(file: StoredFile) {
  const type = file.displayType ?? file.mediaType ?? file.mimeType ?? file.contentType ?? '';
  if (/^image(?:\/|$)|图片/.test(type)) return { label: t("图片"), Icon: FileImage };
  if (/^audio(?:\/|$)|音频/.test(type)) return { label: t("音频"), Icon: FileAudio };
  if (/^video(?:\/|$)|视频/.test(type)) return { label: t("视频"), Icon: FileVideo };
  return { label: /document|pdf|text|文档/.test(type) ? t("文档") : t("文件"), Icon: File };
}
export function ProjectAttachmentBrowser({ project, blocked, onBusyChange }: { project: ProjectInfo; blocked: boolean; onBusyChange: (value: boolean) => void }) {
  const [expanded, setExpanded] = useState(false);
  const [filters, setFilters] = useState(defaultFileFilters);
  const [applied, setApplied] = useState(defaultFileFilters);
  const [result, setResult] = useState<StorageFiles | null>(null);
  const [loading, setLoading] = useState(false);
  const [accessing, setAccessing] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [preview, setPreview] = useState<{ path: string; dataUrl: string; mimeType: string } | null>(null);
  const request = useRef(0);
  const listPending = useRef(false);
  const mounted = useRef(false);
  const busyCallback = useRef(onBusyChange); busyCallback.current = onBusyChange;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; busyCallback.current(false); }; }, []);
  useEffect(() => { busyCallback.current(accessing); }, [accessing]);
  const previewRequest = useRef(0);
  const pending = useRef(false);
  const identity = `${project.id}:${project.documentEpoch}`;
  const scope = useRef(identity); scope.current = identity;
  const blockedRef = useRef(blocked); blockedRef.current = blocked;
  const list = useCallback(async (cursor?: string) => {
    const sequence = ++request.current; listPending.current = true; setLoading(true); setError('');
    try {
      const value = await invoke<StorageFiles>('storage_files', { projectId: project.id, input: { ...applied, type: applied.type === 'all' ? undefined : applied.type, limit: expanded ? 40 : 2, ...(cursor ? { cursor } : {}) } });
      if (sequence === request.current && identity === scope.current) setResult((previous) => ({ ...value, files: cursor ? [...(previous?.files ?? []), ...value.files] : value.files }));
    } catch (cause) { if (sequence === request.current && identity === scope.current) setError(messageFrom(cause)); }
    finally { if (sequence === request.current && identity === scope.current) { listPending.current = false; setLoading(false); } }
  }, [project.id, identity, applied, expanded]);
  useEffect(() => { setExpanded(false); setFilters(defaultFileFilters); setApplied(defaultFileFilters); setNotice(''); setAccessing(false); pending.current = false; }, [identity]);
  useEffect(() => { setResult(null); setPreview(null); previewRequest.current++; void list(); return () => { request.current++; previewRequest.current++; }; }, [list]);
  useEffect(() => { if (blocked) { previewRequest.current++; setPreview(null); } }, [blocked, project.documentEpoch]);
  async function access(file: StoredFile, kind: 'preview' | 'download') {
    if (blockedRef.current || pending.current) return;
    pending.current = true; setAccessing(true); setError(''); setNotice(''); const sequence = ++previewRequest.current;
    const isCurrent = () => mounted.current && sequence === previewRequest.current && identity === scope.current && !blockedRef.current;
    try {
      if (kind === 'preview') {
        const value = await invoke<{ dataUrl: string; mimeType: string }>('storage_preview', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch });
        if (!isCurrent()) return;
        if (!/^(image\/(png|jpeg|gif|webp)|audio\/(mpeg|wav|mp4|ogg|webm)|video\/(mp4|webm))$/.test(value.mimeType) || !value.dataUrl.startsWith(`data:${value.mimeType};base64,`)) throw new Error(t("此格式不支持安全预览，请下载文件。"));
        setPreview({ path: file.path, ...value });
      } else {
        const value = await invoke<{ savedPath: string } | null>('storage_download', { projectId: project.id, path: file.path, documentEpoch: project.documentEpoch });
        if (isCurrent() && value) setNotice(t("已保存到 {value0}", { value0: value.savedPath }));
      }
    } catch (cause) { if (isCurrent()) setError(messageFrom(cause)); }
    finally { if (mounted.current && identity === scope.current) { pending.current = false; setAccessing(false); } }
  }
  const files = expanded ? result?.files : result?.files.slice(0, 2);
  return <section className="project-storage-files" aria-labelledby="project-storage-files-title"><div className="project-storage-files-heading"><h3 id="project-storage-files-title">{t("用户文件")}</h3><button className="text-button" aria-expanded={expanded} onClick={() => { setExpanded((value) => !value); setFilters(defaultFileFilters); setApplied(defaultFileFilters); }}>{expanded ? t("收起文件列表") : t("浏览全部文件")}<ChevronRight className={expanded ? 'is-expanded' : ''} size={16} /></button></div>
    {expanded && <><p className="project-storage-file-note">{t("浏览应用保存或上传的文件。图片、音频和视频可预览（上限 16 MB）；其他文档和较大的文件可下载。")}</p><form className="attachment-filters" onSubmit={(event) => { event.preventDefault(); if (!loading) setApplied({ ...filters }); }}>
      <label>{t("类型")}<select value={filters.type} onChange={(event) => setFilters({ ...filters, type: event.target.value })}>{[['all', t("全部")], ['image', t("图片")], ['audio', t("音频")], ['video', t("视频")], ['document', t("文档")], ['other', t("其他")]].map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <label>{t("目录前缀")}<input placeholder={t("例如 lessons/")} value={filters.prefix} onChange={(event) => setFilters({ ...filters, prefix: event.target.value })} /></label><label>{t("名称搜索")}<input value={filters.query} onChange={(event) => setFilters({ ...filters, query: event.target.value })} /></label>
      <label>{t("排序")}<select value={filters.sort} onChange={(event) => setFilters({ ...filters, sort: event.target.value })}><option value="name">{t("名称")}</option><option value="size">{t("大小")}</option><option value="modifiedAt">{t("修改时间")}</option></select></label><label>{t("顺序")}<select value={filters.order} onChange={(event) => setFilters({ ...filters, order: event.target.value })}><option value="asc">{t("升序")}</option><option value="desc">{t("降序")}</option></select></label><button className="refresh-button" disabled={loading}><Search size={16} />{t("查询")}</button>
    </form></>}
    {blocked && <p className="environment-issue">{t("当前有维护、任务或恢复状态需要处理，暂不能预览或下载文件。")}</p>}
    <div className="project-storage-file-list">{files?.map((file) => { const { label, Icon } = fileType(file); return <div className="project-storage-file-row" key={file.path}><Icon size={20} aria-hidden="true" /><span className="project-storage-file-name" title={file.path}>{file.path}</span><span className="project-storage-file-meta" title={`${exactBytes(file.sizeBytes ?? file.size)} · ${timeText(file.modifiedAt)}`}>{t(label)} · {bytesText(file.sizeBytes ?? file.size)}</span><div className="project-storage-file-actions">{expanded && <button className="icon-button" aria-label={t("预览 {value0}", { value0: file.path })} title={t("预览")} disabled={blocked || accessing} onClick={() => void access(file, 'preview')}><Eye size={18} /></button>}<button className="icon-button" aria-label={t("下载 {value0}", { value0: file.path })} title={t("下载")} disabled={blocked || accessing} onClick={() => void access(file, 'download')}><Download size={18} /></button></div></div>; })}</div>
    {!loading && result && !result.files.length && <p>{t("当前条件下没有用户文件。")}</p>}{loading && <p role="status">{t("正在读取文件列表…")}</p>}
    {expanded && <><p className="project-storage-file-note">{t("已载入 {count} 个文件", { count: result?.files.length ?? 0 })}{result?.nextCursor ? t(" · 还有下一页") : ''}{t("。列表为当前筛选条件下的分页结果。")}</p>{result?.nextCursor && <button className="refresh-button" disabled={loading} onClick={() => { if (!listPending.current) void list(result.nextCursor!); }}>{t("继续读取下一页")}</button>}{result && <p className="project-storage-file-note">{result.complete ? t("本次枚举完整") : t("部分结果")} · {timeText(result.completedAt)}</p>}</>}
    <Issues items={result?.issues} />
    {preview && <div className="attachment-preview"><div><strong>{preview.path}</strong><button className="icon-button" onClick={() => { previewRequest.current++; setPreview(null); }} aria-label={t("关闭文件预览")}><X size={16} /></button></div>{preview.mimeType.startsWith('image/') ? <img src={preview.dataUrl} alt={preview.path} /> : preview.mimeType.startsWith('audio/') ? <audio controls src={preview.dataUrl} /> : <video controls src={preview.dataUrl} />}</div>}
    {error && <p role="alert" className="environment-issue">{t(error)}</p>}{notice && <p role="status">{t(notice)}</p>}
  </section>;
}
