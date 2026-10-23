import { Copy, FolderOpen } from 'lucide-react';
import type { ReactNode } from 'react';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy' | 'codex';

export function CodexHandoffPanel({ handoff, action, disabled, onPrepare, onClear, feedback }: {
  handoff: ProjectHandoff | null;
  action: HandoffAction | null;
  disabled: boolean;
  onPrepare: (action: HandoffAction) => void;
  onClear: () => void;
  feedback?: ReactNode;
}) {
  return <section className="handoff-card" aria-labelledby="handoff-title">
    <div className="section-heading"><div><h3 id="handoff-title">让 AI 继续开发</h3><p>一键复制软件介绍、当前项目目录和开发操作说明。粘贴给 AI 后，再告诉它你的需求。</p></div></div>
    <div className="project-dialog-actions">
      <button className="primary-button" disabled={disabled || action !== null} onClick={() => onPrepare('codex')}><FolderOpen size={16} />{action === 'codex' ? '正在准备并打开…' : '复制说明并在 Codex 打开'}</button>
      <button className="refresh-button" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={16} />{action === 'copy' ? '正在准备说明…' : '复制 AI 接手说明'}</button>
    </div>
    {feedback}
    <p className="project-help">Codex 需已安装在这台电脑上。此操作只在 Codex 新任务草稿中打开项目目录；请粘贴说明、补充需求后自行发送，不会自动提交任务。</p>
    <p className="project-help">接收说明的 AI 需要能访问这台电脑上的项目目录。仅聊天的 AI 也能了解背景，但不能直接修改本机文件。</p>
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>本次项目目录</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor="ai-handoff-text">完整说明 · 可核对或手动复制</label><button className="text-button" disabled={action !== null} onClick={onClear}>收起全文</button></div>
      <textarea id="ai-handoff-text" readOnly value={handoff.text} rows={9} aria-label="给 AI 的完整说明" onFocus={event => event.currentTarget.select()} />
      <p className="project-help">若未能打开，请手动打开 Codex，选择上方项目目录，将完整说明粘贴到新任务，再补充需求并发送。</p>
    </div>}
  </section>;
}
