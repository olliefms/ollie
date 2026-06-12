import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  getTheme, setTheme, resolveTheme, applyTheme, initTheme,
} from '../../static/fleet/utils/theme.js';

function stubMatchMedia(matches) {
  const listeners = [];
  const mql = {
    matches,
    addEventListener: (_e, fn) => listeners.push(fn),
    _fire: (next) => { mql.matches = next; listeners.forEach(fn => fn()); },
  };
  window.matchMedia = vi.fn().mockReturnValue(mql);
  return mql;
}

beforeEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
});

describe('theme', () => {
  it('defaults to system when unset or invalid', () => {
    expect(getTheme()).toBe('system');
    localStorage.setItem('fleet.theme', 'bogus');
    expect(getTheme()).toBe('system');
  });

  it('persists and reads back a valid choice', () => {
    stubMatchMedia(false);
    setTheme('dark');
    expect(getTheme()).toBe('dark');
    expect(localStorage.getItem('fleet.theme')).toBe('dark');
  });

  it('ignores invalid setTheme values', () => {
    setTheme('neon');
    expect(localStorage.getItem('fleet.theme')).toBe(null);
  });

  it('resolves system via prefers-color-scheme', () => {
    stubMatchMedia(true);
    expect(resolveTheme('system')).toBe('dark');
    stubMatchMedia(false);
    expect(resolveTheme('system')).toBe('light');
    expect(resolveTheme('dark')).toBe('dark');
  });

  it('applyTheme writes the resolved value to data-theme', () => {
    stubMatchMedia(true);
    applyTheme('system');
    expect(document.documentElement.dataset.theme).toBe('dark');
    applyTheme('light');
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  it('initTheme re-applies when system preference changes', () => {
    const mql = stubMatchMedia(false);
    setTheme('system');
    initTheme();
    expect(document.documentElement.dataset.theme).toBe('light');
    mql._fire(true);
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
});
