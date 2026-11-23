import { t } from './i18n';
import { useAiTools } from './AiToolsProvider';
import { invoke } from '@tauri-apps/api/core';
import { Copy, ExternalLink, FolderOpen, Sparkles } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { Modal } from './Modal';
import { buildProjectAiPrompt, checkedProjectReviewExcerpt } from './project-ai-help';
import type { ProjectAiHelpContext, ProjectReviewExcerpt } from './project-ai-help';
import { messageFrom } from './ui-utils';
import { useFrontendPlatform, usePlatformCliPath, usePlatformSkillPath } from './frontend-platform';

export type { ProjectAiHelpContext } from './project-ai-help';

let clipboardQueue = Promise.resolve();

/** Longest explanation the native handoff accepts from the manager. */
const MAX_HANDOFF_TEXT = 20000;
const repositoryName = (url?: string) => url?.replace(/\.git$/i, '').replace(/\/+$/, '').split('/').pop()?.slice(0, 200) || undefined;

export function AiProjectHelp({ context, label, disabled = false, className = 'text-button' }: {
  context: ProjectAiHelpContext;
  label?: string;
  disabled?: boolean;
  className?: string;
}) {
  const platform = useFrontendPlatform();
  const ai = useAiTools();
  const skillPath = usePlatformSkillPath();
  const cliPath = usePlatformCliPath();
  const promptOptions = { platform, skillPath, cliPath };
  const [open, setOpen] = useState(false);
  const [target, setTarget] = useState(context.sourcePath ?? context.url ?? '');
  const [aiKind, setAiKind] = useState<'local' | 'web'>('local');
  const [preview, setPreview] = useState(false);
  const [fileHelp, setFileHelp] = useState(false);
  const [excerpt, setExcerpt] = useState<ProjectReviewExcerpt | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [pending, setPending] = useState<'copy' | 'choose' | 'handoff' | 'reveal' | null>(null);
  const [reading, setReading] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [excerptError, setExcerptError] = useState('');
  const alive = useRef(true);
  const sequence = useRef(0);
  const readSequence = useRef(0);
  const operation = useRef(false);
  const promptField = useRef<HTMLTextAreaElement>(null);
  /** One platform reservation per failed link, reused by repeated clicks. */
  const reservation = useRef<{ id: string; path: string } | null>(null);
  const id = useId();
  const signature = JSON.stringify(context);
  const latest = useRef({ signature, disabled }); latest.current = { signature, disabled };
  const evaluation = context.mode === 'evaluation';
  const sourcePath = evaluation ? (platform.isAbsolutePath(target.trim()) ? target.trim() : '') : context.sourcePath ?? '';
  const effectiveContext: ProjectAiHelpContext = evaluation ? {
    ...context,
    sourcePath: sourcePath || undefined,
    url: /^https?:\/\//i.test(target.trim()) ? target.trim() : undefined,
  } : context;
  const text = buildProjectAiPrompt(effectiveContext, {
    aiKind,
    target,
    files: aiKind === 'web' && excerpt ? excerpt.files.filter(file => selectedFiles.includes(file.path)) : [],
    excerptTruncated: excerpt?.truncated,
    ...promptOptions,
  });
  const locked = disabled || pending !== null || ai.busy;
  // Every "hand to AI" entry uses the user's chosen tool where the platform
  // supports it; copying stays available as the secondary action.
  const handoffReady = platform.aiHandoffAvailable && ai.enabled;
  const directHandoff = handoffReady && !evaluation && (context.mode === 'start-failed' ? Boolean(context.appId) : Boolean(context.sourcePath || context.sourceKind === 'git'));
  const triggerLabel = directHandoff
    ? context.sourceKind === 'local' ? t('告诉 {tool} 怎么处理', { tool: ai.buttonTool }) : t('告诉 {tool} 出了什么问题', { tool: ai.buttonTool })
    : label ?? (evaluation ? handoffReady && ai.buttonTool !== 'AI' ? t('不确定能不能用？问问 {tool}', { tool: ai.buttonTool }) : t('不确定能不能用？问问你的 AI 工具') : t("复制提示词"));

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; readSequence.current++; };
  }, []);
  useEffect(() => {
    sequence.current++; readSequence.current++; operation.current = false;
    setOpen(false); setPending(null); setReading(false); setNotice(''); setError('');
    setExcerpt(null); setSelectedFiles([]); setExcerptError('');
    setTarget(context.sourcePath ?? context.url ?? '');
    reservation.current = null;
  }, [signature]);

  useEffect(() => {
    const request = ++readSequence.current;
    setExcerpt(null); setSelectedFiles([]); setExcerptError(''); setReading(false);
    if (!open || aiKind !== 'web' || !sourcePath) return;
    let cancelled = false;
    setReading(true);
    void invoke<ProjectReviewExcerpt>('source_review_context', { sourcePath }).then(value => {
      if (cancelled || !alive.current || readSequence.current !== request) return;
      const checked = checkedProjectReviewExcerpt(value, sourcePath);
      setExcerpt(checked); setSelectedFiles(checked.files.map(file => file.path));
    }).catch(cause => {
      if (!cancelled && alive.current && readSequence.current === request) setExcerptError(messageFrom(cause));
    }).finally(() => {
      if (!cancelled && alive.current && readSequence.current === request) setReading(false);
    });
    return () => { cancelled = true; };
  }, [open, aiKind, sourcePath]);

  function close() {
    if (pending !== null) return;
    sequence.current++; readSequence.current++; setOpen(false); setNotice(''); setError('');
  }
  async function run(kind: NonNullable<typeof pending>, task: (current: () => boolean) => Promise<void>) {
    if (operation.current || latest.current.disabled) return;
    operation.current = true; setPending(kind); setNotice(''); setError('');
    const request = ++sequence.current; const origin = latest.current.signature;
    const current = () => alive.current && sequence.current === request && latest.current.signature === origin && !latest.current.disabled;
    try { await task(current); }
    catch (cause) { if (current()) { setError(messageFrom(cause)); setPreview(true); } }
    finally { if (current()) { operation.current = false; setPending(null); } }
  }
  async function copy(value: string, current: () => boolean): Promise<boolean> {
    let copied = false;
    const queued = clipboardQueue.then(async () => {
      if (!current()) return;
      if (!navigator.clipboard?.writeText) throw new Error(t("无法自动复制。完整提示词仍可在下方预览中选择后手动复制。"));
      await navigator.clipboard.writeText(value); copied = true;
    });
    clipboardQueue = queued.catch(() => {});
    await queued;
    return copied;
  }
  function showDialog() {
    setOpen(true); setAiKind('local'); setPreview(false); setFileHelp(false); setNotice('');
  }
  function openCopyDialog() {
    if (locked) return;
    showDialog(); setError('');
  }
  /** Reserve the app's own empty folder for a failed link, so the AI works there. */
  async function reserveFolder(): Promise<{ id: string; path: string }> {
    const value = await invoke<{ reservation?: unknown; path?: unknown }>('app_reserve', { name: repositoryName(context.url) ?? null });
    if (typeof value?.reservation !== 'string' || typeof value.path !== 'string' || !platform.isAbsolutePath(value.path)) throw new Error(t("暂时无法为这个应用预留文件夹，请复制提示词发给你的 AI 工具。"));
    return { id: value.reservation, path: value.path };
  }
  function handOver() {
    if (locked) return;
    void run('handoff', async current => {
      const appName = context.appName ?? (repositoryName(context.url) || t("本次应用"));
      if (context.mode === 'start-failed' && context.appId) {
        const handoffText = buildProjectAiPrompt(context, { aiKind: 'local', target: context.sourcePath, ...promptOptions });
        if ([...handoffText].length > MAX_HANDOFF_TEXT) { showDialog(); return; }
        await ai.handoff({ appId: context.appId, appName, purpose: 'repair', text: handoffText, entry: 'start-failed' });
        return;
      }
      let folderContext = context;
      let entry = 'add-failed-folder';
      if (!context.sourcePath) {
        // The failed link has no folder yet: reuse the platform's reservation.
        let reserved: { id: string; path: string };
        try { reserved = reservation.current ?? await reserveFolder(); }
        catch (cause) { if (current()) { setError(messageFrom(cause)); showDialog(); } return; }
        if (!current()) return;
        reservation.current = reserved;
        folderContext = { ...context, reservation: reserved };
        entry = 'add-failed-link';
      }
      const directory = folderContext.sourcePath ?? folderContext.reservation!.path;
      const handoffText = buildProjectAiPrompt(folderContext, { aiKind: 'local', target: directory, ...promptOptions });
      if ([...handoffText].length > MAX_HANDOFF_TEXT) { showDialog(); return; }
      await ai.handoff({ sourcePath: directory, appName, text: handoffText, entry });
    });
  }
  function evaluateWithTool() {
    if (!sourcePath || reading || locked) return;
    void run('handoff', async () => {
      await ai.handoff({ sourcePath, appName: platform.pathBaseName(sourcePath) || t("本次应用"), text, entry: 'evaluation' });
    });
  }
  function openHelp() {
    if (locked) return;
    if (directHandoff) { handOver(); return; }
    setOpen(true); setAiKind('local'); setPreview(false); setFileHelp(false); setNotice(''); setError('');
    if (!evaluation) {
      const initialText = buildProjectAiPrompt(context, { aiKind: 'local', target: context.sourcePath ?? context.url, ...promptOptions });
      void run('copy', async current => {
        if (await copy(initialText, current) && current()) setNotice(t("已复制。粘贴给 AI，先看结论和下一步；复制提示词不会自动修改应用。"));
      });
    }
  }
  function copyPrompt() {
    if (reading || locked) return;
    void run('copy', async current => {
      if (await copy(text, current) && current()) setNotice(t("已复制。粘贴给 AI：可以接入就回来添加；需要修改就按它的建议处理后再添加。"));
    });
  }
  function choose() {
    void run('choose', async current => {
      const value = await invoke<string | null>('choose_path', { kind: 'source' });
      if (current() && value) setTarget(value);
    });
  }
  function reveal() {
    if (!sourcePath) return;
    void run('reveal', async current => {
      await invoke('open_project_folder', { path: sourcePath });
      if (current()) setNotice(t(platform.folderRevealNotice));
    });
  }
  function copyPath() {
    void run('copy', async current => { if (await copy(sourcePath, current) && current()) setNotice(t("文件夹路径已复制。")); });
  }

  return <>
    <button type="button" className={className} disabled={locked} onClick={openHelp}>{directHandoff ? <ExternalLink size={15} /> : <Sparkles size={15} />}{pending === 'handoff' ? t('正在打开…') : t(triggerLabel)}</button>
    {directHandoff && <button type="button" className="text-button project-ai-copy-only" disabled={locked} onClick={() => { openCopyDialog(); const initialText = buildProjectAiPrompt(context, { aiKind: 'local', target: context.sourcePath ?? context.url, ...promptOptions }); void run('copy', async current => { if (await copy(initialText, current) && current()) setNotice(t("已复制。粘贴给 AI，先看结论和下一步；复制提示词不会自动修改应用。")); }); }}><Copy size={15} />{t('复制提示词')}</button>}
    {open && <Modal title={evaluation ? t('问问 {tool} 能不能用', { tool: ai.buttonTool }) : t('告诉 {tool} 出了什么问题', { tool: ai.buttonTool })} description={evaluation ? t("复制一份提示词，发给你的 AI 工具。小抽屉不会替你发送，也不会上传应用。") : t("提示词里已经带上出错信息，AI 会先查原因，再告诉你怎么改。小抽屉不会替你发送，也不会上传应用。")} onClose={close} closeDisabled={pending !== null} className="project-ai-help-modal">
      <div className="project-ai-help">
        {evaluation && <><label htmlFor={`${id}-target`}>{t("评估哪个应用（可不填）")}</label><div className="project-ai-target"><input id={`${id}-target`} value={target} disabled={locked} onChange={event => { setTarget(event.target.value); setNotice(''); }} placeholder={t("当前对话里的应用，也可以填写仓库地址或文件夹路径")} /><button type="button" className="refresh-button" disabled={locked} onClick={choose}><FolderOpen size={15} />{t("选择文件夹")}</button></div><p className="project-help">{t("不填写时，请 AI 检查当前对话里正在开发的应用，不用先添加或等到失败。")}</p></>}
        {!evaluation && <div className="project-ai-target-summary"><strong>{context.appName ?? t("本次应用")}</strong><code>{context.sourcePath ?? context.url ?? t("应用位置见检查结果")}</code><p>{t("应用位置、检查原因和可用的诊断会一起写入提示词。")}</p></div>}
        <fieldset className="project-ai-kind" disabled={locked}><legend>{t("你用哪种 AI？")}</legend>
          <label><input type="radio" name={`${id}-kind`} checked={aiKind === 'local'} onChange={() => { setAiKind('local'); setNotice(''); }} /><span><strong>{t("AI 工具（推荐）")}</strong><small>{t("可直接查看本机文件，之后也能帮助修改。")}</small></span></label>
          <label><input type="radio" name={`${id}-kind`} checked={aiKind === 'web'} onChange={() => { setAiKind('web'); setNotice(''); }} /><span><strong>{t("网页版 AI")}</strong><small>{t("通常读不到本机文件，提供的判断只能作参考。")}</small></span></label>
        </fieldset>
        {aiKind === 'web' && <section className="project-ai-excerpts" aria-label={t("将附带的应用材料")}>
          <h3>{t("附带哪些公开材料")}</h3><p>{t("只读取有限摘录，合计不超过 20 KB。复制前请核对文件和预览内容，排除不想分享的资料。")}</p>
          {reading && <p role="status">{t("正在读取可选的公开材料…")}</p>}
          {excerptError && <p className="task-error" role="alert">{t("材料没有读取完成：{error}。可以继续复制不含摘录的提示词。", { error: t(excerptError) })}</p>}
          {!sourcePath && <p>{t("没有可读取的本机文件夹。会附带已填写的仓库地址或应用描述；请让 AI 先说明能否访问。")}</p>}
          {excerpt && <><ul className="project-ai-file-list">{excerpt.files.map(file => <li key={file.path}><label><input type="checkbox" checked={selectedFiles.includes(file.path)} disabled={locked} onChange={event => setSelectedFiles(value => event.target.checked ? [...value, file.path] : value.filter(path => path !== file.path))} /><code>{file.path}</code></label></li>)}</ul><p className="project-help">{t("已选择 {count} 份摘录。", { count: selectedFiles.length })}{excerpt.truncated ? t("部分内容因大小限制未附带。") : ''}{t("摘录不代表完整工程；源文件也可能写有私人内容，请先预览。")}</p></>}
          <p className="project-help">{t("不要发送整个应用、真实 .env、数据库、上传文件或账号信息。")}</p>
        </section>}
        <div className="confirmation-actions modal-footer"><button type="button" className="primary-button" disabled={locked || reading} onClick={copyPrompt}><Copy size={15} />{pending === 'copy' ? t("正在复制…") : evaluation ? t("复制提示词") : t("复制完整提示词")}</button><button type="button" className="refresh-button" disabled={locked} onClick={() => setPreview(value => !value)}>{preview ? t("收起全文") : t("预览全文")}</button>{evaluation && handoffReady && sourcePath && <button type="button" className="refresh-button" disabled={locked || reading || aiKind !== 'local'} onClick={evaluateWithTool}><ExternalLink size={15} />{t('问问 {tool} 能不能用', { tool: ai.buttonTool })}</button>}</div>
        {evaluation && handoffReady && !sourcePath && <p className="project-help">{t("只填了仓库地址时还没有本机文件夹，不能直接打开 AI 工具。请复制提示词，粘贴到你的 AI 工具里。")}</p>}
        {notice && <p className="workspace-inline-notice" role="status">{t(notice)}</p>}
        {error && <p className="task-error" role="alert">{t(error)}</p>}
        {preview && <div className="project-ai-preview"><label htmlFor={`${id}-prompt`}>{t("给 AI 的完整提示词")}</label><textarea ref={promptField} id={`${id}-prompt`} readOnly rows={12} value={text} onFocus={event => event.currentTarget.select()} /><button type="button" className="text-button" onClick={() => { promptField.current?.focus(); promptField.current?.select(); }}>{t("选择完整提示词")}</button></div>}
        <button type="button" className="text-button project-ai-file-help" aria-expanded={fileHelp} onClick={() => setFileHelp(value => !value)}>{t("AI 说读不到文件夹？")}</button>
        {fileHelp && <section className="project-ai-file-help-content"><p>{t("最方便的方式是换一个能读取这个文件夹的 AI，再粘贴同一份提示词。复制本机路径不会让网页版 AI 自动获得文件访问能力。")}</p>{sourcePath && <div className="confirmation-actions"><button type="button" className="refresh-button" disabled={locked} onClick={reveal}><FolderOpen size={15} />{t("打开文件夹")}</button><button type="button" className="refresh-button" disabled={locked} onClick={copyPath}><Copy size={15} />{t("复制文件夹路径")}</button></div>}<button type="button" className="text-button" disabled={locked} onClick={() => { setAiKind('web'); setPreview(true); }}>{t("继续用网页版 AI，选择附带材料")}</button></section>}
      </div>
    </Modal>}
  </>;
}
