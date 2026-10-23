import { Copy, FolderOpen } from 'lucide-react';

export interface ProjectHandoff {
  text: string;
  projectRoot: string;
  instanceId: string;
}
export type HandoffAction = 'copy' | 'codex';

export function CodexHandoffPanel({ handoff, action, feedback, disabled, onPrepare, onClear }: {
  handoff: ProjectHandoff | null;
  action: HandoffAction | null;
  feedback: { error?: string; notice?: string } | null;
  disabled: boolean;
  onPrepare: (action: HandoffAction) => void;
  onClear: () => void;
}) {
  return <section className="handoff-card" aria-labelledby="handoff-title">
    <div className="section-heading"><div><h3 id="handoff-title">让 AI 继续开发</h3><p>复制项目说明，在 AI 中补充开发需求。</p></div></div>
    <div className="project-dialog-actions">
      <button className="primary-button" disabled={disabled || action !== null} onClick={() => onPrepare('codex')}><FolderOpen size={16} />{action === 'codex' ? '正在准备并打开…' : '复制说明并在 Codex 打开'}</button>
      <button className="refresh-button" disabled={disabled || action !== null} onClick={() => onPrepare('copy')}><Copy size={16} />{action === 'copy' ? '正在准备说明…' : '复制 AI 接手说明'}</button>
    </div>
    {feedback?.error && <p className="task-error" role="alert">{feedback.error}</p>}
    {feedback?.notice && <p className="project-help handoff-result" role="status">{feedback.notice}</p>}
    <p className="project-help">在 Codex 草稿中打开源码；粘贴说明后自行发送。</p>
    {handoff && <div className="handoff-preview">
      {handoff.projectRoot && <dl className="path-list"><div className="path-row"><dt>本次项目目录</dt><dd><code>{handoff.projectRoot}</code></dd></div></dl>}
      <div className="section-heading"><label htmlFor="ai-handoff-text">完整说明 · 可核对或手动复制</label><button className="text-button" disabled={action !== null} onClick={onClear}>收起全文</button></div>
      <textarea id="ai-handoff-text" readOnly value={handoff.text} rows={9} aria-label="给 AI 的完整说明" onFocus={event => event.currentTarget.select()} />
      <p className="project-help">未能打开时，请在 Codex 中手动选择此目录。</p>
    </div>}
  </section>;
}
