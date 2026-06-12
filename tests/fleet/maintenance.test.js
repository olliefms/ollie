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
  document.body.innerHTML =
    '<div id="topbar-controls"></div><div id="main-content"></div>';
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

  it('renders an equipment-type filter with truck and trailer options', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 0, items: [] }));

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({});
    await Promise.resolve();

    const selects = document.querySelectorAll('#topbar-controls select');
    const typeSelect = [...selects].find(s =>
      s.getAttribute('aria-label') === 'Filter by equipment type'
    );
    expect(typeSelect).toBeTruthy();

    const values = [...typeSelect.options].map(o => o.value);
    expect(values).toContain('truck');
    expect(values).toContain('trailer');
    expect(values).toContain('');
  });

  it('re-renders with equipment_type in URL when type filter changes', async () => {
    // URL-aware fetch mock: /trucks and /trailers return unit lists;
    // /maintenance returns the maintenance list.
    const fetchMock = vi.fn(url => {
      if (url.includes('/trucks')) {
        return Promise.resolve(jsonResponse({ items: [{ id: 'tk1', unit_number: 'T-101' }] }));
      }
      if (url.includes('/trailers')) {
        return Promise.resolve(jsonResponse({ items: [{ id: 'tr1', unit_number: 'TR-201' }] }));
      }
      // /me or /maintenance
      if (url.includes('/maintenance')) {
        return Promise.resolve(jsonResponse({ returned: 0, items: [] }));
      }
      // fallback (e.g. /me from seedScopes)
      return Promise.resolve(jsonResponse({
        fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
        effective_scopes: ['*'],
      }));
    });
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderMaintenanceView } = await import('../../static/fleet/pages/maintenance.js');
    await renderMaintenanceView({});
    await Promise.resolve();

    const typeSelect = document.querySelector('select[aria-label="Filter by equipment type"]');
    expect(typeSelect).toBeTruthy();

    // Clear previous calls so we can inspect only the re-render calls.
    fetchMock.mockClear();

    // Simulate selecting "truck"
    typeSelect.value = 'truck';
    typeSelect.dispatchEvent(new Event('change'));

    // Wait for the re-render's fetch to complete.
    await vi.waitFor(() => {
      const calls = fetchMock.mock.calls.map(c => c[0]);
      return calls.some(u => u.includes('/maintenance') && u.includes('equipment_type=truck'));
    });

    const maintenanceCalls = fetchMock.mock.calls.filter(c => c[0].includes('/maintenance'));
    expect(maintenanceCalls.length).toBeGreaterThan(0);
    expect(maintenanceCalls[0][0]).toContain('equipment_type=truck');
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
