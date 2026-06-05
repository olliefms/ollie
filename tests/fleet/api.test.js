import { describe, it, expect, beforeEach, vi } from 'vitest';
import { saveToken, clearToken } from '../../static/fleet/utils/auth.js';
import {
  apiFetch, loadMe, getScopes, getIdentity, hasScope, clearMe, API_BASE,
} from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  };
}

beforeEach(() => {
  localStorage.clear();
  clearMe();
  vi.restoreAllMocks();
});

describe('apiFetch', () => {
  it('attaches the bearer token and JSON content-type', async () => {
    saveToken('tok123');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ ok: true }));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/loads`);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [, opts] = fetchMock.mock.calls[0];
    expect(opts.headers.Authorization).toBe('Bearer tok123');
    expect(opts.headers['Content-Type']).toBe('application/json');
  });

  it('does not set JSON content-type for FormData bodies', async () => {
    saveToken('tok123');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/blobs`, { method: 'POST', body: new FormData() });

    const [, opts] = fetchMock.mock.calls[0];
    expect(opts.headers['Content-Type']).toBeUndefined();
  });
});

describe('scope store', () => {
  it('loadMe populates scopes + identity', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({
      fleet_user_id: 'u1', name: 'Jane', email: 'j@x.com', role: 'owner',
      effective_scopes: ['*'],
    })));

    const me = await loadMe();
    expect(me.email).toBe('j@x.com');
    expect(getScopes()).toEqual(['*']);
    expect(getIdentity().name).toBe('Jane');
    expect(hasScope('loads:delete')).toBe(true); // '*' grants everything
  });

  it('hasScope is false before loadMe (fail-safe)', () => {
    expect(getScopes()).toEqual([]);
    expect(hasScope('loads:write')).toBe(false);
  });

  it('loadMe failure yields empty scopes (controls stay hidden)', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({}, 500)));
    const me = await loadMe();
    expect(me).toBe(null);
    expect(getScopes()).toEqual([]);
    expect(hasScope('loads:write')).toBe(false);
  });

  it('clearMe resets the store', async () => {
    saveToken('tok');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({
      role: 'dispatcher', effective_scopes: ['loads:read'],
    })));
    await loadMe();
    expect(getScopes()).toEqual(['loads:read']);
    clearMe();
    expect(getScopes()).toEqual([]);
    expect(getIdentity()).toBe(null);
  });
});
