import { ChevronRight, Copy, ExternalLink } from 'lucide-react';
import { useState } from 'react';
import type { ReactNode } from 'react';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy' | 'codex';

export function CodexHandoffPanel({ handoff, action, disabled, onPrepare, onClear, feedback, children, buildAction }: {
  handoff: ProjectHandoff | null;
  action: HandoffAction | null;
  disabled: boolean;
  onPrepare: (action: HandoffAction) => void;
  onClear: () => void;
  feedback?: ReactNode;
  children?: ReactNode;
  buildAction?: ReactNode;
}) {
  const [showHelp, setShowHelp] = useState(false);
  return <section className="detail-actions" aria-label="应用操作与开发更新">
    <div className="detail-action-row">
      {children}
      <div className="detail-development-actions" aria-label="开发与更新">
        <button className="refresh-button detail-copy-prompt" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={20} />{action === 'copy' ? '正在准备说明…' : '复制提示词'}</button>
        <button className="refresh-button" disabled={disabled || action !== null} onClick={() => onPrepare('codex')}><ExternalLink size={20} />{action === 'codex' ? '正在准备并打开…' : '在 Codex 打开'}</button>
        {buildAction}
      </div>
    </div>
    <div className="detail-action-hint"><p>想修改功能？把需求交给 AI，完成后构建更新。</p><button className="text-button" aria-expanded={showHelp || Boolean(handoff)} onClick={() => { if (showHelp || handoff) { setShowHelp(false); if (handoff) onClear(); } else setShowHelp(true); }}>{showHelp || handoff ? '收起交接说明' : '查看交接说明'}<ChevronRight size={16} /></button></div>
    {feedback}
    {showHelp && <div className="detail-handoff-help"><p>Codex 需已安装在这台电脑上。打开后请粘贴说明、补充需求并自行发送。</p><p>接收说明的 AI 需要能访问这台电脑上的项目目录。仅聊天的 AI 可以了解背景，但不能直接修改本机文件。</p></div>}
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>本次项目目录</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor="ai-handoff-text">完整说明 · 可核对或手动复制</label><button className="text-button" disabled={action !== null} onClick={onClear}>收起全文</button></div>
      <textarea id="ai-handoff-text" readOnly value={handoff.text} rows={9} aria-label="给 AI 的完整说明" onFocus={event => event.currentTarget.select()} />
      <p className="project-help">若未能打开，请手动打开 Codex，选择上方项目目录，将完整说明粘贴到新任务，再补充需求并发送。</p>
    </div>}
  </section>;
}
