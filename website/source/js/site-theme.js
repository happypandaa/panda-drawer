(() => {
  const key = 'panda-site-theme';
  const root = document.documentElement;
  const dark = document.getElementById('site-dark-style');
  const light = document.getElementById('site-light-style');

  function apply(theme) {
    root.dataset.theme = theme;
    dark.media = theme === 'dark' ? 'all' : 'not all';
    light.media = theme === 'light' ? 'all' : 'not all';
  }

  let initialTheme = 'dark';
  try {
    if (localStorage.getItem(key) === 'light') initialTheme = 'light';
  } catch {
    // Storage may be unavailable in private browsing contexts.
  }
  apply(initialTheme);

  document.addEventListener('DOMContentLoaded', () => {
    const toggle = document.getElementById('site-theme-toggle');
    toggle.setAttribute('aria-pressed', String(root.dataset.theme === 'dark'));
    toggle.hidden = false;

    toggle.addEventListener('click', () => {
      const theme = root.dataset.theme === 'dark' ? 'light' : 'dark';
      apply(theme);
      toggle.setAttribute('aria-pressed', String(theme === 'dark'));
      try {
        localStorage.setItem(key, theme);
      } catch {
        // The current page can still switch themes without storage.
      }
    });
  });
})();
