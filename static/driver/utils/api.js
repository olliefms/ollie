const BASE = '/driver/api/v1';

async function tryRefresh() {
  try {
    const res = await fetch(`${BASE}/auth/refresh`, {
      method: 'POST',
      credentials: 'same-origin',
    });
    if (!res.ok) return false;
    const data = await res.json();
    const token = data.token || data.access_token;
    if (!token) return false;
    localStorage.setItem('driver_token', token);
    return true;
  } catch {
    return false;
  }
}

async function apiFetch(path, options = {}) {
  const token = localStorage.getItem('driver_token');
  const headers = { 'Content-Type': 'application/json', ...options.headers };
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const resp = await fetch(`${BASE}${path}`, {
    ...options,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
  });

  if (resp.status === 401) {
    const refreshed = await tryRefresh();
    if (refreshed) {
      const newToken = localStorage.getItem('driver_token');
      const retryHeaders = { 'Content-Type': 'application/json', ...options.headers };
      if (newToken) retryHeaders['Authorization'] = `Bearer ${newToken}`;
      const retry = await fetch(`${BASE}${path}`, {
        ...options,
        headers: retryHeaders,
        body: options.body ? JSON.stringify(options.body) : undefined,
      });
      if (retry.status !== 401) {
        if (!retry.ok) {
          const err = await retry.json().catch(() => ({ error: retry.statusText }));
          throw Object.assign(new Error(err.error || 'Request failed'), { status: retry.status, data: err });
        }
        return retry.json();
      }
    }
    const err = { error: 'Unauthorized' };
    throw Object.assign(new Error('Unauthorized'), { status: 401, data: err });
  }

  if (!resp.ok) {
    const err = await resp.json().catch(() => ({ error: resp.statusText }));
    throw Object.assign(new Error(err.error || 'Request failed'), { status: resp.status, data: err });
  }
  return resp.json();
}

export { apiFetch, tryRefresh };
