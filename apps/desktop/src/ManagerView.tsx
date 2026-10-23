import { ArrowUpRight, FolderOpen, RefreshCw, Search, Settings, ShieldCheck } from 'lucide-react';
import type { FormEvent, ReactNode } from 'react';
import { Modal } from './Modal';
import { ProjectAddMenu } from './ProjectAddMenu';
import { useAppearance } from './appearance';

export type ManagerPage = 'projects' | 'storage';
export type ProjectFilter = 'all' | 'ready' | 'attention';

/** Shared presentation only. Controllers own native requests and capabilities. */
export function ManagerShell({ page, onPageChange, onSettings, navigationDisabled = false,
  settingsDisabled = false, storageUnavailableReason, toolbar, children,
  footerCount, workspaceRoot,
}: {
  page: ManagerPage;
  onPageChange: (page: ManagerPage) => void;
  onSettings: () => void;
  navigationDisabled?: boolean;
  settingsDisabled?: boolean;
  storageUnavailableReason?: string;
  toolbar?: ReactNode;
  children: ReactNode;
  footerCount: ReactNode;
  workspaceRoot?: string;
}) {
  const { theme, setTheme } = useAppearance();
  return <div className="radio-shell" data-theme={theme}>
    <header className="radio-header">
      <div className="radio-brand">
        <img src="/panda-app-drawer.png" alt="" />
        <div><strong className="radio-brand-wordmark">PANDA DRAWER</strong><span className="radio-brand-caption">熊猫应用小抽屉</span></div>
      </div>
      <nav className="radio-primary-nav" aria-label="工作台导航">
        <button aria-current={page === 'projects' ? 'page' : undefined} aria-pressed={page === 'projects'} disabled={navigationDisabled} onClick={() => onPageChange('projects')}>我的项目</button>
        <button aria-current={page === 'storage' ? 'page' : undefined} aria-pressed={page === 'storage'} disabled={navigationDisabled || Boolean(storageUnavailableReason)} title={storageUnavailableReason} onClick={() => onPageChange('storage')}>空间与工作区</button>
      </nav>
      <div className="radio-header-actions">
        <div className="radio-theme-switch" role="group" aria-label="外观主题">
          <button aria-label="切换暗色主题" aria-pressed={theme === 'dark'} onClick={() => setTheme('dark')}>DARK</button>
          <button aria-label="切换亮色主题" aria-pressed={theme === 'light'} onClick={() => setTheme('light')}>LIGHT</button>
        </div>
        <button className="radio-settings-button" aria-label="设置" title="运行环境与关于" disabled={settingsDisabled} onClick={onSettings}><Settings size={17} /></button>
      </div>
    </header>
    {page === 'projects' && toolbar}
    <main className={`radio-main${page === 'storage' ? ' radio-workspace-view' : ''}`}>
      <h1 className="sr-only">{page === 'projects' ? '我的项目' : '空间与工作区'}</h1>
      {children}
    </main>
    <footer className="radio-footer"><span>{footerCount} APPS</span><span title={workspaceRoot}>LOCAL FIRST</span></footer>
  </div>;
}

export function ManagerToolbar({ query, onQueryChange, filter, onFilterChange, loading, checkedAt,
  menuDisabled, refreshDisabled, createDisabled, addUnavailableReason, importUnavailableReason,
  onRefresh, onCreate, onAdd, onImport, onAcquire,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  filter: ProjectFilter;
  onFilterChange: (filter: ProjectFilter) => void;
  loading: boolean;
  checkedAt?: string | null;
  menuDisabled: boolean;
  refreshDisabled: boolean;
  createDisabled: boolean;
  addUnavailableReason?: string;
  importUnavailableReason?: string;
  onRefresh: () => void | Promise<void>;
  onCreate: () => void;
  onAdd?: () => void;
  onImport?: () => void;
  onAcquire?: () => void;
}) {
  return <div className="radio-toolbar">
    <label className="project-search"><Search size={16} /><span className="sr-only">搜索项目</span><input type="search" data-modal-return-focus value={query} onChange={event => onQueryChange(event.target.value)} placeholder="搜索项目…" /></label>
    <label className="project-filter"><span className="sr-only">筛选项目状态</span><select value={filter} onChange={event => onFilterChange(event.target.value as ProjectFilter)}><option value="all">全部项目</option><option value="ready">可进入</option><option value="attention">需要处理</option></select></label>
    <div className="radio-toolbar-spacer" />
    <button className="radio-refresh-button" aria-label="刷新项目和运行状态" aria-busy={loading} title={loading ? '正在刷新' : checkedAt ? `刷新 · 上次更新 ${checkedAt}` : '刷新项目和运行状态'} disabled={loading || refreshDisabled} onClick={() => void onRefresh()}><RefreshCw size={16} className={loading ? 'rotating' : undefined} /></button>
    <ProjectAddMenu disabled={menuDisabled}
      addUnavailableReason={addUnavailableReason} importUnavailableReason={importUnavailableReason}
      onAdd={onAdd} onImport={onImport} onAcquire={onAcquire} />
    <button className="primary-button" onClick={onCreate} disabled={createDisabled}>新建项目</button>
  </div>;
}

