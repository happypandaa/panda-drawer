import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Check, X } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { downloadRoutes, useDownloadRoutes } from './download-routes';
import { formatDateTime, t } from './i18n';
import { Modal } from './Modal';
import { percentOf } from './storage-utils';
import { messageFrom } from './ui-utils';

type UpdatePhase = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'installing' | 'failed';
type UpdateAction = 'check' | 'download' | 'skip' | 'install';

export interface UpdateSnapshot {
  enabled: boolean;
  phase: UpdatePhase;
  currentVersion: string;
  version?: string | null;
  notes?: string | null;
  downloadedBytes?: number | null;
  totalBytes?: number | null;
  lastCheckedAt?: number | null;
  error?: string | null;
  installedVersion?: string | null;
  installedNotes?: string | null;
  readyVersion?: string | null;
  readyNotes?: string | null;
}

interface InstallPlan { items: string[]; blocked: boolean }
interface InstallReview extends InstallPlan { version: string }

export function usePlatformUpdates(enabled: boolean) {
  const [snapshot, setSnapshot] = useState<UpdateSnapshot | null>(null);
  const [pending, setPending] = useState<UpdateAction | null>(null);
  const [error, setError] = useState('');
  const [review, setReview] = useState<InstallReview | null>(null);
  const active = useRef<UpdateAction | null>(null);
  const { mode, detectedRoute } = useDownloadRoutes();
  const route = mode === 'auto' ? detectedRoute ?? downloadRoutes.currentRoute() : mode;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let received = false;
    let stop: (() => void) | undefined;
    void (async () => {
      try {
        stop = await listen<UpdateSnapshot>('platform-update-state', event => {
          received = true;
          if (!disposed) setSnapshot(event.payload);
        });
      } catch { /* The initial snapshot remains available. */ }
      if (disposed) { stop?.(); return; }
      try {
        const state = await invoke<UpdateSnapshot>('platform_update_state');
        if (!disposed && !received) setSnapshot(state);
      } catch (cause) {
        if (!disposed && !received) setError(messageFrom(cause));
      }
    })();
    return () => { disposed = true; stop?.(); };
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    void invoke('platform_update_set_route', { route, mode: mode === 'auto' ? 'automatic' : mode }).catch(() => {
      // The native updater can still check using its own route selection.
    });
  }, [enabled, mode, route]);

  const blockedVersion = review?.blocked ? review.version : null;
  useEffect(() => {
    if (!blockedVersion || pending === 'install') return;
    let disposed = false;
    let timeout: number;
    const poll = async () => {
      try {
        const plan = await invoke<InstallPlan>('platform_update_install_plan', { version: blockedVersion });
        if (disposed) return;
        setReview(current => current?.version === blockedVersion ? { ...plan, version: blockedVersion } : current);
        if (!plan.blocked) return;
      } catch { /* Keep installation blocked until the tasks can be checked again. */ }
      if (!disposed) timeout = window.setTimeout(() => void poll(), 1000);
    };
    timeout = window.setTimeout(() => void poll(), 1000);
    return () => { disposed = true; window.clearTimeout(timeout); };
  }, [blockedVersion, pending]);

  async function run(action: UpdateAction, work: () => Promise<unknown>) {
    if (!enabled || active.current) return false;
    active.current = action;
    setPending(action);
    setError('');
    try { await work(); return true; }
    catch (cause) { setError(messageFrom(cause)); return false; }
    finally { active.current = null; setPending(null); }
  }

  async function requestInstall(version: string) {
    await run('install', async () => {
      const plan = await invoke<InstallPlan>('platform_update_install_plan', { version });
      if (plan.blocked || plan.items.length) setReview({ ...plan, version });
      else await invoke('platform_update_install', { version, confirmed: false });
    });
  }

  return {
    enabled,
    snapshot,
    pending,
    error,
    review,
    check: () => run('check', () => invoke('platform_update_check')),
    download: (version: string) => run('download', () => invoke('platform_update_download', { version })),
    skip: (version: string) => run('skip', () => invoke('platform_update_skip', { version })),
    requestInstall,
    confirmInstall: async (version: string) => {
      if (await run('install', () => invoke('platform_update_install', { version, confirmed: true }))) setReview(null);
    },
    closeReview: () => { setReview(null); setError(''); },
  };
}

export type PlatformUpdates = ReturnType<typeof usePlatformUpdates>;

function downloadProgress(snapshot: UpdateSnapshot) {
  const progress = percentOf(snapshot.downloadedBytes, snapshot.totalBytes);
  const version = snapshot.version ?? '';
  const text = progress === null
    ? t("正在下载新版本 {version}", { version })
    : t("正在下载新版本 {version} · {percent}%", { version, percent: Math.floor(progress) });
  return { progress, text };
}

