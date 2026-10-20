import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { AppWindow, FolderOpen, Globe, Info, Layers3, PanelsTopLeft, Server, Trash2 } from 'lucide-react';
import { bytesText } from './storage-utils';
import type { MaintenanceStatus, ProjectInfo, TaskRecord } from './types';
import type { NodeAppDetail } from './node-app-types';

const labels: Record<ProjectInfo['state'], string> = {
  ready: '可进入', unbuilt: '等待构建', missing: '文件缺失', error: '需要检查', maintenance: '维护中', recoveryRequired: '恢复待处理',
};
const fallbackIcons = [FolderOpen, AppWindow, Layers3, PanelsTopLeft];

function iconSlot(id: string) {
  let value = 0;
  for (const character of id) value = (Math.imul(value, 31) + character.codePointAt(0)!) >>> 0;
  return value % 8;
}

type LauncherAction = { label: string; accessibleLabel: string; disabled: boolean; unavailableReason?: string; danger?: boolean; icon: ReactNode; run: () => void };

export function launcherMenuPosition(anchor: { top: number; bottom: number; right: number }, menu: { width: number; height: number }, viewport: { width: number; height: number }) {
  const margin = 8;
  const gap = 6;
  const maxWidth = Math.max(0, viewport.width - margin * 2);
  const width = Math.min(menu.width, maxWidth);
  const below = Math.max(0, viewport.height - margin - anchor.bottom - gap);
  const above = Math.max(0, anchor.top - margin - gap);
  const placement = menu.height > below && above > below ? 'top' : 'bottom';
  const maxHeight = Math.min(Math.max(0, viewport.height - margin * 2), placement === 'top' ? above : below);
  const height = Math.min(menu.height, maxHeight);
  const clamp = (value: number, maximum: number) => Math.max(margin, Math.min(value, maximum));
  return {
    left: clamp(anchor.right - width, viewport.width - width - margin),
    top: clamp(placement === 'top' ? anchor.top - gap - height : anchor.bottom + gap, viewport.height - height - margin),
    maxWidth,
    maxHeight,
    placement,
  };
}

