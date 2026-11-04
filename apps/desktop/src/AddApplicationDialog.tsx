import { useState } from 'react';
import { FolderOpen, Plus } from 'lucide-react';
import { Modal } from './Modal';
import { t } from './i18n';
import { gitRepositoryAddress } from './git-repository';
import { availableCatalogApps, catalogSourceUrl, useAppCatalog } from './app-catalog';
import type { CatalogApp } from './app-catalog';

export interface ApplicationSource { url: string; branch?: string | null; name?: string; catalogId?: string }

export function RecommendedApplications({ apps, existingUrls, existingIds = [], disabled, compact = false, onAdd }: {
  apps: CatalogApp[] | null; existingUrls: string[]; existingIds?: string[]; disabled: boolean; compact?: boolean;
  onAdd: (source: ApplicationSource) => void;
}) {
  if (!apps) return null;
  const available = availableCatalogApps(apps, existingUrls, existingIds);
  const visible = compact ? available.slice(0, 3) : available;
  return <section className={`app-recommendations${compact ? ' is-compact' : ''}`} aria-label={t('推荐应用')}>
    <h3>{t('推荐应用')}</h3>
    {available.length === 0 ? <p className="project-help">{t('推荐的应用都已添加')}</p> : <div className="app-recommendation-list">{visible.map(app => <article className="app-recommendation" key={app.id}>
      <span className="app-library-monogram" aria-hidden="true">{Array.from(app.name)[0]}</span>
      <div className="app-recommendation-content"><div className="app-recommendation-title"><strong>{app.name}</strong><span className={`app-needs${app.needs.length ? ' needs-setup' : ''}`}>{app.needs.length ? t('需要 {items}', { items: app.needs.join(t('、')) }) : t('无需配置')}</span></div><p>{app.summary}</p>{app.note && <small>{app.note}</small>}</div>
      <button className="primary-button" disabled={disabled} onClick={() => onAdd({ url: catalogSourceUrl(app), branch: app.branch, name: app.name, catalogId: app.id })}>{t('添加')}</button>
    </article>)}</div>}
  </section>;
}

export function AddApplicationDialog({ disabled, existingUrls, existingIds, onAdd, onLocal, onCreate, onClose }: {
  disabled: boolean; existingUrls: string[]; existingIds?: string[]; onAdd: (source: ApplicationSource) => void;
  onLocal: () => void; onCreate: () => void; onClose: () => void;
}) {
  const [url, setUrl] = useState('');
  const apps = useAppCatalog(true);
  return <Modal title={t('添加应用')} onClose={onClose} className="add-application-modal" wide>
    <p className="project-help">{t('每次添加一个应用，开始后请在窗口中等待完成。')}</p>
    <form className="app-link-form" onSubmit={event => { event.preventDefault(); if (!disabled && gitRepositoryAddress(url)) onAdd({ url }); }}>
      <label htmlFor="add-application-link">{t('有应用链接')}</label>
      <div><input id="add-application-link" value={url} onChange={event => setUrl(event.target.value)} placeholder={t('粘贴 GitHub 或 Gitee 上的应用地址')} autoCapitalize="none" autoCorrect="off" spellCheck={false} /><button className="primary-button" disabled={disabled || !gitRepositoryAddress(url)}>{t('添加')}</button></div>
    </form>
    <div className="app-library-alternatives"><button disabled={disabled} onClick={onLocal}><FolderOpen size={20} /><strong>{t('本机文件夹')}</strong><span>{t('添加电脑上已有的应用文件夹')}</span></button><button disabled={disabled} onClick={onCreate}><Plus size={20} /><strong>{t('从零做一个')}</strong><span>{t('生成空白应用，和 AI 多轮合作做出来')}</span></button></div>
    {apps && <><p className="app-library-divider">{t('或者，从推荐里挑一个')}</p><RecommendedApplications apps={apps} existingUrls={existingUrls} existingIds={existingIds} disabled={disabled} onAdd={onAdd} /></>}
  </Modal>;
}
