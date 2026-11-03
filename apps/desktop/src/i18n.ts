import { invoke, isTauri } from '@tauri-apps/api/core';
import { useSyncExternalStore } from 'react';
import common from './locales/en-common';
import apps from './locales/en-apps';
import management from './locales/en-management';
import service from './locales/en-service';
import prompts from './locales/en-prompts';
import aiGuidance from './locales/en-ai-guidance';
import generatedPrompts from './locales/en-generated-prompts';

// Chinese source text is the stable fallback. Add a catalog and registry entry
// here to support another language; language selection never follows the OS.
const catalogs = {
  'zh-CN': {},
  en: { ...common, ...apps, ...management, ...service, ...prompts, ...aiGuidance, ...generatedPrompts },
} satisfies Record<string, Record<string, string>>;
export type Locale = keyof typeof catalogs;
export const supportedLocales: ReadonlyArray<{ id: Locale; label: string }> = [
  { id: 'zh-CN', label: '中文' },
  { id: 'en', label: 'English' },
];
export const LOCALE_KEY = 'panda-drawer:language:v1';
type Parameters = Record<string, string | number | bigint | boolean | null | undefined>;
const listeners = new Set<() => void>();
const validLocale = (value: unknown): value is Locale => typeof value === 'string' && Object.hasOwn(catalogs, value);
let locale: Locale = 'zh-CN';
let localeError: string | null = null;
try { const saved = window.localStorage.getItem(LOCALE_KEY); if (validLocale(saved)) locale = saved; } catch { /* Use Chinese when storage is unavailable. */ }
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
let snapshot: { locale: Locale; localeError: string | null } = { locale, localeError };
function notify() { snapshot = { locale, localeError }; listeners.forEach(listener => listener()); }
const getSnapshot = () => snapshot;
export const getLocale = () => locale;

function applyDocumentLanguage() {
  document.documentElement.lang = locale;
  document.title = t('熊猫小抽屉 · 开发版');
}

/** Read the same explicit preference that native startup dialogs use. */
export async function initializeLocale(): Promise<void> {
  if (isTauri()) {
    try {
      const saved = await invoke<string>('get_ui_locale');
      if (validLocale(saved)) locale = saved;
    } catch { localeError = '无法读取已保存的语言，当前选择仍可在本次使用中生效。'; }
  }
  applyDocumentLanguage();
  notify();
}

// Serialize native writes so rapid language changes are persisted in order.
let saving = Promise.resolve();
export function setLocale(value: Locale): void {
  if (!validLocale(value)) return;
  locale = value;
  localeError = null;
  try { window.localStorage.setItem(LOCALE_KEY, value); }
  catch { if (!isTauri()) localeError = '无法保存语言选择，重新打开后可能需要再次选择。'; }
  applyDocumentLanguage();
  notify();
  if (isTauri()) {
    saving = saving.then(async () => {
      try {
        await invoke('set_ui_locale', { locale: value });
        if (locale === value) { localeError = null; notify(); }
      } catch {
        if (locale === value) { localeError = '界面语言已切换，但原生窗口或语言保存失败，请重新选择后重试。'; notify(); }
      }
    });
  }
}

export function useLocale() {
  return { ...useSyncExternalStore(subscribe, getSnapshot, getSnapshot), setLocale };
}

const placeholder = /\{([A-Za-z][A-Za-z0-9_]*)\}/g;
const english: Record<string, string> = catalogs.en;
const reverse = new Map(Object.entries(english).filter(([, value]) => !value.match(placeholder)).map(([key, value]) => [value, key]));
const escaped = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
// Older backend records contain Chinese prose. Match complete known templates;
// unknown diagnostics and inserted values stay verbatim.
const patterns = Object.keys(english).flatMap(source => {
  const names = [...source.matchAll(placeholder)].map(match => match[1]);
  if (!names.length) return [];
  const parts = source.split(placeholder).filter((_, index) => index % 2 === 0);
  return [{ source, names, expression: new RegExp('^' + parts.map(escaped).join('([\\s\\S]*?)') + '$') }];
});
// Retain message identity when an asynchronous notice was created before a
// language switch. Bounded and memory-only; no user text is written to disk.
const messages = new Map<string, { source: string; parameters: Parameters }>();
function remember(text: string, source: string, parameters: Parameters) {
  if (text === source) return;
  if (messages.size >= 1000) messages.delete(messages.keys().next().value!);
  messages.set(text, { source, parameters });
}
function interpolate(source: string, parameters: Parameters): string {
  return source.replace(placeholder, (match, name: string) => Object.hasOwn(parameters, name) ? String(parameters[name] ?? '') : match);
}

export function t(source: string, parameters?: Parameters): string {
  if (!parameters && !Object.hasOwn(english, source)) {
    const previous = messages.get(source);
    if (previous) return t(previous.source, previous.parameters);
    const original = reverse.get(source);
    if (original) return t(original);
    const coded = source.match(/^([\s\S]+) \(([A-Z][A-Z0-9_]+)\)$/);
    if (coded) return `${t(coded[1])} (${coded[2]})`;
    if (/[\p{Script=Han}]/u.test(source)) {
      for (const pattern of patterns) {
        const matched = pattern.expression.exec(source);
        if (matched) return t(pattern.source, Object.fromEntries(pattern.names.map((name, index) => [name, matched[index + 1]])));
      }
    }
  }
  const catalog: Record<string, string> = catalogs[locale];
  const template = catalog[source] ?? source;
  if (!parameters) return template;
  const output = interpolate(template, parameters);
  remember(output, source, parameters);
  remember(interpolate(source, parameters), source, parameters);
  return output;
}

export function formatDateTime(value: Date | string | number, options: Intl.DateTimeFormatOptions = { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }): string {
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : new Intl.DateTimeFormat(locale, options).format(date);
}
export function formatNumber(value: number | bigint, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(locale, options).format(value);
}
