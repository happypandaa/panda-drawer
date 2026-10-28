import { ChevronRight, Copy } from 'lucide-react';
import { useId, useState } from 'react';
import type { ReactNode } from 'react';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy';

export function CodexHandoffPanel({ handoff, action, disabled, onPrepare, onClear, onShowHelp, feedback, children, afterCopy, buildAction, hint = '想改功能？复制提示词发给 AI，按项目说明完成后续操作。' }: {
  handoff: ProjectHandoff | null;
  action: HandoffAction | null;
  disabled: boolean;
  onPrepare: (action: HandoffAction) => void;
  onClear: () => void;
  onShowHelp?: () => void;
  feedback?: ReactNode;
  children?: ReactNode;
  afterCopy?: ReactNode;
  buildAction?: ReactNode;
  hint?: string;
}) {
  const [showHelp, setShowHelp] = useState(false);
  const id = useId();
  return <section className="detail-actions" aria-label="应用操作与交接说明">
    <div className="detail-action-row">
      {children}
      <div className={afterCopy ? 'detail-copy-actions' : 'detail-development-actions'} aria-label="AI 交接">
        <button className="refresh-button detail-copy-prompt" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={20} />{action === 'copy' ? '正在准备说明…' : '复制提示词'}</button>
        {buildAction}
      </div>
      {afterCopy}
    </div>
    <div className="detail-action-hint"><p>{hint}</p><button className="text-button" disabled={disabled || action !== null} aria-expanded={showHelp || Boolean(handoff)} onClick={() => { if (showHelp || handoff) { setShowHelp(false); if (handoff) onClear(); } else { setShowHelp(true); onShowHelp?.(); } }}>{showHelp || handoff ? '收起交接说明' : '查看交接说明'}<ChevronRight size={16} /></button></div>
    {feedback}
    {showHelp && <div className="detail-handoff-help"><p>接收说明的 AI 需要能访问这台电脑上的项目目录。粘贴说明、补充需求后自行发送；仅聊天的 AI 可以了解背景，但不能直接修改本机文件。</p></div>}
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>本次项目目录</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor={`${id}-handoff-text`}>完整说明 · 可核对或手动复制</label><button className="text-button" disabled={action !== null} onClick={() => { setShowHelp(false); onClear(); }}>收起全文</button></div>
      <textarea id={`${id}-handoff-text`} readOnly value={handoff.text} rows={9} aria-label="给 AI 的完整说明" onFocus={event => event.currentTarget.select()} />
      <p className="project-help">将完整说明发给能读取上方项目目录的 AI，再补充需求并发送。</p>
    </div>}
  </section>;
}
