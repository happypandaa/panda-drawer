import { t } from './i18n';
import { useAppCatalog } from './app-catalog';
import { useAiTools } from './AiToolsProvider';
import { useFrontendPlatform } from './frontend-platform';
import { RecommendedApplications } from './AddApplicationDialog';
import type { ApplicationSource } from './AddApplicationDialog';

export function ApplicationLibraryWelcome({ disabled, existingUrls, onAdd, onLink, onLocal, onCreate }: {
  disabled: boolean; existingUrls: string[]; onAdd: (source: ApplicationSource) => void;
  onLink: () => void; onLocal: () => void; onCreate: () => void;
}) {
  const ai = useAiTools();
  const platform = useFrontendPlatform();
  const apps = useAppCatalog(!disabled);
  const steps = [
    ['选一个应用', '从推荐里挑一个，或添加你已经找到的应用。'],
    ['用起来', '每个应用一个窗口，数据保存在这台电脑上。'],
    ['想改哪里，告诉你的 AI', platform.aiHandoffAvailable ? '从应用详情打开你的 AI 工具，在那里告诉它想改什么。' : '从应用详情复制提示词，在你的 AI 工具中选择项目文件夹，再告诉它想改什么。'],
  ];
  return <section className="app-library-welcome">
    <div><h2>{t('拿来就用，想改就让你的 AI 改')}</h2><p>{t('挑一个现成的小应用用起来。哪里不顺手，就告诉你自己的 AI 工具，把应用改成你想要的样子。熊猫小抽屉不自带 AI，也不需要你懂代码。')}</p></div>
    <div className="app-library-steps">{steps.map(([title, description], index) => <div className="app-library-step" key={title}><span aria-hidden="true">{index + 1}</span><div><strong>{t(title)}</strong><p>{t(description)}</p></div></div>)}</div>
    <div className="app-library-ai-status">{!platform.aiHandoffAvailable ? <p>{t('在应用详情复制提示词，然后交给你的 AI 工具修改。')}</p> : ai.selectedTool ? <>{t('你的 AI 工具：{name}（{capability}）', { name: ai.selectedToolName, capability: ai.selectedTool.oneClick ? t('支持一键打开') : t('打开后粘贴') })}<span> · </span><button className="text-button" disabled={disabled} onClick={() => ai.openChooser()}>{t('更换')}</button></> : <>{t('还没选择 AI 工具')}<span> · </span><button className="text-button" disabled={disabled} onClick={() => ai.openChooser()}>{t('选择')}</button></>}</div>
    <RecommendedApplications apps={apps} existingUrls={existingUrls} disabled={disabled} compact onAdd={onAdd} />
    <div className="app-library-links"><button className="text-button" disabled={disabled} onClick={onLink}>{t('粘贴 GitHub / Gitee 链接')}</button><button className="text-button" disabled={disabled} onClick={onLocal}>{t('选择本机文件夹')}</button><button className="text-button" disabled={disabled} onClick={onCreate}>{t('从零做一个')}</button></div>
  </section>;
}
