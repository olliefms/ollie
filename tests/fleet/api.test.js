import { describe, it, expect, beforeEach, vi } from 'vitest';
import { saveToken, clearToken } from '../../static/fleet/utils/auth.js';
import {
  apiFetch, loadMe, getScopes, getIdentity, hasScope, clearMe, API_BASE, TIMEOUT_MESSAGE,
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

  it('on 401, refreshes the token and retries the request', async () => {
    saveToken('stale');
    // 1) original → 401, 2) /refresh → 200 {token}, 3) retry → 200
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(jsonResponse({}, 401))
      .mockResolvedValueOnce(jsonResponse({ token: 'fresh' }, 200))
      .mockResolvedValueOnce(jsonResponse({ ok: true }, 200));
    vi.stubGlobal('fetch', fetchMock);

    const res = await apiFetch(`${API_BASE}/loads`);

    expect(res.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    // retry carried the refreshed bearer token
    const [, retryOpts] = fetchMock.mock.calls[2];
    expect(retryOpts.headers.Authorization).toBe('Bearer fresh');
  });

  it('on 401 with a failed refresh, clears the token and throws Unauthorized', async () => {
    saveToken('stale');
    // 1) original → 401, 2) /refresh → 401 (refresh fails)
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(jsonResponse({}, 401))
      .mockResolvedValueOnce(jsonResponse({}, 401));
    vi.stubGlobal('fetch', fetchMock);

    await expect(apiFetch(`${API_BASE}/loads`)).rejects.toThrow('Unauthorized — please sign in again.');
    expect(localStorage.getItem('dispatch_token')).toBe(null); // token cleared
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

// A request must never hang forever: the cancel-button freeze investigation showed
// a pending action with no deadline is indistinguishable from a dead page.
describe('apiFetch request deadline', () => {
  it('aborts after the timeout and reports a readable error', async () => {
    saveToken('tok');
    // A fetch that only ever settles when its signal aborts.
    const fetchMock = vi.fn((_path, opts) => new Promise((_resolve, reject) => {
      opts.signal.addEventListener('abort', () => reject(new Error('aborted')));
    }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(apiFetch(`${API_BASE}/loads/x/cancel`, { method: 'POST', timeoutMs: 5 }))
      .rejects.toThrow(TIMEOUT_MESSAGE);
    expect(fetchMock.mock.calls[0][1].signal.aborted).toBe(true);
  });

  it('passes an abort signal by default', async () => {
    saveToken('tok');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/loads`);
    expect(fetchMock.mock.calls[0][1].signal).toBeInstanceOf(AbortSignal);
  });

  it('opts out when timeoutMs is 0, for uploads and downloads', async () => {
    saveToken('tok');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/blobs`, { method: 'POST', body: new FormData(), timeoutMs: 0 });
    expect(fetchMock.mock.calls[0][1].signal).toBeUndefined();
  });

  it('does not leak `timeoutMs` into the fetch init', async () => {
    saveToken('tok');
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);

    await apiFetch(`${API_BASE}/loads`, { method: 'POST', timeoutMs: 1000 });
    expect(fetchMock.mock.calls[0][1].timeoutMs).toBeUndefined();
  });
});