function LauncherTile({ id, name, runtime, iconDataUrl, disabled, title, opening, statuses, description, metadata, actions, onEnter }: {
  id: string;
  name: string;
  runtime: 'static-web' | 'node-service';
  iconDataUrl?: string | null;
  disabled: boolean;
  title?: string;
  opening: boolean;
  statuses: string[];
  description?: string;
  metadata?: string;
  actions: LauncherAction[];
  onEnter: () => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [contextPoint, setContextPoint] = useState<{ x: number; y: number } | null>(null);
  const [menuPosition, setMenuPosition] = useState<ReturnType<typeof launcherMenuPosition> | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const entryRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const descriptionId = useId();
  const slot = iconSlot(id);
  const DefaultIcon = fallbackIcons[slot % fallbackIcons.length];
  const runtimeLabel = runtime === 'node-service' ? 'Node 服务项目' : '网页项目';
  const unavailable = actions.every(action => action.disabled);
  const summary = [runtimeLabel, description, ...statuses].filter(Boolean).join('。');
  const tooltip = [name, runtimeLabel, ...statuses, title].filter(Boolean).join(' · ');

  function restoreFocus() {
    (disabled ? rootRef.current : entryRef.current)?.focus({ preventScroll: true });
  }

  function openMenu(point: { x: number; y: number } | null = null) {
    if (unavailable) return;
    setContextPoint(point);
    setMenuOpen(true);
  }

  function closeMenu(returnFocus = false) {
    setMenuOpen(false);
    if (returnFocus) restoreFocus();
  }

  useLayoutEffect(() => {
    if (!menuOpen) { setMenuPosition(null); return; }
    if (!rootRef.current || !menuRef.current) return;
    const menu = menuRef.current.getBoundingClientRect();
    const anchor = contextPoint ? { top: contextPoint.y, bottom: contextPoint.y, right: contextPoint.x + menu.width } : rootRef.current.getBoundingClientRect();
    setMenuPosition(launcherMenuPosition(anchor, menu, { width: window.innerWidth, height: window.innerHeight }));
  }, [menuOpen, contextPoint]);

  useLayoutEffect(() => {
    if (menuOpen && menuPosition) menuRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus({ preventScroll: true });
  }, [menuOpen, menuPosition]);

  useEffect(() => {
    if (!menuOpen) return;
    function outside(event: Event) {
      if (event.target instanceof Node && !menuRef.current?.contains(event.target)) setMenuOpen(false);
    }
    function viewportChanged(event: Event) {
      if (event.type === 'scroll' && event.target instanceof Node && menuRef.current?.contains(event.target)) return;
      if (menuRef.current?.contains(document.activeElement)) restoreFocus();
      setMenuOpen(false);
    }
    function escape(event: globalThis.KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        setMenuOpen(false);
        restoreFocus();
      }
    }
    document.addEventListener('pointerdown', outside);
    document.addEventListener('focusin', outside);
    document.addEventListener('keydown', escape);
    document.addEventListener('scroll', viewportChanged, true);
    window.addEventListener('resize', viewportChanged);
    return () => {
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('focusin', outside);
      document.removeEventListener('keydown', escape);
      document.removeEventListener('scroll', viewportChanged, true);
      window.removeEventListener('resize', viewportChanged);
    };
  }, [menuOpen, disabled]);

  useEffect(() => { if (unavailable) setMenuOpen(false); }, [unavailable]);

  function moveMenuFocus(event: KeyboardEvent<HTMLDivElement>) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const items = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    const active = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (active + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    items[next]?.focus();
  }

  return <article ref={rootRef} className={`project-card launcher-tile${menuOpen ? ' menu-open' : ''}`} aria-label={name} aria-describedby={descriptionId} aria-haspopup="menu" aria-controls={menuOpen ? menuId : undefined} aria-expanded={menuOpen} aria-keyshortcuts="Shift+F10" tabIndex={disabled && !unavailable ? 0 : -1} title={tooltip}
    onContextMenu={event => { event.preventDefault(); openMenu({ x: event.clientX, y: event.clientY }); }}
    onKeyDown={event => { if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) { event.preventDefault(); openMenu(); } }}>
    <button ref={entryRef} className="primary-button launcher-open" type="button" disabled={disabled} onClick={onEnter} aria-label={`进入项目「${name}」`} aria-describedby={descriptionId} aria-busy={opening} title={tooltip}>
      <span className={`project-icon launcher-icon launcher-color-${slot}`} aria-hidden="true">
        {iconDataUrl ? <img src={iconDataUrl} alt="" /> : <DefaultIcon size={30} strokeWidth={1.6} />}
        {runtime === 'node-service' && <img className="launcher-node-badge" src="/ui/node-mark.png" alt="" />}
      </span>
      <span className="launcher-name" title={name}>{name}</span>
    </button>
    <span id={descriptionId} className="sr-only" aria-live="polite">{summary}</span>
    {menuOpen && createPortal(<div ref={menuRef} id={menuId} className="launcher-menu" role="menu" aria-label={`项目「${name}」操作`} data-placement={menuPosition?.placement} style={{ position: 'fixed', right: 'auto', left: menuPosition?.left ?? 0, top: menuPosition?.top ?? 0, maxWidth: menuPosition?.maxWidth, maxHeight: menuPosition?.maxHeight, overflowY: 'auto', zIndex: 80, visibility: menuPosition ? 'visible' : 'hidden' }} onKeyDown={moveMenuFocus}>
      <div className="launcher-menu-caption" role="presentation">{runtime === 'node-service' ? <Server size={13} aria-hidden="true" /> : <Globe size={13} aria-hidden="true" />}{runtimeLabel}{metadata && <span>{metadata}</span>}</div>
      {statuses.length > 0 && <div className="launcher-menu-status" role="presentation">{statuses.join(' · ')}</div>}
      {actions.map(action => <button key={action.accessibleLabel} role="menuitem" tabIndex={-1} type="button" className={`launcher-menu-item${action.danger ? ' danger' : ''}`} disabled={action.disabled} title={action.unavailableReason} aria-label={action.accessibleLabel} aria-haspopup="dialog" onClick={() => { closeMenu(true); action.run(); }}>{action.icon}{action.label}</button>)}
    </div>, document.body)}
  </article>;
}