function readyVersion(snapshot: UpdateSnapshot) {
  return snapshot.readyVersion || (snapshot.phase === 'ready' ? snapshot.version : null);
}

function offerFor(snapshot: UpdateSnapshot | null) {
  if (!snapshot?.enabled || snapshot.phase === 'installing') return null;
  if (snapshot.phase === 'available' && snapshot.version) {
    return { phase: 'available' as const, version: snapshot.version, notes: snapshot.notes };
  }
  const version = readyVersion(snapshot);
  return version ? { phase: 'ready' as const, version, notes: snapshot.readyNotes ?? snapshot.notes } : null;
}

function UpdateNotes({ notes, compact }: { notes?: string | null; compact?: boolean }) {
  const text = notes?.trim();
  if (!text) return null;
  if (!compact) return <p className="platform-update-notes">{text}</p>;
  const summary = text.split('\n').slice(0, 3).join('\n').slice(0, 180).trimEnd();
  if (summary === text) return <p className="platform-update-notes">{text}</p>;
  return <>
    <p className="platform-update-notes">{summary}…</p>
    <details className="platform-update-more"><summary>{t("查看全部")}</summary><p className="platform-update-notes">{text}</p></details>
  </>;
}

export function PlatformUpdateControl({ updates, obstructed }: { updates: PlatformUpdates; obstructed: boolean }) {
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [focused, setFocused] = useState(() => document.hasFocus() && !document.hidden);
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(() => new Set());
  const [manual, setManual] = useState<string | null>(null);
  const titleId = useId();
  const popoverId = useId();
  const offer = offerFor(updates.snapshot);
  const key = offer ? `${offer.phase}:${offer.version}` : null;
  const open = Boolean(key && !obstructed && !updates.review && (manual === key || focused && !dismissed.has(key)));

  useEffect(() => {
    const update = () => setFocused(document.hasFocus() && !document.hidden);
    window.addEventListener('focus', update);
    window.addEventListener('blur', update);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.removeEventListener('focus', update);
      window.removeEventListener('blur', update);
      document.removeEventListener('visibilitychange', update);
    };
  }, []);

  const dismiss = () => {
    if (key) setDismissed(values => new Set(values).add(key));
    setManual(null);
  };

  useEffect(() => {
    if (!open) return;
    const outside = (event: MouseEvent) => {
      if (!container.current?.contains(event.target as Node)) dismiss();
    };
    document.addEventListener('mousedown', outside);
    return () => document.removeEventListener('mousedown', outside);
  }, [key, open]);

  const snapshot = updates.snapshot;
  if (!snapshot?.enabled) return null;
  if (snapshot.phase === 'downloading') {
    const { progress, text } = downloadProgress(snapshot);
    return <span className="platform-update-indicator" role="progressbar" aria-label={text} aria-valuenow={progress ?? undefined} title={text}>
      <svg className={progress === null ? 'is-indeterminate' : undefined} width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
        <circle className="platform-update-ring-track" cx="12" cy="12" r="9" />
        <circle className="platform-update-ring-value" cx="12" cy="12" r="9" pathLength="100" strokeDasharray={`${progress ?? 30} 100`} />
      </svg>
    </span>;
  }
  if (!offer) return null;
  const { version, phase, notes } = offer;
  const ready = phase === 'ready';
  const title = ready ? t("新版本 {version} 已准备好", { version }) : t("有新版本 {version}", { version });
  const close = () => { dismiss(); trigger.current?.focus(); };
  return <div className="platform-update-control" ref={container} onKeyDown={event => {
    if (open && event.key === 'Escape') { event.stopPropagation(); close(); }
  }}>
    <button ref={trigger} type="button" className="platform-update-entry" aria-controls={open ? popoverId : undefined} aria-expanded={open}
      disabled={obstructed || Boolean(updates.review)} title={title} onClick={() => { if (open) dismiss(); else setManual(key); }}>
      <span className="platform-update-dot" aria-hidden="true" />{t("新版本")}
    </button>
    {open && <section className="platform-update-popover" role="region" id={popoverId} aria-labelledby={titleId}>
      <div className="platform-update-popover-title">
        <strong id={titleId}>{title}</strong>
        <button type="button" className="icon-button" aria-label={t("稍后")} onClick={close}><X size={15} aria-hidden="true" /></button>
      </div>
      <UpdateNotes notes={notes} compact />
      {updates.error && <p className="task-error" role="alert">{t(updates.error)}</p>}
      <div className="platform-update-actions">
        <button type="button" className="primary-button" disabled={updates.pending !== null} onClick={() => {
          if (ready) void updates.requestInstall(version);
          else void updates.download(version);
        }}>{t(ready ? "安装并重启" : "下载")}</button>
        <button type="button" className="refresh-button" onClick={close}>{t("稍后")}</button>
        <button type="button" className="text-button" disabled={updates.pending !== null}
          onClick={() => void updates.skip(version).then(success => { if (success) dismiss(); })}>{t("跳过这个版本")}</button>
      </div>
    </section>}
  </div>;
}

