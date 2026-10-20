import { useEffect, useRef } from 'react';
import { ChevronDown, FolderOpen, RefreshCw, Upload } from 'lucide-react';

type Filter = 'all' | 'ready' | 'attention';
type Props = {
  disabled: boolean;
  refreshing: boolean;
  filter: Filter;
  onFilter: (value: Filter) => void;
  onAdd?: () => void;
  onImport?: () => void;
  addUnavailableReason?: string;
  importUnavailableReason?: string;
  onRefresh: () => void;
};

export function ProjectAddMenu({ disabled, refreshing, filter, onFilter, onAdd, onImport, onRefresh, addUnavailableReason, importUnavailableReason }: Props) {
  const disclosure = useRef<HTMLDetailsElement>(null);
  const trigger = useRef<HTMLElement>(null);
  const close = () => { disclosure.current?.removeAttribute('open'); };
  useEffect(() => {
    if (disabled) close();
  }, [disabled]);
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !disclosure.current?.contains(event.target)) close();
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, []);
  const run = (action: () => void) => { trigger.current?.focus({ preventScroll: true }); close(); action(); };
  return <details ref={disclosure} className="radio-add-menu" onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) close();
  }} onKeyDown={(event) => {
    if (event.key === 'Escape' && disclosure.current?.open) {
      event.preventDefault(); event.stopPropagation(); close(); trigger.current?.focus();
    }
  }}>
    <summary ref={trigger} aria-disabled={disabled} onClick={(event) => { if (disabled) event.preventDefault(); }}><ChevronDown size={16} />添加</summary>
    <div className="radio-add-menu-content" aria-label="项目操作">
      <button disabled={disabled || !onAdd || Boolean(addUnavailableReason)} title={addUnavailableReason} onClick={() => { if (onAdd && !addUnavailableReason) run(onAdd); }}><FolderOpen size={15} />添加本地应用</button>
      <button disabled={disabled || !onImport || Boolean(importUnavailableReason)} title={importUnavailableReason} onClick={() => { if (onImport && !importUnavailableReason) run(onImport); }}><Upload size={15} />导入项目</button>
      <button disabled={disabled || refreshing} aria-label="刷新项目和运行状态" onClick={() => run(onRefresh)}><RefreshCw size={15} />{refreshing ? '读取中…' : '刷新项目和运行状态'}</button>
      <label className="project-filter"><span>筛选项目状态</span><select value={filter} onChange={(event) => onFilter(event.target.value as Filter)}><option value="all">全部项目</option><option value="ready">可进入</option><option value="attention">需要处理</option></select></label>
    </div>
  </details>;
}
