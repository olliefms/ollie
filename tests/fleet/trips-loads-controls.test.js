import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { clearMe } from '../../static/fleet/utils/api.js';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { matchRoute } from '../../static/fleet/router.js';

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

  it('renders the origin → destination facility names in the Route column', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({
      returned: 1,
      items: [{
        id: 'l1',
        load_number: 'L-100',
        status: 'planned',
        customer_name: 'Acme',
        stops: [
          { name: 'Chicago DC', scheduled_arrive: '2026-07-01T12:00:00Z' },
          { name: 'Dallas Yard', scheduled_arrive: '2026-07-02T12:00:00Z' },
        ],
      }],
    }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    const main = document.getElementById('main-content').innerHTML;
    expect(main).toContain('Chicago DC');
    expect(main).toContain('Dallas Yard');
    expect(main).not.toContain('— → —');
  });
});

function loadRows(count, startAt = 0) {
  return Array.from({ length: count }, (_, i) => ({
    id: `l${startAt + i}`,
    load_number: `L-${startAt + i}`,
    status: 'planned',
    customer_name: 'Acme',
    stops: [{ name: 'A', scheduled_arrive: '2026-07-01T12:00:00Z' }, { name: 'B' }],
  }));
}

describe('list filters are URL-driven', () => {
  it('loads: a status change pushes ?status= and the next fetch carries it', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValue(jsonResponse({ returned: 0, total: 0, items: [] }));
    const pushSpy = vi.spyOn(history, 'pushState');

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    const sel = document.getElementById('status-filter');
    sel.value = 'planned';
    sel.dispatchEvent(new Event('change'));

    const pushed = pushSpy.mock.calls.at(-1)[2];
    expect(pushed).toBe('/fleet/loads?status=planned');

    await renderLoadsView(matchRoute(pushed).params);
    await Promise.resolve();
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('status=planned');
  });

  it('trips: a status change pushes ?status= and the next fetch carries it', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValue(jsonResponse({ items: [] }));
    const pushSpy = vi.spyOn(history, 'pushState');

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    const sel = document.getElementById('trip-status-filter');
    sel.value = 'dispatched';
    sel.dispatchEvent(new Event('change'));

    const pushed = pushSpy.mock.calls.at(-1)[2];
    expect(pushed).toBe('/fleet/trips?status=dispatched');

    await renderTripsView(matchRoute(pushed).params);
    await Promise.resolve();
    expect(fetchMock.mock.calls.at(-1)[0]).toContain('status=dispatched');
  });
});

describe('loads pagination', () => {
  it('requests a bounded page and appends the next one on Load more', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 20, total: 30, items: loadRows(20) }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    const firstUrl = fetchMock.mock.calls.at(-1)[0];
    expect(firstUrl).toContain('limit=20');
    expect(firstUrl).toContain('offset=0');

    const moreBtn = document.getElementById('loads-load-more');
    expect(moreBtn).toBeTruthy();

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 10, total: 30, items: loadRows(10, 20) }));
    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    expect(fetchMock.mock.calls.at(-1)[0]).toContain('offset=20');
    const main = document.getElementById('main-content').innerHTML;
    expect(main).toContain('L-0');
    expect(main).toContain('L-25');
    expect((main.match(/data-load-id=/g) || []).length).toBe(30);
    expect(document.getElementById('loads-load-more')).toBeFalsy();
  });

  it('shows no Load more when the first page is the whole result set', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 3, total: 3, items: loadRows(3) }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    expect(document.getElementById('loads-load-more')).toBeFalsy();
    expect(document.getElementById('main-content').innerHTML).not.toContain('Showing the most recent');
  });

  it('stops paging on an empty page even when total says more', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 20, total: 100, items: loadRows(20) }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    const moreBtn = document.getElementById('loads-load-more');
    expect(moreBtn).toBeTruthy();

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 0, total: 100, items: [] }));
    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    expect(document.getElementById('loads-load-more')).toBeFalsy();
  });

  it('caps the pager at the DB scan ceiling and shows the cap banner', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 2500, total: 2500, items: loadRows(2000) }));

    const { renderLoadsView } = await import('../../static/fleet/pages/loads.js');
    await renderLoadsView({});
    await Promise.resolve();

    expect(document.getElementById('loads-load-more')).toBeFalsy();
    const main = document.getElementById('main-content').innerHTML;
    expect(main).toContain('Showing the most recent 2000 of 2500 loads');
  }, 20_000);
});

function tripRows(count, startAt = 0) {
  return Array.from({ length: count }, (_, i) => ({
    id: `t${startAt + i}`,
    trip_number: `T-${startAt + i}`,
    load_number: `L-${startAt + i}`,
    status: 'planned',
    driver_name: 'Dana',
    stops: [{ name: 'A', scheduled_arrive: '2026-07-01T12:00:00Z' }, { name: 'B' }],
  }));
}

describe('trips pagination', () => {
  it('requests a bounded page and appends the next one on Load more', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 20, total: 30, items: tripRows(20) }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    const firstUrl = fetchMock.mock.calls.at(-1)[0];
    expect(firstUrl).toContain('limit=20');
    expect(firstUrl).toContain('offset=0');

    const moreBtn = document.getElementById('trips-load-more');
    expect(moreBtn).toBeTruthy();

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 10, total: 30, items: tripRows(10, 20) }));
    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    expect(fetchMock.mock.calls.at(-1)[0]).toContain('offset=20');
    const main = document.getElementById('main-content').innerHTML;
    expect(main).toContain('T-0');
    expect(main).toContain('T-25');
    expect((main.match(/data-trip-id=/g) || []).length).toBe(30);
    expect(document.getElementById('trips-load-more')).toBeFalsy();
  });

  it('shows no Load more when the first page is the whole result set', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 3, total: 3, items: tripRows(3) }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    expect(document.getElementById('trips-load-more')).toBeFalsy();
  });

  it('stops paging on an empty page even when total says more', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 20, total: 100, items: tripRows(20) }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({});
    await Promise.resolve();

    const moreBtn = document.getElementById('trips-load-more');
    expect(moreBtn).toBeTruthy();

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 0, total: 100, items: [] }));
    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    expect(document.getElementById('trips-load-more')).toBeFalsy();
  });

  it('carries the status filter on every page fetch', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 20, total: 25, items: tripRows(20) }));

    const { renderTripsView } = await import('../../static/fleet/pages/trips.js');
    await renderTripsView({ status: 'dispatched' });
    await Promise.resolve();

    expect(fetchMock.mock.calls.at(-1)[0]).toContain('status=dispatched');

    fetchMock.mockResolvedValueOnce(jsonResponse({ returned: 5, total: 25, items: tripRows(5, 20) }));
    document.getElementById('trips-load-more').click();
    await new Promise(r => setTimeout(r, 0));

    const nextUrl = fetchMock.mock.calls.at(-1)[0];
    expect(nextUrl).toContain('status=dispatched');
    expect(nextUrl).toContain('offset=20');
  });
});
