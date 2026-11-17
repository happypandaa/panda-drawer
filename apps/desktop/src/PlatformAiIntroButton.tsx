import { t } from './i18n';
import { Check, Copy, Eye, Sparkles } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { buildPlatformAiIntroPrompt } from './platform-ai-intro';
import { useFrontendPlatform, usePlatformSkillPath, usePlatformCliPath } from './frontend-platform';

/** Copies the project-independent platform introduction; falls back to a selectable preview. */
export function PlatformAiIntroButton({ platformVersion, className = 'text-button', label = t('复制平台介绍'), disabled = false }: {
  platformVersion?: string;
  className?: string;
  label?: string;
  disabled?: boolean;
}) {
  const frontendPlatform = useFrontendPlatform();
  const skillPath = usePlatformSkillPath();
  const cliPath = usePlatformCliPath();
  const [notice, setNotice] = useState('');
  const [preview, setPreview] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  const alive = useRef(true);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; if (timer.current) clearTimeout(timer.current); };
  }, []);
  async function copy() {
    if (copying || disabled) return;
    const text = buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath, cliPath);
    setCopying(true); setNotice('');
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(text);
      if (!alive.current) return;
      setNotice(t("已复制，可以发给任意 AI。"));
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => { if (alive.current) setNotice(''); }, 4000);
    } catch {
      if (alive.current) setPreview(text);
    } finally { if (alive.current) setCopying(false); }
  }
  return <>
    <button type="button" className={className} disabled={disabled || copying} onClick={() => void copy()} title={t("不针对某个应用，只介绍熊猫小抽屉。做新应用之前，或想知道已有的程序能不能放进小抽屉时，发给 AI。")}><Sparkles size={15} aria-hidden="true" />{copying ? t("正在复制…") : t(label)}</button>
    {notice && <span className="platform-ai-intro-notice" role="status">{t(notice)}</span>}
    {preview !== null && <Modal title={t("平台介绍")} description={t("无法自动复制。请选中下方全部内容，手动复制后发给 AI。")} onClose={() => setPreview(null)}>
      <div className="project-ai-preview"><label htmlFor="platform-ai-intro-text">{t("平台介绍")}</label><textarea id="platform-ai-intro-text" readOnly rows={14} value={buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath, cliPath)} onFocus={event => event.currentTarget.select()} /></div>
    </Modal>}
  </>;
}

/** Compact home entry: label, icon-only copy with in-place confirmation, and a full-text viewer. */
export function PlatformAiIntroCompact({ platformVersion, disabled = false }: { platformVersion?: string; disabled?: boolean }) {
  const frontendPlatform = useFrontendPlatform();
  const skillPath = usePlatformSkillPath();
  const cliPath = usePlatformCliPath();
  const [copied, setCopied] = useState(false);
  const [copying, setCopying] = useState(false);
  const [viewing, setViewing] = useState(false);
  const [viewCopied, setViewCopied] = useState<'idle' | 'done' | 'failed'>('idle');
  const alive = useRef(true);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; if (timer.current) clearTimeout(timer.current); };
  }, []);
  const text = () => buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath, cliPath);
  async function write(value: string) {
    if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
    await navigator.clipboard.writeText(value);
  }
  async function copy() {
    if (copying || disabled) return;
    setCopying(true);
    try {
      await write(text());
      if (!alive.current) return;
      setCopied(true);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => { if (alive.current) setCopied(false); }, 2000);
    } catch {
      if (alive.current) { setViewCopied('failed'); setViewing(true); }
    } finally { if (alive.current) setCopying(false); }
  }
  async function copyFromView() {
    try { await write(text()); if (alive.current) setViewCopied('done'); }
    catch { if (alive.current) setViewCopied('failed'); }
  }
  return <div className="ai-intro-chip" role="group" aria-label={t("平台介绍")} title={t("不针对某个应用，只介绍熊猫小抽屉。做新应用之前，或想知道已有的程序能不能放进小抽屉时，发给 AI。")}>
    <span className="ai-intro-chip-label"><Sparkles size={14} aria-hidden="true" />{t("平台介绍")}</span>
    <button type="button" className="ai-intro-chip-button" aria-label={copied ? t("已复制平台介绍") : t("复制平台介绍")} title={copied ? t("已复制") : t("复制，发给任意 AI")} disabled={disabled || copying} onClick={() => void copy()}>{copied ? <Check size={15} aria-hidden="true" /> : <Copy size={15} aria-hidden="true" />}</button>
    <button type="button" className="ai-intro-chip-button" aria-label={t("查看平台介绍")} title={t("查看全文")} disabled={disabled} onClick={() => { setViewCopied('idle'); setViewing(true); }}><Eye size={15} aria-hidden="true" /></button>
    <span className="sr-only" role="status">{copied ? t("已复制，可以发给任意 AI。") : ''}</span>
    {viewing && <Modal title={t("平台介绍")} description={viewCopied === 'failed' ? t("无法自动复制。请选中下方全部内容，手动复制后发给 AI。") : t("不针对某个应用，只介绍熊猫小抽屉。做新应用之前，或想知道已有的程序能不能放进小抽屉时，发给 AI。")} onClose={() => setViewing(false)}>
      <div className="project-ai-preview"><label htmlFor="platform-ai-intro-view">{t("平台介绍")}</label><textarea id="platform-ai-intro-view" readOnly rows={14} value={text()} onFocus={event => event.currentTarget.select()} /></div>
      <div className="ai-intro-view-actions modal-footer">{viewCopied === 'done' && <span role="status">{t("已复制")}</span>}<button type="button" className="primary-button" onClick={() => void copyFromView()}><Copy size={15} aria-hidden="true" />{t("复制全文")}</button></div>
    </Modal>}
  </div>;
}
