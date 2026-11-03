import { t } from './i18n';
import { Check, Copy, Eye, Sparkles } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { buildPlatformAiIntroPrompt } from './platform-ai-intro';
import { useFrontendPlatform, usePlatformSkillPath } from './frontend-platform';

/** Copies the project-independent platform introduction; falls back to a selectable preview. */
export function PlatformAiIntroButton({ platformVersion, className = 'text-button', label = t('复制平台说明给 AI'), disabled = false }: {
  platformVersion?: string;
  className?: string;
  label?: string;
  disabled?: boolean;
}) {
  const frontendPlatform = useFrontendPlatform();
  const skillPath = usePlatformSkillPath();
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
    const text = buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath);
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
    <button type="button" className={className} disabled={disabled || copying} onClick={() => void copy()} title={t("不带任何项目，只介绍平台：新项目开工前，或判断自己的项目能否接入时使用")}><Sparkles size={15} aria-hidden="true" />{copying ? t("正在复制…") : t(label)}</button>
    {notice && <span className="platform-ai-intro-notice" role="status">{t(notice)}</span>}
    {preview !== null && <Modal title={t("平台说明")} description={t("无法自动复制。请选中下方全部内容，手动复制后发给 AI。")} onClose={() => setPreview(null)}>
      <div className="project-ai-preview"><label htmlFor="platform-ai-intro-text">{t("给 AI 的平台说明")}</label><textarea id="platform-ai-intro-text" readOnly rows={14} value={buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath)} onFocus={event => event.currentTarget.select()} /></div>
    </Modal>}
  </>;
}

/** Compact home entry: label, icon-only copy with in-place confirmation, and a full-text viewer. */
export function PlatformAiIntroCompact({ platformVersion, disabled = false }: { platformVersion?: string; disabled?: boolean }) {
  const frontendPlatform = useFrontendPlatform();
  const skillPath = usePlatformSkillPath();
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
  const text = () => buildPlatformAiIntroPrompt(platformVersion, frontendPlatform.aiPlatformDescription, skillPath);
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
  return <div className="ai-intro-chip" role="group" aria-label={t("给 AI 的平台说明")} title={t("不带任何项目，只介绍平台：新项目开工前，或判断项目能否接入时发给 AI")}>
    <span className="ai-intro-chip-label"><Sparkles size={14} aria-hidden="true" />{t("平台说明")}</span>
    <button type="button" className="ai-intro-chip-button" aria-label={copied ? t("已复制平台说明") : t("复制平台说明")} title={copied ? t("已复制") : t("复制，发给任意 AI")} disabled={disabled || copying} onClick={() => void copy()}>{copied ? <Check size={15} aria-hidden="true" /> : <Copy size={15} aria-hidden="true" />}</button>
    <button type="button" className="ai-intro-chip-button" aria-label={t("查看平台说明")} title={t("查看全文")} disabled={disabled} onClick={() => { setViewCopied('idle'); setViewing(true); }}><Eye size={15} aria-hidden="true" /></button>
    <span className="sr-only" role="status">{copied ? t("已复制，可以发给任意 AI。") : ''}</span>
    {viewing && <Modal title={t("平台说明")} description={viewCopied === 'failed' ? t("无法自动复制。请选中下方全部内容，手动复制后发给 AI。") : t("发给任意 AI，让它了解熊猫小抽屉能做什么。新项目开工前，或判断项目能否接入时使用。")} onClose={() => setViewing(false)}>
      <div className="project-ai-preview"><label htmlFor="platform-ai-intro-view">{t("给 AI 的平台说明")}</label><textarea id="platform-ai-intro-view" readOnly rows={14} value={text()} onFocus={event => event.currentTarget.select()} /></div>
      <div className="ai-intro-view-actions">{viewCopied === 'done' && <span role="status">{t("已复制")}</span>}<button type="button" className="primary-button" onClick={() => void copyFromView()}><Copy size={15} aria-hidden="true" />{t("复制全文")}</button></div>
    </Modal>}
  </div>;
}
