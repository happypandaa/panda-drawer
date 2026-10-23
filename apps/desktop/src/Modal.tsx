import { createContext, useContext, useId, useLayoutEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

type ModalProps = {
  title: string;
  titleIcon?: ReactNode;
  headerAside?: ReactNode;
  description?: string;
  children: ReactNode;
  onClose: () => void;
  closeDisabled?: boolean;
  beforeClose?: () => boolean;
  wide?: boolean;
  className?: string;
};

type ModalIdentity = { parent: ModalIdentity | null };
type ModalEntry = {
  identity: ModalIdentity;
  layer: HTMLDivElement;
  dialog: HTMLDivElement;
  heading: HTMLHeadingElement;
  returnFocus: Element | null;
  requestClose: () => void;
};
const ModalParent = createContext<ModalIdentity | null>(null);
const modalStacks = new WeakMap<Document, ModalEntry[]>();

function focusable(entry: ModalEntry) {
  const elements = entry.dialog.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, summary, [tabindex]');
  return Array.from(elements).filter((element) => {
    const nativeSummary = element.tagName === 'SUMMARY' && !element.hasAttribute('tabindex');
    if ((element.tabIndex < 0 && !nativeSummary) || element.matches(':disabled') || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
    let parent: HTMLElement | null = element;
    while (parent && parent !== entry.layer) {
      if (parent.tagName === 'DETAILS' && !(parent as HTMLDetailsElement).open && !parent.querySelector(':scope > summary')?.contains(element)) return false;
      const style = getComputedStyle(parent);
      if (style.display === 'none' || style.visibility === 'hidden') return false;
      parent = parent.parentElement;
    }
    return true;
  });
}

function registerModal(entry: ModalEntry) {
  const owner = entry.layer.ownerDocument;
  let stack = modalStacks.get(owner);
  if (!stack) {
    stack = [];
    modalStacks.set(owner, stack);
  }
  const entries = stack;
  // Descendant effects can mount before their parent. Keep the child on top in
  // that case too, rather than relying on React's effect or portal DOM order.
  const descendant = entries.findIndex((candidate) => {
    let parent = candidate.identity.parent;
    while (parent) {
      if (parent === entry.identity) return true;
      parent = parent.parent;
    }
    return false;
  });
  entries.splice(descendant < 0 ? entries.length : descendant, 0, entry);
  modalSessions.get(owner)?.sync();
  return () => {
    const wasTop = entries.at(-1) === entry;
    const index = entries.indexOf(entry);
    // Layout cleanup runs before React removes this portal from the DOM.
    // Deactivate it before exposing the next dialog to accessibility clients.
    entry.dialog.removeAttribute('aria-modal');
    if (index >= 0) entries.splice(index, 1);
    modalSessions.get(owner)?.sync();
    const top = entries.at(-1);
    if (wasTop && top) {
      if (entry.returnFocus instanceof HTMLElement && entry.returnFocus.isConnected && focusable(top).includes(entry.returnFocus)) entry.returnFocus.focus();
      else top.heading.focus();
    }
  };
}

// A single document listener owns keyboard/focus handling and the original
// background state for the whole stack. Closing a parent first must not unlock
// the page or restore focus behind a still-open child.
const modalSessions = new WeakMap<Document, { retain: () => () => void; sync: () => void }>();
function retainModalSession(owner: Document) {
  let session = modalSessions.get(owner);
  if (!session) {
    const previousFocus = owner.activeElement;
    const previousOverflow = owner.body.style.overflow;
    const previousInert = new Map<Element, boolean>();
    let inertSiblings = new Set<Element>();
    let activePath = new Set<Element>();
    let count = 0;
    const top = () => modalStacks.get(owner)?.at(-1);
    function sync() {
      const active = top();
      if (!active) return;
      const path = new Set<Element>();
      const siblings = new Set<Element>();
      let modalRoot = active.dialog;
      let current: Element | null = active.layer;
      while (current && current !== owner.body) {
        path.add(current);
        const parent: Element | null = current.parentElement;
        for (const sibling of Array.from(parent?.children ?? [])) {
          if (sibling !== current) siblings.add(sibling);
        }
        if (parent?.matches('.modal-dialog[role="dialog"]')) modalRoot = parent as HTMLDivElement;
        current = parent;
      }
      // Nested dialogs stay inside the existing ARIA modal root. Switching
      // sibling roots in WebKit can leave the entire accessibility tree empty.
      modalStacks.get(owner)?.forEach((entry) => {
        if (entry.dialog !== modalRoot) entry.dialog.removeAttribute('aria-modal');
      });
      // Restore branches owned by the preceding top dialog, then protect every
      // sibling along the new active path without hiding any of its ancestors.
      for (const element of inertSiblings) {
        if (!siblings.has(element) && !previousInert.get(element)) element.removeAttribute('inert');
      }
      for (const element of activePath) {
        if (!path.has(element) && previousInert.get(element)) element.setAttribute('inert', '');
      }
      for (const element of path) {
        if (!previousInert.has(element)) previousInert.set(element, element.hasAttribute('inert'));
        if (element.hasAttribute('inert')) element.removeAttribute('inert');
      }
      for (const element of siblings) {
        if (!previousInert.has(element)) previousInert.set(element, element.hasAttribute('inert'));
        if (!element.hasAttribute('inert')) element.setAttribute('inert', '');
      }
      inertSiblings = siblings;
      activePath = path;
      modalStacks.get(owner)?.forEach((entry, index) => {
        entry.layer.style.zIndex = String(100 + index);
      });
      if (modalRoot.getAttribute('aria-modal') !== 'true') modalRoot.setAttribute('aria-modal', 'true');
    }
    function onKeyDown(event: KeyboardEvent) {
      const active = top();
      if (!active) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        active.requestClose();
      } else if (event.key === 'Tab') {
        const elements = focusable(active);
        const first = elements[0];
        const last = elements.at(-1);
        const focused = owner.activeElement;
        if (!first) { event.preventDefault(); active.heading.focus(); }
        else if (event.shiftKey && (focused === first || !elements.includes(focused as HTMLElement))) {
          event.preventDefault(); last?.focus();
        } else if (!event.shiftKey && (focused === last || !elements.includes(focused as HTMLElement))) {
          event.preventDefault(); first.focus();
        }
      }
    }
    function onFocusIn(event: FocusEvent) {
      const active = top();
      if (active && event.target instanceof Node && !active.dialog.contains(event.target)) active.heading.focus();
    }
    const observer = new MutationObserver(sync);
    observer.observe(owner.body, { childList: true, subtree: true });
    owner.addEventListener('keydown', onKeyDown, true);
    owner.addEventListener('focusin', onFocusIn);
    owner.body.style.overflow = 'hidden';
    session = { sync, retain() {
      count++;
      return () => {
        if (--count > 0) return;
        observer.disconnect();
        owner.removeEventListener('keydown', onKeyDown, true);
        owner.removeEventListener('focusin', onFocusIn);
        owner.body.style.overflow = previousOverflow;
        previousInert.forEach((inert, element) => { if (inert) element.setAttribute('inert', ''); else element.removeAttribute('inert'); });
        modalSessions.delete(owner);
        modalStacks.delete(owner);
        if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
        else owner.querySelector<HTMLElement>('[data-modal-return-focus]')?.focus();
      };
    } };
    modalSessions.set(owner, session);
  }
  return session.retain();
}

export function Modal({ title, titleIcon, headerAside, description, children, onClose, closeDisabled = false, beforeClose, wide = false, className = '' }: ModalProps) {
  const titleId = useId();
  const descriptionId = useId();
  const layer = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const parent = useContext(ModalParent);
  const identity = useRef<ModalIdentity>({ parent });
  identity.current.parent = parent;
  const options = useRef({ onClose, closeDisabled, beforeClose });
  options.current = { onClose, closeDisabled, beforeClose };

  function requestClose() {
    const owner = layer.current?.ownerDocument;
    if (!owner || modalStacks.get(owner)?.at(-1)?.identity !== identity.current) return;
    const current = options.current;
    if (!current.closeDisabled && (!current.beforeClose || current.beforeClose())) current.onClose();
  }

  useLayoutEffect(() => {
    if (!layer.current || !dialog.current || !heading.current) return;
    const owner = layer.current.ownerDocument;
    const releaseSession = retainModalSession(owner);
    const entry = { identity: identity.current, layer: layer.current, dialog: dialog.current, heading: heading.current, returnFocus: owner.activeElement, requestClose };
    const unregister = registerModal(entry);
    if (modalStacks.get(owner)?.at(-1) === entry) entry.heading.focus();
    return () => {
      unregister();
      releaseSession();
    };
  }, []);

  const contents = (
    <ModalParent.Provider value={identity.current}><div ref={layer} className="modal-layer" onClick={(event) => { if (event.target === event.currentTarget) requestClose(); }}>
      <div ref={dialog} className={`modal-dialog${wide ? ' modal-wide' : ''}${className ? ` ${className}` : ''}`} role="dialog" aria-labelledby={titleId} aria-describedby={description ? descriptionId : undefined}>
        <header className="modal-header">
          <div className="modal-title-group">{titleIcon && <span className="modal-title-icon">{titleIcon}</span>}<div><h2 ref={heading} id={titleId} title={title} tabIndex={-1}>{title}</h2>{description && <p id={descriptionId}>{description}</p>}</div></div>{headerAside}
          <button className="icon-button modal-close" type="button" onClick={requestClose} disabled={closeDisabled} aria-label={`关闭${title}`}><X size={20} /></button>
        </header>
        <div className="modal-content">{children}</div>
      </div>
    </div></ModalParent.Provider>
  );
  return parent ? contents : createPortal(contents, document.body);
}
