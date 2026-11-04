import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import { gitRepositoryAddress } from './git-repository';

export interface CatalogRepositories { github?: string; gitee?: string }

export interface CatalogApp {
  id: string;
  name: string;
  summary: string;
  repo: CatalogRepositories;
  branch?: string;
  needs: string[];
  note?: string;
}
const CACHE_KEY = 'panda-drawer:app-catalog:v1';
const nonempty = (value: unknown): value is string => typeof value === 'string' && value.trim().length > 0;

function repositories(value: unknown): CatalogRepositories {
  // Keep previously cached single-address catalogs readable.
  if (typeof value === 'string') {
    const repository = gitRepositoryAddress(value);
    if (!repository) throw new Error('Invalid catalog repository');
    return repository.provider === 'GitHub' ? { github: value } : { gitee: value };
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid catalog repositories');
  const result: CatalogRepositories = {};
  for (const provider of ['github', 'gitee'] as const) {
    const url = (value as Record<string, unknown>)[provider];
    if (url === undefined) continue;
    if (!nonempty(url) || gitRepositoryAddress(url)?.provider !== (provider === 'github' ? 'GitHub' : 'Gitee')) {
      throw new Error('Invalid catalog repository provider');
    }
    result[provider] = url;
  }
  if (!result.github && !result.gitee) throw new Error('Catalog repositories are empty');
  return result;
}

export function catalogSourceUrl(app: CatalogApp, preferred: keyof CatalogRepositories = 'github'): string {
  return app.repo[preferred] ?? app.repo.github ?? app.repo.gitee!;
}

export function availableCatalogApps(apps: CatalogApp[], existingUrls: string[], existingIds: string[] = []): CatalogApp[] {
  const existing = new Set(existingUrls.map(catalogRepositoryKey).filter(Boolean));
  return apps.filter(app => !existingIds.includes(app.id)
    && !Object.values(app.repo).some(url => existing.has(catalogRepositoryKey(url))));
}

export function parseAppCatalog(value: unknown): CatalogApp[] {
  if (!value || typeof value !== 'object' || !('version' in value) || value.version !== 1
    || !('apps' in value) || !Array.isArray(value.apps)) throw new Error('Invalid app catalog');
  const ids = new Set<string>();
  return value.apps.map((item: unknown) => {
    if (!item || typeof item !== 'object') throw new Error('Invalid catalog entry');
    const entry = item as Record<string, unknown>;
    if (!nonempty(entry.id) || ids.has(entry.id) || !nonempty(entry.name) || !nonempty(entry.summary)
      || entry.branch !== undefined && !nonempty(entry.branch)
      || !Array.isArray(entry.needs) || entry.needs.some(need => !nonempty(need))
      || entry.note !== undefined && typeof entry.note !== 'string') throw new Error('Invalid catalog entry');
    ids.add(entry.id);
    return { id: entry.id, name: entry.name, summary: entry.summary, repo: repositories(entry.repo),
      branch: entry.branch as string | undefined, needs: entry.needs as string[], note: entry.note as string | undefined };
  });
}

export function catalogRepositoryKey(url: string): string | undefined {
  return gitRepositoryAddress(url)?.url.toLowerCase();
}

function cachedCatalog(): CatalogApp[] | null {
  try { return parseAppCatalog(JSON.parse(localStorage.getItem(CACHE_KEY) ?? 'null')); }
  catch { return null; }
}
let reading: Promise<CatalogApp[] | null> | null = null;
async function readCatalog(): Promise<CatalogApp[] | null> {
  if (reading) return reading;
  reading = (async () => {
    try {
      const value = await invoke<unknown>('app_catalog');
      if (value === null) return cachedCatalog();
      const apps = parseAppCatalog(value);
      try { localStorage.setItem(CACHE_KEY, JSON.stringify({ version: 1, apps })); } catch { /* Recommendations still work this time. */ }
      return apps;
    } catch { return cachedCatalog(); }
  })().finally(() => { reading = null; });
  return reading;
}

export function useAppCatalog(active: boolean): CatalogApp[] | null {
  const [apps, setApps] = useState<CatalogApp[] | null>(null);
  useEffect(() => {
    if (!active) return;
    let live = true;
    setApps(cachedCatalog());
    void readCatalog().then(value => { if (live) setApps(value); });
    return () => { live = false; };
  }, [active]);
  return apps;
}
