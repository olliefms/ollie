import { getToken, saveToken, clearToken } from './auth.js';
import { scopeGranted } from '../components/scope-gate.js';

export const API_BASE = '/fleet/api/v1';
export const AUTH_BASE = '/fleet/auth';

// ─── 401 handler injection ───────────────────────────────────
// 0b-ii wires this to the router's login gate. Default: no-op.
let _onUnauthorized = () => {};
export function setOnUnauthorized(fn) { _onUnauthorized = fn || (() => {}); }

// ─── Token refresh ───────────────────────────────────────────
export async function tryRefresh() {
  try {
    const res = await fetch(`${AUTH_BASE}/refresh`, { method: 'POST', credentials: 'same-origin' });
    if (!res.ok) return false;
    const data = await res.json();
    const token = data.token || data.access_token;
    if (!token) return false;
    saveToken(token);
    return true;
  } catch {
    return false;
  }
}

// ─── API fetch wrapper ───────────────────────────────────────
export async function apiFetch(path, options = {}) {
  const token = getToken();
  const isFormData = options.body instanceof FormData;
  const headers = {
    ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(options.headers || {}),
  };

  const res = await fetch(path, { ...options, headers });

  if (res.status === 401) {
    const refreshed = await tryRefresh();
    if (refreshed) {
      const newToken = getToken();
      const retryHeaders = {
        ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
        ...(newToken ? { Authorization: `Bearer ${newToken}` } : {}),
        ...(options.headers || {}),
      };
      const retry = await fetch(path, { ...options, headers: retryHeaders });
      if (retry.status !== 401) return retry;
    }
    clearToken();
    clearMe();
    _onUnauthorized();
    throw new Error('Unauthorized — please sign in again.');
  }

  return res;
}

// ─── /me scope store ─────────────────────────────────────────
let _scopes = null;
let _identity = null;

/** Fetch /me and cache identity + effective scopes. Returns the body, or
 *  null on failure (scopes reset to empty so controls stay hidden). */
export async function loadMe() {
  try {
    const res = await apiFetch(`${API_BASE}/me`);
    if (!res.ok) { _scopes = []; _identity = null; return null; }
    const me = await res.json();
    _scopes = Array.isArray(me.effective_scopes) ? me.effective_scopes : [];
    _identity = me;
    return me;
  } catch {
    _scopes = []; _identity = null;
    return null;
  }
}

export function getScopes() { return _scopes || []; }
export function getIdentity() { return _identity; }
export function clearMe() { _scopes = null; _identity = null; }

/** Store-aware authority check used by pages to gate controls. */
export function hasScope(required) {
  return scopeGranted(getScopes(), required);
}
