import { Copy, Eye, Sparkles } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { useAiTools } from './AiToolsProvider';
import { useFrontendPlatform, usePlatformSkillPath, usePlatformCliPath } from './frontend-platform';
import { t } from './i18n';
import { Modal } from './Modal';
import { buildPlatformAiIntroPrompt } from './platform-ai-intro';

export const platformIntroDescription = '不针对某个应用，只介绍熊猫小抽屉。想把开源软件改成小抽屉应用、做新应用，或判断已有程序能不能放进来时，发给你的 AI 工具。';

interface IntroProps {
  platformVersion?: string;
  disabled?: boolean;
  sourceUrl?: string;
  variant?: 'settings' | 'home' | 'card';
}

export function PlatformAiIntroCompact({ platformVersion, disabled }: IntroProps) {
  return <PlatformAiIntroButton variant="home" platformVersion={platformVersion} disabled={disabled} />;
}

export function PlatformAiIntroButton({ platformVersion, disabled, sourceUrl, variant = 'settings' }: IntroProps) {
  const platform = useFrontendPlatform();
  const ai = useAiTools();
  const tool = ai.selectedToolName && ai.selectedTool?.id !== 'other' ? ai.selectedToolName : t('你的 AI 工具');
  const skillPath = usePlatformSkillPath();
  const cliPath = usePlatformCliPath();
  const text = () => buildPlatformAiIntroPrompt(platformVersion, platform.aiPlatformDescription, skillPath, cliPath, sourceUrl);
  const id = useId();
  const [notice, setNotice] = useState<string | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [viewState, setViewState] = useState<'idle' | 'failed' | 'done'>('idle');
  const [copying, setCopying] = useState(false);
  const alive = useRef(true);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; if (timer.current) clearTimeout(timer.current); };
  }, []);

  function showPreview(value: string, failed = false) {
    setNotice(null);
    setViewState(failed ? 'failed' : 'idle');
    setPreview(value);
  }
  async function copy(value: string, fromPreview = false) {
    if (copying || disabled) return;
    setCopying(true);
    try {
      await navigator.clipboard.writeText(value);
      if (!alive.current) return;
      if (fromPreview) setViewState('done');
      else {
        setNotice(value);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setNotice(null), 6500);
      }
    } catch {
      if (alive.current) showPreview(value, true);
    } finally {
      if (alive.current) setCopying(false);
    }
  }

  const settings = variant === 'settings';
  return <div className={`platform-intro platform-intro-${variant}`}>
    <button type="button" className={settings ? 'primary-button' : variant === 'home' ? 'refresh-button' : 'app-port-card'} disabled={disabled || copying} title={t(platformIntroDescription)} onClick={() => void copy(text())}>
      <Sparkles size={variant === 'card' ? 20 : 15} aria-hidden="true" />
      {variant === 'card' ? <><strong>{t('让 AI 改造开源软件')}</strong><span>{t('把 GitHub、Gitee 上的软件交给你的 AI，改成小抽屉应用')}</span></> : copying ? t('正在复制…') : t('复制平台介绍')}
    </button>
    {settings && <button type="button" className="refresh-button" onClick={() => showPreview(text())}><Eye size={15} aria-hidden="true" />{t('查看全文')}</button>}
    {notice !== null && <div className="platform-ai-intro-notice">
      <span role="status">{settings ? t('已复制，粘贴到 {tool} 里', { tool }) : t('已复制。粘贴给 {tool}，再发上开源软件地址，或说说你想做什么', { tool })}</span>
      {!settings && <button type="button" className="text-button" onClick={() => showPreview(notice)}>{t('查看全文')}</button>}
    </div>}
    {preview !== null && <Modal title={t('平台介绍')} description={viewState === 'failed' ? t('无法自动复制。请选中下方全部内容，手动复制后发给 AI。') : t(platformIntroDescription)} onClose={() => setPreview(null)}>
      <div className="project-ai-preview"><label htmlFor={id}>{t('平台介绍')}</label><textarea id={id} readOnly rows={14} value={preview} onFocus={event => event.currentTarget.select()} /></div>
      <div className="ai-intro-view-actions modal-footer">{viewState === 'done' && <span role="status">{t('已复制')}</span>}
        <button type="button" className="primary-button" disabled={copying} onClick={() => void copy(preview, true)}><Copy size={15} aria-hidden="true" />{t('复制全文')}</button>
      </div>
    </Modal>}
  </div>;
}