/** Both runtime kinds share one collection; readiness is decided by controllers. */
export function ProjectCollection({ totalCount, filteredCount, hasFilter, loading, connected,
  createDisabled, onCreate, onResetFilters, emptyTitle, emptyDescription, children,
}: {
  totalCount: number;
  filteredCount: number;
  hasFilter: boolean;
  loading: boolean;
  connected: boolean;
  createDisabled: boolean;
  onCreate: () => void;
  onResetFilters: () => void;
  emptyTitle?: string;
  emptyDescription?: ReactNode;
  children: ReactNode;
}) {
  return <>
    <div className="sr-only" role="status">{hasFilter ? `${filteredCount} / ${totalCount} 个项目` : `${totalCount} 个项目`}{loading ? '，正在读取' : ''}</div>
    {filteredCount > 0 ? <section className="project-grid radio-project-grid" aria-label="项目列表">{children}</section>
      : totalCount > 0 ? <section className="project-search-empty"><Search size={28} /><h2>没有找到匹配的项目</h2><p>试试其他关键词，或清除状态筛选。</p><button className="refresh-button" onClick={onResetFilters}>显示全部项目</button></section>
      : <section className="workspace-card empty-projects">
        <div className="workspace-art" aria-hidden="true"><div className="art-sheet art-sheet-back" /><div className="art-sheet art-sheet-front"><FolderOpen size={38} strokeWidth={1.2} /><span /><span /></div><span className="art-small-square" /></div>
        <div className="workspace-message"><h2>{emptyTitle ?? (loading ? '正在读取你的项目…' : connected ? '从第一个项目开始' : '连接桌面应用，查看项目')}</h2><p>{emptyDescription ?? (connected ? '创建自己的项目，即可进入初始应用。每个项目的数据和文件都保存在本机。' : '实际项目列表由桌面应用读取。')}</p>{connected && <button className="primary-button" disabled={createDisabled} onClick={onCreate}>新建项目</button>}</div>
      </section>}
  </>;
}

export interface CreateProjectFormProps {
  name: string;
  description: string;
  onNameChange: (name: string) => void;
  onDescriptionChange: (description: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void | Promise<void>;
  onClose: () => void;
  creating: boolean;
  disabled?: boolean;
  feedback?: ReactNode;
  title?: string;
  subtitle?: string;
  nameMaxLength?: number;
  descriptionMaxLength?: number;
  footerNote?: ReactNode;
}

export function CreateProjectForm({ name, description, onNameChange, onDescriptionChange,
  onSubmit, onClose, creating, disabled = false, feedback, title = '新建项目',
  subtitle = '起一个名字，即可开始使用。', nameMaxLength = 200, descriptionMaxLength = 2000,
  footerNote = '保存在当前开发工作区',
}: CreateProjectFormProps) {
  return <Modal title={title} description={subtitle} closeDisabled={creating} onClose={onClose}>
    {feedback}
    <form className="create-project-form" onSubmit={event => void onSubmit(event)}>
      <label htmlFor="project-name">项目名称 <span className="required-label">必填</span></label>
      <input id="project-name" required value={name} onChange={event => onNameChange(event.target.value)} placeholder="例如：孩子的英语学习" readOnly={creating || disabled} autoComplete="off" maxLength={nameMaxLength} />
      <label htmlFor="project-description">简介 <span className="optional-label">选填</span></label>
      <textarea id="project-description" value={description} onChange={event => onDescriptionChange(event.target.value)} placeholder="这个项目准备帮你做什么？" rows={3} readOnly={creating || disabled} maxLength={descriptionMaxLength} />
      <div className="form-footer"><span><ShieldCheck size={14} />{footerNote}</span><button className="primary-button" type="submit" disabled={creating || disabled || !name.trim()}>{creating ? '正在创建…' : '创建项目'}<ArrowUpRight size={15} /></button></div>
    </form>
  </Modal>;
}
