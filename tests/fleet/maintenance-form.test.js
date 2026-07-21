import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { saveToken } from '../../static/fleet/utils/auth.js';
import { clearMe } from '../../static/fleet/utils/api.js';

function jsonResponse(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, json: async () => body };
}

const LINKED_RECORD = {
  id: 'm1', equipment_type: 'truck', equipment_id: 't1',
  service_date: '2026-06-01', category: 'repair', description: 'alternator',
  cost: 412.5, odometer: 88000, vendor: 'Acme', invoice_ref: 'INV-1',
  expense_id: 'exp1',
};

const UNLINKED_RECORD = {
  id: 'm2', equipment_type: 'truck', equipment_id: 't1',
  service_date: '2026-06-01', category: 'repair', description: 'brakes',
  cost: 200, odometer: 90000, vendor: 'Acme', invoice_ref: 'INV-2',
  expense_id: null,
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

describe('renderMaintenanceForm — linked-record cost suppression', () => {
  it('omits the Cost field and shows a hint when editing a record with expense_id set', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(jsonResponse(LINKED_RECORD)));
    vi.stubGlobal('fetch', fetchMock);

    const { renderMaintenanceForm } = await import('../../static/fleet/pages/maintenance-form.js');
    await renderMaintenanceForm('m1');
    await Promise.resolve();

    expect(document.querySelector('[data-field="cost"]')).toBeFalsy();
    expect(document.getElementById('main-content').innerHTML).toContain('Cost is managed by the linked expense.');
  });

  it('keeps the Cost field for a record with no expense_id', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(jsonResponse(UNLINKED_RECORD)));
    vi.stubGlobal('fetch', fetchMock);

    const { renderMaintenanceForm } = await import('../../static/fleet/pages/maintenance-form.js');
    await renderMaintenanceForm('m2');
    await Promise.resolve();

    const costInput = document.querySelector('[data-field="cost"]');
    expect(costInput).toBeTruthy();
    expect(costInput.value).toBe('200');
    expect(document.getElementById('main-content').innerHTML).not.toContain('Cost is managed by the linked expense.');
  });

  it('submits an edit payload for a linked record with no cost key', async () => {
    const fetchMock = vi.fn((url) => {
      if (url.includes('/maintenance/m1') && !fetchMock.mock.calls.some(c => c[1] && c[1].method === 'PATCH')) {
        return Promise.resolve(jsonResponse(LINKED_RECORD));
      }
      return Promise.resolve(jsonResponse({ ...LINKED_RECORD, description: 'alternator replaced' }));
    });
    vi.stubGlobal('fetch', fetchMock);

    const { renderMaintenanceForm } = await import('../../static/fleet/pages/maintenance-form.js');
    await renderMaintenanceForm('m1');
    await Promise.resolve();

    const descInput = document.querySelector('[data-field="description"]');
    descInput.value = 'alternator replaced';
    document.querySelector('[data-form-submit]').click();
    await Promise.resolve(); await Promise.resolve();

    const patchCall = fetchMock.mock.calls.find(c => c[1] && c[1].method === 'PATCH');
    expect(patchCall).toBeTruthy();
    const body = JSON.parse(patchCall[1].body);
    expect('cost' in body).toBe(false);
    expect(body.description).toBe('alternator replaced');
  });
});
