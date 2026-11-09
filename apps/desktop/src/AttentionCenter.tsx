import { t } from './i18n';
import { X } from 'lucide-react';
import { useEffect, useId, useRef } from 'react';
import { launcherMonogram } from './ProjectCard';

export interface AttentionAction { label: string; run: () => void }
export interface AttentionItem {
  key: string;
  name: string;
  slot: number;
  iconDataUrl?: string | null;
  title: string;
  detail?: string;
  primary: AttentionAction;
  secondary?: AttentionAction;
  dismiss?: () => void;
}

/**
 * One place for everything an app needs from the user. App problems live on the tile
 * and here, never as cards stacked above the grid.
 */
export function AttentionCenter({ items, open, disabled, onOpenChange }: { items: AttentionItem[]; open: boolean; disabled?: boolean; onOpenChange: (open: boolean) => void }) {
  const id = useId();
  const container = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); onOpenChange(false); button.current?.focus(); } };
    const onPointer = (event: MouseEvent) => { if (container.current && !container.current.contains(event.target as Node)) onOpenChange(false); };
    document.addEventListener('keydown', onKey);
    document.addEventListener('mousedown', onPointer);
    return () => { document.removeEventListener('keydown', onKey); document.removeEventListener('mousedown', onPointer); };
  }, [open, onOpenChange]);
  useEffect(() => { if (!items.length && open) onOpenChange(false); }, [items.length, open, onOpenChange]);
  if (!items.length) return null;
  const run = (action: AttentionAction) => { onOpenChange(false); action.run(); };
  return <div className="attention-center" ref={container}>
    <button ref={button} type="button" className="attention-button" aria-expanded={open} aria-controls={`${id}-panel`} disabled={disabled} onClick={() => onOpenChange(!open)}>
      <span className="attention-dot" aria-hidden="true" />{t("需要处理 {p0}", { p0: String(items.length) })}
    </button>
    {open && <section className="attention-panel" id={`${id}-panel`} aria-label={t("需要处理")}>
      <div className="attention-heading"><h2>{t("需要处理")}</h2><span>{t("点图标也能处理")}</span></div>
      <ul>{items.map(item => <li key={item.key} className="attention-item">
        <span className={`attention-icon launcher-color-${item.slot}`} aria-hidden="true">{item.iconDataUrl ? <img src={item.iconDataUrl} alt="" /> : launcherMonogram(item.name)}</span>
        <div className="attention-body">
          <strong>{item.title}</strong>
          {item.detail && <p>{item.detail}</p>}
          <div className="attention-actions">
            <button type="button" className="primary-button" onClick={() => run(item.primary)}>{item.primary.label}</button>
            {item.secondary && <button type="button" className="refresh-button" onClick={() => run(item.secondary!)}>{item.secondary.label}</button>}
          </div>
        </div>
        {item.dismiss && <button type="button" className="icon-button attention-dismiss" aria-label={t("忽略这条提示")} onClick={item.dismiss}><X size={14} aria-hidden="true" /></button>}
      </li>)}</ul>
    </section>}
  </div>;
}
