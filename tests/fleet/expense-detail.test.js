import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

async function seedScopes(fetchMock, scopes) {
  const { loadMe } = await import('../../static/fleet/utils/api.js');
  fetchMock.mockResolvedValueOnce(jsonResponse({
    fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
    effective_scopes: scopes,
  }));
  await loadMe();
}

// URL-aware mock: /expenses/:id → the given expense; anything else (e.g. /me
// re-fetch after a review save) → identity with the same scopes.
function urlMock(expense, scopes) {
  return vi.fn((url) => {
    if (url.includes('/expenses/')) return Promise.resolve(jsonResponse(expense));
    return Promise.resolve(jsonResponse({
      fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
      effective_scopes: scopes,
    }));
  });
}

const SUGGESTED_EXPENSE = {
  id: 'e1', status: 'submitted', category: 'fuel',
  driver_id: null, trip_id: null, equipment_type: null, equipment_id: null,
  maintenance_id: null, vendor: null, expense_date: null,
  submitted_by: 'driver:d1', amount: null, approved_amount: null,
  payment_method: null, reimbursement: null, deduction: null,
  review_note: null, reviewed_by: null, blob_ids: [],
  suggested_amount: 120.5, suggested_date: null, suggested_vendor: null,
  suggested_card_last4: null,
};

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

describe('renderExpenseDetail — suggestion amount button', () => {
  it('carries the raw numeric suggested_amount as data-value, not the formatted string', async () => {
    const fetchMock = urlMock(SUGGESTED_EXPENSE, ['expenses:read', 'expenses:approve']);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    const btn = document.querySelector('[data-use-suggestion="amount"]');
    expect(btn).toBeTruthy();
    // Regression guard: the visible label is formatted money, the data-value
    // driving the number input must be the raw numeric string.
    expect(btn.textContent).toBe('Use suggestion');
    expect(btn.dataset.value).toBe('120.5');
    expect(btn.dataset.value).not.toBe('$120.50');
  });

  it('clicking the button populates the number input (formatted string would be rejected)', async () => {
    const fetchMock = urlMock(SUGGESTED_EXPENSE, ['expenses:read', 'expenses:approve']);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    const btn = document.querySelector('[data-use-suggestion="amount"]');
    const input = document.getElementById('review-amount');
    input.value = '';
    btn.click();

    expect(input.value).toBe('120.5');
  });
});

describe('renderExpenseDetail — suggestions panel gated on review ability', () => {
  it('shows the AI suggestions panel + Use buttons for an expenses:approve reviewer', async () => {
    const fetchMock = urlMock(SUGGESTED_EXPENSE, ['expenses:read', 'expenses:approve']);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    expect(document.getElementById('main-content').innerHTML).toContain('AI suggestions');
    expect(document.querySelector('[data-use-suggestion="amount"]')).toBeTruthy();
  });

  it('hides the AI suggestions panel + Use buttons for a read-only expenses:read viewer', async () => {
    const fetchMock = urlMock(SUGGESTED_EXPENSE, ['expenses:read']);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    expect(document.getElementById('main-content').innerHTML).not.toContain('AI suggestions');
    expect(document.querySelector('[data-use-suggestion]')).toBeFalsy();
  });
});
