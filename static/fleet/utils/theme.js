const KEY = 'fleet.theme';
const VALID = ['light', 'dark', 'system'];

export function getTheme() {
  const v = localStorage.getItem(KEY);
  return VALID.includes(v) ? v : 'system';
}

function prefersDark() {
  return typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function resolveTheme(choice) {
  if (choice === 'dark') return 'dark';
  if (choice === 'light') return 'light';
  return prefersDark() ? 'dark' : 'light';
}

export function applyTheme(choice = getTheme()) {
  document.documentElement.dataset.theme = resolveTheme(choice);
}

export function setTheme(choice) {
  if (!VALID.includes(choice)) return;
  localStorage.setItem(KEY, choice);
  applyTheme(choice);
}

export function initTheme() {
  applyTheme();
  if (typeof window.matchMedia === 'function') {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    mql.addEventListener('change', () => {
      if (getTheme() === 'system') applyTheme('system');
    });
  }
}
