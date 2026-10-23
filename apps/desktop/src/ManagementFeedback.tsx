import { Check, CircleAlert, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';

interface FeedbackContext { scope: string; version: number }
interface FeedbackMessage extends FeedbackContext {
  id: number;
  kind: 'error' | 'success';
  text: string;
  slot: string;
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
    const message: FeedbackMessage = { id: ++sequence.current, kind, text, slot, scope: destination, version: origin.version + (destination === origin.scope ? 0 : 1) };
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
  const visible = messages.filter((item) => item.scope === scope && item.version === context.current.version);
  return { capture, isCurrent, error, success, recover, reset, dismiss, messages: visible };
}

export function FeedbackNotices({ messages, dismiss, slot = 'main' }: { messages: FeedbackMessage[]; dismiss: (id: number) => void; slot?: string }) {
  return <>{messages.filter((item) => item.slot === slot).map((item) => <FeedbackNotice key={item.id} message={item} dismiss={dismiss} />)}</>;
}

function FeedbackNotice({ message, dismiss }: { message: FeedbackMessage; dismiss: (id: number) => void }) {
  useEffect(() => {
    if (message.kind !== 'success') return;
    const timer = window.setTimeout(() => dismiss(message.id), 5000);
    return () => window.clearTimeout(timer);
  }, [message.id, message.kind, dismiss]);
  return message.kind === 'error'
    ? <div className="notice notice-error" role="alert"><CircleAlert size={20} /><div><strong>请查看处理</strong><p>{message.text}</p></div><button className="icon-button" aria-label="关闭错误提示" onClick={() => dismiss(message.id)}><X size={16} /></button></div>
    : <div className="notice notice-success" role="status"><Check size={18} /><p>{message.text}</p><button className="icon-button" aria-label="关闭操作提示" onClick={() => dismiss(message.id)}><X size={16} /></button></div>;
}
