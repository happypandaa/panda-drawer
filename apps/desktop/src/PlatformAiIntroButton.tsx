import { Sparkles } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { buildPlatformAiIntroPrompt } from './platform-ai-intro';

/** Copies the project-independent platform introduction; falls back to a selectable preview. */
export function PlatformAiIntroButton({ platformVersion, className = 'text-button', label = '复制平台说明给 AI', disabled = false }: {
  platformVersion?: string;
  className?: string;
  label?: string;
  disabled?: boolean;
}) {
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
    const text = buildPlatformAiIntroPrompt(platformVersion);
    setCopying(true); setNotice('');
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(text);
      if (!alive.current) return;
      setNotice('已复制，可以发给任意 AI。');
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => { if (alive.current) setNotice(''); }, 4000);
    } catch {
      if (alive.current) setPreview(text);
    } finally { if (alive.current) setCopying(false); }
  }
  return <>
    <button type="button" className={className} disabled={disabled || copying} onClick={() => void copy()} title="不带任何项目，只介绍平台：新项目开工前，或判断自己的项目能否接入时使用"><Sparkles size={15} aria-hidden="true" />{copying ? '正在复制…' : label}</button>
    {notice && <span className="platform-ai-intro-notice" role="status">{notice}</span>}
    {preview !== null && <Modal title="平台说明" description="无法自动复制。请选中下方全部内容，手动复制后发给 AI。" onClose={() => setPreview(null)}>
      <div className="project-ai-preview"><label htmlFor="platform-ai-intro-text">给 AI 的平台说明</label><textarea id="platform-ai-intro-text" readOnly rows={14} value={preview} onFocus={event => event.currentTarget.select()} /></div>
    </Modal>}
  </>;
}
