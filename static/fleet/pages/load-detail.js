import { apiFetch, API_BASE, hasScope } from '../utils/api.js';
import {
  escHtml, badge, shortId, fmtDate, fmtArrivalWindow,
  fmtBytes, fmtUSD, fmtMiles,
} from '../utils/format.js';
import { setContent, navigate, goBack } from '../utils/dom.js';

const PRE_DELIVERY = ['planned', 'assigned', 'dispatched', 'in_transit'];

export async function renderLoadDetail(id) {
  const topbarTitle = document.getElementById('topbar-title');
  if (topbarTitle) topbarTitle.textContent = 'Load';

  setContent('<div class="state-loading"><div class="spinner"></div></div>');

  try {
    const [loadRes, tripsRes] = await Promise.all([
      apiFetch(`${API_BASE}/loads/${id}`),
      apiFetch(`${API_BASE}/trips?load_id=${id}`),
    ]);

    if (!loadRes.ok) throw new Error(`Load fetch HTTP ${loadRes.status}`);
    const load = await loadRes.json();

    if (topbarTitle) topbarTitle.textContent = `Load ${load.load_number || shortId(id)}`;

    let trips = [];
    if (tripsRes.ok) {
      const tripsData = await tripsRes.json();
      trips = tripsData.trips || tripsData.items || (Array.isArray(tripsData) ? tripsData : []);
    }

    const stops = load.stops || [];
    let stopsHtml = '';
    if (stops.length > 0) {
      const legs = (load.mileage_summary && load.mileage_summary.legs) || [];
      const stopRows = stops.map((stop, i) => {
        const milesCell = i === 0 ? '—' : fmtMiles(legs[i - 1] ? legs[i - 1].miles : null);
        return `
        <tr>
          <td>${i + 1}</td>
          <td>${escHtml(stop.facility_name || '—')}</td>
          <td>${escHtml(stop.stop_type || '—')}</td>
          <td>${fmtArrivalWindow(stop.scheduled_arrive, stop.scheduled_arrive_end)}</td>
          <td>${fmtDate(stop.actual_arrive)}</td>
          <td>${fmtDate(stop.actual_depart)}</td>
          <td style="text-align:right; font-variant-numeric: tabular-nums;">${milesCell}</td>
        </tr>
      `;
      }).join('');

      const totalMiles = load.mileage_summary ? fmtMiles(load.mileage_summary.total_miles) : '—';

      stopsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Stops</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead>
                <tr>
                  <th>#</th>
                  <th>Facility</th>
                  <th>Type</th>
                  <th>Scheduled Arrive</th>
                  <th>Actual Arrive</th>
                  <th>Actual Depart</th>
                  <th style="text-align:right;">Miles</th>
                </tr>
              </thead>
              <tbody>${stopRows}</tbody>
              <tfoot>
                <tr><td colspan="6" style="font-weight:600;">Total Miles</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${totalMiles}</td></tr>
              </tfoot>
            </table>
          </div>
        </div>
      `;
    }

    let tripsHtml = '';
    if (trips.length > 0) {
      const tripRows = trips.map(trip => `
        <tr data-trip-id="${trip.id}" style="cursor:pointer;">
          <td style="font-variant-numeric: tabular-nums;">${escHtml(trip.trip_number || shortId(trip.id))}</td>
          <td>${badge(trip.status)}</td>
          <td>${escHtml(trip.driver_name || '—')}</td>
          <td>${escHtml(trip.truck_unit || '—')}</td>
        </tr>
      `).join('');

      tripsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Trips</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead>
                <tr>
                  <th>Trip #</th>
                  <th>Status</th>
                  <th>Driver</th>
                  <th>Truck</th>
                </tr>
              </thead>
              <tbody id="load-trips-tbody">${tripRows}</tbody>
            </table>
          </div>
        </div>
      `;
    } else {
      tripsHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Trips</div>
          <div class="state-empty" style="min-height: 80px;">No trips for this load</div>
        </div>
      `;
    }

    let docsHtml = '';
    const blobIds = load.blob_ids || [];
    if (blobIds.length > 0) {
      const blobResults = await Promise.all(
        blobIds.map(bid =>
          apiFetch(`${API_BASE}/blob/${bid}`, {
            headers: { Accept: 'application/json' },
          })
            .then(r => (r.ok ? r.json() : null))
            .catch(() => null)
        )
      );
      const validBlobs = blobResults.filter(Boolean);
      if (validBlobs.length > 0) {
        const docRows = validBlobs
          .map(
            b => `
          <tr class="doc-row" data-blob-id="${b.id}" style="cursor:pointer;">
            <td>${escHtml(b.name) || '—'}</td>
            <td style="font-size:var(--text-sm);color:var(--color-text-muted);">${escHtml((b.mime_type || '').split('/').pop())}</td>
            <td>${fmtBytes(b.size)}</td>
            <td>${badge(b.status)}</td>
            <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escHtml(b.summary) || '—'}</td>
          </tr>
        `
          )
          .join('');

        docsHtml = `
          <div class="detail-card">
            <div class="detail-card__title">Documents</div>
            <div class="table-wrapper">
              <table class="data-table">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Type</th>
                    <th>Size</th>
                    <th>Status</th>
                    <th>Summary</th>
                  </tr>
                </thead>
                <tbody>${docRows}</tbody>
              </table>
            </div>
          </div>
        `;
      }
    }

    let rateHtml = '';
    const rateItems = load.rate_items || [];
    if (rateItems.length > 0) {
      const rateRows = rateItems.map(r => {
        const amountStyle = `text-align:right; font-variant-numeric: tabular-nums;${r.amount_usd < 0 ? ' color: var(--color-danger);' : ''}`;
        return `
          <tr>
            <td>${escHtml(r.description || '—')}</td>
            <td style="${amountStyle}">${fmtUSD(r.amount_usd)}</td>
          </tr>
        `;
      }).join('');
      rateHtml = `
        <div class="detail-card">
          <div class="detail-card__title">Rate</div>
          <div class="table-wrapper">
            <table class="data-table">
              <thead><tr><th>Description</th><th style="text-align:right;">Amount</th></tr></thead>
              <tbody>${rateRows}</tbody>
              <tfoot>
                <tr><td style="font-weight:600;">Total</td><td style="text-align:right; font-weight:600; font-variant-numeric: tabular-nums;">${fmtUSD(load.total_rate_usd)}</td></tr>
              </tfoot>
            </table>
          </div>
        </div>
      `;
    }

    const canWrite = hasScope('loads:write');
    const canInvoice = hasScope('loads:invoice') && load.status === 'delivered';
    const canSettle = hasScope('loads:settle') && load.status === 'invoiced';
    const canDelete = hasScope('loads:delete');
    const canCancel = canWrite && PRE_DELIVERY.includes(load.status);

    const actionBtns = [
      canWrite ? `<button class="btn btn--secondary" id="load-action-edit">Edit</button>` : '',
      canCancel ? `<button class="btn btn--secondary" id="load-action-cancel">Cancel Load</button>` : '',
      canInvoice ? `<button class="btn btn--secondary" id="load-action-invoice">Invoice</button>` : '',
      canSettle ? `<button class="btn btn--secondary" id="load-action-settle">Settle</button>` : '',
      canDelete ? `<button class="btn btn--secondary" id="load-action-delete">Delete</button>` : '',
    ].join('');

    const html = `
      <button class="back-link" id="back-to-loads">← Back to Loads</button>

      <div class="detail-card">
        <div class="detail-card__title">Load Details</div>
        <div class="detail-grid">
          <div class="detail-item">
            <div class="detail-item__label">Load #</div>
            <div class="detail-item__value" style="font-variant-numeric: tabular-nums;">${escHtml(load.load_number || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Status</div>
            <div class="detail-item__value">${badge(load.status)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Customer</div>
            <div class="detail-item__value">${escHtml(load.customer || load.customer_name || '—')}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Created</div>
            <div class="detail-item__value">${fmtDate(load.created_at)}</div>
          </div>
          <div class="detail-item">
            <div class="detail-item__label">Updated</div>
            <div class="detail-item__value">${fmtDate(load.updated_at)}</div>
          </div>
          ${load.customer_ref ? `
          <div class="detail-item">
            <div class="detail-item__label">Customer Ref</div>
            <div class="detail-item__value">${escHtml(load.customer_ref)}</div>
          </div>` : ''}
          ${load.commodity ? `
          <div class="detail-item">
            <div class="detail-item__label">Commodity</div>
            <div class="detail-item__value">${escHtml(load.commodity)}</div>
          </div>` : ''}
          ${load.weight_lbs != null ? `
          <div class="detail-item">
            <div class="detail-item__label">Weight</div>
            <div class="detail-item__value">${Number(load.weight_lbs).toLocaleString()} lbs</div>
          </div>` : ''}
          <div class="detail-item">
            <div class="detail-item__label">Miles</div>
            <div class="detail-item__value">${load.mileage_summary ? fmtMiles(load.mileage_summary.total_miles) : '—'}</div>
          </div>
          ${load.invoice_number ? `
          <div class="detail-item">
            <div class="detail-item__label">Invoice #</div>
            <div class="detail-item__value">${escHtml(load.invoice_number)}</div>
          </div>` : ''}
          ${load.cancel_reason ? `
          <div class="detail-item">
            <div class="detail-item__label">Cancel Reason</div>
            <div class="detail-item__value">${escHtml(load.cancel_reason)}</div>
          </div>` : ''}
          ${load.notes ? `
          <div class="detail-item">
            <div class="detail-item__label">Notes</div>
            <div class="detail-item__value">${escHtml(load.notes)}</div>
          </div>` : ''}
          ${load.tags && load.tags.length > 0 ? `
          <div class="detail-item">
            <div class="detail-item__label">Tags</div>
            <div class="detail-item__value">${load.tags.map(t => escHtml(t)).join(', ')}</div>
          </div>` : ''}
        </div>
        ${actionBtns ? `<div class="form-panel__actions">${actionBtns}</div>` : ''}
      </div>

      <div id="load-action-status" class="alert" hidden style="margin-top:var(--space-3);"></div>

      ${rateHtml}
      ${stopsHtml}
      ${tripsHtml}
      ${docsHtml}
    `;

    setContent(html);

    document.getElementById('back-to-loads').addEventListener('click', goBack);

    document.querySelectorAll('#load-trips-tbody tr[data-trip-id]').forEach(row => {
      row.addEventListener('click', () => navigate('trip-detail', { id: row.dataset.tripId }));
    });

    document.querySelectorAll('.doc-row').forEach(row => {
      row.addEventListener('click', () => {
        navigate('document', { id: row.dataset.blobId });
      });
    });

    const statusEl = document.getElementById('load-action-status');

    document.getElementById('load-action-edit')?.addEventListener('click', () => {
      navigate('load-edit', { id });
    });

    document.getElementById('load-action-cancel')?.addEventListener('click', () => {
      cancelLoad(statusEl, id);
    });

    document.getElementById('load-action-invoice')?.addEventListener('click', () => {
      invoiceLoad(statusEl, id);
    });

    document.getElementById('load-action-settle')?.addEventListener('click', () => {
      settleLoad(statusEl, id);
    });

    document.getElementById('load-action-delete')?.addEventListener('click', () => {
      deleteLoad(statusEl, id, load.load_number);
    });
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') {
      setContent(`<div class="state-error">Failed to load data: ${escHtml(err.message)}</div>`);
    }
  }
}

function showError(statusEl, text) {
  statusEl.hidden = false;
  statusEl.className = 'alert alert--error';
  statusEl.textContent = text;
}

async function cancelLoad(statusEl, id) {
  const reason = window.prompt('Cancel reason (optional):');
  if (reason === null) return;
  try {
    const body = reason ? { reason } : {};
    const res = await apiFetch(`${API_BASE}/loads/${id}/cancel`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (res.ok) { renderLoadDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Cancel failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Cancel failed: ${err.message}`);
  }
}

async function invoiceLoad(statusEl, id) {
  const invoiceNumber = window.prompt('Invoice number (optional):');
  if (invoiceNumber === null) return;
  try {
    const body = invoiceNumber ? { invoice_number: invoiceNumber } : {};
    const res = await apiFetch(`${API_BASE}/loads/${id}/invoice`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (res.ok) { renderLoadDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Invoice failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Invoice failed: ${err.message}`);
  }
}

async function settleLoad(statusEl, id) {
  if (!confirm('Settle this load? This cannot be undone.')) return;
  try {
    const res = await apiFetch(`${API_BASE}/loads/${id}/settle`, { method: 'POST' });
    if (res.ok) { renderLoadDetail(id); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Settle failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Settle failed: ${err.message}`);
  }
}

async function deleteLoad(statusEl, id, loadNumber) {
  const label = loadNumber || 'DELETE';
  const prompt = loadNumber
    ? `Permanently delete load "${loadNumber}"? This cannot be undone.\nType the load number to confirm:`
    : 'Permanently delete this load? This cannot be undone.\nType DELETE to confirm:';
  const typed = window.prompt(prompt);
  if (typed === null) return;
  if (typed !== label) { showError(statusEl, 'Load number did not match — delete cancelled.'); return; }
  try {
    const res = await apiFetch(`${API_BASE}/loads/${id}`, { method: 'DELETE' });
    if (res.ok || res.status === 204) { navigate('loads'); return; }
    const data = await res.json().catch(() => ({}));
    showError(statusEl, data.error || `Delete failed (HTTP ${res.status}).`);
  } catch (err) {
    if (err.message !== 'Unauthorized — please sign in again.') showError(statusEl, `Delete failed: ${err.message}`);
  }
}
