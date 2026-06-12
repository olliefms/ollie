import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

// Seed scopes directly so hasScope('maintenance:write') returns true.
// We stub fetch to return a /me response before each test that needs scopes.
async function seedScopes(fetchMock, scopes = ['*']) {
  const { loadMe } = await import('../../static/fleet/utils/api.js');
  fetchMock.mockResolvedValueOnce(jsonResponse({
    fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
    effective_scopes: scopes,
  }));
  await loadMe();
}

beforeEach(() => {
  document.body.innerHTML = '<div id="main-content"></div>';
  localStorage.clear();
  clearMe();
  saveToken('test-token');
  vi.restoreAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('renderMaintenanceView', () => {
  it('lists maintenance entries returned by the API', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    fetchMock.mockResolvedValueOnce(jsonResponse({
      returned: 1,
      items: [{
        id: 'm1', equipment_type: 'truck', equipment_id: 't1',
        service_date: '2026-06-01', category: 'repair',
        description: 'alternator', cost: 412.5, vendor: 'Acme',
      }],
    }));

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({});
    await Promise.resolve();

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('alternator');
    expect(html).toContain('Repair');
    expect(html).toContain('$412.50');

    const maintenanceCall = fetchMock.mock.calls.find(c => c[0].includes('/maintenance'));
    expect(maintenanceCall).toBeTruthy();
    expect(maintenanceCall[0]).toContain('/maintenance');
  });

  it('passes equipment filters as query params', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 0, items: [] }));

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({ equipment_type: 'trailer', equipment_id: 'tr1' });
    await Promise.resolve();

    const maintenanceCall = fetchMock.mock.calls.find(c => c[0].includes('/maintenance'));
    expect(maintenanceCall).toBeTruthy();
    const url = maintenanceCall[0];
    expect(url).toContain('equipment_type=trailer');
    expect(url).toContain('equipment_id=tr1');
  });
});

describe('appendMaintenanceHistory', () => {
  it('renders a table of entries for the equipment', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    fetchMock.mockResolvedValueOnce(jsonResponse({
      returned: 1,
      items: [{
        id: 'm9', service_date: '2026-05-01', category: 'tire',
        description: 'new tires', cost: 800, vendor: 'TireCo',
      }],
    }));

    const { appendMaintenanceHistory } = await import('../../static/fleet/pages/_maintenance-history.js');
    await appendMaintenanceHistory('truck', 't1');
    await Promise.resolve();

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('Maintenance History');
    expect(html).toContain('new tires');
    expect(html).toContain('$800.00');

    const url = fetchMock.mock.calls[0][0];
    expect(url).toContain('equipment_type=truck');
    expect(url).toContain('equipment_id=t1');
  });

  it('shows an empty state when there are no entries', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 0, items: [] }));

    const { appendMaintenanceHistory } = await import('../../static/fleet/pages/_maintenance-history.js');
    await appendMaintenanceHistory('trailer', 'x1');
    await Promise.resolve();

    expect(document.getElementById('main-content').innerHTML).toContain('No maintenance entries.');
  });
});
