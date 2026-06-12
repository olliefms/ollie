import { describe, it, expect, beforeEach, vi } from 'vitest';
import { initials, roleLabel, renderAccountFooter } from '../../static/fleet/components/account-footer.js';

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = vi.fn().mockReturnValue({ matches: false, addEventListener() {} });
});

describe('initials', () => {
  it('takes first letters of the first two name words', () => {
    expect(initials('Jim Phillips', 'x@y.com')).toBe('JP');
  });
  it('handles a single-word name', () => {
    expect(initials('Jim', 'x@y.com')).toBe('J');
  });
  it('falls back to the email initial when name is blank', () => {
    expect(initials('', 'dispatch@acme.com')).toBe('D');
  });
  it('returns ? when nothing is available', () => {
    expect(initials('', '')).toBe('?');
  });
});

describe('roleLabel', () => {
  it('title-cases known roles', () => {
    expect(roleLabel('owner')).toBe('Owner');
    expect(roleLabel('fleet_manager')).toBe('Fleet Manager');
    expect(roleLabel('dispatcher')).toBe('Dispatcher');
  });
  it('passes through an unknown role', () => {
    expect(roleLabel('auditor')).toBe('auditor');
  });
});

describe('renderAccountFooter', () => {
  const identity = { name: 'Jim Phillips', email: 'jim@acme.com', role: 'owner' };
  let host;
  beforeEach(() => { host = document.createElement('div'); document.body.appendChild(host); });

  it('renders the user chip with initials, name and role', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    expect(host.querySelector('.sidebar__avatar').textContent).toBe('JP');
    expect(host.textContent).toContain('Jim Phillips');
    expect(host.textContent).toContain('Owner');
  });

  it('menu starts closed and toggles open on chip click', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const menu = host.querySelector('.sidebar__menu');
    expect(menu.hidden).toBe(true);
    host.querySelector('.sidebar__account').click();
    expect(menu.hidden).toBe(false);
  });

  it('closes on Escape and on outside click', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const menu = host.querySelector('.sidebar__menu');
    host.querySelector('.sidebar__account').click();
    expect(menu.hidden).toBe(false);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    expect(menu.hidden).toBe(true);
    host.querySelector('.sidebar__account').click();
    document.body.click();
    expect(menu.hidden).toBe(true);
  });

  it('shows the Account link only with api_keys:read', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    expect(host.querySelector('a[href="/fleet/account"]')).not.toBe(null);

    const host2 = document.createElement('div');
    renderAccountFooter(host2, { identity, scopes: ['loads:read'], onSignOut() {} });
    expect(host2.querySelector('a[href="/fleet/account"]')).toBe(null);
  });

  it('invokes onSignOut when Sign out is clicked', () => {
    const onSignOut = vi.fn();
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut });
    host.querySelector('.sidebar__account').click();
    host.querySelector('[data-action="sign-out"]').click();
    expect(onSignOut).toHaveBeenCalledTimes(1);
  });

  it('theme buttons mark the active choice', () => {
    renderAccountFooter(host, { identity, scopes: ['*'], onSignOut() {} });
    const dark = host.querySelector('[data-theme-choice="dark"]');
    dark.click();
    expect(dark.classList.contains('is-active')).toBe(true);
    expect(localStorage.getItem('fleet.theme')).toBe('dark');
  });
});
