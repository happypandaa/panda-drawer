import { t } from './i18n';
import { FolderOpen, Link, Sparkles } from 'lucide-react';
import { useAppCatalog } from './app-catalog';
import { useAiTools } from './AiToolsProvider';
import { useFrontendPlatform } from './frontend-platform';
import { RecommendedApplications } from './AddApplicationDialog';
import type { ApplicationSource } from './AddApplicationDialog';

/** First run: put the recommended apps, and what each one needs, right on the page. */
export function ApplicationLibraryWelcome({ disabled, existingUrls, existingIds, onAdd, onLink, onLocal, onCreate }: {
  disabled: boolean; existingUrls: string[]; existingIds?: string[]; onAdd: (source: ApplicationSource) => void;
  onLink: () => void; onLocal: () => void; onCreate: () => void;
}) {
  const ai = useAiTools();
  const platform = useFrontendPlatform();
  const apps = useAppCatalog(!disabled);
  return <section className="app-library-welcome">
    <div className="app-library-hero">
      <img src="/panda-app-drawer.png" alt="" />
      <div>
        <h2>{t('抽屉还是空的，先放一个应用进来')}</h2>
        <p>{platform.aiHandoffAvailable ? t('拿来就能用。想改哪里，在应用窗口点「按你的想法改」，交给你的 AI。') : t('拿来就能用。想改哪里，从应用详情复制提示词，交给你的 AI。')}</p>
      </div>
    </div>
    <RecommendedApplications apps={apps} existingUrls={existingUrls} existingIds={existingIds} disabled={disabled} compact onAdd={onAdd} />
    <div className="app-library-other">
      <span>{t('已经有想用的？')}</span>
      <div className="app-library-links">
        <button type="button" disabled={disabled} onClick={onLink}><Link size={16} aria-hidden="true" />{t('粘贴 GitHub / Gitee 链接')}</button>
        <button type="button" disabled={disabled} onClick={onLocal}><FolderOpen size={16} aria-hidden="true" />{t('选择本机文件夹')}</button>
        <button type="button" disabled={disabled} onClick={onCreate}><Sparkles size={16} aria-hidden="true" />{t('让 AI 新做一个')}</button>
      </div>
    </div>
    {platform.aiHandoffAvailable && <div className="app-library-ai-status">{ai.selectedTool
      ? <><span className="app-library-ai-dot is-ready" aria-hidden="true" />{t('你的 AI 工具：{name}', { name: ai.selectedToolName })}<button className="text-button" disabled={disabled} onClick={() => ai.openChooser()}>{t('换一个')}</button></>
      : <><span className="app-library-ai-dot" aria-hidden="true" />{t('还没选择 AI 工具')}<button className="text-button" disabled={disabled} onClick={() => ai.openChooser()}>{t('选择')}</button></>}</div>}
  </section>;
}
