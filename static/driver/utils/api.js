const BASE = '/driver/api/v1';

async function apiFetch(path, options = {}) {
  const token = localStorage.getItem('driver_token');
  const headers = { 'Content-Type': 'application/json', ...options.headers };
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const resp = await fetch(`${BASE}${path}`, {
    ...options,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
  });

  if (!resp.ok) {
    const err = await resp.json().catch(() => ({ error: resp.statusText }));
    throw Object.assign(new Error(err.error || 'Request failed'), { status: resp.status, data: err });
  }
  return resp.json();
}

export { apiFetch };