function updateStatus(snapshot: UpdateSnapshot) {
  const { version, lastCheckedAt } = snapshot;
  switch (snapshot.phase) {
    case 'checking': return t("正在检查更新…");
    case 'available': return t("有新版本 {version}", { version });
    case 'downloading': return downloadProgress(snapshot).text;
    case 'ready': return t("新版本 {version} 已准备好", { version: readyVersion(snapshot) });
    case 'installing': return t("正在安装更新…");
    case 'failed': return snapshot.error ? t(snapshot.error) : t("检查更新失败");
    case 'idle': return t(lastCheckedAt ? "已是最新版本" : "尚未检查更新");
  }
}

export function PlatformUpdateSettings({ updates }: { updates: PlatformUpdates }) {
  const { snapshot, pending, enabled, error } = updates;
  if (!enabled) return <div className="platform-update"><strong>{t("更新状态")}</strong><p>{t("开发版不检查更新")}</p></div>;
  const active = snapshot?.enabled && snapshot.phase !== 'installing';
  const available = snapshot?.phase === 'available' ? snapshot.version : null;
  const ready = snapshot ? readyVersion(snapshot) : null;
  const progress = snapshot?.phase === 'downloading' ? downloadProgress(snapshot) : null;
  const notes = snapshot?.notes ?? snapshot?.readyNotes ?? snapshot?.installedNotes;
  return <div className="platform-update">
    <strong>{t("更新状态")}</strong>
    <p role="status">{pending === 'check' ? t("正在检查更新…") : snapshot ? updateStatus(snapshot) : t("尚未检查更新")}</p>
    {progress && <progress className="platform-update-progress" aria-label={progress.text} max={100} value={progress.progress ?? undefined} />}
    {snapshot?.lastCheckedAt && <p>{t("上次检查：{time}", { time: formatDateTime(snapshot.lastCheckedAt) })}</p>}
    <div className="platform-update-actions">
      <button type="button" className="refresh-button" disabled={!active || pending !== null || snapshot?.phase === 'checking' || snapshot?.phase === 'downloading'}
        onClick={() => void updates.check()}>{t(pending === 'check' || snapshot?.phase === 'checking' ? "正在检查更新…" : "检查更新")}</button>
      {active && available && <button type="button" className="primary-button" disabled={pending !== null} onClick={() => void updates.download(available)}>{t("下载")}</button>}
      {active && ready && <button type="button" className="primary-button" disabled={pending !== null} onClick={() => void updates.requestInstall(ready)}>{t("安装并重启")}</button>}
    </div>
    {error && <p className="task-error" role="alert">{t(error)}</p>}
    {notes && <><h4>{t("更新内容")}</h4><UpdateNotes notes={notes} /></>}
  </div>;
}

export function PlatformInstallConfirmation({ updates }: { updates: PlatformUpdates }) {
  const { review, pending, error } = updates;
  if (!review) return null;
  return <Modal title={t("安装前需要关闭这些应用")} className="platform-update-confirmation" closeDisabled={pending === 'install'} onClose={updates.closeReview}>
    {review.items.length > 0 && <ul>{review.items.map((item, index) => <li key={`${index}:${item}`}>{t(item)}</li>)}</ul>}
    {review.blocked && <p role="status">{t("等这些任务完成后再安装")}</p>}
    {error && <p role="alert" className="task-error">{t(error)}</p>}
    <div className="confirmation-actions modal-footer">
      <button type="button" className="primary-button" disabled={review.blocked || pending !== null} onClick={() => void updates.confirmInstall(review.version)}>{t("关闭它们并安装")}</button>
      <button type="button" className="refresh-button" disabled={pending === 'install'} onClick={updates.closeReview}>{t("取消")}</button>
    </div>
  </Modal>;
}

export function PlatformUpdatedNotice({ version, onNotes }: { version?: string | null; onNotes: () => void }) {
  const [hidden, setHidden] = useState<string | null>(null);
  useEffect(() => {
    if (!version) return;
    const timer = window.setTimeout(() => setHidden(version), 5000);
    return () => window.clearTimeout(timer);
  }, [version]);
  if (!version || version === hidden) return null;
  return createPortal(<div className="feedback-toasts"><div className="notice notice-success" role="status">
    <Check size={16} aria-hidden="true" />
    <p>{t("已更新到 {version}", { version })}</p>
    <button type="button" className="platform-update-toast-action" onClick={() => { setHidden(version); onNotes(); }}>{t("查看更新内容")}</button>
    <button type="button" className="icon-button" aria-label={t("关闭操作提示")} onClick={() => setHidden(version)}><X size={14} aria-hidden="true" /></button>
  </div></div>, document.body);
}
