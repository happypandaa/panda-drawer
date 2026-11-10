import { useEffect, useRef, useState } from 'react';
import { FolderOpen, Plus } from 'lucide-react';
import { Modal } from './Modal';
import { t, useLocale } from './i18n';
import { iconSlot, launcherMonogram } from './ProjectCard';
import { gitRepositoryAddress } from './git-repository';
import { availableCatalogApps, catalogSourceUrl, localizedCatalogApp, useAppCatalogState } from './app-catalog';
import type { CatalogApp, CatalogRepositories } from './app-catalog';
import { downloadRoutes, otherDownloadRoute } from './download-routes';
import type { DownloadRoute } from './download-routes';
import { DownloadRouteFailure } from './DownloadRouteFailure';

export interface ApplicationSource { url: string; branch?: string | null; name?: string; catalogId?: string; catalogRepositories?: CatalogRepositories; catalogApp?: CatalogApp; downloadRoute?: DownloadRoute; agent?: boolean }

export function RecommendedApplications({ apps, existingUrls, existingIds = [], disabled, compact = false, onAdd }: {
  apps: CatalogApp[] | null; existingUrls: string[]; existingIds?: string[]; disabled: boolean; compact?: boolean;
  onAdd: (source: ApplicationSource) => void;
}) {
  const { locale } = useLocale();
  const [selecting, setSelecting] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  async function add(app: CatalogApp) {
    if (selecting || disabled) return;
    setSelecting(true);
    try {
      const route = await downloadRoutes.getRoute();
      const url = catalogSourceUrl(app, route === 'cn' ? 'gitee' : 'github');
      const actualRoute = gitRepositoryAddress(url)?.provider === 'Gitee' ? 'cn' : 'global';
      const original = apps?.find(value => value.id === app.id) ?? app;
      if (alive.current) onAdd({ url,
        branch: app.branch, name: original.name, catalogId: app.id, catalogRepositories: app.repo, catalogApp: original, downloadRoute: actualRoute,
        ...(app.agent ? { agent: true } : {}) });
    } finally { if (alive.current) setSelecting(false); }
  }
  if (!apps) return null;
  const available = availableCatalogApps(apps, existingUrls, existingIds).map(app => localizedCatalogApp(app, locale));
  const needs = (app: CatalogApp) => [...(app.agent ? [t('需要你的 AI 工具配合')] : []), ...(app.needs.length ? [t('需要 {items}', { items: app.needs.join(t('、')) })] : [])].join(t('；'));
  if (compact) return <section className="app-recommendations is-compact" aria-label={t('推荐应用')}>
    <div className="app-recommendations-heading"><h3>{t('推荐应用')}</h3><span>{t('都是开源的，加进来后代码就在你电脑上')}</span></div>
    {available.length === 0 ? <p className="project-help">{t('推荐的应用都已添加')}</p> : <div className="app-recommendation-list">{available.slice(0, 4).map(app => <article className="app-recommendation" key={app.id}>
      <span className={`app-library-monogram launcher-color-${iconSlot(app.id)}`} aria-hidden="true">{launcherMonogram(app.name)}</span>
      <div className="app-recommendation-content"><strong>{app.name}</strong><p>{app.summary}</p><span className={`app-needs${app.agent ? ' is-agent' : app.needs.length ? ' needs-setup' : ' is-ready'}`} title={app.note}>{needs(app) || t('不用配置，加完就能用')}</span></div>
      <button className="refresh-button app-recommendation-add" disabled={disabled || selecting} onClick={() => void add(app)}>{t('添加')}</button>
    </article>)}</div>}
  </section>;
  return <section className="app-recommendations" aria-label={t('推荐应用')}>
    <h3>{t('推荐应用')}</h3>
    {available.length === 0 ? <p className="project-help">{t('推荐的应用都已添加')}</p> : <div className="app-recommendation-list">{available.map(app => <article className="app-recommendation" key={app.id}>
      <span className="app-library-monogram" aria-hidden="true">{Array.from(app.name)[0]}</span>
      <div className="app-recommendation-content"><div className="app-recommendation-title"><strong>{app.name}</strong><span className={`app-needs${app.needs.length || app.agent ? ' needs-setup' : ''}`}>{needs(app) || t('无需配置')}</span></div><p>{app.summary}</p>{app.note && <small>{app.note}</small>}</div>
      <button className="primary-button" disabled={disabled || selecting} onClick={() => void add(app)}>{t('添加')}</button>
    </article>)}</div>}
  </section>;
}

export function AddApplicationDialog({ disabled, existingUrls, existingIds, onAdd, onLocal, onCreate, onClose }: {
  disabled: boolean; existingUrls: string[]; existingIds?: string[]; onAdd: (source: ApplicationSource) => void;
  onLocal: () => void; onCreate: () => void; onClose: () => void;
}) {
  const [url, setUrl] = useState('');
  const { apps, error: catalogError } = useAppCatalogState(true);
  return <Modal title={t('添加应用')} onClose={onClose} className="add-application-modal" wide>
    <p className="project-help">{t('每次添加一个应用，开始后请在窗口中等待完成。')}</p>
    <form className="app-link-form" onSubmit={event => { event.preventDefault(); if (!disabled && gitRepositoryAddress(url)) onAdd({ url }); }}>
      <label htmlFor="add-application-link">{t('有应用链接')}</label>
      <div><input id="add-application-link" value={url} onChange={event => setUrl(event.target.value)} placeholder={t('粘贴 GitHub 或 Gitee 上的应用地址')} autoCapitalize="none" autoCorrect="off" spellCheck={false} /><button className="primary-button" disabled={disabled || !gitRepositoryAddress(url)}>{t('添加')}</button></div>
    </form>
    <div className="app-library-alternatives"><button disabled={disabled} onClick={onLocal}><FolderOpen size={20} /><strong>{t('本机文件夹')}</strong><span>{t('添加电脑上已有的应用文件夹')}</span></button><button disabled={disabled} onClick={onCreate}><Plus size={20} /><strong>{t('新建')}</strong><span>{t('生成一个起始项目，和你的 AI 一起做出来')}</span></button></div>
    {catalogError && catalogError.mode !== 'auto' && <DownloadRouteFailure route={catalogError.route} disabled={disabled}
      onRetry={() => downloadRoutes.setMode(otherDownloadRoute(catalogError.route))} />}
    {apps && <><p className="app-library-divider">{t('或者，从推荐里挑一个')}</p><RecommendedApplications apps={apps} existingUrls={existingUrls} existingIds={existingIds} disabled={disabled} onAdd={onAdd} /></>}
  </Modal>;
}
