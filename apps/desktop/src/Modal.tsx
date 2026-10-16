import { useEffect, useId, useRef } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

type ModalProps = {
  title: string;
  description?: string;
  children: ReactNode;
  onClose: () => void;
  closeDisabled?: boolean;
  beforeClose?: () => boolean;
  wide?: boolean;
};

export function Modal({ title, description, children, onClose, closeDisabled = false, beforeClose, wide = false }: ModalProps) {
  const titleId = useId();
  const descriptionId = useId();
  const layer = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const options = useRef({ onClose, closeDisabled, beforeClose });
  options.current = { onClose, closeDisabled, beforeClose };

  function requestClose() {
    const current = options.current;
    if (!current.closeDisabled && (!current.beforeClose || current.beforeClose())) current.onClose();
  }

  useEffect(() => {
    const previousFocus = document.activeElement;
    const previousOverflow = document.body.style.overflow;
    const siblings = Array.from(document.body.children).filter((element) => element !== layer.current);
    const previousInert = siblings.map((element) => element.hasAttribute('inert'));
    siblings.forEach((element) => element.setAttribute('inert', ''));
    document.body.style.overflow = 'hidden';
    heading.current?.focus();

    function focusable() {
      const elements = dialog.current?.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]');
      return Array.from(elements ?? []).filter((element) => {
        if (element.tabIndex < 0 || element.matches(':disabled') || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
        let parent: HTMLElement | null = element;
        while (parent && parent !== dialog.current?.parentElement) {
          const style = getComputedStyle(parent);
          if (style.display === 'none' || style.visibility === 'hidden') return false;
          parent = parent.parentElement;
        }
        return true;
      });
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        requestClose();
      } else if (event.key === 'Tab') {
        const elements = focusable();
        const first = elements[0];
        const last = elements.at(-1);
        const active = document.activeElement;
        if (!first) { event.preventDefault(); heading.current?.focus(); }
        else if (event.shiftKey && (active === first || !elements.includes(active as HTMLElement))) {
          event.preventDefault(); last?.focus();
        } else if (!event.shiftKey && (active === last || !elements.includes(active as HTMLElement))) {
          event.preventDefault(); first.focus();
        }
      }
    }
    function onFocusIn(event: FocusEvent) {
      if (event.target instanceof Node && !dialog.current?.contains(event.target)) heading.current?.focus();
    }
    document.addEventListener('keydown', onKeyDown, true);
    document.addEventListener('focusin', onFocusIn);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('focusin', onFocusIn);
      document.body.style.overflow = previousOverflow;
      siblings.forEach((element, index) => { if (!previousInert[index]) element.removeAttribute('inert'); });
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
      else document.querySelector<HTMLElement>('[data-modal-return-focus]')?.focus();
    };
  }, []);

  return createPortal(
    <div ref={layer} className="modal-layer" onClick={(event) => { if (event.target === event.currentTarget) requestClose(); }}>
      <div ref={dialog} className={`modal-dialog${wide ? ' modal-wide' : ''}`} role="dialog" aria-modal="true" aria-labelledby={titleId} aria-describedby={description ? descriptionId : undefined}>
        <header className="modal-header">
          <div><h2 ref={heading} id={titleId} title={title} tabIndex={-1}>{title}</h2>{description && <p id={descriptionId}>{description}</p>}</div>
          <button className="icon-button modal-close" type="button" onClick={requestClose} disabled={closeDisabled} aria-label={`关闭${title}`}><X size={20} /></button>
        </header>
        <div className="modal-content">{children}</div>
      </div>
    </div>, document.body,
  );
}
