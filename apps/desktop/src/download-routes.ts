import { invoke } from '@tauri-apps/api/core';
import { useSyncExternalStore } from 'react';
import { parseAppCatalog } from './app-catalog-model';
import { DownloadRouteController, guessDownloadRoute } from './download-route-policy';
import type { DownloadRoute } from './download-route-policy';
export * from './download-route-policy';

export async function fetchRouteCatalog(route: DownloadRoute) {
  return parseAppCatalog(await invoke<unknown>('app_catalog', { route }));
}
let storage: Storage | undefined;
try { storage = window.localStorage; } catch { /* Preferences still work for this session. */ }
export const downloadRoutes = new DownloadRouteController({
  storage,
  probe: async route => { await fetchRouteCatalog(route); },
  systemRoute: () => {
    try {
      return guessDownloadRoute(navigator.languages?.[0] ?? navigator.language, Intl.DateTimeFormat().resolvedOptions().timeZone);
    } catch { return 'global'; }
  },
});
export const useDownloadRoutes = () => useSyncExternalStore(downloadRoutes.subscribe, downloadRoutes.getSnapshot, downloadRoutes.getSnapshot);
