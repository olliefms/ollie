import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { clearMe } from '../../static/fleet/utils/api.js';
import { saveToken } from '../../static/fleet/utils/auth.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

async function seedScopes(fetchMock) {
  const { loadMe } = await import('../../static/fleet/utils/api.js');
  fetchMock.mockResolvedValueOnce(jsonResponse({
    fleet_user_id: 'u1', name: 'T', email: 't@x.com', role: 'owner',
    effective_scopes: ['*'],
  }));
  await loadMe();
}

beforeEach(() => {
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
  vi.restoreAllMocks();
});
afterEach(() => vi.restoreAllMocks());

describe('trips list header', () => {
  it('renders the status filter + New Trip in the topbar, table in content', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ items: [] }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #trip-status-filter')).toBeTruthy();
    expect(document.querySelector('#topbar-controls #new-trip')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(main).toContain('Trip #'); // table header still in content
  });
});

describe('loads list header', () => {
  it('renders the status filter + New Load in the topbar, table in content', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ loads: [] }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    expect(document.querySelector('#topbar-controls #status-filter')).toBeTruthy();
    expect(document.querySelector('#topbar-controls #new-load')).toBeTruthy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).not.toContain('page-title');
    expect(main).toContain('Load #');
  });
});
