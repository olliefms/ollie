import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

// Flush pending promise chains (fetch → json → re-render) across a real
// macrotask boundary, since a chain of several microtasks needs more than
// a couple of `await Promise.resolve()` to fully settle.
function flushAsync() {
  return new Promise((resolve) => setTimeout(resolve, 0));
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

// Mock that also handles the review POST, routing the parsed body to
// reviewHandler and recording every call on fetchMock.mock.calls.
function reviewMock(expense, scopes, reviewHandler) {
  return vi.fn((url, opts) => {
    if (opts && opts.method === 'POST' && url.includes('/review')) {
      return Promise.resolve(reviewHandler(JSON.parse(opts.body)));
    }
    if (url.includes('/expenses/')) return Promise.resolve(jsonResponse(expense));
    return Promise.resolve(jsonResponse({
      fleet_user_id: 'u1', name: 'Test', email: 't@x.com', role: 'owner',
      effective_scopes: scopes,
    }));
  });
}

function reviewPostCalls(fetchMock) {
  return fetchMock.mock.calls.filter(([url, opts]) => opts && opts.method === 'POST' && url.includes('/review'));
}

describe('renderExpenseDetail — Approve all / Reject all one-click review', () => {
  it('approve-all with a suggested amount + selected payment method saves in one click', async () => {
    const fetchMock = reviewMock(SUGGESTED_EXPENSE, ['expenses:read', 'expenses:approve'], () => jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    // Harness caveat: happy-dom mis-reports select.value when `selected`
    // comes from innerHTML — set it explicitly.
    document.getElementById('review-method').value = 'company';

    document.getElementById('review-approve-all').click();
    await flushAsync();

    const calls = reviewPostCalls(fetchMock);
    expect(calls.length).toBe(1);
    const body = JSON.parse(calls[0][1].body);
    expect(body.amount).toBe(120.5);
    expect(body.approved_amount).toBe(body.amount);
  });

  it('reject-all posts approved_amount 0 while keeping the resolved amount', async () => {
    const fetchMock = reviewMock(SUGGESTED_EXPENSE, ['expenses:read', 'expenses:approve'], () => jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    document.getElementById('review-method').value = 'company';

    document.getElementById('review-reject-all').click();
    await flushAsync();

    const calls = reviewPostCalls(fetchMock);
    expect(calls.length).toBe(1);
    const body = JSON.parse(calls[0][1].body);
    expect(body.amount).toBe(120.5);
    expect(body.approved_amount).toBe(0);
  });

  it('with no amount and no suggestion, shows a specific message and issues no request', async () => {
    // Amount starts populated (so the button is enabled at render), no
    // suggested_amount exists as a fallback.
    const expense = { ...SUGGESTED_EXPENSE, amount: 50, suggested_amount: null };
    const fetchMock = reviewMock(expense, ['expenses:read', 'expenses:approve'], () => jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    // User clears the amount they were given, leaving nothing to resolve.
    document.getElementById('review-amount').value = '';
    document.getElementById('review-method').value = 'company';

    document.getElementById('review-approve-all').click();
    await Promise.resolve();

    expect(reviewPostCalls(fetchMock).length).toBe(0);
    const errEl = document.getElementById('review-error');
    expect(errEl.hidden).toBe(false);
    expect(errEl.textContent).toBe('No amount available — enter an amount first');
  });

  it('with an amount but no payment method, fills fields, blocks the save, and focuses the select', async () => {
    const expense = { ...SUGGESTED_EXPENSE, amount: 75, suggested_amount: null };
    const fetchMock = reviewMock(expense, ['expenses:read', 'expenses:approve'], () => jsonResponse({}));
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    const methodEl = document.getElementById('review-method');
    expect(methodEl.value).toBe('');

    document.getElementById('review-approve-all').click();
    await Promise.resolve();

    expect(reviewPostCalls(fetchMock).length).toBe(0);
    expect(document.getElementById('review-amount').value).toBe('75');
    expect(document.getElementById('review-approved').value).toBe('75');
    const errEl = document.getElementById('review-error');
    expect(errEl.hidden).toBe(false);
    expect(errEl.textContent).toBe('Select a payment method to approve');
    expect(document.activeElement).toBe(methodEl);
  });

  it('disables approve-all/reject-all with a title hint when there is no amount and no suggestion', async () => {
    const expense = { ...SUGGESTED_EXPENSE, amount: null, suggested_amount: null };
    const fetchMock = urlMock(expense, ['expenses:read', 'expenses:approve']);
    vi.stubGlobal('fetch', fetchMock);
    await seedScopes(fetchMock, ['expenses:read', 'expenses:approve']);

    const { renderExpenseDetail } = await import('../../static/fleet/pages/expense-detail.js');
    await renderExpenseDetail('e1');
    await Promise.resolve();

    const approveBtn = document.getElementById('review-approve-all');
    const rejectBtn = document.getElementById('review-reject-all');
    expect(approveBtn.disabled).toBe(true);
    expect(rejectBtn.disabled).toBe(true);
    expect(approveBtn.title).toBe('No amount available — enter an amount first');
    expect(approveBtn.getAttribute('type')).toBe('button');
    expect(rejectBtn.getAttribute('type')).toBe('button');
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
