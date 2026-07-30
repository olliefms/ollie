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

// ─── Request timeout ─────────────────────────────────────────
// No request may hang forever: a stuck backend must surface as an error the page
// can render, not as a control that never comes back. Callers can override per
// request with `timeoutMs` (0 or null disables, for uploads).
export const DEFAULT_TIMEOUT_MS = 30000;
export const TIMEOUT_MESSAGE = 'Request timed out — the server did not respond. Please try again.';

/** Run `fetch` with an abort-based deadline, translating an abort into a
 *  recognizable error rather than a bare DOMException.
 *
 *  A caller-supplied `signal` is rejected rather than merged: forwarding it would
 *  detach the deadline (the timer would abort a controller the request isn't
 *  listening to) and the "cannot hang" guarantee would quietly stop holding —
 *  the same silent-failure shape this deadline exists to prevent. A caller that
 *  needs its own cancellation passes `timeoutMs: 0` and owns the lifecycle. */
async function fetchWithTimeout(path, init, timeoutMs) {
  if (!timeoutMs) return fetch(path, init);
  if (init.signal) {
    throw new Error('apiFetch: pass timeoutMs: 0 to supply your own AbortSignal');
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(path, { ...init, signal: controller.signal });
  } catch (err) {
    if (controller.signal.aborted) throw new Error(TIMEOUT_MESSAGE);
    throw err;
  } finally {
    clearTimeout(timer);
  }
}

// ─── API fetch wrapper ───────────────────────────────────────
export async function apiFetch(path, options = {}) {
  const token = getToken();
  const isFormData = options.body instanceof FormData;
  const { timeoutMs = DEFAULT_TIMEOUT_MS, ...init } = options;
  const headers = {
    ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
    ...(options.headers || {}),
  };

  const res = await fetchWithTimeout(path, { ...init, headers }, timeoutMs);

  if (res.status === 401) {
    const refreshed = await tryRefresh();
    if (refreshed) {
      const newToken = getToken();
      const retryHeaders = {
        ...(isFormData ? {} : { 'Content-Type': 'application/json' }),
        ...(newToken ? { Authorization: `Bearer ${newToken}` } : {}),
        ...(options.headers || {}),
      };
      const retry = await fetchWithTimeout(path, { ...init, headers: retryHeaders }, timeoutMs);
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
