import { apiFetch, API_BASE } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent } from '../utils/dom.js';
import { renderEntityList } from './_list.js';
import { money } from '../utils/maintenance-meta.js';
import {
  EXPENSE_CATEGORY_OPTIONS, PAYMENT_METHOD_OPTIONS,
  expenseCategoryLabel, statusBadge,
} from '../utils/expense-meta.js';

// Status filter: label → ?status= value. '' means all.
const STATUS_FILTERS = [
  { value: '', label: 'All Statuses' },
  { value: 'submitted', label: 'Needs review' },
  { value: 'reviewed', label: 'Reviewed' },
  { value: 'settled', label: 'Settled' },
];

function paymentMethodLabel(v) {
  const hit = PAYMENT_METHOD_OPTIONS.find(o => o.value === v);
  return hit ? hit.label : '—';
}

// submitted_by is "driver:<uuid>" or "fleet_user:<uuid>"; never surface the UUID.
function submittedByLabel(submittedBy, driverMap) {
  if (!submittedBy) return '—';
  if (submittedBy.startsWith('driver:')) {
    return driverMap.get(submittedBy.slice('driver:'.length)) || 'Driver';
  }
  if (submittedBy.startsWith('fleet_user:')) return 'Fleet user';
  return '—';
}

function expenseDate(e) {
  if (e.expense_date) return e.expense_date;
  if (e.created_at) return String(e.created_at).slice(0, 10);
  return '—';
}

async function loadDrivers() {
  try {
    const r = await apiFetch(`${API_BASE}/drivers`);
    if (!r.ok) return [];
    const d = await r.json();
    return d.items || (Array.isArray(d) ? d : []);
  } catch {
    return [];
  }
}

export async function renderExpensesView(params = {}) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  const qs = new URLSearchParams();
  if (params.status) qs.set('status', params.status);
  if (params.category) qs.set('category', params.category);
  if (params.driver_id) qs.set('driver_id', params.driver_id);
  if (params.from) qs.set('from', params.from);
  if (params.to) qs.set('to', params.to);
  const suffix = qs.toString() ? `?${qs.toString()}` : '';

  try {
    const drivers = await loadDrivers();
    const driverMap = new Map(drivers.map(d => [String(d.id), d.name || '—']));

    const res = await apiFetch(`${API_BASE}/expenses${suffix}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const items = data.items || (Array.isArray(data) ? data : []);

    renderEntityList({
      title: 'Expenses',
      createView: 'expense-new',
      createScope: 'expenses:write',
      createLabel: '+ New Expense',
      detailView: 'expense-detail',
      emptyText: 'No expenses found.',
      columns: [
        { header: 'Date',         cell: e => expenseDate(e) },
        { header: 'Category',     cell: e => expenseCategoryLabel(e.category) },
        { header: 'Driver',       cell: e => (e.driver_id && driverMap.get(String(e.driver_id))) || '—' },
        { header: 'Amount',       cell: e => money(e.amount) },
        { header: 'Approved',     cell: e => money(e.approved_amount) },
        { header: 'Method',       cell: e => (e.payment_method ? paymentMethodLabel(e.payment_method) : '—') },
        { header: 'Status',       cell: e => statusBadge(e.status) },
        { header: 'Submitted by', cell: e => submittedByLabel(e.submitted_by, driverMap) },
      ],
      rows: items,
      extraControls: (controlsEl) => {
        // ── Status select ──────────────────────────────────────
        const statusSel = document.createElement('select');
        statusSel.className = 'form-select';
        statusSel.setAttribute('aria-label', 'Filter by status');
        for (const { value, label } of STATUS_FILTERS) {
          const opt = document.createElement('option');
          opt.value = value;
          opt.textContent = label;
          if ((params.status || '') === value) opt.selected = true;
          statusSel.appendChild(opt);
        }
        statusSel.addEventListener('change', () => {
          renderExpensesView({ ...params, status: statusSel.value || undefined });
        });

        // ── Category select ────────────────────────────────────
        const catSel = document.createElement('select');
        catSel.className = 'form-select';
        catSel.setAttribute('aria-label', 'Filter by category');
        const allCat = document.createElement('option');
        allCat.value = '';
        allCat.textContent = 'All Categories';
        catSel.appendChild(allCat);
        for (const { value, label } of EXPENSE_CATEGORY_OPTIONS) {
          const opt = document.createElement('option');
          opt.value = value;
          opt.textContent = label;
          if (params.category === value) opt.selected = true;
          catSel.appendChild(opt);
        }
        catSel.addEventListener('change', () => {
          renderExpensesView({ ...params, category: catSel.value || undefined });
        });

        // ── Driver select ──────────────────────────────────────
        const driverSel = document.createElement('select');
        driverSel.className = 'form-select';
        driverSel.setAttribute('aria-label', 'Filter by driver');
        const allDrv = document.createElement('option');
        allDrv.value = '';
        allDrv.textContent = 'All Drivers';
        driverSel.appendChild(allDrv);
        for (const d of drivers) {
          const opt = document.createElement('option');
          opt.value = d.id;
          opt.textContent = d.name || d.id;
          if (params.driver_id && String(d.id) === String(params.driver_id)) opt.selected = true;
          driverSel.appendChild(opt);
        }
        driverSel.addEventListener('change', () => {
          renderExpensesView({ ...params, driver_id: driverSel.value || undefined });
        });

        // ── Date range ─────────────────────────────────────────
        const fromInput = document.createElement('input');
        fromInput.type = 'date';
        fromInput.className = 'form-input';
        fromInput.setAttribute('aria-label', 'From date');
        if (params.from) fromInput.value = params.from;
        fromInput.addEventListener('change', () => {
          renderExpensesView({ ...params, from: fromInput.value || undefined });
        });

        const toInput = document.createElement('input');
        toInput.type = 'date';
        toInput.className = 'form-input';
        toInput.setAttribute('aria-label', 'To date');
        if (params.to) toInput.value = params.to;
        toInput.addEventListener('change', () => {
          renderExpensesView({ ...params, to: toInput.value || undefined });
        });

        // Insert filters left-to-right ahead of the Create button.
        controlsEl.insertBefore(toInput, controlsEl.firstChild);
        controlsEl.insertBefore(fromInput, controlsEl.firstChild);
        controlsEl.insertBefore(driverSel, controlsEl.firstChild);
        controlsEl.insertBefore(catSel, controlsEl.firstChild);
        controlsEl.insertBefore(statusSel, controlsEl.firstChild);
      },
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load expenses: ${escHtml(err.message)}</div>`);
    }
  }
}
