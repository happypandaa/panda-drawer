import { t } from './i18n';
import { Check, CircleAlert, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

interface FeedbackContext { scope: string; version: number }
interface FeedbackMessage extends FeedbackContext {
  id: number;
  kind: 'error' | 'success';
  text: string;
  slot: string;
  backgroundKey?: string;
}

// A context version changes even when the user leaves and returns to the same
// page. An in-flight operation may update data, but cannot revive its old UI.
export function useManagementFeedback(scope: string) {
  const context = useRef<FeedbackContext>({ scope, version: 0 });
  if (context.current.scope !== scope) context.current = { scope, version: context.current.version + 1 };
  const alive = useRef(true);
  const sequence = useRef(0);
  const reported = useRef(new Map<string, Set<string>>());
  const [messages, setMessages] = useState<FeedbackMessage[]>([]);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const capture = useCallback(() => context.current, []);
  const isCurrent = useCallback((origin: FeedbackContext) => alive.current && origin.scope === context.current.scope && origin.version === context.current.version, []);
  const publish = useCallback((origin: FeedbackContext, kind: FeedbackMessage['kind'], text: string, slot = 'main', options?: { nextScope?: string; backgroundKey?: string }) => {
    if (!isCurrent(origin)) return;
    const key = options?.backgroundKey;
    if (key) {
      const previous = reported.current.get(key) ?? new Set<string>();
      if (previous.has(text)) return;
      previous.add(text); reported.current.set(key, previous);
    }
    const destination = options?.nextScope ?? origin.scope;
    const message: FeedbackMessage = { id: ++sequence.current, kind, text, slot, scope: destination, backgroundKey: key, version: origin.version + (destination === origin.scope ? 0 : 1) };
    setMessages((current) => {
      const retained = current.filter((item) => item.version >= origin.version);
      if (kind === 'error' && retained.some((item) => item.kind === kind && item.version === message.version && item.scope === destination && item.slot === slot && item.text === text)) return retained;
      return [...retained.filter((item) => kind !== 'success' || item.kind !== 'success' || item.scope !== destination || item.slot !== slot), message];
    });
  }, [isCurrent]);
  const error = useCallback((origin: FeedbackContext, text: string, slot = 'main', options?: { nextScope?: string; backgroundKey?: string }) => publish(origin, 'error', text, slot, options), [publish]);
  const success = useCallback((origin: FeedbackContext, text: string, slot = 'main', nextScope?: string) => publish(origin, 'success', text, slot, { nextScope }), [publish]);
  const recover = useCallback((key: string) => { reported.current.delete(key); }, []);
  const reset = useCallback(() => {
    context.current = { ...context.current, version: context.current.version + 1 };
    reported.current.clear();
    setMessages([]);
    return context.current;
  }, []);
  const dismiss = useCallback((id: number) => setMessages((current) => current.filter((item) => item.id !== id)), []);
  const forgetProject = useCallback((id: string, taskIds: string[]) => {
    const belongs = (value: string) => [id, ...taskIds].some(token => value.split(':').includes(token));
    for (const key of reported.current.keys()) if (belongs(key)) reported.current.delete(key);
    setMessages(current => current.filter(item => !belongs(item.scope) && !belongs(item.slot) && !belongs(item.backgroundKey ?? '')));
  }, []);
  const visible = messages.filter((item) => item.scope === scope && item.version === context.current.version);
  return { capture, isCurrent, error, success, recover, reset, dismiss, forgetProject, messages: visible };
}

export function FeedbackNotices({ messages, dismiss, slot = 'main', toast = false }: { messages: FeedbackMessage[]; dismiss: (id: number) => void; slot?: string; toast?: boolean }) {
  const own = messages.filter((item) => item.slot === slot);
  const [expanded, setExpanded] = useState(false);
  if (!toast) return <>{own.map((item) => <FeedbackNotice key={item.id} message={item} dismiss={dismiss} />)}</>;
  // Page level: successes float as a short toast in the corner and never move the grid;
  // errors collapse into one bar, with older ones behind "还有 N 条".
  const toasts = own.filter((item) => item.kind === 'success').slice(-2);
  const errors = own.filter((item) => item.kind !== 'success');
  const shown = expanded ? errors : errors.slice(-1);
  const hidden = errors.length - shown.length;
  return <>
    {shown.map((item) => <FeedbackNotice key={item.id} message={item} dismiss={dismiss} />)}
    {hidden > 0 && <button type="button" className="text-button notice-more" onClick={() => setExpanded(true)}>{t("还有 {p0} 条提示", { p0: String(hidden) })}</button>}
    {toasts.length > 0 && typeof document !== 'undefined' && createPortal(<div className="feedback-toasts">{toasts.map((item) => <FeedbackNotice key={item.id} message={item} dismiss={dismiss} duration={3000} />)}</div>, document.body)}
  </>;
}

function FeedbackNotice({ message, dismiss, duration = 5000 }: { message: FeedbackMessage; dismiss: (id: number) => void; duration?: number }) {
  useEffect(() => {
    if (message.kind !== 'success') return;
    const timer = window.setTimeout(() => dismiss(message.id), duration);
    return () => window.clearTimeout(timer);
  }, [message.id, message.kind, dismiss, duration]);
  // The message itself says what happened; no generic "请查看处理" heading.
  return message.kind === 'error'
    ? <div className="notice notice-error" role="alert"><CircleAlert size={18} /><p>{t(message.text)}</p><button className="icon-button" aria-label={t("关闭错误提示")} onClick={() => dismiss(message.id)}><X size={16} /></button></div>
    : <div className="notice notice-success" role="status"><Check size={16} /><p>{t(message.text)}</p><button className="icon-button" aria-label={t("关闭操作提示")} onClick={() => dismiss(message.id)}><X size={14} /></button></div>;
}
