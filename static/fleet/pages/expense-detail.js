import { apiFetch, API_BASE, hasScope, getIdentity } from '../utils/api.js';
import { escHtml } from '../utils/format.js';
import { setContent, goBack, navigate } from '../utils/dom.js';
import { detailLink } from './_detail.js';
import { confirmDelete } from '../components/confirm.js';
import { money } from '../utils/maintenance-meta.js';
import {
  PAYMENT_METHOD_OPTIONS, expenseCategoryLabel, statusBadge, dispositionLabel,
} from '../utils/expense-meta.js';

function paymentMethodLabel(v) {
  const hit = PAYMENT_METHOD_OPTIONS.find(o => o.value === v);
  return hit ? hit.label : '—';
}

function submittedByLabel(submittedBy, driverName) {
  if (!submittedBy) return '—';
  if (submittedBy.startsWith('driver:')) return driverName || 'Driver';
  if (submittedBy.startsWith('fleet_user:')) return 'Fleet user';
  return '—';
}

async function resolveDriverName(driverId) {
  if (!driverId) return null;
  try {
    const r = await apiFetch(`${API_BASE}/drivers`);
    if (!r.ok) return null;
    const d = await r.json();
    const drivers = d.items || (Array.isArray(d) ? d : []);
    const hit = drivers.find(x => String(x.id) === String(driverId));
    return hit ? (hit.name || null) : null;
  } catch {
    return null;
  }
}

async function resolveUnitNumber(equipmentType, equipmentId) {
  if (!equipmentType || !equipmentId) return null;
  try {
    const endpoint = equipmentType === 'truck' ? `${API_BASE}/trucks` : `${API_BASE}/trailers`;
    const r = await apiFetch(endpoint);
    if (!r.ok) return null;
    const d = await r.json();
    const units = d.items || (Array.isArray(d) ? d : []);
    const hit = units.find(u => String(u.id) === String(equipmentId));
    return hit ? (hit.unit_number || null) : null;
  } catch {
    return null;
  }
}

function factRow(label, valueHtml) {
  return `
    <div class="detail-item">
      <div class="detail-item__label">${escHtml(label)}</div>
      <div class="detail-item__value">${valueHtml}</div>
    </div>`;
}

function isOwn(e) {
  const uid = getIdentity() && getIdentity().fleet_user_id;
  return !!uid && e.submitted_by === `fleet_user:${uid}`;
}

