import { localizePlatformPrompt } from './platform-prompt';
import { t } from './i18n';
import { ChevronRight, Copy, ExternalLink, FolderOpen } from 'lucide-react';
import { useId, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAiTools } from './AiToolsProvider';
import { aiToolFolderTerm } from './ai-tools';
import type { AiHandoffTarget } from './ai-tools';
import { useFrontendPlatform } from './frontend-platform';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy';

export function CodexHandoffPanel({ handoff, action, disabled, onPrepare, onClear, onShowHelp, onOpenFolder, feedback, target, hint = t("复制提示词发给你的 AI 工具，AI 改完会告诉你。") }: {
  handoff: ProjectHandoff | null;
  action: HandoffAction | null;
  disabled: boolean;
  onPrepare: (action: HandoffAction) => void;
  onClear: () => void;
  onShowHelp?: () => void;
  onOpenFolder?: () => void;
  feedback?: ReactNode;
  hint?: string;
  target?: AiHandoffTarget;
}) {
  const ai = useAiTools();
  const platform = useFrontendPlatform();
  const [showHelp, setShowHelp] = useState(false);
  const [pathNotice, setPathNotice] = useState('');
  const id = useId();
  const newHandoff = platform.appLibraryAvailable && Boolean(target);
  const automaticHandoff = newHandoff && platform.aiHandoffAvailable;
  const locked = disabled || action !== null || automaticHandoff && (!ai.enabled || ai.busy);
  const tool = ai.buttonTool;
  const named = tool !== 'AI';
  const create = target?.purpose === 'create';
  const selected = ai.lostToolName ? null : ai.selectedTool;
  // One line under the button: what the click does with the chosen tool.
  const explanation = selected?.capability === 'full' ? t('会打开 {tool}，选好应用文件夹、新建对话并填好提示词。你看一眼，再发送。', { tool })
    : selected?.capability === 'folder-prefill' ? t('会用 {tool} 打开应用文件夹，并尽量填好提示词；没填上就粘贴，已经复制好了。', { tool })
    : selected?.capability === 'folder' ? t('会用 {tool} 打开应用文件夹，提示词已复制。新建对话后粘贴。', { tool })
    : selected?.capability === 'launch' ? t('会打开 {tool}，提示词已复制。你在 {tool} 里选中应用文件夹（它叫「{term}」），再粘贴。', { tool, term: t(aiToolFolderTerm(selected.id)) })
    : t('会复制提示词并打开应用文件夹。在你的 AI 工具里选中这个文件夹，新建对话，粘贴发送。');
  async function copyPath() {
    const appId = target?.appId ?? target?.projectId;
    if (!appId) return;
    try {
      const prepared = await invoke<{ projectRoot: string }>('app_handoff', { appId, purpose: null });
      await navigator.clipboard.writeText(prepared.projectRoot);
      setPathNotice(t('应用文件夹的路径已复制。'));
    } catch { setPathNotice(t('路径没能复制，请点「{open}」后在文件管理器里查看。', { open: t(platform.openFolderLabel) })); }
  }
  return <section className="detail-actions detail-handoff" aria-label={t("应用操作与提示词")}>
    {newHandoff ? <>
      <div className="ai-handoff-heading">{create
        ? <><h3>{named ? t('和 {tool} 一起做出来', { tool }) : t('和你的 AI 工具一起做出来')}</h3><p>{named ? t('点下面的按钮，在 {tool} 里说你想做个什么样的应用。不用懂代码。', { tool }) : t('点下面的按钮，在你的 AI 工具里说你想做个什么样的应用。不用懂代码。')}</p></>
        : <><h3>{t('改成你想要的样子')}</h3><p>{named ? t('点下面的按钮，在 {tool} 里说你想怎么改。不用懂代码。', { tool }) : t('点下面的按钮，在你的 AI 工具里说你想怎么改。不用懂代码。')}</p></>}</div>
      {automaticHandoff ? <>
      <div className="ai-handoff-actions"><button type="button" className="primary-button" disabled={locked} onClick={() => { if (target) void ai.handoff(target); }}><ExternalLink size={16} />{ai.busy ? t('正在打开…') : create ? t('告诉 {tool} 你想做什么', { tool }) : t('告诉 {tool} 怎么改', { tool })}</button>{ai.lostToolName && <span className="ai-handoff-lost">{t('没找到 {tool}', { tool: ai.lostToolName })}</span>}{(ai.selectedTool || ai.lostToolName) && <button type="button" className="text-button ai-handoff-change" disabled={locked} onClick={ai.openChooser}>{t('换一个')}</button>}</div>
      <p className="ai-handoff-explanation">{explanation}</p>
      <div className="ai-handoff-manual"><span>{t('自己操作：')}</span><button type="button" className="text-button" disabled={locked} onClick={() => onPrepare('copy')}><Copy size={14} />{action === 'copy' ? t('正在准备提示词…') : t('复制提示词')}</button>{onOpenFolder && <button type="button" className="text-button" disabled={locked} onClick={onOpenFolder}><FolderOpen size={14} />{t(platform.openFolderLabel)}</button>}<button type="button" className="text-button" disabled={locked} onClick={() => void copyPath()}>{t('复制路径')}</button></div>
      {pathNotice && <p className="project-help" role="status">{pathNotice}</p>}
      </> : <>
        <div className="ai-handoff-actions"><button type="button" className="primary-button" disabled={locked} onClick={() => onPrepare('copy')}><Copy size={16} />{action === 'copy' ? t('正在准备提示词…') : t('复制提示词')}</button><button type="button" className="refresh-button" disabled={locked || !onOpenFolder} onClick={onOpenFolder}><FolderOpen size={16} />{t(platform.openFolderLabel)}</button></div>
        <p className="ai-handoff-explanation">{t('复制提示词后，打开你的 AI 工具，选中这个应用的文件夹，新建对话，粘贴发送。AI 改完会告诉你。')}</p>
      </>}
    </> : <>
    <div className="detail-action-hint"><p>{t("想改功能？告诉你的 AI 工具")}</p>
    <div className="detail-action-row">
      <div className="detail-copy-actions" aria-label={t("告诉 AI")}>
        <button className="refresh-button detail-copy-prompt" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={20} />{action === 'copy' ? t("正在准备提示词…") : t("复制提示词")}</button>
      </div>
    </div>
    </div>
    <div className="detail-action-hint"><p>{t(hint)}</p><button className="text-button" disabled={disabled || action !== null} aria-expanded={showHelp || Boolean(handoff)} onClick={() => { if (showHelp || handoff) { setShowHelp(false); if (handoff) onClear(); } else { setShowHelp(true); onShowHelp?.(); } }}>{showHelp || handoff ? t("收起") : t("查看怎么做")}<ChevronRight size={16} /></button></div>
    </>}
    {feedback}
    {showHelp && <div className="detail-handoff-help"><p>{t("要用能读写这台电脑上应用文件夹的 AI 工具。粘贴提示词后自己点发送；网页版 AI 改不了电脑上的文件。")}</p></div>}
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>{t("应用文件夹")}</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor={`${id}-handoff-text`}>{t("完整提示词 · 可核对或手动复制")}</label><button className="text-button" disabled={action !== null} onClick={() => { setShowHelp(false); onClear(); }}>{t("收起全文")}</button></div>
      <textarea id={`${id}-handoff-text`} readOnly value={target?.text !== undefined ? handoff.text : localizePlatformPrompt(handoff.text)} rows={9} aria-label={t("给 AI 的完整提示词")} onFocus={event => event.currentTarget.select()} />
      <p className="project-help">{t("把完整提示词发给能读写上面应用文件夹的 AI 工具，自己点发送。")}</p>
    </div>}
  </section>;
}