export function ProjectCard({ project, status, task, blocked, opening, deletingDisabled, detailsDisabled = false, readOnly = false, deleteUnavailableReason, onEnter, onDetails, onDelete }: {
  project: ProjectInfo;
  status?: MaintenanceStatus;
  task?: TaskRecord;
  blocked: boolean;
  opening: boolean;
  deletingDisabled: boolean;
  detailsDisabled?: boolean;
  readOnly?: boolean;
  deleteUnavailableReason?: string;
  onEnter: () => void;
  onDetails: () => void;
  onDelete: () => void;
}) {
  const state = status?.recoveryRequired ? 'recoveryRequired' : status?.mode === 'maintenance' ? 'maintenance' : project.state;
  const busy = task && !task.final;
  const size = project.storage?.currentProject;
  const metadata = size?.totalBytes != null ? `${bytesText(size.totalBytes)}${size.stale ? ' · 待扫描' : size.complete === false ? ' · 部分统计' : ''}` : '空间尚未统计';
  return <LauncherTile id={project.id} name={project.name} runtime="static-web" iconDataUrl={project.iconDataUrl}
    disabled={blocked || project.state !== 'ready' || opening} opening={opening} onEnter={onEnter}
    title={project.state !== 'ready' ? '请先在详情中检查状态或构建项目' : blocked ? '请先处理项目维护或等待工作区任务完成' : undefined}
    description={project.description} metadata={metadata}
    statuses={[readOnly && state === 'ready' ? '可只读打开' : null, state !== 'ready' ? labels[state] : null, busy ? '任务进行中' : null, opening ? '进入中…' : null].filter((value): value is string => Boolean(value))}
    actions={[
      { label: '查看详情', accessibleLabel: `查看项目「${project.name}」的详情`, disabled: detailsDisabled, icon: <Info size={15} aria-hidden="true" />, run: onDetails },
      { label: '删除项目', accessibleLabel: `删除项目「${project.name}」`, disabled: deletingDisabled || Boolean(deleteUnavailableReason), unavailableReason: deleteUnavailableReason, danger: true, icon: <Trash2 size={15} aria-hidden="true" />, run: onDelete },
    ]} />;
}

export function NodeProjectCard({ app, blocked, opening, onEnter, onDetails }: {
  app: NodeAppDetail; blocked: boolean; opening: boolean; onEnter: () => void; onDetails: () => void;
}) {
  const needsConfiguration = !app.configuration.canStart;
  const runtimeState = app.state === 'running' ? '运行中' : app.state === 'starting' ? '打开中' : app.state === 'stopping' ? '关闭中' : app.state === 'failed' ? '需要检查' : null;
  return <LauncherTile id={app.id} name={app.name} runtime="node-service" disabled={blocked || opening} opening={opening} onEnter={onEnter}
    description={app.lastError ?? (needsConfiguration ? '进入前请先完成应用配置。' : undefined)}
    metadata={app.restartRequired ? '已保存配置' : needsConfiguration ? undefined : '配置完整'}
    statuses={[runtimeState, needsConfiguration ? '需要配置' : null, app.restartRequired ? '待重启' : null, opening && !['starting', 'stopping'].includes(app.state) ? '进入中…' : null].filter((value): value is string => Boolean(value))}
    actions={[{ label: '查看详情', accessibleLabel: `查看项目「${app.name}」的详情`, disabled: opening, icon: <Info size={15} aria-hidden="true" />, run: onDetails }]} />;
}
