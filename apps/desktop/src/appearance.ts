import { useEffect, useState } from 'react';

export type Appearance = 'light' | 'dark';
export const APPEARANCE_KEY = 'panda-drawer:appearance:v1';

export function useAppearance() {
  const [theme, setTheme] = useState<Appearance>(() => {
    try {
      return window.localStorage.getItem(APPEARANCE_KEY) === 'dark' ? 'dark' : 'light';
    } catch {
      return 'light';
    }
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try { window.localStorage.setItem(APPEARANCE_KEY, theme); }
    catch { /* Appearance still works when storage is unavailable. */ }
  }, [theme]);

  return { theme, setTheme };
}
