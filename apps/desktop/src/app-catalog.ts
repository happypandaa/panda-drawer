import { useEffect, useState } from 'react';
import { downloadRoutes, fetchRouteCatalog, RouteDownloadError, useDownloadRoutes } from './download-routes';
import { parseAppCatalog } from './app-catalog-model';
import type { CatalogApp } from './app-catalog-model';
export * from './app-catalog-model';

const CACHE_KEY = 'panda-drawer:app-catalog:v1';
function cachedCatalog(): CatalogApp[] | null {
  try { return parseAppCatalog(JSON.parse(localStorage.getItem(CACHE_KEY) ?? 'null')); }
  catch { return null; }
}
interface CatalogState { apps: CatalogApp[] | null; error: RouteDownloadError | null }
const readings = new Map<string, Promise<CatalogState>>();
function preferenceKey() {
  const state = downloadRoutes.getSnapshot();
  return state.mode + '/' + (state.detectedRoute ?? '');
}
async function readCatalog(): Promise<CatalogState> {
  const key = preferenceKey();
  const previous = readings.get(key);
  if (previous) return previous;
  const reading = (async () => {
    try {
      const apps = await downloadRoutes.run(fetchRouteCatalog);
      if (key === preferenceKey()) {
        try { localStorage.setItem(CACHE_KEY, JSON.stringify({ version: 1, apps })); }
        catch { /* Recommendations still work for this session. */ }
      }
      return { apps, error: null };
    } catch (cause) {
      return { apps: cachedCatalog(), error: cause instanceof RouteDownloadError ? cause : null };
    }
  })().finally(() => { readings.delete(key); });
  readings.set(key, reading);
  return reading;
}

export function useAppCatalogState(active: boolean): CatalogState {
  const route = useDownloadRoutes();
  const [state, setState] = useState<CatalogState>(() => ({ apps: cachedCatalog(), error: null }));
  useEffect(() => {
    if (!active) return;
    let live = true;
    setState({ apps: cachedCatalog(), error: null });
    void readCatalog().then(value => { if (live) setState(value); });
    return () => { live = false; };
  }, [active, route.mode, route.detectedRoute]);
  return state;
}
export function useAppCatalog(active: boolean): CatalogApp[] | null {
  return useAppCatalogState(active).apps;
}
