import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

async function seedScopes(fetchMock, scopes = ['*']) {
  const { loadMe } = await import('../../static/fleet/utils/api.js');
  fetchMock.mockResolvedValueOnce(jsonResponse({
    fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
    effective_scopes: scopes,
  }));
  await loadMe();
}

// URL-aware mock: /drivers → driver list, /expenses → expense list, /me → identity.
function urlMock(expenses, drivers = []) {
  return vi.fn((url) => {
    if (url.includes('/drivers')) return Promise.resolve(jsonResponse({ items: drivers }));
    if (url.includes('/expenses')) return Promise.resolve(jsonResponse({ returned: expenses.length, total: expenses.length, items: expenses }));
    return Promise.resolve(jsonResponse({
      fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner', effective_scopes: ['*'],
    }));
  });
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

describe('renderExpensesView', () => {
  it('renders expense rows from the mocked API response', async () => {
    const fetchMock = urlMock(
      [{
        id: 'e1', status: 'submitted', category: 'fuel',
        driver_id: 'd1', amount: 120.5, approved_amount: null,
        payment_method: 'personal', submitted_by: 'driver:d1',
        expense_date: '2026-07-01', created_at: '2026-07-01T00:00:00Z',
      }],
      [{ id: 'd1', name: 'Jane Driver' }],
    );
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({});
    await Promise.resolve();

    const html = document.getElementById('main-content').innerHTML;
    expect(html).toContain('Fuel');
    expect(html).toContain('Jane Driver');
    expect(html).toContain('$120.50');
    // A submitted row shows the friendly status badge text.
    expect(html).toContain('Needs review');

    const call = fetchMock.mock.calls.find(c => c[0].includes('/expenses'));
    expect(call).toBeTruthy();
  });

  it('renders a status filter that includes the Needs-review option', async () => {
    const fetchMock = urlMock([]);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({});
    await Promise.resolve();

    const statusSelect = document.querySelector('#topbar-controls select[aria-label="Filter by status"]');
    expect(statusSelect).toBeTruthy();
    const labels = [...statusSelect.options].map(o => o.textContent);
    expect(labels).toContain('Needs review');
    const values = [...statusSelect.options].map(o => o.value);
    expect(values).toContain('submitted');
  });

  it('passes status/category/driver filters as query params', async () => {
    const fetchMock = urlMock([]);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({ status: 'submitted', category: 'fuel', driver_id: 'd1' });
    await Promise.resolve();

    const call = fetchMock.mock.calls.find(c => c[0].includes('/expenses'));
    expect(call).toBeTruthy();
    const url = call[0];
    expect(url).toContain('status=submitted');
    expect(url).toContain('category=fuel');
    expect(url).toContain('driver_id=d1');
  });
});

function expenseRows(count, startAt = 0) {
  return Array.from({ length: count }, (_, i) => ({
    id: `e${startAt + i}`, status: 'submitted', category: 'fuel',
    amount: 10, approved_amount: null, payment_method: 'personal',
    submitted_by: 'fleet_user:u1', expense_date: '2026-07-01',
  }));
}

// Mock whose /expenses response honours the requested limit against a total.
function pagedMock(total) {
  return vi.fn((url) => {
    if (url.includes('/drivers')) return Promise.resolve(jsonResponse({ items: [] }));
    if (url.includes('/expenses')) {
      const limit = Number(new URLSearchParams(url.split('?')[1] || '').get('limit')) || total;
      const items = expenseRows(Math.min(limit, total));
      return Promise.resolve(jsonResponse({ returned: items.length, total, items }));
    }
    return Promise.resolve(jsonResponse({
      fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner', effective_scopes: ['*'],
    }));
  });
}

describe('renderExpensesView pagination', () => {
  it('sends limit + offset and offers Load more until the total is reached', async () => {
    const fetchMock = pagedMock(150);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({});
    await Promise.resolve();

    const firstUrl = fetchMock.mock.calls.filter(c => c[0].includes('/expenses')).at(-1)[0];
    expect(firstUrl).toContain('limit=100');
    expect(firstUrl).toContain('offset=0');

    const moreBtn = document.getElementById('expenses-load-more');
    expect(moreBtn).toBeTruthy();

    moreBtn.click();
    await new Promise(r => setTimeout(r, 0));

    const nextUrl = fetchMock.mock.calls.filter(c => c[0].includes('/expenses')).at(-1)[0];
    expect(nextUrl).toContain('limit=200');
    expect(document.getElementById('expenses-load-more')).toBeFalsy();
  });

  it('hides Load more when everything is already loaded', async () => {
    const fetchMock = pagedMock(3);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({});
    await Promise.resolve();

    expect(document.getElementById('expenses-load-more')).toBeFalsy();
  });
});

describe('renderExpensesView filters live in the URL', () => {
  it('a status change navigates instead of re-rendering in place', async () => {
    const fetchMock = urlMock([]);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    const pushSpy = vi.spyOn(history, 'pushState');

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({});
    await Promise.resolve();

    const statusSel = document.querySelector('#topbar-controls select[aria-label="Filter by status"]');
    statusSel.value = 'submitted';
    statusSel.dispatchEvent(new Event('change'));

    const pushed = pushSpy.mock.calls.at(-1)[2];
    expect(pushed).toBe('/fleet/expenses?status=submitted');

    const { matchRoute } = await import('../../static/fleet/router.js');
    await renderExpensesView(matchRoute(pushed).params);
    await Promise.resolve();
    expect(fetchMock.mock.calls.filter(c => c[0].includes('/expenses')).at(-1)[0])
      .toContain('status=submitted');
  });

  it('a category change keeps the existing status filter in the URL', async () => {
    const fetchMock = urlMock([]);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock);
    const pushSpy = vi.spyOn(history, 'pushState');

    const { renderExpensesView } = await import('../../static/fleet/pages/expenses.js');
    await renderExpensesView({ status: 'submitted' });
    await Promise.resolve();

    const catSel = document.querySelector('#topbar-controls select[aria-label="Filter by category"]');
    catSel.value = 'fuel';
    catSel.dispatchEvent(new Event('change'));

    expect(pushSpy.mock.calls.at(-1)[2]).toBe('/fleet/expenses?status=submitted&category=fuel');
  });
});
