import { localizePlatformPrompt } from './platform-prompt';
import { t } from './i18n';
import { ChevronRight, Copy, ExternalLink, FolderOpen } from 'lucide-react';
import { useId, useState } from 'react';
import type { ReactNode } from 'react';
import { useAiTools } from './AiToolsProvider';
import type { AiHandoffTarget } from './ai-tools';
import { useFrontendPlatform } from './frontend-platform';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy';

export function CodexHandoffPanel({ handoff, action, disabled, onPrepare, onClear, onShowHelp, onOpenFolder, feedback, target, hint = t("复制提示词发给 AI，按项目说明完成后续操作。") }: {
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
  const id = useId();
  const newHandoff = platform.appLibraryAvailable && Boolean(target);
  const automaticHandoff = newHandoff && platform.aiHandoffAvailable;
  const locked = disabled || action !== null || automaticHandoff && (!ai.enabled || ai.busy);
  return <section className="detail-actions detail-handoff" aria-label={t("应用操作与交接说明")}>
    {newHandoff ? <>
      <div className="ai-handoff-heading">{target?.purpose === 'create' ? <><h3>{t('和你的 AI 一起做出来')}</h3><p>{t('把这个新应用交给你的 AI 工具，在那里告诉它你想做什么。不用懂代码。')}</p></> : <><h3>{t('改成你想要的样子')}</h3><p>{t('把这个应用交给你的 AI 工具，在那里告诉它想改什么。不用懂代码。')}</p></>}</div>
      {automaticHandoff ? <>
      <div className="ai-handoff-actions"><button type="button" className="primary-button" disabled={locked} onClick={() => { if (ai.selectedTool && target) void ai.handoff(target); else ai.openChooser(); }}><ExternalLink size={16} />{ai.busy ? t('正在交接…') : ai.selectedTool ? target?.purpose === 'create' ? t('用 {tool} 开始做', { tool: ai.selectedTool.name }) : t('用 {tool} 改', { tool: ai.selectedTool.name }) : t('选择 AI 工具')}</button><button type="button" className="refresh-button" disabled={locked} onClick={() => onPrepare('copy')}><Copy size={16} />{action === 'copy' ? t('正在准备说明…') : t('只复制提示词')}</button>{ai.selectedTool && <button type="button" className="text-button ai-handoff-change" disabled={locked} onClick={ai.openChooser}>{t('更换 AI 工具')}</button>}</div>
      <p className="ai-handoff-explanation">{ai.selectedTool?.oneClick
        ? t('将尝试在 {tool} 中新开对话，请检查项目文件夹是 {app}，并检查提示词。发送后，AI 会先读懂这个应用，再问你想怎么改。修改完成后，按 AI 的说明操作。', { tool: ai.selectedTool.name, app: target?.appName ?? '' })
        : ai.selectedTool?.id === 'qoder'
          ? t('将尝试打开 Qoder 并复制提示词。请在 Qoder 中选择或确认这个应用的项目文件夹，再粘贴到能修改文件的智能体对话中发送。')
        : ai.selectedTool && ai.selectedTool.id !== 'other'
          ? t('将尝试用 {tool} 打开项目文件夹，并复制提示词。请在能修改文件的智能体对话中粘贴并发送，修改完成后按 AI 的说明操作。', { tool: ai.selectedTool.name })
          : t('选择你的 AI 工具，或只复制提示词，手动打开项目文件夹并粘贴到能修改文件的 AI 对话。')}
        {ai.selectedTool && ` ${t('若未能打开或填入提示词，可点“只复制提示词”，手动打开工具并粘贴。')}`}</p>
      </> : <>
        <div className="ai-handoff-actions"><button type="button" className="primary-button" disabled={locked} onClick={() => onPrepare('copy')}><Copy size={16} />{action === 'copy' ? t('正在准备说明…') : t('只复制提示词')}</button><button type="button" className="refresh-button" disabled={locked || !onOpenFolder} onClick={onOpenFolder}><FolderOpen size={16} />{t(platform.openFolderLabel)}</button></div>
        <p className="ai-handoff-explanation">{t('复制提示词后，手动打开你的 AI 工具，选择这个应用的项目文件夹，在能修改文件的智能体对话中粘贴并发送。修改完成后，按 AI 的说明操作。')}</p>
      </>}
    </> : <>
    <div className="detail-action-hint"><p>{t("想改功能？交给 AI")}</p>
    <div className="detail-action-row">
      <div className="detail-copy-actions" aria-label={t("AI 交接")}>
        <button className="refresh-button detail-copy-prompt" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={20} />{action === 'copy' ? t("正在准备说明…") : t("复制提示词")}</button>
      </div>
    </div>
    </div>
    <div className="detail-action-hint"><p>{t(hint)}</p><button className="text-button" disabled={disabled || action !== null} aria-expanded={showHelp || Boolean(handoff)} onClick={() => { if (showHelp || handoff) { setShowHelp(false); if (handoff) onClear(); } else { setShowHelp(true); onShowHelp?.(); } }}>{showHelp || handoff ? t("收起交接说明") : t("查看交接说明")}<ChevronRight size={16} /></button></div>
    </>}
    {feedback}
    {showHelp && <div className="detail-handoff-help"><p>{t("接收说明的 AI 需要能访问这台电脑上的项目目录。粘贴说明、补充需求后自行发送；仅聊天的 AI 可以了解背景，但不能直接修改本机文件。")}</p></div>}
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>{t("本次项目目录")}</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor={`${id}-handoff-text`}>{t("完整说明 · 可核对或手动复制")}</label><button className="text-button" disabled={action !== null} onClick={() => { setShowHelp(false); onClear(); }}>{t("收起全文")}</button></div>
      <textarea id={`${id}-handoff-text`} readOnly value={automaticHandoff ? handoff.text : localizePlatformPrompt(handoff.text)} rows={9} aria-label={t("给 AI 的完整说明")} onFocus={event => event.currentTarget.select()} />
      <p className="project-help">{t("将完整说明发给能读取上方项目目录的 AI，再补充需求并发送。")}</p>
    </div>}
  </section>;
}
