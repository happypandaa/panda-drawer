import { useEffect, useRef } from 'react';
import { ChevronDown, FolderOpen, Upload } from 'lucide-react';

type Props = {
  disabled: boolean;
  onAdd: () => void;
  onImport: () => void;
};

export function ProjectAddMenu({ disabled, onAdd, onImport }: Props) {
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
    // WebKit can blur the summary without focusing the clicked button. Keep it
    // visible until that click runs; outside pointers already close the menu.
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node)) close();
  }} onKeyDown={(event) => {
    if (event.key === 'Escape' && disclosure.current?.open) {
      event.preventDefault(); event.stopPropagation(); close(); trigger.current?.focus();
    }
  }}>
    <summary ref={trigger} aria-disabled={disabled} onClick={(event) => { if (disabled) event.preventDefault(); }}><ChevronDown size={16} />添加</summary>
    <div className="radio-add-menu-content" aria-label="项目操作">
      <button disabled={disabled} onClick={() => run(onAdd)}><FolderOpen size={15} />添加本地应用</button>
      <button disabled={disabled} onClick={() => run(onImport)}><Upload size={15} />导入项目</button>
    </div>
  </details>;
}
