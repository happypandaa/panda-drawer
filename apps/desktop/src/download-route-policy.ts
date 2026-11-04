export type DownloadRoute = 'cn' | 'global';
export type DownloadMode = 'auto' | DownloadRoute;
export interface DownloadRouteState {
  mode: DownloadMode;
  detectedRoute: DownloadRoute | null;
  detectedAt: number | null;
  detecting: boolean;
  error: string;
}
interface Dependencies {
  storage?: Pick<Storage, 'getItem' | 'setItem'>;
  probe: (route: DownloadRoute) => Promise<void>;
  systemRoute: () => DownloadRoute;
  now?: () => number;
}
export const DOWNLOAD_ROUTE_KEY = 'panda-drawer:download-route:v1';
const week = 7 * 24 * 60 * 60 * 1000;
export const otherDownloadRoute = (route: DownloadRoute): DownloadRoute => route === 'cn' ? 'global' : 'cn';
const isRoute = (value: unknown): value is DownloadRoute => value === 'cn' || value === 'global';

export function guessDownloadRoute(language: string, timeZone: string): DownloadRoute {
  const simplifiedChinese = /^zh-(?:CN|SG|Hans)(?:-|$)/i.test(language);
  const mainland = ['Asia/Shanghai', 'Asia/Chongqing', 'Asia/Chungking', 'Asia/Harbin', 'Asia/Urumqi', 'China Standard Time'].includes(timeZone);
  return simplifiedChinese && mainland ? 'cn' : 'global';
}

export class RouteDownloadError extends Error {
  constructor(public route: DownloadRoute, public mode: DownloadMode, cause: unknown) {
    super('Download failed', { cause });
  }
}

export class DownloadRouteController {
  private state: DownloadRouteState = { mode: 'auto', detectedRoute: null, detectedAt: null, detecting: false, error: '' };
  private listeners = new Set<() => void>();
  private flight: Promise<DownloadRoute> | null = null;
  private revision = 0;
  private lastAttempt = -Infinity;
  private now: () => number;

  constructor(private dependencies: Dependencies) {
    this.now = dependencies.now ?? Date.now;
    try {
      const saved = JSON.parse(dependencies.storage?.getItem(DOWNLOAD_ROUTE_KEY) ?? 'null');
      if (saved && ['auto', 'cn', 'global'].includes(saved.mode)) {
        this.state.mode = saved.mode;
        if (isRoute(saved.detectedRoute) && typeof saved.detectedAt === 'number' && Number.isFinite(saved.detectedAt) && saved.detectedAt >= 0) {
          this.state.detectedRoute = saved.detectedRoute;
          this.state.detectedAt = saved.detectedAt;
        }
      }
    } catch { /* An unreadable preference uses automatic mode. */ }
  }

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private update(value: Partial<DownloadRouteState>, persist = false) {
    this.state = { ...this.state, ...value };
    if (persist) {
      try {
        this.dependencies.storage?.setItem(DOWNLOAD_ROUTE_KEY, JSON.stringify({
          mode: this.state.mode, detectedRoute: this.state.detectedRoute, detectedAt: this.state.detectedAt,
        }));
      } catch { this.state = { ...this.state, error: '下载线路已生效，但未能保存，请重新选择后重试。' }; }
    }
    this.listeners.forEach(listener => listener());
  }

  setMode(mode: DownloadMode) {
    if (!['auto', 'cn', 'global'].includes(mode)) throw new Error('Invalid download mode');
    this.revision++;
    this.flight = null;
    this.lastAttempt = -Infinity;
    this.update({ mode, detecting: false, error: '' }, true);
  }

  currentRoute(): DownloadRoute {
    return this.state.mode === 'auto' ? this.state.detectedRoute ?? this.dependencies.systemRoute() : this.state.mode;
  }

  async getRoute(): Promise<DownloadRoute> {
    if (this.state.mode !== 'auto') return this.state.mode;
    if (!this.state.detectedRoute) {
      if (this.flight || this.now() - this.lastAttempt >= 30_000) return this.detect();
      return this.currentRoute();
    }
    if (this.state.detectedAt === null || this.now() - this.state.detectedAt >= week || this.state.detectedAt > this.now()) {
      if (!this.flight && this.now() - this.lastAttempt >= 30_000) void this.detect();
    }
    return this.currentRoute();
  }

  detect(): Promise<DownloadRoute> {
    if (this.state.mode !== 'auto') return Promise.resolve(this.state.mode);
    if (this.flight) return this.flight;
    const revision = this.revision;
    this.lastAttempt = this.now();
    this.update({ detecting: true, error: '' });
    const flight = Promise.any((['cn', 'global'] as const).map(async route => {
      await this.dependencies.probe(route);
      return route;
    })).then(route => {
      if (revision === this.revision && this.state.mode === 'auto') this.recordSuccess(route);
      return this.currentRoute();
    }).catch(() => this.currentRoute()).finally(() => {
      if (this.flight === flight) {
        this.flight = null;
        this.update({ detecting: false });
      }
    });
    this.flight = flight;
    return flight;
  }

  recordSuccess(route: DownloadRoute) {
    if (this.state.mode === 'auto') this.update({ detectedRoute: route, detectedAt: this.now() }, true);
  }

  async run<T>(work: (route: DownloadRoute) => Promise<T>): Promise<T> {
    const route = await this.getRoute();
    const mode = this.state.mode;
    const revision = this.revision;
    try { return await work(route); }
    catch (cause) {
      if (mode !== 'auto' || this.state.mode !== 'auto' || revision !== this.revision) {
        throw new RouteDownloadError(route, mode, cause);
      }
      const fallback = otherDownloadRoute(route);
      try {
        const result = await work(fallback);
        if (revision === this.revision) this.recordSuccess(fallback);
        return result;
      } catch (fallbackCause) { throw new RouteDownloadError(route, mode, fallbackCause); }
    }
  }
}
