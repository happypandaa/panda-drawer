import {
  Activity, ArrowUpRight, Boxes, Folder, FolderOpen, Info,
  LayoutGrid, Package, Plus, RefreshCw, Search, ShieldCheck, Trash2, Upload,
} from 'lucide-react';
import type { ComponentProps, FormEvent, ReactNode } from 'react';
import { Modal } from './Modal';
import { ProjectCard } from './ProjectCard';
import type { ProjectInfo } from './types';

export type ManagerPage = 'projects' | 'runtime' | 'storage' | 'about';
export type ProjectFilter = 'all' | 'ready' | 'attention';

export interface ManagerShellProps {
  page: ManagerPage;
  onPageChange: (page: ManagerPage) => void;
  projectCount: number;
  title: string;
  description: ReactNode;
  actions: ReactNode;
  children: ReactNode;
  overlays?: ReactNode;
  stageLabel?: string;
  versionLabel?: string;
  sidebarDescription?: ReactNode;
  disabledPages?: Partial<Record<ManagerPage, string>>;
}

const navigation = [
  { page: 'projects', label: '我的项目', icon: LayoutGrid },
  { page: 'runtime', label: '运行环境', icon: Activity },
  { page: 'storage', label: '空间与工作区', icon: Trash2 },
  { page: 'about', label: '关于平台', icon: Package },
] as const;

/** Shared presentation only. Controllers own native requests and capabilities. */
export function ManagerShell({
  page, onPageChange, projectCount, title, description, actions, children, overlays,
  stageLabel = 'M5 · 开发验证', versionLabel = 'M5',
  sidebarDescription = '桌面窗口和浏览器连接同一个本地服务，实时读取项目数据。',
  disabledPages = {},
}: ManagerShellProps) {
  return <div className="app-shell">
    <aside className="sidebar" aria-label="工作台导航">
      <div className="brand">
        <span className="brand-mark" aria-hidden="true"><Boxes size={22} strokeWidth={1.6} /></span>
        <div><strong>本地项目</strong><span>一个属于你的工作空间</span></div>
      </div>
      <div className="sidebar-section-label">工作台</div>
      <nav className="side-nav">
        {navigation.map(({ page: target, label, icon: Icon }) => <button
          key={target}
          type="button"
          className={page === target ? 'nav-current' : 'nav-item'}
          aria-current={page === target ? 'page' : undefined}
          disabled={disabledPages[target] !== undefined}
          title={disabledPages[target]}
          onClick={() => onPageChange(target)}
        ><Icon size={17} /><span>{label}</span>{target === 'projects' && <small>{projectCount}</small>}</button>)}
      </nav>
      <div className="sidebar-stage">
        <span className="eyebrow">LOCAL FIRST</span>
        <strong>项目各自独立，数据留在本机。</strong>
        <p>{sidebarDescription}</p>
        <span className="subtle-tag">{stageLabel}</span>
      </div>
      <div className="sidebar-bottom"><span className="environment-dot" />内部开发版<span className="version">{versionLabel}</span></div>
    </aside>
    <div className="content-shell">
      <header className="topbar">
        <span>工作台 <span className="breadcrumb-slash">/</span> <strong>{title}</strong></span>
        <span className="development-badge"><ShieldCheck size={13} />独立开发环境</span>
      </header>
      <main>
        <div className="page-heading">
          <div><h1>{title}</h1><p>{description}</p></div>
          <div className="heading-actions">{actions}</div>
        </div>
        {children}
      </main>
      {overlays}
    </div>
  </div>;
}

export interface ManagerActionsProps {
  loading: boolean;
  inDesktop?: boolean;
  refreshDisabled: boolean;
  createDisabled: boolean;
  importDisabled: boolean;
  showProjectActions: boolean;
  onRefresh: () => void | Promise<void>;
  onCreate: () => void;
  onImport?: () => void;
  importUnavailableReason?: string;
}

export function ManagerActions({
  loading, inDesktop = true, refreshDisabled, createDisabled, importDisabled,
  showProjectActions, onRefresh, onCreate, onImport, importUnavailableReason,
}: ManagerActionsProps) {
  return <>
    <button className="refresh-button" onClick={() => void onRefresh()} disabled={loading || !inDesktop || refreshDisabled} aria-label="刷新项目和运行状态"><RefreshCw size={15} className={loading ? 'rotating' : undefined} />{loading ? '读取中' : '刷新'}</button>
    {showProjectActions && <button className="refresh-button" disabled={importDisabled || !onImport} onClick={onImport} title={importUnavailableReason}><Upload size={15} />导入项目</button>}
    {showProjectActions && <button className="primary-button" onClick={onCreate} disabled={createDisabled}><Plus size={16} />新建项目</button>}
  </>;
}