export async function renderExpenseDetail(id) {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
  try {
    const res = await apiFetch(`${API_BASE}/expenses/${encodeURIComponent(id)}`);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const e = await res.json();

    const driverName = await resolveDriverName(e.driver_id);
    const unitNumber = await resolveUnitNumber(e.equipment_type, e.equipment_id);

    const driverHtml = e.driver_id && driverName
      ? detailLink('driver-detail', e.driver_id, driverName)
      : '—';
    const tripHtml = e.trip_id ? detailLink('trip-detail', e.trip_id, 'View trip') : '—';
    const equipHtml = (e.equipment_type && e.equipment_id)
      ? detailLink(
          e.equipment_type === 'truck' ? 'truck-detail' : 'trailer-detail',
          e.equipment_id,
          `${e.equipment_type} ${unitNumber || ''}`.trim(),
        )
      : '—';
    const maintHtml = e.maintenance_id
      ? detailLink('maintenance-detail', e.maintenance_id, 'View maintenance')
      : '—';

    const settled = e.status === 'settled';

    const facts = [
      factRow('Status', escHtml(statusBadge(e.status))),
      factRow('Category', escHtml(expenseCategoryLabel(e.category))),
      factRow('Driver', driverHtml),
      factRow('Trip', tripHtml),
      factRow('Equipment', equipHtml),
      factRow('Maintenance', maintHtml),
      factRow('Vendor', escHtml(e.vendor || '—')),
      factRow('Expense Date', escHtml(e.expense_date || '—')),
      factRow('Submitted By', escHtml(submittedByLabel(e.submitted_by, driverName))),
      factRow('Amount', escHtml(money(e.amount))),
      factRow('Approved', escHtml(money(e.approved_amount))),
      factRow('Method', escHtml(e.payment_method ? paymentMethodLabel(e.payment_method) : '—')),
      factRow('Reimbursement', escHtml(money(e.reimbursement))),
      factRow('Deduction', escHtml(money(e.deduction))),
      factRow('Disposition', escHtml(dispositionLabel(e))),
      factRow('Review Note', escHtml(e.review_note || '—')),
      factRow('Reviewer', escHtml(e.reviewed_by ? 'Fleet user' : '—')),
    ].join('');

    // ── AI suggestions panel ─────────────────────────────────
    const hasSuggestions = e.suggested_amount != null || e.suggested_date
      || e.suggested_vendor || e.suggested_card_last4;
    const showSuggestions = e.status === 'submitted' && hasSuggestions;
    let suggestionsHtml = '';
    if (showSuggestions) {
      const line = (label, valueText, field) => {
        const useBtn = field
          ? `<button class="btn btn--secondary" data-use-suggestion="${escHtml(field)}" data-value="${escHtml(String(valueText))}">Use suggestion</button>`
          : '';
        return `
          <div class="detail-item">
            <div class="detail-item__label">${escHtml(label)}</div>
            <div class="detail-item__value" style="display:flex;align-items:center;gap:var(--space-2);">
              <span>${escHtml(String(valueText))}</span>${useBtn}
            </div>
          </div>`;
      };
      const rows = [];
      if (e.suggested_amount != null) rows.push(line('Suggested amount', money(e.suggested_amount), 'amount'));
      if (e.suggested_date) rows.push(line('Suggested date', e.suggested_date, 'expense_date'));
      if (e.suggested_vendor) rows.push(line('Suggested vendor', e.suggested_vendor, 'vendor'));
      if (e.suggested_card_last4) rows.push(line(`Suggested card (••${e.suggested_card_last4})`, `••${e.suggested_card_last4}`, null));
      suggestionsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">AI suggestions</div>
          <div class="detail-grid">${rows.join('')}</div>
        </div>`;
    }

    // ── Receipts panel ───────────────────────────────────────
    let receiptsHtml = '';
    const blobIds = Array.isArray(e.blob_ids) ? e.blob_ids : [];
    if (blobIds.length) {
      const links = blobIds
        .map((bid, i) => `<div class="detail-item__value">${detailLink('document', bid, `Receipt ${i + 1}`)}</div>`)
        .join('');
      receiptsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Receipts</div>
          <div class="detail-grid">${links}</div>
        </div>`;
    }

    // ── Review form (expenses:approve, hidden when settled) ──
    let reviewHtml = '';
    const canReview = hasScope('expenses:approve') && !settled;
    if (canReview) {
      const amountVal = e.amount != null ? e.amount : (e.suggested_amount != null ? e.suggested_amount : '');
      const approvedVal = e.approved_amount != null ? e.approved_amount : '';
      const dateVal = e.expense_date || e.suggested_date || '';
      const vendorVal = e.vendor || e.suggested_vendor || '';
      const noteVal = e.review_note || '';
      const methodOpts = PAYMENT_METHOD_OPTIONS
        .map(o => `<option value="${escHtml(o.value)}" ${o.value === e.payment_method ? 'selected' : ''}>${escHtml(o.label)}</option>`)
        .join('');
      reviewHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Review</div>
          <div class="alert alert--error" id="review-error" hidden></div>
          <div class="form-group">
            <label class="form-label">Amount</label>
            <input class="form-input" type="number" step="any" id="review-amount" value="${escHtml(String(amountVal))}">
          </div>
          <div class="form-group">
            <label class="form-label">Approved Amount</label>
            <input class="form-input" type="number" step="any" id="review-approved" value="${escHtml(String(approvedVal))}">
          </div>
          <div class="form-group">
            <label class="form-label">Payment Method</label>
            <select class="form-input" id="review-method"><option value=""></option>${methodOpts}</select>
          </div>
          <div class="form-group">
            <label class="form-label">Expense Date</label>
            <input class="form-input" type="date" id="review-date" value="${escHtml(String(dateVal))}">
          </div>
          <div class="form-group">
            <label class="form-label">Vendor</label>
            <input class="form-input" type="text" id="review-vendor" value="${escHtml(String(vendorVal))}">
          </div>
          <div class="form-group">
            <label class="form-label">Review Note</label>
            <textarea class="form-input" id="review-note">${escHtml(String(noteVal))}</textarea>
          </div>
          <div class="form-panel__actions">
            <button class="btn btn--secondary" id="review-approve-all">Approve all</button>
            <button class="btn btn--secondary" id="review-reject-all">Reject all</button>
            <button class="btn btn--primary" id="review-save">Save review</button>
          </div>
        </div>`;
    }

    setContent(`
      <button class="back-link" id="detail-back">← Back</button>
      <div class="detail-card">
        <div class="detail-card__title">Expense — ${escHtml(expenseCategoryLabel(e.category))}</div>
        <div class="detail-grid">${facts}</div>
        <div class="form-panel__actions" id="expense-actions"></div>
      </div>
      ${suggestionsHtml}
      ${receiptsHtml}
      ${reviewHtml}
      <div id="detail-status" class="alert" hidden style="margin-top:var(--space-3);"></div>
    `);

    document.getElementById('detail-back').addEventListener('click', goBack);

    // Wire detail-link navigation (driver/trip/equipment/maintenance/receipts).
    for (const a of document.querySelectorAll('.detail-link[data-nav-view]')) {
      a.addEventListener('click', (ev) => {
        ev.preventDefault();
        navigate(a.dataset.navView, { id: a.dataset.navId });
      });
    }

    // Wire "Use suggestion" buttons → copy value into the matching review input.
    for (const btn of document.querySelectorAll('[data-use-suggestion]')) {
      btn.addEventListener('click', () => {
        const field = btn.dataset.useSuggestion;
        const value = btn.dataset.value;
        const inputId = { amount: 'review-amount', expense_date: 'review-date', vendor: 'review-vendor' }[field];
        const input = inputId && document.getElementById(inputId);
        if (input) input.value = value;
      });
    }

    // Review actions.
    if (canReview) {
      const amountEl = document.getElementById('review-amount');
      const approvedEl = document.getElementById('review-approved');
      document.getElementById('review-approve-all').addEventListener('click', () => {
        approvedEl.value = amountEl.value;
      });
      document.getElementById('review-reject-all').addEventListener('click', () => {
        approvedEl.value = '0';
      });
      document.getElementById('review-save').addEventListener('click', () => saveReview(id, e));
    }

    // Action buttons.
    const actionsEl = document.getElementById('expense-actions');
    const statusEl = document.getElementById('detail-status');

    if (!settled && hasScope('expenses:write')) {
      const editBtn = document.createElement('button');
      editBtn.className = 'btn btn--secondary';
      editBtn.textContent = 'Edit';
      editBtn.addEventListener('click', () => navigate('expense-edit', { id }));
      actionsEl.appendChild(editBtn);
    }

    if (e.category === 'repair' && e.equipment_id && !e.maintenance_id && hasScope('maintenance:write')) {
      const mBtn = document.createElement('button');
      mBtn.className = 'btn btn--secondary';
      mBtn.textContent = 'Create maintenance record';
      mBtn.addEventListener('click', () => navigate('maintenance-new', {
        equipment_type: e.equipment_type,
        equipment_id: e.equipment_id,
        expense_id: id,
      }));
      actionsEl.appendChild(mBtn);
    }

    const canDelete = !settled && (hasScope('expenses:approve')
      || (hasScope('expenses:write') && e.status === 'submitted' && isOwn(e)));
    if (canDelete) {
      const delBtn = document.createElement('button');
      delBtn.className = 'btn btn--secondary';
      delBtn.textContent = 'Delete';
      delBtn.addEventListener('click', () => deleteExpense(statusEl, id));
      actionsEl.appendChild(delBtn);
    }

    if (!actionsEl.children.length) actionsEl.remove();
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load expense: ${escHtml(err.message)}</div>`);
    }
  }
}

