import { t } from './i18n';
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

export function AiProjectHelp({ context, label, disabled = false, className = 'text-button', allowCodex = true }: {
  context: ProjectAiHelpContext;
  label?: string;
  disabled?: boolean;
  className?: string;
  allowCodex?: boolean;
}) {
  const platform = useFrontendPlatform();
  const skillPath = usePlatformSkillPath();
  const cliPath = usePlatformCliPath();
  const [open, setOpen] = useState(false);
  const [target, setTarget] = useState(context.sourcePath ?? context.url ?? '');
  const [aiKind, setAiKind] = useState<'local' | 'web'>('local');
  const [preview, setPreview] = useState(false);
  const [fileHelp, setFileHelp] = useState(false);
  const [excerpt, setExcerpt] = useState<ProjectReviewExcerpt | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [codexAvailable, setCodexAvailable] = useState(false);
  const [pending, setPending] = useState<'copy' | 'choose' | 'codex' | 'reveal' | null>(null);
  const [reading, setReading] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [excerptError, setExcerptError] = useState('');
  const alive = useRef(true);
  const sequence = useRef(0);
  const readSequence = useRef(0);
  const operation = useRef(false);
  const promptField = useRef<HTMLTextAreaElement>(null);
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
    skillPath,
    cliPath,
  });
  const locked = disabled || pending !== null;
  const triggerLabel = label ?? (evaluation ? t("不确定能不能用？先复制说明问问 AI") : t("复制说明给 AI"));

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; sequence.current++; readSequence.current++; };
  }, []);
  useEffect(() => {
    sequence.current++; readSequence.current++; operation.current = false;
    setOpen(false); setPending(null); setReading(false); setNotice(''); setError('');
    setExcerpt(null); setSelectedFiles([]); setExcerptError(''); setCodexAvailable(false);
    setTarget(context.sourcePath ?? context.url ?? '');
  }, [signature]);

  useEffect(() => {
    if (!open || !allowCodex) return;
    let cancelled = false;
    void invoke<{ codexAvailable: boolean }>('source_review_capabilities').then(value => {
      if (!cancelled && alive.current && latest.current.signature === signature) setCodexAvailable(value?.codexAvailable === true);
    }).catch(() => { /* Unavailable capability checks never imply an installed app. */ });
    return () => { cancelled = true; };
  }, [open, signature, allowCodex]);

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
      if (!navigator.clipboard?.writeText) throw new Error(t("无法自动复制。完整说明仍可在下方预览中选择后手动复制。"));
      await navigator.clipboard.writeText(value); copied = true;
    });
    clipboardQueue = queued.catch(() => {});
    await queued;
    return copied;
  }
  function openHelp() {
    if (locked) return;
    setOpen(true); setAiKind('local'); setPreview(false); setFileHelp(false); setNotice(''); setError('');
    if (!evaluation) {
      const initialText = buildProjectAiPrompt(context, { aiKind: 'local', target: context.sourcePath ?? context.url, skillPath, cliPath });
      void run('copy', async current => {
        if (await copy(initialText, current) && current()) setNotice(t("已复制。粘贴给 AI，先看结论和下一步；复制说明不会自动修改项目。"));
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
  function openCodex() {
    if (!allowCodex || !sourcePath || !codexAvailable || reading) return;
    void run('codex', async current => {
      const copied = await copy(text, current).catch(() => false);
      if (!current()) return;
      const result = await invoke<{ requested: boolean }>('source_review_open_codex', { sourcePath });
      if (!current()) return;
      if (!result?.requested) throw new Error(t("无法确认 Codex 的草稿打开请求，请手动选择项目并粘贴说明。"));
      setNotice(copied ? t("已复制说明并请求打开 Codex 的未发送草稿。请粘贴说明，检查后自行发送。") : t("已请求打开 Codex 的未发送草稿。请从“预览全文”手动复制说明，粘贴后自行发送。"));
      if (!copied) setPreview(true);
    });
  }
  function reveal() {
    if (!sourcePath) return;
    void run('reveal', async current => {
      await invoke('open_project_folder', { path: sourcePath });
      if (current()) setNotice(t("已请求在 Finder 中显示项目文件夹。"));
    });
  }
  function copyPath() {
    void run('copy', async current => { if (await copy(sourcePath, current) && current()) setNotice(t("文件夹路径已复制。")); });
  }

  return <>
    <button type="button" className={className} disabled={locked} onClick={openHelp}><Sparkles size={15} />{t(triggerLabel)}</button>
    {open && <Modal title={evaluation ? t("先让 AI 看看能不能用") : t("把问题交给 AI")} description={t("复制一份说明，发给原来帮你开发项目的 AI。平台不会自动发送，也不会上传项目。")} onClose={close} closeDisabled={pending !== null} className="project-ai-help-modal">
      <div className="project-ai-help">
        {evaluation && <><label htmlFor={`${id}-target`}>{t("评估哪个项目（可不填）")}</label><div className="project-ai-target"><input id={`${id}-target`} value={target} disabled={locked} onChange={event => { setTarget(event.target.value); setNotice(''); }} placeholder={t("当前对话里的项目，也可以填写仓库地址或文件夹路径")} /><button type="button" className="refresh-button" disabled={locked} onClick={choose}><FolderOpen size={15} />{t("选择文件夹")}</button></div><p className="project-help">{t("不填写时，请 AI 检查当前对话里正在开发的项目，不用先添加或等到失败。")}</p></>}
        {!evaluation && <div className="project-ai-target-summary"><strong>{context.appName ?? t("本次项目")}</strong><code>{context.sourcePath ?? context.url ?? t("项目位置见检查结果")}</code><p>{t("项目位置、检查原因和可用的诊断会一起写入说明。")}</p></div>}
        <fieldset className="project-ai-kind" disabled={locked}><legend>{t("准备交给哪种 AI？")}</legend>
          <label><input type="radio" name={`${id}-kind`} checked={aiKind === 'local'} onChange={() => { setAiKind('local'); setNotice(''); }} /><span><strong>{t("能读取项目文件夹的 AI（推荐）")}</strong><small>{t("可直接查看本机文件，之后也能帮助修改。")}</small></span></label>
          <label><input type="radio" name={`${id}-kind`} checked={aiKind === 'web'} onChange={() => { setAiKind('web'); setNotice(''); }} /><span><strong>{t("网页聊天 AI")}</strong><small>{t("通常读不到本机文件，提供的判断只能作参考。")}</small></span></label>
        </fieldset>
        {aiKind === 'web' && <section className="project-ai-excerpts" aria-label={t("将附带的项目材料")}>
          <h3>{t("附带哪些公开材料")}</h3><p>{t("只读取有限摘录，合计不超过 20 KB。复制前请核对文件和预览内容，排除不想分享的资料。")}</p>
          {reading && <p role="status">{t("正在读取可选的公开材料…")}</p>}
          {excerptError && <p className="task-error" role="alert">{t("材料没有读取完成：{error}。可以继续复制不含摘录的说明。", { error: t(excerptError) })}</p>}
          {!sourcePath && <p>{t("没有可读取的本机文件夹。会附带已填写的仓库地址或项目描述；请让 AI 先说明能否访问。")}</p>}
          {excerpt && <><ul className="project-ai-file-list">{excerpt.files.map(file => <li key={file.path}><label><input type="checkbox" checked={selectedFiles.includes(file.path)} disabled={locked} onChange={event => setSelectedFiles(value => event.target.checked ? [...value, file.path] : value.filter(path => path !== file.path))} /><code>{file.path}</code></label></li>)}</ul><p className="project-help">{t("已选择 {count} 份摘录。", { count: selectedFiles.length })}{excerpt.truncated ? t("部分内容因大小限制未附带。") : ''}{t("摘录不代表完整工程；源文件也可能写有私人内容，请先预览。")}</p></>}
          <p className="project-help">{t("不要发送整个项目、真实 .env、数据库、上传文件或账号信息。")}</p>
        </section>}
        <div className="confirmation-actions"><button type="button" className="primary-button" disabled={locked || reading} onClick={copyPrompt}><Copy size={15} />{pending === 'copy' ? t("正在复制…") : evaluation ? t("复制评估说明") : t("复制完整说明")}</button><button type="button" className="refresh-button" disabled={locked} onClick={() => setPreview(value => !value)}>{preview ? t("收起全文") : t("预览全文")}</button>{allowCodex && sourcePath && codexAvailable && <button type="button" className="refresh-button" disabled={locked || reading} onClick={openCodex}><ExternalLink size={15} />{t("在 Codex 中打开草稿")}</button>}</div>
        {notice && <p className="workspace-inline-notice" role="status">{t(notice)}</p>}
        {error && <p className="task-error" role="alert">{t(error)}</p>}
        {preview && <div className="project-ai-preview"><label htmlFor={`${id}-prompt`}>{t("给 AI 的完整说明")}</label><textarea ref={promptField} id={`${id}-prompt`} readOnly rows={12} value={text} onFocus={event => event.currentTarget.select()} /><button type="button" className="text-button" onClick={() => { promptField.current?.focus(); promptField.current?.select(); }}>{t("选择完整说明")}</button></div>}
        <button type="button" className="text-button project-ai-file-help" aria-expanded={fileHelp} onClick={() => setFileHelp(value => !value)}>{t("AI 说读不到文件夹？")}</button>
        {fileHelp && <section className="project-ai-file-help-content"><p>{t("最方便的方式是换一个能读取这个文件夹的 AI，再粘贴同一份说明。复制本机路径不会让网页 AI 自动获得文件访问能力。")}</p>{sourcePath && <div className="confirmation-actions"><button type="button" className="refresh-button" disabled={locked} onClick={reveal}><FolderOpen size={15} />{t("打开文件夹")}</button><button type="button" className="refresh-button" disabled={locked} onClick={copyPath}><Copy size={15} />{t("复制文件夹路径")}</button></div>}<button type="button" className="text-button" disabled={locked} onClick={() => { setAiKind('web'); setPreview(true); }}>{t("继续用网页 AI，选择附带材料")}</button></section>}
      </div>
    </Modal>}
  </>;
}