export interface ProjectCollectionProps {
  projects: ProjectInfo[];
  query: string;
  onQueryChange: (query: string) => void;
  filter: ProjectFilter;
  onFilterChange: (filter: ProjectFilter) => void;
  isProjectReady: (project: ProjectInfo) => boolean;
  cardProps: (project: ProjectInfo) => Omit<ComponentProps<typeof ProjectCard>, 'project'>;
  loading: boolean;
  checkedAt: string | null;
  connected: boolean;
  createDisabled: boolean;
  onCreate: () => void;
  guide?: ReactNode;
  workspaceRoot?: string | null;
  footerHint?: ReactNode;
  emptyTitle?: string;
  emptyDescription?: ReactNode;
}

export function ProjectCollection({
  projects, query, onQueryChange, filter, onFilterChange, isProjectReady, cardProps,
  loading, checkedAt, connected, createDisabled, onCreate, guide, workspaceRoot,
  footerHint = '删除后的项目可在回收站恢复', emptyTitle, emptyDescription,
}: ProjectCollectionProps) {
  const filteredProjects = projects.filter((project) => {
    const matchesQuery = `${project.name} ${project.description ?? ''} ${project.id}`
      .toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
    const ready = isProjectReady(project);
    return matchesQuery && (filter === 'all' || (filter === 'ready' ? ready : !ready));
  });
  return <>
    <div className="project-toolbar">
      <label className="project-search"><Search size={17} /><span className="sr-only">搜索项目</span><input type="search" data-modal-return-focus value={query} onChange={(event) => onQueryChange(event.target.value)} placeholder="搜索名称、简介或项目 ID" /></label>
      <label className="project-filter"><span className="sr-only">筛选项目状态</span><select value={filter} onChange={(event) => onFilterChange(event.target.value as ProjectFilter)}><option value="all">全部项目</option><option value="ready">可进入</option><option value="attention">需要处理</option></select></label>
    </div>
    <div className="project-list-heading"><span>{query.trim() || filter !== 'all' ? `${filteredProjects.length} / ${projects.length} 个项目` : `${projects.length} 个项目`}</span><span>{loading ? '正在读取…' : checkedAt ? `更新于 ${checkedAt}` : '等待桌面连接'}</span></div>
    {filteredProjects.length > 0 ? <section className="project-grid" aria-label="项目列表">
      {filteredProjects.map((project) => <ProjectCard key={project.id} project={project} {...cardProps(project)} />)}
    </section> : projects.length > 0 ? <section className="project-search-empty"><Search size={28} /><h2>没有找到匹配的项目</h2><p>试试其他关键词，或清除状态筛选。</p><button className="refresh-button" onClick={() => { onQueryChange(''); onFilterChange('all'); }}>显示全部项目</button></section> : <section className="workspace-card empty-projects">
      <div className="workspace-art" aria-hidden="true"><div className="art-sheet art-sheet-back" /><div className="art-sheet art-sheet-front"><FolderOpen size={38} strokeWidth={1.2} /><span /><span /></div><span className="art-small-square" /></div>
      <div className="workspace-message"><h2>{emptyTitle ?? (loading ? '正在读取你的项目…' : connected ? '从第一个项目开始' : '连接桌面应用，查看项目')}</h2><p>{emptyDescription ?? (connected ? '创建自己的项目，即可进入初始应用。每个项目的数据和文件都保存在本机。' : '实际项目列表由桌面应用读取。')}</p>{connected && <button className="primary-button" disabled={createDisabled} onClick={onCreate}><Plus size={16} />新建项目</button>}</div>
    </section>}
    {guide && <details className="guide-disclosure"><summary><Info size={16} />使用指南</summary>{guide}</details>}
    <div className="workspace-location"><Folder size={14} /><span>工作区</span><code>{workspaceRoot ?? '尚未读取'}</code></div>
    <footer className="page-footer"><span>所有数据保存在当前工作区</span><span>{footerHint}</span></footer>
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

export function CreateProjectForm({
  name, description, onNameChange, onDescriptionChange, onSubmit, onClose, creating,
  disabled = false, feedback, title = '新建项目', subtitle = '起一个名字，即可开始使用。',
  nameMaxLength = 200, descriptionMaxLength = 2000, footerNote = '保存在当前开发工作区',
}: CreateProjectFormProps) {
  return <Modal title={title} description={subtitle} closeDisabled={creating} onClose={onClose}>
    {feedback}
    <form className="create-project-form" onSubmit={(event) => void onSubmit(event)}>
      <label htmlFor="project-name">项目名称 <span className="required-label">必填</span></label>
      <input id="project-name" required value={name} onChange={(event) => onNameChange(event.target.value)} placeholder="例如：孩子的英语学习" readOnly={creating || disabled} autoComplete="off" maxLength={nameMaxLength} />
      <label htmlFor="project-description">简介 <span className="optional-label">选填</span></label>
      <textarea id="project-description" value={description} onChange={(event) => onDescriptionChange(event.target.value)} placeholder="这个项目准备帮你做什么？" rows={3} readOnly={creating || disabled} maxLength={descriptionMaxLength} />
      <div className="form-footer"><span><ShieldCheck size={14} />{footerNote}</span><button className="primary-button" type="submit" disabled={creating || disabled || !name.trim()}>{creating ? '正在创建…' : '创建项目'}<ArrowUpRight size={15} /></button></div>
    </form>
  </Modal>;
}