async function saveReview(id, e) {
  const errEl = document.getElementById('review-error');
  const amount = parseFloat(document.getElementById('review-amount').value);
  const approved = parseFloat(document.getElementById('review-approved').value);
  const method = document.getElementById('review-method').value;
  const expenseDate = document.getElementById('review-date').value;
  const vendor = document.getElementById('review-vendor').value;
  const note = document.getElementById('review-note').value;

  const showErr = (msg) => { errEl.textContent = msg; errEl.hidden = false; };

  if (Number.isNaN(amount)) return showErr('Amount is required.');
  if (Number.isNaN(approved)) return showErr('Approved amount is required.');
  if (!method) return showErr('Payment method is required.');
  errEl.hidden = true;

  const body = { amount, approved_amount: approved, payment_method: method };
  if (expenseDate) body.expense_date = expenseDate;
  if (vendor) body.vendor = vendor;
  if (note) body.review_note = note;

  const saveBtn = document.getElementById('review-save');
  saveBtn.disabled = true;
  try {
    const res = await apiFetch(`${API_BASE}/expenses/${encodeURIComponent(id)}/review`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (res.ok) { renderExpenseDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showErr(data.error || `Review failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      showErr(`Review failed: ${err.message}`);
    }
  } finally {
    saveBtn.disabled = false;
  }
}

async function deleteExpense(statusEl, id) {
  if (!confirmDelete('this expense')) return;
  try {
    const res = await apiFetch(`${API_BASE}/expenses/${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('expenses'); return; }
    const data = await res.json().catch(() => ({}));
    statusEl.hidden = false;
    statusEl.className = 'alert alert--error';
    statusEl.textContent = data.error || `Delete failed (HTTP ${res.status}).`;
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      statusEl.hidden = false;
      statusEl.className = 'alert alert--error';
      statusEl.textContent = `Delete failed: ${err.message}`;
    }
  }
}
